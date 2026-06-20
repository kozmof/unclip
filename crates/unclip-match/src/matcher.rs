//! daachorse-backed multi-pattern scanner.
//!
//! The matcher is built from database-derived patterns (DRAFT §17: "Build
//! daachorse automata from database state. Do not make daachorse the
//! database."). Matching is case-insensitive (patterns and haystack are
//! lowercased), so reported offsets are into the lowercased text.

use std::collections::HashMap;

use daachorse::DoubleArrayAhoCorasick;

use crate::dictionary::{PatternEntry, PatternHit};

/// A compiled multi-pattern matcher.
pub struct Matcher {
    automaton: Option<DoubleArrayAhoCorasick<u32>>,
    /// Entries grouped by (lowercased) pattern; index aligns with automaton
    /// values, so several entries may share one pattern string.
    groups: Vec<Vec<PatternEntry>>,
}

impl Matcher {
    /// Build a matcher from pattern entries. Empty patterns are ignored, and
    /// entries that share a pattern string are grouped (daachorse rejects
    /// duplicate patterns).
    pub fn build(entries: Vec<PatternEntry>) -> anyhow::Result<Self> {
        let mut order: Vec<String> = Vec::new();
        let mut groups: Vec<Vec<PatternEntry>> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();

        for entry in entries {
            if entry.pattern.trim().is_empty() {
                continue;
            }
            let key = entry.pattern.to_lowercase();
            let i = *index.entry(key.clone()).or_insert_with(|| {
                order.push(key);
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[i].push(entry);
        }

        let automaton = if order.is_empty() {
            None
        } else {
            Some(
                DoubleArrayAhoCorasick::new(order)
                    .map_err(|e| anyhow::anyhow!("failed to build matcher: {e}"))?,
            )
        };
        Ok(Self { automaton, groups })
    }

    /// Whether the matcher has no patterns.
    pub fn is_empty(&self) -> bool {
        self.automaton.is_none()
    }

    /// Find all (overlapping) pattern hits in `text`.
    pub fn scan(&self, text: &str) -> Vec<PatternHit> {
        let Some(automaton) = &self.automaton else {
            return Vec::new();
        };
        let haystack = text.to_lowercase();
        let mut hits = Vec::new();
        for m in automaton.find_overlapping_iter(&haystack) {
            let group = &self.groups[m.value() as usize];
            for entry in group {
                hits.push(PatternHit {
                    pattern: entry.pattern.clone(),
                    start: m.start(),
                    end: m.end(),
                    target: entry.target.clone(),
                });
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::PatternTarget;

    #[test]
    fn empty_matcher_scans_nothing() {
        let m = Matcher::build(vec![]).unwrap();
        assert!(m.is_empty());
        assert!(m.scan("anything").is_empty());
    }

    #[test]
    fn case_insensitive_overlapping_hits() {
        let entries = vec![
            PatternEntry::new(
                "coin locker",
                PatternTarget::O2m {
                    name: "object".into(),
                    value: "locker".into(),
                },
            ),
            PatternEntry::new(
                "red",
                PatternTarget::O2m {
                    name: "color.dominant".into(),
                    value: "red".into(),
                },
            ),
        ];
        let m = Matcher::build(entries).unwrap();
        let hits = m.scan("A RED Coin Locker by the wall");
        let targets: Vec<_> = hits.iter().map(|h| h.target.describe()).collect();
        assert!(targets.contains(&"o2m object=locker".to_string()));
        assert!(targets.contains(&"o2m color.dominant=red".to_string()));
    }

    #[test]
    fn duplicate_patterns_grouped() {
        // Same text mapped to two different targets.
        let entries = vec![
            PatternEntry::new(
                "locker",
                PatternTarget::O2m {
                    name: "object".into(),
                    value: "locker".into(),
                },
            ),
            PatternEntry::new(
                "locker",
                PatternTarget::O2o {
                    name: "axis".into(),
                    value: "place".into(),
                },
            ),
        ];
        let m = Matcher::build(entries).unwrap();
        let hits = m.scan("the locker");
        assert_eq!(hits.len(), 2);
    }
}
