//! Pattern dictionary types (DRAFT §18).

use serde::{Deserialize, Serialize};

/// Where a matched text pattern maps to in the structured model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PatternTarget {
    O2m { name: String, value: String },
    O2o { name: String, value: String },
    Branch { path: String },
    CollapsePattern { path: String },
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

/// A single match of a pattern within scanned text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternHit {
    pub pattern: String,
    pub start: usize,
    pub end: usize,
    pub target: PatternTarget,
}
