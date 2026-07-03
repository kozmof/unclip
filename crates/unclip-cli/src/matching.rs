//! `scan`, `suggest-o2m`, and pattern-dictionary (`pattern add`/`patterns`)
//! handlers, built on the daachorse matcher.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context};
use unclip_core::validate_pattern_entry;
use unclip_match::{branch_text, suggest_o2m, Matcher, PatternEntry, PatternTarget};
use unclip_store::{BranchRepository, SeaOrmPatternRepository};

/// Build a matcher from database state: o2o/o2m catalogs, branch titles, and
/// user-defined pattern entries.
pub async fn build_matcher(
    branches: &impl BranchRepository,
    patterns: &SeaOrmPatternRepository,
) -> anyhow::Result<Matcher> {
    let mut entries: Vec<PatternEntry> = Vec::new();

    for v in branches.o2o_catalog(None).await? {
        entries.push(PatternEntry::new(
            v.value.clone(),
            PatternTarget::O2o {
                name: v.name,
                value: v.value,
            },
        ));
    }
    for v in branches.o2m_catalog(None).await? {
        entries.push(PatternEntry::new(
            v.value.clone(),
            PatternTarget::O2m {
                name: v.name,
                value: v.value,
            },
        ));
    }
    for (path, title) in branches.titles().await? {
        entries.push(PatternEntry::new(title, PatternTarget::Branch { path }));
    }

    entries.extend(patterns.all_enabled().await?);
    Matcher::build(entries)
}

/// `unclip scan <file>` — report which structured patterns appear in text.
pub async fn scan_cmd(
    branches: &impl BranchRepository,
    patterns: &SeaOrmPatternRepository,
    file: &Path,
) -> anyhow::Result<()> {
    let text = unclip_io::read_text_file(file, "scan file")?;
    let matcher = build_matcher(branches, patterns).await?;

    // Aggregate identical (pattern → target) hits with a count.
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for hit in matcher.scan(&text) {
        *counts
            .entry((hit.pattern, hit.target.describe()))
            .or_default() += 1;
    }

    if counts.is_empty() {
        crate::output::errln!("(no pattern hits)");
        return Ok(());
    }
    for ((pattern, target), n) in counts {
        crate::output::outln!("{pattern}\t→ {target}\t({n})");
    }
    Ok(())
}

/// `unclip suggest-o2m <path>` — suggest o2m values mentioned in a branch's
/// text that it does not already carry.
pub async fn suggest_o2m_cmd(
    branches: &impl BranchRepository,
    patterns: &SeaOrmPatternRepository,
    path: &str,
) -> anyhow::Result<()> {
    let branch = branches
        .get(path)
        .await?
        .with_context(|| format!("branch not found: {path}"))?;
    let matcher = build_matcher(branches, patterns).await?;

    let suggestions = suggest_o2m(&matcher, &branch_text(&branch), &branch.o2m);
    if suggestions.is_empty() {
        crate::output::errln!("(no o2m suggestions for {path})");
        return Ok(());
    }
    for (name, value) in suggestions {
        crate::output::outln!("{name}={value}");
    }
    Ok(())
}

/// Target options for `pattern add` (exactly one required).
pub struct PatternAddInput {
    pub pattern: String,
    pub o2m: Option<(String, String)>,
    pub o2o: Option<(String, String)>,
    pub branch: Option<String>,
    pub collapse: Option<String>,
}

pub async fn pattern_add_cmd(
    patterns: &SeaOrmPatternRepository,
    input: PatternAddInput,
) -> anyhow::Result<()> {
    let targets = [
        input.o2m.is_some(),
        input.o2o.is_some(),
        input.branch.is_some(),
        input.collapse.is_some(),
    ]
    .iter()
    .filter(|x| **x)
    .count();
    if targets != 1 {
        bail!("provide exactly one of --o2m, --o2o, --branch, --collapse");
    }

    let target = if let Some((name, value)) = input.o2m {
        PatternTarget::O2m { name, value }
    } else if let Some((name, value)) = input.o2o {
        PatternTarget::O2o { name, value }
    } else if let Some(path) = input.branch {
        PatternTarget::Branch { path }
    } else if let Some(path) = input.collapse {
        PatternTarget::CollapsePattern { path }
    } else {
        // Unreachable: the `targets != 1` guard above ensures exactly one is
        // Some. Return an error rather than panic if that invariant ever breaks.
        bail!("provide exactly one of --o2m, --o2o, --branch, --collapse");
    };

    let entry = PatternEntry::new(input.pattern.clone(), target);
    validate_pattern_entry(&entry)?;
    let id = patterns.add(&entry).await?;
    crate::output::outln!(
        "added pattern #{id}: {} → {}",
        input.pattern,
        entry.target.describe()
    );
    Ok(())
}

/// `unclip pattern remove <id>` — delete a pattern entry.
pub async fn pattern_remove_cmd(patterns: &SeaOrmPatternRepository, id: i64) -> anyhow::Result<()> {
    if patterns.remove(id).await? {
        crate::output::outln!("removed pattern #{id}");
        Ok(())
    } else {
        bail!("no pattern entry with id {id}")
    }
}

/// `unclip pattern enable|disable <id>` — toggle a pattern entry.
pub async fn pattern_set_enabled_cmd(
    patterns: &SeaOrmPatternRepository,
    id: i64,
    enabled: bool,
) -> anyhow::Result<()> {
    if patterns.set_enabled(id, enabled).await? {
        let state = if enabled { "enabled" } else { "disabled" };
        crate::output::outln!("{state} pattern #{id}");
        Ok(())
    } else {
        bail!("no pattern entry with id {id}")
    }
}

/// `unclip patterns` — list stored pattern entries.
pub async fn patterns_cmd(patterns: &SeaOrmPatternRepository) -> anyhow::Result<()> {
    let stored = patterns.list().await?;
    if stored.is_empty() {
        crate::output::errln!("(no pattern entries)");
        return Ok(());
    }
    for p in stored {
        let flag = if p.enabled { "" } else { "\t[disabled]" };
        crate::output::outln!(
            "{}\t{}\t→ {}{flag}",
            p.id,
            p.entry.pattern,
            p.entry.target.describe()
        );
    }
    Ok(())
}
