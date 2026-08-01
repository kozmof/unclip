//! Read-only views of the archive: `show`, `ls`, `tree`, `query`, `o2o`, `o2m`.

use std::collections::BTreeMap;

use anyhow::{bail, Context};
use unclip_core::{validate_path, Branch, SampleQuery, Slot};
use unclip_store::{BranchReader, IndexedValue, PageCursor};

use super::{merge_o2o, print_path_line};

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
    //
    // The scope is stripped rather than sliced by byte offset: `descendants`
    // only ever returns strict descendants today, but an indexing bug here
    // would be a panic, and `strip_prefix` keeps that invariant local.
    let mut entries: BTreeMap<String, Option<Branch>> = BTreeMap::new();
    for branch in descendants {
        let Some(rest) = branch
            .path
            .strip_prefix(path)
            .and_then(|rest| rest.strip_prefix('/'))
        else {
            continue;
        };
        let segment_end = rest
            .find('/')
            .map_or(branch.path.len(), |i| branch.path.len() - rest.len() + i);
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
    let mut q = match input.frame_slot {
        Some(slot) => SampleQuery::from_slot(slot, input.under),
        None => SampleQuery {
            under: input.under,
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
    let mut pages = PageCursor::new();
    let mut any = false;
    while let Some(page) = pages.next(repo, &q).await? {
        for branch in &page {
            any = true;
            print_path_line(branch)?;
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
