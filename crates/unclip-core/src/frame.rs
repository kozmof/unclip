use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Branch;

/// A frame is a reusable constraint set composed of named slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub slots: Vec<Slot>,
}

/// A single slot within a frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Hard o2o exclusions. Several values of one name can be avoided at once
    /// (a branch carries at most one of them, so avoiding many only widens the
    /// exclusion) — matching `avoid_o2m` and the query's `avoid_o2o` shape.
    /// For frame files written before this was a list, a bare string value is
    /// still accepted and read as a one-element list.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "one_or_many_map"
    )]
    pub avoid_o2o: BTreeMap<String, Vec<String>>,

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

/// Deserialize a map whose values are either one string or a list of strings,
/// normalizing to a list. Keeps pre-list `avoid_o2o: {name: value}` frame
/// files parseable while the canonical (serialized) shape is always a list.
fn one_or_many_map<'de, D>(deserializer: D) -> Result<BTreeMap<String, Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let raw: BTreeMap<String, OneOrMany> = BTreeMap::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|(name, values)| {
            let values = match values {
                OneOrMany::One(value) => vec![value],
                OneOrMany::Many(values) => values,
            };
            (name, values)
        })
        .collect())
}

/// `Default` mirrors the serde defaults: every constraint map empty and
/// `count` = 1. The empty `name` is not a valid persisted slot (the store
/// rejects it); the impl exists so constructors and tests can write
/// `Slot { name: ..., ..Default::default() }` instead of listing every field.
impl Default for Slot {
    fn default() -> Self {
        Self {
            name: String::new(),
            under: None,
            require_o2o: BTreeMap::new(),
            default_o2o: BTreeMap::new(),
            avoid_o2o: BTreeMap::new(),
            require_o2m: BTreeMap::new(),
            prefer_o2m: BTreeMap::new(),
            avoid_o2m: BTreeMap::new(),
            count: default_count(),
            avoid_recent: false,
            weighted: false,
            metadata_suggest: Vec::new(),
        }
    }
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
