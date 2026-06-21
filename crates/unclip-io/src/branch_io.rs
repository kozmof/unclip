//! Branch import and export in YAML / JSON / JSONL.
//!
//! Import accepts either a top-level sequence of branches or a mapping with a
//! `branches:` key. JSONL files (one branch per line) are detected by the
//! `.jsonl` extension. Export mirrors these shapes so a scope round-trips.

use std::path::Path;

use serde::{Deserialize, Serialize};
use unclip_core::Branch;

use crate::packet::Format;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BranchesIn {
    Wrapped { branches: Vec<Branch> },
    List(Vec<Branch>),
}

#[derive(Debug, Serialize)]
struct BranchesOut<'a> {
    branches: &'a [Branch],
}

/// Parse a YAML/JSON branches document (list or `branches:`-wrapped).
pub fn parse_branches(text: &str) -> anyhow::Result<Vec<Branch>> {
    let parsed: BranchesIn = serde_yaml::from_str(text)?;
    Ok(match parsed {
        BranchesIn::Wrapped { branches } => branches,
        BranchesIn::List(branches) => branches,
    })
}

/// Parse a JSONL branches document (one branch per non-empty line).
pub fn parse_branches_jsonl(text: &str) -> anyhow::Result<Vec<Branch>> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| serde_json::from_str::<Branch>(line).map_err(Into::into))
        .collect()
}

/// Load branches from a file, choosing JSONL when the extension says so.
pub fn load_branches_file(path: &Path) -> anyhow::Result<Vec<Branch>> {
    let text = std::fs::read_to_string(path)?;
    if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
        parse_branches_jsonl(&text)
    } else {
        parse_branches(&text)
    }
}

/// Render branches for export.
///
/// - YAML / JSON: a `branches:`-wrapped document.
/// - JSONL: one branch per line.
pub fn render_branches(branches: &[Branch], format: Format) -> anyhow::Result<String> {
    Ok(match format {
        Format::Yaml => serde_yaml::to_string(&BranchesOut { branches })?,
        Format::Json => format!(
            "{}\n",
            serde_json::to_string_pretty(&BranchesOut { branches })?
        ),
        Format::Jsonl => {
            let mut out = String::new();
            for branch in branches {
                out.push_str(&serde_json::to_string(branch)?);
                out.push('\n');
            }
            out
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Branch> {
        let mut a = Branch::new("/ikebukuro/station/coin-locker");
        a.title = Some("Locker".into());
        a.o2o.insert("domain".into(), "story".into());
        a.o2m.insert("topic".into(), vec!["locker".into()]);
        let b = Branch::new("/ikebukuro/last-train");
        vec![a, b]
    }

    #[test]
    fn accepts_wrapped_and_list() {
        let wrapped = "branches:\n  - path: /a\n  - path: /b\n";
        assert_eq!(parse_branches(wrapped).unwrap().len(), 2);
        let list = "- path: /a\n- path: /b\n";
        assert_eq!(parse_branches(list).unwrap().len(), 2);
    }

    #[test]
    fn yaml_export_import_roundtrip() {
        let branches = sample();
        let text = render_branches(&branches, Format::Yaml).unwrap();
        let back = parse_branches(&text).unwrap();
        assert_eq!(back, branches);
    }

    #[test]
    fn jsonl_export_import_roundtrip() {
        let branches = sample();
        let text = render_branches(&branches, Format::Jsonl).unwrap();
        assert_eq!(text.lines().count(), 2);
        let back = parse_branches_jsonl(&text).unwrap();
        assert_eq!(back, branches);
    }
}
