//! Explicit typed expectations for behavior under an independence assumption.

use std::{collections::BTreeSet, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{MeasurementKind, MeasurementValue, PairwiseMatrix, RankedState, Reading};

/// The two representations covered by the public matrix measurement kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "representation", rename_all = "snake_case", deny_unknown_fields)]
pub enum IndependentMatrix {
    Dense { values: Arc<Vec<Vec<f64>>> },
    Pairwise { matrix: PairwiseMatrix },
}

/// A declared behavior under independence, kept in its original measurement kind.
///
/// No variant supplies a conventional default. `Undefined` records that a rule is
/// unavailable for a kind instead of silently treating missing structure as zero.
///
/// Each payload is shared in the same shape [`MeasurementValue`] holds it, so
/// [`Self::reading`] hands out the declared expectation rather than copying it.
/// One expectation is read once per selected comparator, and what it declares is
/// a whole matrix, ranked state, or structured tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedIndependentBehavior {
    Scalar {
        value: f64,
    },
    Vector {
        values: Arc<Vec<f64>>,
    },
    Matrix {
        value: IndependentMatrix,
    },
    Distribution {
        values: Arc<Vec<(String, f64)>>,
    },
    Events {
        values: Arc<Vec<serde_json::Value>>,
    },
    Graph {
        value: Arc<serde_json::Value>,
    },
    Ranking {
        value: Arc<RankedState>,
    },
    Partition {
        groups: Arc<Vec<Vec<String>>>,
    },
    Structured {
        value: Arc<serde_json::Value>,
    },
    Undefined {
        measurement_kind: MeasurementKind,
        reason: Arc<str>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidIndependentBehavior {
    #[error("independence values must be finite")]
    NonFiniteValue,
    #[error("independence matrices must be rectangular")]
    RaggedMatrix,
    #[error("independence distributions require unique nonempty categories and finite nonnegative values")]
    InvalidDistribution,
    #[error("undefined independence behavior requires a nonempty reason")]
    EmptyReason,
}

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
            Self::Vector { values } => MeasurementValue::Vector(Arc::clone(values)),
            Self::Matrix {
                value: IndependentMatrix::Dense { values },
            } => MeasurementValue::Matrix(Arc::clone(values)),
            Self::Matrix {
                value: IndependentMatrix::Pairwise { matrix },
            } => MeasurementValue::PairwiseMatrix(matrix.clone()),
            Self::Distribution { values } => MeasurementValue::Distribution(Arc::clone(values)),
            Self::Events { values } => MeasurementValue::Events(Arc::clone(values)),
            Self::Graph { value } => MeasurementValue::Graph(Arc::clone(value)),
            Self::Ranking { value } => MeasurementValue::Ranking(Arc::clone(value)),
            Self::Partition { groups } => MeasurementValue::Partition(Arc::clone(groups)),
            Self::Structured { value } => MeasurementValue::Structured(Arc::clone(value)),
            Self::Undefined { reason, .. } => {
                return Reading::NotApplicable {
                    reason: Arc::clone(reason),
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
            ExpectedIndependentBehavior::Vector {
                values: vec![1.0].into(),
            },
            ExpectedIndependentBehavior::Matrix {
                value: IndependentMatrix::Dense {
                    values: vec![vec![1.0]].into(),
                },
            },
            ExpectedIndependentBehavior::Distribution {
                values: vec![("a".into(), 1.0)].into(),
            },
            ExpectedIndependentBehavior::Events {
                values: vec![].into(),
            },
            ExpectedIndependentBehavior::Graph {
                value: serde_json::json!({"nodes": [], "edges": []}).into(),
            },
            ExpectedIndependentBehavior::Ranking {
                value: RankedState {
                    tiers: vec![vec![UnitId::new("a")]],
                    unknown: vec![],
                    unresolved: vec![],
                }
                .into(),
            },
            ExpectedIndependentBehavior::Partition {
                groups: vec![vec!["a".into()]].into(),
            },
            ExpectedIndependentBehavior::Structured {
                value: serde_json::json!({"association": 0.0}).into(),
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

    /// One expectation is read once per selected comparator, and what it
    /// declares is a whole matrix, ranked state, or structured tree. So
    /// `reading` hands out the declared payload rather than copying it. Only
    /// pointer identity catches a regression: a copy compares equal.
    #[test]
    fn reading_an_expectation_shares_the_payload_it_declares() {
        let graph = Arc::new(serde_json::json!({"nodes": [], "edges": []}));
        let Reading::Value {
            value: MeasurementValue::Graph(reported),
        } = (ExpectedIndependentBehavior::Graph {
            value: Arc::clone(&graph),
        })
        .reading()
        else {
            panic!("a graph expectation reads as a graph")
        };
        assert!(Arc::ptr_eq(&graph, &reported));

        let ranked = Arc::new(RankedState {
            tiers: vec![vec![UnitId::new("a")]],
            unknown: vec![],
            unresolved: vec![],
        });
        let Reading::Value {
            value: MeasurementValue::Ranking(reported),
        } = (ExpectedIndependentBehavior::Ranking {
            value: Arc::clone(&ranked),
        })
        .reading()
        else {
            panic!("a ranking expectation reads as a ranking")
        };
        assert!(Arc::ptr_eq(&ranked, &reported));

        let reason: Arc<str> = Arc::from("no rule was selected");
        let Reading::NotApplicable { reason: reported } =
            (ExpectedIndependentBehavior::Undefined {
                measurement_kind: MeasurementKind::Ranking,
                reason: Arc::clone(&reason),
            })
            .reading()
        else {
            panic!("an undefined expectation reads as not applicable")
        };
        assert!(Arc::ptr_eq(&reason, &reported));
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
                    values: vec![vec![1.0], vec![]].into()
                }
            }
            .validate(),
            Err(InvalidIndependentBehavior::RaggedMatrix)
        );
        assert_eq!(
            ExpectedIndependentBehavior::Distribution {
                values: vec![("a".into(), 0.5), ("a".into(), 0.5)].into()
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
