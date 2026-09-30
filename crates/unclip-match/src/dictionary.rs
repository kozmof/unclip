//! Pattern dictionary types.
//!
//! `PatternEntry`/`PatternTarget` are plain data: a text pattern and the
//! structured target a match maps to. They live here, at the bottom of the
//! tree, rather than in `unclip-core`, because both subsystems match text and
//! only one of them owns the branch archive.
//!
//! They were in `unclip-core`, which made this crate depend on it, which put
//! the possibility space's foundation underneath the leveling stack:
//! `unclip-infer` matches patterns, so `unclip-engine` linked `unclip-core` to
//! reach one `&str` per hit. The types are no larger here and the arrow now
//! runs the other way — `unclip-core` re-exports them and keeps
//! `validate_pattern_entry`, because validating a `Branch` target against the
//! branch-path rules is core's business, not the matcher's.

use serde::{Deserialize, Serialize};

/// Where a matched text pattern maps to in the structured model.
///
/// `Hash`/`Ord` are derived so scan results can be aggregated in a map keyed by
/// `&PatternTarget` borrowed straight from the matcher, instead of by an owned
/// `describe()` string rebuilt for every hit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PatternTarget {
    O2m {
        name: String,
        value: String,
    },
    O2o {
        name: String,
        value: String,
    },
    Branch {
        path: String,
    },
    /// Reserved: a pattern that collapses a match down to a branch reference.
    /// It can be stored and is surfaced by `scan`, but no automatic collapse
    /// behavior is implemented yet — it carries no special matching semantics
    /// beyond being reported.
    CollapsePattern {
        path: String,
    },
}

impl PatternTarget {
    /// Short, stable label for display (`o2m`, `o2o`, `branch`, `collapse`).
    pub fn kind_label(&self) -> &'static str {
        match self {
            PatternTarget::O2m { .. } => "o2m",
            PatternTarget::O2o { .. } => "o2o",
            PatternTarget::Branch { .. } => "branch",
            PatternTarget::CollapsePattern { .. } => "collapse",
        }
    }

    /// Human-readable target, e.g. `o2m topic=locker` or `branch /a/b`.
    pub fn describe(&self) -> String {
        match self {
            PatternTarget::O2m { name, value } => format!("o2m {name}={value}"),
            PatternTarget::O2o { name, value } => format!("o2o {name}={value}"),
            PatternTarget::Branch { path } => format!("branch {path}"),
            PatternTarget::CollapsePattern { path } => format!("collapse {path}"),
        }
    }

    /// The matched side of the target: an indexed value, or a branch path.
    ///
    /// Callers that treat every target uniformly — the pattern inferrer reports
    /// what a hit *said*, not where it would be filed — need one string per
    /// target without re-deriving the match per variant.
    pub fn value(&self) -> &str {
        match self {
            PatternTarget::O2m { value, .. } | PatternTarget::O2o { value, .. } => value,
            PatternTarget::Branch { path } | PatternTarget::CollapsePattern { path } => path,
        }
    }
}

/// A text pattern mapped to a structured target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternEntry {
    pub pattern: String,
    pub target: PatternTarget,
}

impl PatternEntry {
    pub fn new(pattern: impl Into<String>, target: PatternTarget) -> Self {
        Self {
            pattern: pattern.into(),
            target,
        }
    }
}

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
