use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::reference::Reference;

/// A branch is an addressable possibility in the archive.
///
/// `BTreeMap` is used for o2o/o2m so that YAML/JSON output is deterministically
/// ordered, which keeps round-trips and golden tests stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    /// Database id. Never part of the serialized representation.
    #[serde(skip)]
    pub id: Option<i64>,

    pub path: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// One-to-one indexed values: exactly one value per name per branch.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub o2o: BTreeMap<String, String>,

    /// One-to-many indexed values: many values per name per branch.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub o2m: BTreeMap<String, Vec<String>>,

    #[serde(default = "default_weight")]
    pub weight: f64,

    /// Rich payload carried into output packets. Not indexed.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<Reference>,
}

pub(crate) fn default_weight() -> f64 {
    1.0
}

impl Branch {
    /// Create a minimal branch addressed only by its path.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            id: None,
            path: path.into(),
            title: None,
            description: None,
            o2o: BTreeMap::new(),
            o2m: BTreeMap::new(),
            weight: default_weight(),
            metadata: serde_json::Value::Null,
            references: Vec::new(),
        }
    }

    /// The parent scope of this branch, if any.
    ///
    /// `/a/b/c` -> `/a/b`; a top-level branch like `/a` has no parent.
    pub fn parent_path(&self) -> Option<String> {
        parent_of(&self.path)
    }
}

/// Compute the parent path of a slash-separated scope address.
pub fn parent_of(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let idx = trimmed.rfind('/')?;
    if idx == 0 {
        // e.g. "/a" -> top level, no parent.
        None
    } else {
        Some(trimmed[..idx].to_string())
    }
}
