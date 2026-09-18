//! Typed, sparse measurement profiles that preserve sensor disagreement.
//!
//! Profiles intentionally have no scalar conversion.
//!
//! ```compile_fail
//! use unclip_measure::MeasurementProfile;
//!
//! let score: f64 = MeasurementProfile::default().into();
//! ```
//!
//! ```compile_fail
//! use unclip_measure::MeasurementProfile;
//!
//! fn requires_order<T: Ord>() {}
//! requires_order::<MeasurementProfile>();
//! ```

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;
use unclip_epistemic::PluginId;
use unclip_observe::ObservedUnitId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementKind {
    Scalar,
    Vector,
    Matrix,
    Distribution,
    Events,
    Graph,
    Ranking,
    Partition,
    Structured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedState {
    pub tiers: Vec<Vec<UnitId>>,
    pub unknown: Vec<UnitId>,
    pub unresolved: Vec<ObservedUnitId>,
}

impl RankedState {
    pub fn is_total(&self, frame_size: usize) -> bool {
        self.unknown.is_empty()
            && self.unresolved.is_empty()
            && self.tiers.iter().all(|tier| tier.len() == 1)
            && self.tiers.len() == frame_size
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MeasurementValue {
    Scalar(f64),
    Vector(Vec<f64>),
    Matrix(Vec<Vec<f64>>),
    Distribution(Vec<(String, f64)>),
    Events(Vec<serde_json::Value>),
    Graph(serde_json::Value),
    Ranking(RankedState),
    Partition(Vec<Vec<String>>),
    Structured(serde_json::Value),
}

impl MeasurementValue {
    pub fn kind(&self) -> MeasurementKind {
        match self {
            Self::Scalar(_) => MeasurementKind::Scalar,
            Self::Vector(_) => MeasurementKind::Vector,
            Self::Matrix(_) => MeasurementKind::Matrix,
            Self::Distribution(_) => MeasurementKind::Distribution,
            Self::Events(_) => MeasurementKind::Events,
            Self::Graph(_) => MeasurementKind::Graph,
            Self::Ranking(_) => MeasurementKind::Ranking,
            Self::Partition(_) => MeasurementKind::Partition,
            Self::Structured(_) => MeasurementKind::Structured,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Reading {
    Value { value: MeasurementValue },
    NotApplicable { reason: String },
    NotMeasured,
    InsufficientEvidence { have: usize, need: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MeasurementContext {
    #[serde(default)]
    pub values: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub sensor: PluginId,
    pub sensor_version: semver::Version,
    pub reading: Reading,
    pub confidence: Option<f64>,
    pub sample_count: Option<usize>,
    pub context: MeasurementContext,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MeasurementProfile {
    pub measurements: Vec<Measurement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub comparator: PluginId,
    pub value: MeasurementValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmpiricalStructure {
    pub kind: String,
    pub value: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_a_value_not_a_sparse_status() {
        let reading = Reading::Value {
            value: MeasurementValue::Scalar(0.0),
        };
        assert!(matches!(reading, Reading::Value { .. }));
    }

    #[test]
    fn sparse_states_and_measured_zero_are_independently_representable() {
        let readings = [
            Reading::NotApplicable {
                reason: "outside domain".into(),
            },
            Reading::NotMeasured,
            Reading::InsufficientEvidence { have: 0, need: 1 },
            Reading::Value {
                value: MeasurementValue::Scalar(0.0),
            },
        ];

        for (left_index, left) in readings.iter().enumerate() {
            for right in readings.iter().skip(left_index + 1) {
                assert_ne!(left, right);
            }
        }
        assert!(matches!(
            &readings[3],
            Reading::Value {
                value: MeasurementValue::Scalar(value)
            } if *value == 0.0
        ));
    }

    #[test]
    fn kinds_are_explicit() {
        assert_eq!(
            MeasurementValue::Partition(vec![]).kind(),
            MeasurementKind::Partition
        );
    }
}
