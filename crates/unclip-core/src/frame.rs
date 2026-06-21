use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Branch;

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

    /// Hard o2m requirements: a candidate must carry every listed value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub require_o2m: BTreeMap<String, Vec<String>>,
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

impl Slot {
    /// Build a skeleton branch for this slot (`create --frame`).
    ///
    /// The skeleton seeds o2o from `require_o2o` plus `default_o2o`, leaves o2m
    /// empty, and adds each `metadata_suggest` field as a null placeholder for
    /// the author to fill in.
    pub fn skeleton(&self, path: impl Into<String>) -> Branch {
        let mut branch = Branch::new(path);
        for (name, value) in &self.require_o2o {
            branch.o2o.insert(name.clone(), value.clone());
        }
        for (name, value) in &self.default_o2o {
            branch.o2o.insert(name.clone(), value.clone());
        }
        if !self.metadata_suggest.is_empty() {
            let map: serde_json::Map<String, serde_json::Value> = self
                .metadata_suggest
                .iter()
                .map(|k| (k.clone(), serde_json::Value::Null))
                .collect();
            branch.metadata = serde_json::Value::Object(map);
        }
        branch
    }
}
