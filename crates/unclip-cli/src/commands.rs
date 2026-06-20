//! Command handlers for the unclip CLI.

use std::collections::BTreeMap;

use anyhow::{bail, Context};
use unclip_core::{Branch, SampleQuery};
use unclip_store::{BranchRepository, IndexedValue, SeaOrmBranchRepository};

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
    if repo.get(&input.path).await?.is_some() {
        bail!("branch already exists: {}", input.path);
    }

    let mut branch = Branch::new(&input.path);
    branch.title = input.title;
    branch.description = input.description;
    branch.weight = input.weight;

    for (name, value) in input.o2o {
        // o2o is one-to-one: a repeated name is a usage error (DRAFT §5).
        if branch.o2o.insert(name.clone(), value).is_some() {
            bail!("duplicate o2o name `{name}` (o2o values are one-to-one)");
        }
    }

    let mut o2m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in input.o2m {
        let bucket = o2m.entry(name).or_default();
        if !bucket.contains(&value) {
            bucket.push(value);
        }
    }
    for values in o2m.values_mut() {
        values.sort();
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
    pub require_o2o: Vec<(String, String)>,
    pub avoid_o2o: Vec<(String, String)>,
    pub avoid_o2m: Vec<(String, String)>,
}

pub async fn query(repo: &impl BranchRepository, input: QueryInput) -> anyhow::Result<()> {
    let mut q = SampleQuery {
        under: input.under,
        ..Default::default()
    };
    for (name, value) in input.require_o2o {
        if q.require_o2o.insert(name.clone(), value).is_some() {
            bail!("duplicate required o2o name `{name}` (o2o values are one-to-one)");
        }
    }
    for (name, value) in input.avoid_o2o {
        q.avoid_o2o.insert(name, value);
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
pub async fn o2o(repo: &SeaOrmBranchRepository, selector: Option<String>) -> anyhow::Result<()> {
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
pub async fn o2m(repo: &SeaOrmBranchRepository, selector: Option<String>) -> anyhow::Result<()> {
    match parse_selector(selector)? {
        Selector::All => print_catalog(repo.o2m_catalog(None).await?),
        Selector::Name(name) => print_catalog(repo.o2m_catalog(Some(&name)).await?),
        Selector::Pair(name, value) => {
            print_branches(repo.branches_with_o2m(&name, &value).await?)
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
