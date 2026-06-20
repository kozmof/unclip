use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A frame is a reusable constraint set composed of named slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub slots: Vec<Slot>,
}

/// A single slot within a frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub name: String,

    /// Optional path-scope restriction for candidates of this slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<String>,

    /// Hard one-to-one requirements.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub require_o2o: BTreeMap<String, String>,
    /// o2o values added during data creation from this slot.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub default_o2o: BTreeMap<String, String>,
    /// Hard o2o exclusions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub avoid_o2o: BTreeMap<String, String>,

    /// o2m values that increase score.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prefer_o2m: BTreeMap<String, Vec<String>>,
    /// o2m values that exclude candidates.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub avoid_o2m: BTreeMap<String, Vec<String>>,

    #[serde(default = "default_count")]
    pub count: usize,
    #[serde(default)]
    pub avoid_recent: bool,
    #[serde(default)]
    pub weighted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata_suggest: Vec<String>,
}

fn default_count() -> usize {
    1
}

impl Frame {
    /// Look up a slot by name.
    pub fn slot(&self, name: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.name == name)
    }
}
