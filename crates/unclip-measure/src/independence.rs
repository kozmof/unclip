//! Explicit typed expectations for behavior under an independence assumption.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{MeasurementKind, MeasurementValue, PairwiseMatrix, RankedState, Reading};

/// The two representations covered by the public matrix measurement kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "representation", rename_all = "snake_case", deny_unknown_fields)]
pub enum IndependentMatrix {
    Dense { values: Vec<Vec<f64>> },
    Pairwise { matrix: PairwiseMatrix },
}

/// A declared behavior under independence, kept in its original measurement kind.
///
/// No variant supplies a conventional default. `Undefined` records that a rule is
/// unavailable for a kind instead of silently treating missing structure as zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedIndependentBehavior {
    Scalar {
        value: f64,
    },
    Vector {
        values: Vec<f64>,
    },
    Matrix {
        value: IndependentMatrix,
    },
    Distribution {
        values: Vec<(String, f64)>,
    },
    Events {
        values: Vec<serde_json::Value>,
    },
    Graph {
        value: serde_json::Value,
    },
    Ranking {
        value: RankedState,
    },
    Partition {
        groups: Vec<Vec<String>>,
    },
    Structured {
        value: serde_json::Value,
    },
    Undefined {
        measurement_kind: MeasurementKind,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidIndependentBehavior {
    NonFiniteValue,
    RaggedMatrix,
    InvalidDistribution,
    EmptyReason,
}

impl std::fmt::Display for InvalidIndependentBehavior {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteValue => write!(formatter, "independence values must be finite"),
            Self::RaggedMatrix => write!(formatter, "independence matrices must be rectangular"),
            Self::InvalidDistribution => write!(
                formatter,
                "independence distributions require unique nonempty categories and finite nonnegative values"
            ),
            Self::EmptyReason => write!(
                formatter,
                "undefined independence behavior requires a nonempty reason"
            ),
        }
    }
}

impl std::error::Error for InvalidIndependentBehavior {}

impl ExpectedIndependentBehavior {
    pub fn kind(&self) -> MeasurementKind {
        match self {
            Self::Scalar { .. } => MeasurementKind::Scalar,
            Self::Vector { .. } => MeasurementKind::Vector,
            Self::Matrix { .. } => MeasurementKind::Matrix,
            Self::Distribution { .. } => MeasurementKind::Distribution,
            Self::Events { .. } => MeasurementKind::Events,
            Self::Graph { .. } => MeasurementKind::Graph,
            Self::Ranking { .. } => MeasurementKind::Ranking,
            Self::Partition { .. } => MeasurementKind::Partition,
            Self::Structured { .. } => MeasurementKind::Structured,
            Self::Undefined {
                measurement_kind, ..
            } => *measurement_kind,
        }
    }

    pub fn validate(&self) -> Result<(), InvalidIndependentBehavior> {
        match self {
            Self::Scalar { value } if !value.is_finite() => {
                Err(InvalidIndependentBehavior::NonFiniteValue)
            }
            Self::Vector { values } if values.iter().any(|value| !value.is_finite()) => {
                Err(InvalidIndependentBehavior::NonFiniteValue)
            }
            Self::Matrix {
                value: IndependentMatrix::Dense { values },
            } => {
                if values.iter().flatten().any(|value| !value.is_finite()) {
                    return Err(InvalidIndependentBehavior::NonFiniteValue);
                }
                let width = values.first().map_or(0, Vec::len);
                if values.iter().any(|row| row.len() != width) {
                    return Err(InvalidIndependentBehavior::RaggedMatrix);
                }
                Ok(())
            }
            Self::Distribution { values } => {
                let mut categories = BTreeSet::new();
                if values.iter().any(|(category, value)| {
                    category.trim().is_empty()
                        || !value.is_finite()
                        || *value < 0.0
                        || !categories.insert(category)
                }) {
                    return Err(InvalidIndependentBehavior::InvalidDistribution);
                }
                Ok(())
            }
            Self::Undefined { reason, .. } if reason.trim().is_empty() => {
                Err(InvalidIndependentBehavior::EmptyReason)
            }
            _ => Ok(()),
        }
    }

