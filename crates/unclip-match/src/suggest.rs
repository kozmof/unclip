//! o2m suggestion: scan a branch's free text for known o2m values it lacks.

use std::collections::{BTreeMap, BTreeSet};

use unclip_core::Branch;

use crate::dictionary::PatternTarget;
use crate::matcher::Matcher;

/// The scannable free text of a branch: title, description, and metadata.
///
/// o2o/o2m values are intentionally excluded — we want to discover qualities
/// mentioned in prose, not echo the existing index.
pub fn branch_text(branch: &Branch) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(title) = &branch.title {
        parts.push(title.clone());
    }
    if let Some(description) = &branch.description {
        parts.push(description.clone());
    }
    if !branch.metadata.is_null() {
        parts.push(branch.metadata.to_string());
    }
    parts.join("\n")
}

/// Suggest o2m `(name, value)` pairs found in `text` that `existing` lacks.
/// Results are de-duplicated and sorted.
pub fn suggest_o2m(
    matcher: &Matcher,
    text: &str,
    existing: &BTreeMap<String, Vec<String>>,
) -> Vec<(String, String)> {
    let mut found: BTreeSet<(String, String)> = BTreeSet::new();
    for hit in matcher.scan(text) {
        if let PatternTarget::O2m { name, value } = hit.target {
            let already = existing
                .get(&name)
                .is_some_and(|values| values.contains(&value));
            if !already {
                found.insert((name, value));
            }
        }
    }
    found.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::{PatternEntry, PatternTarget};

    fn matcher() -> Matcher {
        Matcher::build(vec![
            PatternEntry::new(
                "crowded",
                PatternTarget::O2m {
                    name: "density".into(),
                    value: "crowded".into(),
                },
            ),
            PatternEntry::new(
                "locker",
                PatternTarget::O2m {
                    name: "object".into(),
                    value: "locker".into(),
                },
            ),
        ])
        .unwrap()
    }

    #[test]
    fn suggests_only_missing_values() {
        let mut existing = BTreeMap::new();
        existing.insert("object".to_string(), vec!["locker".to_string()]);

        let text = "a crowded corridor with a locker";
        let suggestions = suggest_o2m(&matcher(), text, &existing);
        // `locker` already present; only `density=crowded` is suggested.
        assert_eq!(
            suggestions,
            vec![("density".to_string(), "crowded".to_string())]
        );
    }

    #[test]
    fn branch_text_includes_metadata() {
        let mut b = Branch::new("/x");
        b.title = Some("Coin Locker".into());
        b.metadata = serde_json::json!({ "note": "a crowded place" });
        let text = branch_text(&b);
        assert!(text.contains("Coin Locker"));
        assert!(text.contains("crowded"));
    }
}
