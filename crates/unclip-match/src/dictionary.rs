//! Pattern dictionary types.
//!
//! The data types `PatternEntry`/`PatternTarget` live in `unclip-core` (so the
//! store can persist them without depending on the matcher) and are re-exported
//! here for matcher-facing code. `PatternHit` — a match result with offsets —
//! is matcher-specific and defined here.

pub use unclip_core::{PatternEntry, PatternTarget};

/// A single match of a pattern within scanned text.
///
/// `start`/`end` are byte offsets into the **lowercased** haystack
/// (`text.to_lowercase()`), not the original input. For most text the two share
/// a byte layout, but `to_lowercase` can change a string's length for some
/// Unicode characters, so these offsets must not be used to slice the original
/// `text`. They are only guaranteed valid against the lowercased haystack (which
/// is how `Matcher::scan` uses them internally, for word-boundary checks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternHit {
    pub pattern: String,
    pub start: usize,
    pub end: usize,
    pub target: PatternTarget,
}
