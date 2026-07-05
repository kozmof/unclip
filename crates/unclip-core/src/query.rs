use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::frame::Slot;

/// What to draw from: the hard scope/o2o/o2m filters plus the soft `prefer_o2m`
/// scoring signal. *How* to draw — count, weighting, recency — lives in
/// [`SampleParams`], so the type the store's `find` consumes carries only
/// fields it actually filters on (no sampling-only knobs riding along).
///
/// `Serialize` is derived so a query can be embedded verbatim in a packet's
/// `query` field; new fields are then carried automatically (no hand-written
/// JSON projection to keep in sync). `Deserialize` (with `serde(default)`, so
/// packets written before a field existed still parse) is what lets `replay`
/// reconstruct the filter from that provenance. Unknown keys are deliberately
/// tolerated: the packet's `query` object also carries the flattened
/// [`SampleParams`] fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SampleQuery {
    pub under: Option<String>,

    pub require_o2o: BTreeMap<String, String>,
    /// Excluded o2o values. Unlike `require_o2o` (one required value per
    /// name), several values of the same name can be avoided at once.
    pub avoid_o2o: BTreeMap<String, Vec<String>>,

    pub require_o2m: BTreeMap<String, Vec<String>>,
    pub prefer_o2m: BTreeMap<String, Vec<String>>,
    pub avoid_o2m: BTreeMap<String, Vec<String>>,
}

impl SampleQuery {
    /// Derive the filter from a frame slot, optionally overriding the scope.
    pub fn from_slot(slot: &Slot, under_override: Option<String>) -> Self {
        Self {
            under: under_override.or_else(|| slot.under.clone()),
            require_o2o: slot.require_o2o.clone(),
            avoid_o2o: slot.avoid_o2o.clone(),
            require_o2m: slot.require_o2m.clone(),
            prefer_o2m: slot.prefer_o2m.clone(),
            avoid_o2m: slot.avoid_o2m.clone(),
        }
    }
}

/// How to draw from the filtered candidates: how many, whether to weight by
/// `weight`, and whether to penalize recently-used branches. Separate from
/// [`SampleQuery`] so filter-only callers (`find`, `stats`, `stale`, `export`)
/// never have to invent a count. Deserializable for the same reason as
/// [`SampleQuery`]: `replay` parses both from one packet-provenance object.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SampleParams {
    pub count: usize,
    pub weighted: bool,
    pub avoid_recent: bool,
}

impl SampleParams {
    /// Derive the sampling controls from a frame slot.
    pub fn from_slot(slot: &Slot) -> Self {
        Self {
            count: slot.count,
            weighted: slot.weighted,
            avoid_recent: slot.avoid_recent,
        }
    }
}
