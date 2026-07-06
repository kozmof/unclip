//! Command handlers for the unclip CLI.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use anyhow::{bail, Context};
use unclip_core::{
    validate_branch, validate_branch_record, validate_packet, validate_path, Branch, Frame,
    Reference, SampleQuery, SelectionPacket, Slot,
};
use unclip_io::split_frame_selector;
use unclip_store::{BranchReader, BranchRepository, BranchWriter, FrameRepository, IndexedValue};

/// Parse a `name=value` pair, used for `--o2o` / `--o2m` flags.
pub fn parse_kv(raw: &str) -> anyhow::Result<(String, String)> {
    let (name, value) = raw
        .split_once('=')
        .with_context(|| format!("expected name=value, got `{raw}`"))?;
    if name.is_empty() {
        bail!("empty name in `{raw}`");
    }
    if value.is_empty() {
        bail!("empty value in `{raw}`");
    }
    Ok((name.to_string(), value.to_string()))
}

/// Merge `name=value` pairs into a one-to-one o2o map, rejecting any name that
/// is already present. o2o values are one-to-one, so a repeated name
/// — whether across flags or colliding with a frame slot's base value — is a
/// usage error.
pub fn merge_o2o(
    map: &mut BTreeMap<String, String>,
    pairs: Vec<(String, String)>,
) -> anyhow::Result<()> {
    for (name, value) in pairs {
        match map.entry(name) {
            Entry::Occupied(entry) => {
                bail!(
                    "duplicate o2o name `{}` (o2o values are one-to-one)",
                    entry.key()
                );
            }
            Entry::Vacant(entry) => {
                entry.insert(value);
            }
        }
    }
    Ok(())
}

/// Arguments for `add`, parsed directly by clap and flattened into the
/// `add` subcommand.
#[derive(clap::Args)]
pub struct AddInput {
    /// Slash-separated scope address, e.g. /ikebukuro/station/exit.
    pub path: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, default_value_t = 1.0)]
    pub weight: f64,
    /// One-to-one indexed value, name=value (repeatable).
    #[arg(long = "o2o", value_parser = parse_kv)]
    pub o2o: Vec<(String, String)>,
    /// One-to-many indexed value, name=value (repeatable).
    #[arg(long = "o2m", value_parser = parse_kv)]
    pub o2m: Vec<(String, String)>,
}

pub async fn add(repo: &impl BranchWriter, input: AddInput) -> anyhow::Result<()> {
    validate_path(&input.path)?;
    // The repository validates the full record (finite weight, o2o/o2m value
    // shape) and reports a duplicate path atomically at the insert itself.
    let mut branch = Branch::new(&input.path);
    branch.title = input.title;
    branch.description = input.description;
    branch.weight = input.weight;

    merge_o2o(&mut branch.o2o, input.o2o)?;

    // o2m is a set, but the store layer enforces that (dedup + canonical order)
    // on the way to SQL, so we just group the flag values by name here.
    let mut o2m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in input.o2m {
        o2m.entry(name).or_default().push(value);
    }
    branch.o2m = o2m;

    repo.add(branch).await?;
    crate::output::outln!("added {}", input.path);
    Ok(())
}

/// Arguments for `edit`, parsed directly by clap and flattened into the
/// `edit` subcommand.
///
/// Every field is an additive/overwriting patch over an existing branch; a
/// `None`/empty value leaves the corresponding part untouched. `metadata` is
/// intentionally not editable here — it is free-form JSON and is better managed
/// by re-`import`.
#[derive(clap::Args)]
pub struct EditInput {
    /// Branch path to edit.
    pub path: String,
    #[arg(long)]
    pub title: Option<String>,
    /// Remove the title.
    #[arg(long = "clear-title")]
    pub clear_title: bool,
    #[arg(long)]
    pub description: Option<String>,
    /// Remove the description.
    #[arg(long = "clear-description")]
    pub clear_description: bool,
    #[arg(long)]
    pub weight: Option<f64>,
    /// Set (overwrite) a one-to-one value, name=value (repeatable).
    #[arg(long = "o2o", value_parser = parse_kv)]
    pub o2o: Vec<(String, String)>,
    /// Remove a one-to-one value by name (repeatable).
    #[arg(long = "remove-o2o")]
    pub remove_o2o: Vec<String>,
    /// Add a one-to-many value, name=value (repeatable).
    #[arg(long = "add-o2m", value_parser = parse_kv)]
    pub add_o2m: Vec<(String, String)>,
    /// Remove a one-to-many value, name=value (repeatable).
    #[arg(long = "remove-o2m", value_parser = parse_kv)]
    pub remove_o2m: Vec<(String, String)>,
}

