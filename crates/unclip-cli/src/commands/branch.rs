//! Branch mutation: `add`, `edit`, `rm`, `import`, `attach`, `refs`.

use std::collections::BTreeMap;

use anyhow::{bail, Context};
use unclip_core::{validate_branch_record, validate_path, Branch, Reference};
use unclip_store::{BranchReader, BranchRepository, BranchWriter};

use super::{merge_o2o, parse_kv};

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

    repo.add(&branch).await?;
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
///
/// Both paths are pure writes now that the leaf case does its own existence and
/// descendant checks inside the delete transaction, so this needs no read half.
pub async fn rm(repo: &impl BranchWriter, path: &str, recursive: bool) -> anyhow::Result<()> {
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

    // A non-recursive target is exact, so a missing branch is an error (unlike
    // the repository's idempotent `delete`), matching `edit`. The existence
    // check and the descendant probe belong to the delete's own transaction —
    // doing them here would leave a window in which a concurrent `add` could
    // orphan a descendant.
    repo.delete_leaf(path).await?;
    crate::output::outln!("deleted {path}");
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
    let reference = Reference { kind, value, note };
    repo.attach_reference(path, &reference).await?;
    crate::output::outln!(
        "attached {} `{}` to {path}",
        reference.kind,
        reference.value
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_reference_kind_distinguishes_urls_from_files() {
        assert_eq!(infer_reference_kind("https://example.com"), "url");
        assert_eq!(infer_reference_kind("http://example.com"), "url");
        assert_eq!(infer_reference_kind("./notes/plan.md"), "file");
        assert_eq!(infer_reference_kind("ftp://host/x"), "file");
    }
}
