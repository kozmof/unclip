//! Pattern dictionary types (DRAFT §18).
//!
//! The data types `PatternEntry`/`PatternTarget` live in `unclip-core` (so the
//! store can persist them without depending on the matcher) and are re-exported
//! here for matcher-facing code. `PatternHit` — a match result with offsets —
//! is matcher-specific and defined here.

pub use unclip_core::{PatternEntry, PatternTarget};

/// A single match of a pattern within scanned text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternHit {
    pub pattern: String,
    pub start: usize,
    pub end: usize,
    pub target: PatternTarget,
}