pub async fn edit(repo: &impl BranchRepository, input: EditInput) -> anyhow::Result<()> {
    if input.title.is_some() && input.clear_title {
        bail!("--title and --clear-title are mutually exclusive");
    }
    if input.description.is_some() && input.clear_description {
        bail!("--description and --clear-description are mutually exclusive");
    }

    let mut branch = repo
        .get(&input.path)
        .await?
        .with_context(|| format!("branch not found: {}", input.path))?;

    let mut changed = false;

    if let Some(title) = input.title {
        branch.title = Some(title);
        changed = true;
    } else if input.clear_title {
        branch.title = None;
        changed = true;
    }

    if let Some(description) = input.description {
        branch.description = Some(description);
        changed = true;
    } else if input.clear_description {
        branch.description = None;
        changed = true;
    }

    if let Some(weight) = input.weight {
        // A non-finite weight is rejected by `repo.update`'s record validation.
        branch.weight = weight;
        changed = true;
    }

    // o2o is one-to-one, so editing *overwrites* a present name rather than
    // rejecting it (unlike `add`, where a repeated name is a usage error).
    for (name, value) in input.o2o {
        branch.o2o.insert(name, value);
        changed = true;
    }
    // Removing an absent o2o name is treated as a mistake (it is exact and
    // explicit), so report it rather than silently succeeding.
    for name in input.remove_o2o {
        if branch.o2o.remove(&name).is_none() {
            bail!("branch {} has no o2o name `{name}`", input.path);
        }
        changed = true;
    }

    // o2m is a set: add inserts (the store dedups on write) and remove drops a
    // single value, pruning the name once its last value is gone. Removing an
    // absent value is a harmless no-op, matching set semantics — but it does
    // not count as a change, so a patch made only of no-op removals is still
    // reported as "no changes requested" instead of rewriting the branch.
    for (name, value) in input.add_o2m {
        branch.o2m.entry(name).or_default().push(value);
        changed = true;
    }
    for (name, value) in input.remove_o2m {
        if let Some(values) = branch.o2m.get_mut(&name) {
            let before = values.len();
            values.retain(|v| v != &value);
            changed |= values.len() != before;
            if values.is_empty() {
                branch.o2m.remove(&name);
            }
        }
    }

    if !changed {
        bail!("no changes requested (see `unclip edit --help`)");
    }

    repo.update(branch).await?;
    crate::output::outln!("edited {}", input.path);
    Ok(())
}

/// `unclip rm <path> [--recursive]` — delete a branch or its whole subtree.
pub async fn rm(repo: &impl BranchRepository, path: &str, recursive: bool) -> anyhow::Result<()> {
    let path = path.trim_end_matches('/');

    // A recursive target is a *scope*: it need not exist as a branch itself
    // (`/tokyo` may only exist through `/tokyo/...` descendants). Deleting is
    // still explicit, so an empty scope is an error rather than a no-op.
    if recursive {
        let deleted = repo.delete_subtree(path).await?;
        if deleted == 0 {
            bail!("no branches under {path}");
        }
        crate::output::outln!("deleted {path} ({deleted} branch(es))");
        return Ok(());
    }

    // A non-recursive target is exact, so a missing branch is an error
    // (unlike the repository's idempotent `delete`), matching `edit`.
    if repo.get(path).await?.is_none() {
        bail!("branch not found: {path}");
    }

    // Paths are independent rows (`/a/b/c` can exist without `/a/b`), so probe
    // the scope for any strictly-descendant row rather than only children.
    let query = SampleQuery {
        under: Some(path.to_string()),
        ..Default::default()
    };
    let descendant = repo.find_page(&query, Some(path), 1).await?;
    if !descendant.is_empty() {
        bail!("branch {path} has descendants; pass --recursive to delete the subtree");
    }
    repo.delete(path).await?;
    crate::output::outln!("deleted {path}");
    Ok(())
}

