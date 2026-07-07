//! daachorse-backed multi-pattern scanner.
//!
//! The matcher is built from database-derived patterns ("Build
//! daachorse automata from database state. Do not make daachorse the
//! database."). Matching is case-insensitive (patterns and haystack are
//! lowercased), so reported offsets always refer to the original text.
//! Hits are word-boundary constrained, except next to CJK characters, whose
//! scripts have no word delimiters.

use std::collections::HashMap;

use daachorse::DoubleArrayAhoCorasick;

use crate::dictionary::{HitRef, PatternEntry, PatternHit};

/// Upper bound for distinct patterns compiled into one in-memory automaton.
///
/// Matcher inputs come from several database catalogs and may contain duplicate
/// pattern strings, so the limit is applied after grouping — a catalog that
/// dedups to a small automaton is not rejected for its raw entry count.
const MAX_PATTERN_ENTRIES: usize = 100_000;

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
            let i = match index.get(&key) {
                Some(&i) => i,
                None => {
                    let i = groups.len();
                    index.insert(key.clone(), i);
                    order.push(key);
                    groups.push(Vec::new());
                    i
                }
            };
            groups[i].push(entry);
        }

        anyhow::ensure!(
            order.len() <= MAX_PATTERN_ENTRIES,
            "matcher contains more than {MAX_PATTERN_ENTRIES} distinct patterns; narrow the archive before scanning"
        );
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
    /// Matches are constrained to word boundaries: a hit is rejected only when
    /// a neighbouring character and the adjacent pattern-edge character are
    /// both spaced-word characters (alphanumeric and not CJK). This stops
    /// short values from matching inside larger words (e.g. `red` inside
    /// `predator`, `tense` inside `intense`) while still allowing multi-word
    /// patterns and values separated by punctuation. CJK scripts have no
    /// spaces between words, so no boundary is required next to a CJK
    /// character — CJK patterns match as substrings of running text.
    /// Returned byte offsets are valid boundaries in the original input, even
    /// when Unicode lowercasing changes the haystack byte length.
    pub fn scan(&self, text: &str) -> Vec<PatternHit> {
        let mut hits = Vec::new();
        self.for_each_hit(text, |hit| {
            hits.push(PatternHit {
                pattern: hit.pattern.to_string(),
                start: hit.start,
                end: hit.end,
                target: hit.target.clone(),
            })
        });
        hits
    }

    /// Visit hits as they are found without retaining all of them in memory.
    ///
    /// Production consumers that aggregate results should prefer this method:
    /// repeated or overlapping patterns can otherwise make the number of hits
    /// much larger than the scanned text. Hits are borrowed views; a visitor
    /// clones only the parts it keeps.
    pub fn for_each_hit(&self, text: &str, mut visit: impl FnMut(HitRef<'_>)) {
        let Some(automaton) = &self.automaton else {
            return;
        };
        let (haystack, original_boundaries) = lowercase_with_original_boundaries(text);
        for m in automaton.find_overlapping_iter(&haystack) {
            if !at_word_boundary(&haystack, m.start(), m.end()) {
                continue;
            }
            // A lowercase expansion can create internal byte boundaries that
            // do not exist in the original character. Ignore such partial
            // matches rather than returning offsets that cannot slice `text`.
            let (Some(start), Some(end)) =
                (original_boundaries[m.start()], original_boundaries[m.end()])
            else {
                continue;
            };
            let group = &self.groups[m.value() as usize];
            for entry in group {
                visit(HitRef {
                    pattern: &entry.pattern,
                    start,
                    end,
                    target: &entry.target,
                });
            }
        }
    }
}

/// Lowercase text while mapping valid lowercase byte boundaries back to byte
/// boundaries in the original string.
fn lowercase_with_original_boundaries(text: &str) -> (String, Vec<Option<usize>>) {
    let mut lowered = String::with_capacity(text.len());
    let mut boundaries = vec![Some(0)];

    for (original_start, ch) in text.char_indices() {
        let original_end = original_start + ch.len_utf8();
        let lowered_start = lowered.len();
        lowered.extend(ch.to_lowercase());
        let lowered_end = lowered.len();

        boundaries.resize(lowered_end + 1, None);
        boundaries[lowered_start] = Some(original_start);
        boundaries[lowered_end] = Some(original_end);
    }

    (lowered, boundaries)
}

