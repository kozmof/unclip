//! Versioned semantic domains and measurement coordinate frames.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unclip_epistemic::{DomainVersion, FrameVersion};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
        }
    };
}

string_id!(DomainId);
string_id!(UnitId);
string_id!(RelationId);
string_id!(FrameId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    AtomicMeaning,
    CompositeMeaning,
    SemanticRole,
    GraphMotif,
    Transformation,
    DynamicCoupling,
    LatentAxis,
    CrossDomainStructure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropertyValue {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    Text(String),
    Structured(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub id: UnitId,
    pub kind: UnitKind,
    pub label: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub id: RelationId,
    pub source: UnitId,
    pub target: UnitId,
    pub kind: String,
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainSnapshot {
    pub id: DomainId,
    pub version: DomainVersion,
    pub units: BTreeMap<UnitId, Unit>,
    pub relations: BTreeMap<RelationId, Relation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameAxis {
    pub unit: UnitId,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementFrame {
    pub id: FrameId,
    pub version: FrameVersion,
    pub axes: Vec<FrameAxis>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_and_frame_versions_are_independent() {
        let domain = DomainSnapshot {
            id: DomainId::new("coffee"),
            version: DomainVersion::new("7"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("coffee.general"),
            version: FrameVersion::new("2"),
            axes: Vec::new(),
        };
        assert_eq!(domain.version.0, "7");
        assert_eq!(frame.version.0, "2");
    }
}