/// `unclip rm-frame <name>` — delete a frame and its slots.
pub async fn rm_frame(repo: &impl FrameRepository, name: &str) -> anyhow::Result<()> {
    if repo.get_frame(name).await?.is_none() {
        bail!("frame not found: {name}");
    }
    repo.delete_frame(name).await?;
    crate::output::outln!("deleted frame {name}");
    Ok(())
}

pub async fn show(repo: &impl BranchReader, path: &str) -> anyhow::Result<()> {
    match repo.get(path).await? {
        Some(branch) => {
            crate::output::out!("{}", serde_norway::to_string(&branch)?);
            Ok(())
        }
        None => bail!("branch not found: {path}"),
    }
}

pub async fn ls(repo: &impl BranchReader, path: &str) -> anyhow::Result<()> {
    // Accept a trailing slash the same way `tree` does; paths are compared
    // without it, so `/a/` would otherwise silently match nothing.
    let path = path.trim_end_matches('/');
    // Scan the whole subtree (sharing `tree`'s bulk bound) rather than the
    // exact-parent index: a path segment that was never added as a branch
    // still scopes deeper branches, and must be listed to be discoverable.
    let descendants = repo.descendants(path).await?;

    // One entry per immediate child path; a child that is itself a branch
    // carries it, a child that only exists as a scope stays `None`.
    let mut entries: BTreeMap<String, Option<Branch>> = BTreeMap::new();
    for branch in descendants {
        let segment_end = branch.path[path.len() + 1..]
            .find('/')
            .map(|i| path.len() + 1 + i)
            .unwrap_or(branch.path.len());
        let child_path = branch.path[..segment_end].to_string();
        let is_direct_child = segment_end == branch.path.len();
        let entry = entries.entry(child_path).or_insert(None);
        if is_direct_child {
            *entry = Some(branch);
        }
    }

    if entries.is_empty() {
        crate::output::errln!("(no children under {})", display_scope(path));
        return Ok(());
    }
    for (child_path, branch) in entries {
        match branch {
            Some(Branch {
                title: Some(title), ..
            }) => crate::output::outln!("{child_path}\t{title}"),
            Some(_) => crate::output::outln!("{child_path}"),
            // Scope-only segment: the trailing slash marks a path that can be
            // listed further but not shown as a branch.
            None => crate::output::outln!("{child_path}/"),
        }
    }
    Ok(())
}

pub async fn tree(repo: &impl BranchReader, root: &str) -> anyhow::Result<()> {
    let root = root.trim_end_matches('/');
    let mut nodes = repo.descendants(root).await?;
    if let Some(node) = repo.get(root).await? {
        nodes.push(node);
    }
    if nodes.is_empty() {
        crate::output::errln!("(no branches under {})", display_scope(root));
        return Ok(());
    }

    // One display row per path, including scope-only segments between the
    // root and each branch — without them, siblings under different implicit
    // parents would render as if nested under the previous branch.
    let mut rows: BTreeMap<String, Option<Branch>> = BTreeMap::new();
    for node in nodes {
        for (i, _) in node.path.match_indices('/') {
            if i > root.len() {
                rows.entry(node.path[..i].to_string()).or_insert(None);
            }
        }
        rows.insert(node.path.clone(), Some(node));
    }

    let root_depth = segment_count(root);
    for (path, node) in rows {
        let depth = segment_count(&path).saturating_sub(root_depth);
        let indent = "  ".repeat(depth);
        let label = last_segment(&path);
        match node {
            Some(Branch {
                title: Some(title), ..
            }) => crate::output::outln!("{indent}{label}\t{title}"),
            Some(_) => crate::output::outln!("{indent}{label}"),
            None => crate::output::outln!("{indent}{label}/"),
        }
    }
    Ok(())
}

