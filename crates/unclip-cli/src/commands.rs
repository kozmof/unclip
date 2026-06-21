//! Command handlers for the unclip CLI.

use std::collections::BTreeMap;

use anyhow::{bail, Context};
use unclip_core::{
    validate_branch, validate_packet, validate_path, Branch, Frame, Reference, SampleQuery,
    SelectionPacket, Slot,
};
use unclip_io::split_frame_selector;
use unclip_store::{BranchRepository, FrameRepository, IndexedValue};

/// Parse a `name=value` pair, used for `--o2o` / `--o2m` flags.
pub fn parse_kv(raw: &str) -> anyhow::Result<(String, String)> {
    let (name, value) = raw
        .split_once('=')
        .with_context(|| format!("expected name=value, got `{raw}`"))?;
    if name.is_empty() {
        bail!("empty name in `{raw}`");
    }
    Ok((name.to_string(), value.to_string()))
}

/// Merge `name=value` pairs into a one-to-one o2o map, rejecting any name that
/// is already present. o2o values are one-to-one (DRAFT §5), so a repeated name
/// — whether across flags or colliding with a frame slot's base value — is a
/// usage error.
pub fn merge_o2o(
    map: &mut BTreeMap<String, String>,
    pairs: Vec<(String, String)>,
) -> anyhow::Result<()> {
    for (name, value) in pairs {
        if map.insert(name.clone(), value).is_some() {
            bail!("duplicate o2o name `{name}` (o2o values are one-to-one)");
        }
    }
    Ok(())
}

/// Arguments for `add`, assembled by clap in `main`.
pub struct AddInput {
    pub path: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub weight: f64,
    pub o2o: Vec<(String, String)>,
    pub o2m: Vec<(String, String)>,
}

pub async fn add(repo: &impl BranchRepository, input: AddInput) -> anyhow::Result<()> {
    validate_path(&input.path)?;
    // A non-finite weight (NaN/inf) would poison the sampler's score sum, so
    // reject it at the boundary rather than persisting an unusable branch.
    if !input.weight.is_finite() {
        bail!("weight must be a finite number, got {}", input.weight);
    }
    if repo.get(&input.path).await?.is_some() {
        bail!("branch already exists: {}", input.path);
    }

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
    println!("added {}", input.path);
    Ok(())
}

pub async fn show(repo: &impl BranchRepository, path: &str) -> anyhow::Result<()> {
    match repo.get(path).await? {
        Some(branch) => {
            print!("{}", serde_yaml::to_string(&branch)?);
            Ok(())
        }
        None => bail!("branch not found: {path}"),
    }
}

pub async fn ls(repo: &impl BranchRepository, path: &str) -> anyhow::Result<()> {
    let mut children = repo.children(path).await?;
    children.sort_by(|a, b| a.path.cmp(&b.path));
    if children.is_empty() {
        eprintln!("(no children under {path})");
        return Ok(());
    }
    for child in children {
        match &child.title {
            Some(title) => println!("{}\t{}", child.path, title),
            None => println!("{}", child.path),
        }
    }
    Ok(())
}

