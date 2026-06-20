use std::collections::BTreeMap;

use crate::frame::Slot;

/// An independent set of filters used by sampling and composition.
#[derive(Debug, Clone, Default)]
pub struct SampleQuery {
    pub under: Option<String>,

    pub require_o2o: BTreeMap<String, String>,
    pub avoid_o2o: BTreeMap<String, String>,

    pub prefer_o2m: BTreeMap<String, Vec<String>>,
    pub avoid_o2m: BTreeMap<String, Vec<String>>,

    pub avoid_recent: bool,
    pub weighted: bool,
    pub count: usize,
}

impl SampleQuery {
    /// Derive a query from a frame slot, optionally overriding the scope.
    pub fn from_slot(slot: &Slot, under_override: Option<String>) -> Self {
        Self {
            under: under_override.or_else(|| slot.under.clone()),
            require_o2o: slot.require_o2o.clone(),
            avoid_o2o: slot.avoid_o2o.clone(),
            prefer_o2m: slot.prefer_o2m.clone(),
            avoid_o2m: slot.avoid_o2m.clone(),
            avoid_recent: slot.avoid_recent,
            weighted: slot.weighted,
            count: slot.count,
        }
    }
}