/// A scope path for messages: the bare root trims to an empty string and
/// would otherwise print as nothing.
fn display_scope(path: &str) -> &str {
    if path.is_empty() {
        "/"
    } else {
        path
    }
}

/// Arguments for `query`, assembled by clap in `main`.
pub struct QueryInput {
    pub under: Option<String>,
    /// Slot resolved from `--frame name.slot`, if given.
    pub frame_slot: Option<Slot>,
    pub require_o2o: Vec<(String, String)>,
    pub avoid_o2o: Vec<(String, String)>,
    pub require_o2m: Vec<(String, String)>,
    pub avoid_o2m: Vec<(String, String)>,
}

pub async fn query(repo: &impl BranchReader, input: QueryInput) -> anyhow::Result<()> {
    if let Some(under) = &input.under {
        validate_path(under).with_context(|| format!("invalid --under scope `{under}`"))?;
    }
    // A frame slot supplies the base query; explicit flags merge on top.
    let mut q = match &input.frame_slot {
        Some(slot) => SampleQuery::from_slot(slot, input.under.clone()),
        None => SampleQuery {
            under: input.under.clone(),
            ..Default::default()
        },
    };
    merge_o2o(&mut q.require_o2o, input.require_o2o)?;
    // avoid_o2o accumulates: several values of one name can be excluded at
    // once (matching avoid_o2m), unlike require_o2o which is one per name.
    for (name, value) in input.avoid_o2o {
        q.avoid_o2o.entry(name).or_default().push(value);
    }
    for (name, value) in input.require_o2m {
        q.require_o2m.entry(name).or_default().push(value);
    }
    for (name, value) in input.avoid_o2m {
        q.avoid_o2m.entry(name).or_default().push(value);
    }

    // Stream one page at a time instead of retaining the full result set, so
    // a broad query is bounded by the page size, not the archive size. Pages
    // arrive in path order, so no re-sort is needed.
    const QUERY_PAGE_SIZE: u64 = 1_000;
    let mut after_path: Option<String> = None;
    let mut any = false;
    loop {
        let page = repo
            .find_page(&q, after_path.as_deref(), QUERY_PAGE_SIZE)
            .await?;
        let done = (page.len() as u64) < QUERY_PAGE_SIZE;
        after_path = page.last().map(|branch| branch.path.clone());
        for branch in &page {
            any = true;
            print_path_line(branch)?;
        }
        if done {
            break;
        }
    }
    if !any {
        crate::output::errln!("(no matching branches)");
    }
    Ok(())
}

/// `unclip o2o [name | name=value]` — catalog or branch lookup over o2o.
pub async fn o2o(repo: &impl BranchReader, selector: Option<String>) -> anyhow::Result<()> {
    match parse_selector(selector)? {
        Selector::All => print_catalog(repo.o2o_catalog(None).await?)?,
        Selector::Name(name) => print_catalog(repo.o2o_catalog(Some(&name)).await?)?,
        Selector::Pair(name, value) => {
            print_branches(repo.branches_with_o2o(&name, &value).await?)?
        }
    }
    Ok(())
}

/// `unclip o2m [name | name=value]` — catalog or branch lookup over o2m.
pub async fn o2m(repo: &impl BranchReader, selector: Option<String>) -> anyhow::Result<()> {
    match parse_selector(selector)? {
        Selector::All => print_catalog(repo.o2m_catalog(None).await?)?,
        Selector::Name(name) => print_catalog(repo.o2m_catalog(Some(&name)).await?)?,
        Selector::Pair(name, value) => {
            print_branches(repo.branches_with_o2m(&name, &value).await?)?
        }
    }
    Ok(())
}

