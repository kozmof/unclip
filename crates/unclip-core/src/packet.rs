use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::branch::Branch;

/// The current selection-packet schema version.
pub const PACKET_VERSION: u32 = 1;
/// The `kind` discriminator for selection packets.
pub const PACKET_KIND: &str = "unclip.selection";

/// A sampled, structured result. Editable, transformable, not a final prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPacket {
    pub version: u32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<serde_json::Value>,
    pub selections: Vec<Selection>,
}

/// A single selected branch, optionally bound to a frame slot.
///
/// `branch` is `Rc<Branch>` rather than `Branch` so a candidate drawn from a
/// pool shared across many packets (e.g. `compose`'s per-slot candidate set)
/// can be attached to each packet with a cheap refcount bump instead of a
/// deep clone of the whole aggregate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    pub branch: Rc<Branch>,
}

impl SelectionPacket {
    /// Construct an empty packet with the current version and kind.
    pub fn new(frame: Option<String>, seed: Option<u64>) -> Self {
        Self {
            version: PACKET_VERSION,
            kind: PACKET_KIND.to_string(),
            frame,
            seed,
            created_at: None,
            query: None,
            selections: Vec::new(),
        }
    }
}