    pub fn reading(&self) -> Reading {
        let value = match self {
            Self::Scalar { value } => MeasurementValue::Scalar(*value),
            Self::Vector { values } => MeasurementValue::Vector(values.clone()),
            Self::Matrix {
                value: IndependentMatrix::Dense { values },
            } => MeasurementValue::Matrix(values.clone()),
            Self::Matrix {
                value: IndependentMatrix::Pairwise { matrix },
            } => MeasurementValue::PairwiseMatrix(matrix.clone()),
            Self::Distribution { values } => MeasurementValue::Distribution(values.clone()),
            Self::Events { values } => MeasurementValue::Events(values.clone()),
            Self::Graph { value } => MeasurementValue::Graph(value.clone()),
            Self::Ranking { value } => MeasurementValue::Ranking(value.clone()),
            Self::Partition { groups } => MeasurementValue::Partition(groups.clone()),
            Self::Structured { value } => MeasurementValue::Structured(value.clone()),
            Self::Undefined { reason, .. } => {
                return Reading::NotApplicable {
                    reason: reason.clone(),
                };
            }
        };
        Reading::Value { value }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unclip_domain::UnitId;

    #[test]
    fn every_measurement_kind_has_an_explicit_behavior_without_coercion() {
        let values = vec![
            ExpectedIndependentBehavior::Scalar { value: 0.0 },
            ExpectedIndependentBehavior::Vector { values: vec![1.0] },
            ExpectedIndependentBehavior::Matrix {
                value: IndependentMatrix::Dense {
                    values: vec![vec![1.0]],
                },
            },
            ExpectedIndependentBehavior::Distribution {
                values: vec![("a".into(), 1.0)],
            },
            ExpectedIndependentBehavior::Events { values: vec![] },
            ExpectedIndependentBehavior::Graph {
                value: serde_json::json!({"nodes": [], "edges": []}),
            },
            ExpectedIndependentBehavior::Ranking {
                value: RankedState {
                    tiers: vec![vec![UnitId::new("a")]],
                    unknown: vec![],
                    unresolved: vec![],
                },
            },
            ExpectedIndependentBehavior::Partition {
                groups: vec![vec!["a".into()]],
            },
            ExpectedIndependentBehavior::Structured {
                value: serde_json::json!({"association": 0.0}),
            },
        ];
        let kinds = values
            .iter()
            .map(ExpectedIndependentBehavior::kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                MeasurementKind::Scalar,
                MeasurementKind::Vector,
                MeasurementKind::Matrix,
                MeasurementKind::Distribution,
                MeasurementKind::Events,
                MeasurementKind::Graph,
                MeasurementKind::Ranking,
                MeasurementKind::Partition,
                MeasurementKind::Structured,
            ]
        );
        for value in values {
            value.validate().unwrap();
            let Reading::Value { value: reading } = value.reading() else {
                panic!("expected a typed reading")
            };
            assert_eq!(reading.kind(), value.kind());
        }
    }

    #[test]
    fn undefined_and_invalid_expectations_remain_explicit() {
        let undefined = ExpectedIndependentBehavior::Undefined {
            measurement_kind: MeasurementKind::Ranking,
            reason: "no cross-domain order rule was selected".into(),
        };
        assert_eq!(undefined.kind(), MeasurementKind::Ranking);
        assert_eq!(
            undefined.reading(),
            Reading::NotApplicable {
                reason: "no cross-domain order rule was selected".into()
            }
        );
        assert_eq!(
            ExpectedIndependentBehavior::Scalar { value: f64::NAN }.validate(),
            Err(InvalidIndependentBehavior::NonFiniteValue)
        );
        assert_eq!(
            ExpectedIndependentBehavior::Matrix {
                value: IndependentMatrix::Dense {
                    values: vec![vec![1.0], vec![]]
                }
            }
            .validate(),
            Err(InvalidIndependentBehavior::RaggedMatrix)
        );
        assert_eq!(
            ExpectedIndependentBehavior::Distribution {
                values: vec![("a".into(), 0.5), ("a".into(), 0.5)]
            }
            .validate(),
            Err(InvalidIndependentBehavior::InvalidDistribution)
        );
        assert_eq!(
            ExpectedIndependentBehavior::Undefined {
                measurement_kind: MeasurementKind::Graph,
                reason: " ".into()
            }
            .validate(),
            Err(InvalidIndependentBehavior::EmptyReason)
        );
    }
}