/// `unclip import <file>` — bulk import branches (upsert by path).
pub async fn import(repo: &impl BranchWriter, branches: Vec<Branch>) -> anyhow::Result<()> {
    if branches.is_empty() {
        crate::output::errln!("(no branches in file)");
        return Ok(());
    }
    // Reject a malformed file before any write, rather than partially importing.
    for branch in &branches {
        validate_branch_record(branch)
            .with_context(|| format!("invalid branch in import: {}", branch.path))?;
    }
    let (added, updated) = repo.upsert_many(branches).await?;
    crate::output::outln!(
        "imported {} branch(es): {added} added, {updated} updated",
        added + updated
    );
    Ok(())
}

/// `unclip attach <path> <value>` — attach a reference to a branch.
pub async fn attach(
    repo: &impl BranchWriter,
    path: &str,
    value: String,
    kind: Option<String>,
    note: Option<String>,
) -> anyhow::Result<()> {
    let kind = kind.unwrap_or_else(|| infer_reference_kind(&value));
    let reference = Reference {
        kind: kind.clone(),
        value: value.clone(),
        note,
    };
    repo.attach_reference(path, &reference).await?;
    crate::output::outln!("attached {kind} `{value}` to {path}");
    Ok(())
}

/// `unclip refs <path>` — list a branch's references.
pub async fn refs(repo: &impl BranchReader, path: &str) -> anyhow::Result<()> {
    let branch = repo
        .get(path)
        .await?
        .with_context(|| format!("branch not found: {path}"))?;
    if branch.references.is_empty() {
        crate::output::errln!("(no references on {path})");
        return Ok(());
    }
    for r in branch.references {
        match &r.note {
            Some(note) => crate::output::outln!("{}\t{}\t{}", r.kind, r.value, note),
            None => crate::output::outln!("{}\t{}", r.kind, r.value),
        }
    }
    Ok(())
}

/// Infer a reference type from its value: URLs vs. local files.
fn infer_reference_kind(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        "url".to_string()
    } else {
        "file".to_string()
    }
}

/// Import frames parsed from a frames file.
pub async fn import_frames(repo: &impl FrameRepository, frames: Vec<Frame>) -> anyhow::Result<()> {
    if frames.is_empty() {
        crate::output::errln!("(no frames in file)");
        return Ok(());
    }
    // Capture summaries before the batch consumes the frames, so the per-frame
    // output can be printed only after the whole import commits atomically.
    let summaries: Vec<(String, usize)> = frames
        .iter()
        .map(|frame| (frame.name.clone(), frame.slots.len()))
        .collect();
    repo.save_frames(frames).await?;
    for (name, slots) in summaries {
        crate::output::outln!("imported frame {name} ({slots} slot(s))");
    }
    Ok(())
}

/// `unclip frames` — list stored frames.
pub async fn frames_list(repo: &impl FrameRepository) -> anyhow::Result<()> {
    let frames = repo.list_frames().await?;
    if frames.is_empty() {
        crate::output::errln!("(no frames)");
        return Ok(());
    }
    for info in frames {
        match &info.description {
            Some(desc) => {
                crate::output::outln!("{}\t{} slot(s)\t{}", info.name, info.slot_count, desc)
            }
            None => crate::output::outln!("{}\t{} slot(s)", info.name, info.slot_count),
        }
    }
    Ok(())
}

/// `unclip frame <name>` or `unclip frame <name>.<slot>` — show as YAML.
pub async fn frame_show(repo: &impl FrameRepository, selector: &str) -> anyhow::Result<()> {
    let (frame_name, slot_name) = split_frame_selector(selector);
    let frame = repo
        .get_frame(frame_name)
        .await?
        .with_context(|| format!("frame not found: {frame_name}"))?;
    match slot_name {
        None => crate::output::out!("{}", serde_norway::to_string(&frame)?),
        Some(slot_name) => {
            let slot = frame
                .slot(slot_name)
                .with_context(|| format!("frame `{frame_name}` has no slot `{slot_name}`"))?;
            crate::output::out!("{}", serde_norway::to_string(slot)?);
        }
    }
    Ok(())
}