pub async fn tree(repo: &impl BranchRepository, root: &str) -> anyhow::Result<()> {
    let root = root.trim_end_matches('/');
    let mut nodes = repo.descendants(root).await?;
    if let Some(node) = repo.get(root).await? {
        nodes.push(node);
    }
    if nodes.is_empty() {
        eprintln!("(no branches under {root})");
        return Ok(());
    }
    nodes.sort_by(|a, b| a.path.cmp(&b.path));

    let root_depth = segment_count(root);
    for node in nodes {
        let depth = segment_count(&node.path).saturating_sub(root_depth);
        let indent = "  ".repeat(depth);
        let label = last_segment(&node.path);
        match &node.title {
            Some(title) => println!("{indent}{label}\t{title}"),
            None => println!("{indent}{label}"),
        }
    }
    Ok(())
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

pub async fn query(repo: &impl BranchRepository, input: QueryInput) -> anyhow::Result<()> {
    // A frame slot supplies the base query; explicit flags merge on top.
    let mut q = match &input.frame_slot {
        Some(slot) => SampleQuery::from_slot(slot, input.under.clone()),
        None => SampleQuery {
            under: input.under.clone(),
            ..Default::default()
        },
    };
    merge_o2o(&mut q.require_o2o, input.require_o2o)?;
    // avoid_o2o is one value per name (DRAFT §5); reject a repeated name rather
    // than silently keeping the last, matching require_o2o.
    merge_o2o(&mut q.avoid_o2o, input.avoid_o2o)?;
    for (name, value) in input.require_o2m {
        q.require_o2m.entry(name).or_default().push(value);
    }
    for (name, value) in input.avoid_o2m {
        q.avoid_o2m.entry(name).or_default().push(value);
    }

    let mut found = repo.find(q).await?;
    found.sort_by(|a, b| a.path.cmp(&b.path));
    if found.is_empty() {
        eprintln!("(no matching branches)");
        return Ok(());
    }
    for branch in found {
        print_path_line(&branch);
    }
    Ok(())
}

/// `unclip o2o [name | name=value]` — catalog or branch lookup over o2o.
pub async fn o2o(repo: &impl BranchRepository, selector: Option<String>) -> anyhow::Result<()> {
    match parse_selector(selector)? {
        Selector::All => print_catalog(repo.o2o_catalog(None).await?),
        Selector::Name(name) => print_catalog(repo.o2o_catalog(Some(&name)).await?),
        Selector::Pair(name, value) => {
            print_branches(repo.branches_with_o2o(&name, &value).await?)
        }
    }
    Ok(())
}

/// `unclip o2m [name | name=value]` — catalog or branch lookup over o2m.
pub async fn o2m(repo: &impl BranchRepository, selector: Option<String>) -> anyhow::Result<()> {
    match parse_selector(selector)? {
        Selector::All => print_catalog(repo.o2m_catalog(None).await?),
        Selector::Name(name) => print_catalog(repo.o2m_catalog(Some(&name)).await?),
        Selector::Pair(name, value) => {
            print_branches(repo.branches_with_o2m(&name, &value).await?)
        }
    }
    Ok(())
}

/// `unclip import <file>` — bulk import branches (upsert by path).
pub async fn import(
    repo: &(impl BranchRepository + Sync),
    branches: Vec<Branch>,
) -> anyhow::Result<()> {
    if branches.is_empty() {
        eprintln!("(no branches in file)");
        return Ok(());
    }
    // Reject a malformed file before any write, rather than partially importing.
    for branch in &branches {
        validate_path(&branch.path)
            .with_context(|| format!("invalid branch in import: {}", branch.path))?;
    }
    let (added, updated) = repo.upsert_many(branches).await?;
    println!("imported {} branch(es): {added} added, {updated} updated", added + updated);
    Ok(())
}

/// `unclip attach <path> <value>` — attach a reference to a branch.
pub async fn attach(
    repo: &impl BranchRepository,
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
    println!("attached {kind} `{value}` to {path}");
    Ok(())
}

/// `unclip refs <path>` — list a branch's references.
pub async fn refs(repo: &impl BranchRepository, path: &str) -> anyhow::Result<()> {
    let branch = repo
        .get(path)
        .await?
        .with_context(|| format!("branch not found: {path}"))?;
    if branch.references.is_empty() {
        eprintln!("(no references on {path})");
        return Ok(());
    }
    for r in branch.references {
        match &r.note {
            Some(note) => println!("{}\t{}\t{}", r.kind, r.value, note),
            None => println!("{}\t{}", r.kind, r.value),
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
        eprintln!("(no frames in file)");
        return Ok(());
    }
    for frame in frames {
        let name = frame.name.clone();
        let slots = frame.slots.len();
        repo.save_frame(frame).await?;
        println!("imported frame {name} ({slots} slot(s))");
    }
    Ok(())
}

/// `unclip frames` — list stored frames.
pub async fn frames_list(repo: &impl FrameRepository) -> anyhow::Result<()> {
    let frames = repo.list_frames().await?;
    if frames.is_empty() {
        eprintln!("(no frames)");
        return Ok(());
    }
    for info in frames {
        match &info.description {
            Some(desc) => println!("{}\t{} slot(s)\t{}", info.name, info.slot_count, desc),
            None => println!("{}\t{} slot(s)", info.name, info.slot_count),
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
        None => print!("{}", serde_yaml::to_string(&frame)?),
        Some(slot_name) => {
            let slot = frame
                .slot(slot_name)
                .with_context(|| format!("frame `{frame_name}` has no slot `{slot_name}`"))?;
            print!("{}", serde_yaml::to_string(slot)?);
        }
    }
    Ok(())
}

/// `unclip create <path> --frame <name.slot>` — create a skeleton branch.
pub async fn create(
    branch_repo: &impl BranchRepository,
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

    if branch_repo.get(&path).await?.is_some() {
        bail!("branch already exists: {path}");
    }
    let branch = slot.skeleton(&path);
    branch_repo.add(branch.clone()).await?;
    println!("created {path} from {selector}");
    print!("{}", serde_yaml::to_string(&branch)?);
    Ok(())
}

/// `unclip validate <target> --frame <selector>`.
///
/// `name.slot` validates a stored branch (by path); a frame-only selector
/// validates a packet file (by path on disk).
pub async fn validate(
    branch_repo: &impl BranchRepository,
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
            let text = std::fs::read_to_string(target)
                .with_context(|| format!("cannot read packet file: {target}"))?;
            let packet: SelectionPacket = serde_yaml::from_str(&text)?;
            validate_packet(&frame, &packet)
        }
    };

    if violations.is_empty() {
        println!("OK: {target} satisfies {selector}");
        Ok(())
    } else {
        for reason in &violations {
            eprintln!("- {reason}");
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
                Ok(Selector::Pair(name.to_string(), value.to_string()))
            }
            None => Ok(Selector::Name(s)),
        },
    }
}

fn print_catalog(rows: Vec<IndexedValue>) {
    if rows.is_empty() {
        eprintln!("(no indexed values)");
        return;
    }
    for row in rows {
        println!("{}={}\t{}", row.name, row.value, row.count);
    }
}

fn print_branches(mut branches: Vec<Branch>) {
    branches.sort_by(|a, b| a.path.cmp(&b.path));
    if branches.is_empty() {
        eprintln!("(no matching branches)");
        return;
    }
    for branch in branches {
        print_path_line(&branch);
    }
}

fn print_path_line(branch: &Branch) {
    match &branch.title {
        Some(title) => println!("{}\t{}", branch.path, title),
        None => println!("{}", branch.path),
    }
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