/// Whether the `[start, end)` byte range in `haystack` is delimited by word
/// boundaries. A side fails only when the neighbouring character and the
/// pattern-edge character next to it are both spaced-word characters; the
/// string edge is always a boundary.
fn at_word_boundary(haystack: &str, start: usize, end: usize) -> bool {
    let matched = &haystack[start..end];
    let boundary = |neighbour: Option<char>, edge: Option<char>| match (neighbour, edge) {
        (Some(n), Some(e)) => !(needs_word_boundary(n) && needs_word_boundary(e)),
        _ => true,
    };
    boundary(
        haystack[..start].chars().next_back(),
        matched.chars().next(),
    ) && boundary(haystack[end..].chars().next(), matched.chars().next_back())
}

/// Whether `c` belongs to a script that separates words with delimiters, so a
/// match edge touching it must fall on a word boundary. CJK characters do not
/// qualify: those scripts run words together, and requiring a boundary there
/// would reject every match inside CJK prose.
fn needs_word_boundary(c: char) -> bool {
    c.is_alphanumeric() && !is_cjk(c)
}

/// Heuristic membership test for the major CJK blocks (Han, kana, Hangul).
/// Not exhaustive — rare historic blocks are omitted — but misclassifying one
/// only re-imposes the word-boundary rule on that character.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{309F}'   // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{31F0}'..='\u{31FF}' // Katakana Phonetic Extensions
        | '\u{FF66}'..='\u{FF9D}' // Halfwidth Katakana
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{20000}'..='\u{2FA1F}' // CJK Extension B and beyond
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
    )
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
    fn cjk_patterns_match_inside_running_text() {
        let entries = vec![PatternEntry::new(
            "駅前",
            PatternTarget::O2m {
                name: "話題".into(),
                value: "交通".into(),
            },
        )];
        let m = Matcher::build(entries).unwrap();

        // Kana/kanji neighbours are not word boundaries in the ASCII sense;
        // the pattern must still match, and offsets must slice the original.
        let text = "夕方の駅前は混雑していた";
        let hits = m.scan(text);
        assert_eq!(hits.len(), 1);
        assert_eq!(&text[hits[0].start..hits[0].end], "駅前");
    }

    #[test]
    fn script_change_counts_as_a_boundary() {
        let entries = vec![
            PatternEntry::new(
                "red",
                PatternTarget::O2m {
                    name: "color".into(),
                    value: "red".into(),
                },
            ),
            PatternEntry::new(
                "駅前",
                PatternTarget::O2m {
                    name: "話題".into(),
                    value: "交通".into(),
                },
            ),
        ];
        let m = Matcher::build(entries).unwrap();

        // An ASCII pattern flanked by CJK, and a CJK pattern flanked by
        // ASCII, both match: the script change is the boundary.
        assert_eq!(m.scan("その日redを見た").len(), 1);
        assert_eq!(m.scan("a駅前b").len(), 1);

        // ASCII-in-ASCII substrings stay rejected.
        assert!(m.scan("predator").is_empty());
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

    #[test]
    fn offsets_slice_the_original_text_after_unicode_expansion() {
        let entries = vec![PatternEntry::new(
            "red",
            PatternTarget::O2m {
                name: "color".into(),
                value: "red".into(),
            },
        )];
        let matcher = Matcher::build(entries).unwrap();
        // U+0130 lowercases to two scalar values, changing the byte layout
        // before the match from two bytes to three.
        let text = "İ RED";
        let hit = &matcher.scan(text)[0];

        assert_eq!(&text[hit.start..hit.end], "RED");
        assert_eq!((hit.start, hit.end), (3, 6));
    }

    #[test]
    fn partial_matches_inside_a_lowercase_expansion_are_ignored() {
        let entry = PatternEntry::new("i", PatternTarget::Branch { path: "/i".into() });
        let matcher = Matcher::build(vec![entry]).unwrap();

        assert!(matcher.scan("İ").is_empty());
    }

    #[test]
    fn rejects_an_unbounded_pattern_dictionary() {
        let entries: Vec<PatternEntry> = (0..=MAX_PATTERN_ENTRIES)
            .map(|i| {
                PatternEntry::new(format!("p{i}"), PatternTarget::Branch { path: "/x".into() })
            })
            .collect();
        let error = Matcher::build(entries)
            .err()
            .expect("limit error")
            .to_string();

        assert!(error.contains("more than 100000 distinct patterns"));
    }

    #[test]
    fn duplicate_patterns_do_not_count_against_the_limit() {
        // The same string many times dedups to one automaton pattern, so an
        // entry count above the limit is fine when the distinct count is not.
        let entry = PatternEntry::new("x", PatternTarget::Branch { path: "/x".into() });
        let entries = vec![entry; MAX_PATTERN_ENTRIES + 1];
        let matcher = Matcher::build(entries).unwrap();
        assert!(!matcher.is_empty());
    }
}