/// `unclip create <path> --frame <name.slot>` — create a skeleton branch.
pub async fn create(
    branch_repo: &impl BranchWriter,
    frame_repo: &impl FrameRepository,
    path: String,
    selector: &str,
) -> anyhow::Result<()> {
    validate_path(&path)?;
    let (frame_name, slot_name) = split_frame_selector(selector);
    let Some(slot_name) = slot_name else {
        bail!("create requires a frame.slot selector, e.g. story.place");
    };
    let frame = frame_repo
        .get_frame(frame_name)
        .await?
        .with_context(|| format!("frame not found: {frame_name}"))?;
    let slot = frame
        .slot(slot_name)
        .with_context(|| format!("frame `{frame_name}` has no slot `{slot_name}`"))?;

    // A duplicate path is reported atomically by the repository insert.
    let branch = slot.skeleton(&path);
    branch_repo.add(branch.clone()).await?;
    crate::output::outln!("created {path} from {selector}");
    crate::output::out!("{}", serde_norway::to_string(&branch)?);
    Ok(())
}

/// `unclip validate <target> --frame <selector>`.
///
/// `name.slot` validates a stored branch (by path); a frame-only selector
/// validates a packet file (by path on disk).
pub async fn validate(
    branch_repo: &impl BranchReader,
    frame_repo: &impl FrameRepository,
    target: &str,
    selector: &str,
) -> anyhow::Result<()> {
    let (frame_name, slot_name) = split_frame_selector(selector);
    let frame = frame_repo
        .get_frame(frame_name)
        .await?
        .with_context(|| format!("frame not found: {frame_name}"))?;

    let violations = match slot_name {
        Some(slot_name) => {
            let slot = frame
                .slot(slot_name)
                .with_context(|| format!("frame `{frame_name}` has no slot `{slot_name}`"))?;
            let branch = branch_repo
                .get(target)
                .await?
                .with_context(|| format!("branch not found: {target}"))?;
            validate_branch(slot, &branch)
        }
        None => {
            let text = unclip_io::read_text_file(std::path::Path::new(target), "packet file")?;
            let packet: SelectionPacket = serde_norway::from_str(&text)?;
            validate_packet(&frame, &packet)
        }
    };

    if violations.is_empty() {
        crate::output::outln!("OK: {target} satisfies {selector}");
        Ok(())
    } else {
        for reason in &violations {
            crate::output::errln!("- {reason}");
        }
        bail!("{} violation(s)", violations.len());
    }
}

enum Selector {
    All,
    Name(String),
    Pair(String, String),
}

fn parse_selector(selector: Option<String>) -> anyhow::Result<Selector> {
    match selector {
        None => Ok(Selector::All),
        Some(s) => match s.split_once('=') {
            Some((name, value)) => {
                if name.is_empty() {
                    bail!("empty name in `{s}`");
                }
                // Indexed values are never empty (the store rejects them), so
                // an empty value is a usage error, not a search for "".
                if value.is_empty() {
                    bail!("empty value in `{s}`");
                }
                Ok(Selector::Pair(name.to_string(), value.to_string()))
            }
            None => Ok(Selector::Name(s)),
        },
    }
}

fn print_catalog(rows: Vec<IndexedValue>) -> anyhow::Result<()> {
    if rows.is_empty() {
        crate::output::errln!("(no indexed values)");
        return Ok(());
    }
    for row in rows {
        crate::output::outln!("{}={}\t{}", row.name, row.value, row.count);
    }
    Ok(())
}

fn print_branches(mut branches: Vec<Branch>) -> anyhow::Result<()> {
    branches.sort_by(|a, b| a.path.cmp(&b.path));
    if branches.is_empty() {
        crate::output::errln!("(no matching branches)");
        return Ok(());
    }
    for branch in branches {
        print_path_line(&branch)?;
    }
    Ok(())
}

