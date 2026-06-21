//! daachorse-backed multi-pattern scanner.
//!
//! The matcher is built from database-derived patterns ("Build
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
    ///
    /// Matches are constrained to word boundaries: a hit counts only when the
    /// characters immediately before and after it are non-alphanumeric (or the
    /// string edge). This stops short values from matching inside larger words
    /// (e.g. `red` inside `predator`, `tense` inside `intense`) while still
    /// allowing multi-word patterns and values separated by punctuation.
    pub fn scan(&self, text: &str) -> Vec<PatternHit> {
        let Some(automaton) = &self.automaton else {
            return Vec::new();
        };
        let haystack = text.to_lowercase();
        let mut hits = Vec::new();
        for m in automaton.find_overlapping_iter(&haystack) {
            if !at_word_boundary(&haystack, m.start(), m.end()) {
                continue;
            }
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

/// Whether the `[start, end)` byte range in `haystack` is delimited by word
/// boundaries — i.e. the neighbouring characters are not alphanumeric.
fn at_word_boundary(haystack: &str, start: usize, end: usize) -> bool {
    let before_ok = haystack[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric());
    let after_ok = haystack[end..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric());
    before_ok && after_ok
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
    fn only_matches_on_word_boundaries() {
        let entries = vec![
            PatternEntry::new(
                "red",
                PatternTarget::O2m {
                    name: "color".into(),
                    value: "red".into(),
                },
            ),
            PatternEntry::new(
                "tense",
                PatternTarget::O2m {
                    name: "mood".into(),
                    value: "tense".into(),
                },
            ),
        ];
        let m = Matcher::build(entries).unwrap();

        // Substrings inside larger words must not match.
        assert!(m.scan("a predator in the intense dark").is_empty());

        // Whole words, and words bounded by punctuation, do match.
        assert_eq!(m.scan("a RED, tense room").len(), 2);
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
