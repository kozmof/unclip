//! Pattern dictionary types.
//!
//! The data types `PatternEntry`/`PatternTarget` live in `unclip-core` (so the
//! store can persist them without depending on the matcher) and are re-exported
//! here for matcher-facing code. `PatternHit` — a match result with offsets —
//! is matcher-specific and defined here.

pub use unclip_core::{PatternEntry, PatternTarget};

/// A single match of a pattern within scanned text.
///
/// `start`/`end` are byte offsets into the original text passed to
/// [`crate::Matcher::scan`] and are guaranteed to be valid UTF-8 boundaries, so
/// callers may use them to slice that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternHit {
    pub pattern: String,
    pub start: usize,
    pub end: usize,
    pub target: PatternTarget,
}

/// A borrowed view of one hit, visited by [`crate::Matcher::for_each_hit`].
///
/// Repeated or overlapping patterns can produce far more hits than the text
/// is long; borrowing keeps that streaming path allocation-free, and visitors
/// clone only what they keep.
#[derive(Debug, Clone, Copy)]
pub struct HitRef<'a> {
    pub pattern: &'a str,
    pub start: usize,
    pub end: usize,
    pub target: &'a PatternTarget,
}