fn print_path_line(branch: &Branch) -> anyhow::Result<()> {
    match &branch.title {
        Some(title) => crate::output::outln!("{}\t{}", branch.path, title),
        None => crate::output::outln!("{}", branch.path),
    }
    Ok(())
}

fn segment_count(path: &str) -> usize {
    path.trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .count()
}

fn last_segment(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kv_splits_on_first_equals() {
        assert_eq!(
            parse_kv("place=cafe").unwrap(),
            ("place".to_string(), "cafe".to_string())
        );
        // Only the first `=` is a separator; later ones belong to the value.
        assert_eq!(
            parse_kv("expr=a=b").unwrap(),
            ("expr".to_string(), "a=b".to_string())
        );
    }

    #[test]
    fn parse_kv_rejects_missing_separator_or_empty_parts() {
        assert!(parse_kv("nope").is_err());
        assert!(parse_kv("=value").is_err());
        assert!(parse_kv("name=").is_err());
    }

    #[test]
    fn merge_o2o_inserts_distinct_names() {
        let mut map = BTreeMap::new();
        merge_o2o(
            &mut map,
            vec![
                ("place".into(), "cafe".into()),
                ("mood".into(), "calm".into()),
            ],
        )
        .unwrap();
        assert_eq!(map.get("place").map(String::as_str), Some("cafe"));
        assert_eq!(map.get("mood").map(String::as_str), Some("calm"));
    }

    #[test]
    fn merge_o2o_rejects_duplicate_name() {
        let mut map = BTreeMap::new();
        map.insert("place".to_string(), "cafe".to_string());
        // Colliding with an existing entry (e.g. a frame slot's base value) is a
        // usage error because o2o is one-to-one.
        let err = merge_o2o(&mut map, vec![("place".into(), "park".into())]).unwrap_err();
        assert!(err.to_string().contains("duplicate o2o name `place`"));
        // The colliding pair is rejected without touching the existing entry.
        assert_eq!(map.get("place").map(String::as_str), Some("cafe"));
    }

    #[test]
    fn merge_o2o_rejects_duplicate_within_same_batch() {
        let mut map = BTreeMap::new();
        let err = merge_o2o(
            &mut map,
            vec![("k".into(), "1".into()), ("k".into(), "2".into())],
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate o2o name `k`"));
    }

    #[test]
    fn parse_selector_maps_each_form() {
        assert!(matches!(parse_selector(None).unwrap(), Selector::All));
        assert!(matches!(
            parse_selector(Some("place".to_string())).unwrap(),
            Selector::Name(n) if n == "place"
        ));
        match parse_selector(Some("place=cafe".to_string())).unwrap() {
            Selector::Pair(name, value) => {
                assert_eq!(name, "place");
                assert_eq!(value, "cafe");
            }
            _ => panic!("expected a name=value pair"),
        }
    }

    #[test]
    fn parse_selector_rejects_empty_name_or_value() {
        assert!(parse_selector(Some("=cafe".to_string())).is_err());
        assert!(parse_selector(Some("place=".to_string())).is_err());
    }

    #[test]
    fn infer_reference_kind_distinguishes_urls_from_files() {
        assert_eq!(infer_reference_kind("https://example.com"), "url");
        assert_eq!(infer_reference_kind("http://example.com"), "url");
        assert_eq!(infer_reference_kind("./notes/plan.md"), "file");
        assert_eq!(infer_reference_kind("ftp://host/x"), "file");
    }

    #[test]
    fn segment_count_counts_non_empty_segments() {
        assert_eq!(segment_count("/a/b/c"), 3);
        assert_eq!(segment_count("/a"), 1);
        // Trailing slash and the bare root contribute no segments.
        assert_eq!(segment_count("/a/b/"), 2);
        assert_eq!(segment_count("/"), 0);
    }

    #[test]
    fn last_segment_returns_final_component() {
        assert_eq!(last_segment("/a/b/c"), "c");
        assert_eq!(last_segment("/a"), "a");
        // A trailing slash is ignored before taking the final component.
        assert_eq!(last_segment("/a/b/"), "b");
    }
}
