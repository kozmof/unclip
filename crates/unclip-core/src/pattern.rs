//! Pattern-dictionary data types.
//!
//! These are plain domain values describing how a matched text pattern maps to
//! a structured target. They live in core (not in `unclip-match`) so the store
//! can persist them without depending on the matching engine; `unclip-match`
//! re-exports them for matcher-facing code.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::validate::{validate_path, MAX_DOMAIN_STRING_BYTES};

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

/// Validate a pattern dictionary entry before persistence or matching.
pub fn validate_pattern_entry(entry: &PatternEntry) -> Result<()> {
    let invalid = |reason: &str| CoreError::InvalidPattern(reason.to_string());

    if entry.pattern.trim().is_empty() {
        return Err(invalid("pattern must not be empty or whitespace-only"));
    }
    if entry.pattern.len() > MAX_DOMAIN_STRING_BYTES {
        return Err(invalid("pattern is oversized"));
    }
    if entry.pattern.chars().any(char::is_control) {
        return Err(invalid("pattern must not contain control characters"));
    }

    match &entry.target {
        PatternTarget::O2m { name, value } | PatternTarget::O2o { name, value } => {
            if name.is_empty()
                || name.len() > MAX_DOMAIN_STRING_BYTES
                || name.chars().any(char::is_control)
            {
                return Err(invalid(
                    "target name must not be empty or contain control characters",
                ));
            }
            if value.is_empty()
                || value.len() > MAX_DOMAIN_STRING_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(invalid(
                    "target value must not be empty or contain control characters",
                ));
            }
        }
        PatternTarget::Branch { path } | PatternTarget::CollapsePattern { path } => {
            validate_path(path)
                .map_err(|_| invalid("branch target must be a valid absolute branch path"))?;
        }
    }
    Ok(())
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
}

/// A text pattern mapped to a structured target.
#[derive(Debug, Clone, PartialEq, Eq)]
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
