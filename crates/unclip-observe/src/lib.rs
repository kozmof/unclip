//! Independent observations, uncertain alignments, and partial rankings.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;
use unclip_epistemic::SourceRef;

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

string_id!(ObservationId);
string_id!(ObservedUnitId);
string_id!(ObservedRelationId);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedUnit {
    pub id: ObservedUnitId,
    pub label: String,
    pub salience: Option<f64>,
    pub uncertainty: Option<f64>,
    #[serde(default)]
    pub context: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedRelation {
    pub id: ObservedRelationId,
    pub source: ObservedUnitId,
    pub target: ObservedUnitId,
    pub kind: String,
    pub uncertainty: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub id: ObservationId,
    pub source: SourceRef,
    pub observed_at: Option<String>,
    pub units: Vec<ObservedUnit>,
    pub relations: Vec<ObservedRelation>,
    #[serde(default)]
    pub context: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentCandidate {
    pub observed: ObservedUnitId,
    pub domain: UnitId,
    pub confidence: f64,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    pub observation: ObservationId,
    pub candidates: Vec<AlignmentCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankTier {
    pub units: Vec<ObservedUnitId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialRanking {
    pub observation: ObservationId,
    pub tiers: Vec<RankTier>,
    #[serde(default)]
    pub unknown: Vec<ObservedUnitId>,
}

impl PartialRanking {
    pub fn is_total(&self) -> bool {
        self.unknown.is_empty() && self.tiers.iter().all(|tier| tier.units.len() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ties_and_unknown_tail_are_not_total() {
        let ranking = PartialRanking {
            observation: ObservationId::new("x"),
            tiers: vec![RankTier {
                units: vec![ObservedUnitId::new("a"), ObservedUnitId::new("b")],
            }],
            unknown: vec![ObservedUnitId::new("c")],
        };
        assert!(!ranking.is_total());
    }
}
