//! Scale-free movement of explicit cross-domain interaction axes.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use unclip_domain::{ProductFrameAxis, UnitId};
use unclip_observe::ObservationId;

use crate::{CrossDomainSample, ObservationSequence, ProductMeasurementBinding};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainInteractionMovementConfig {
    pub minimum_transitions: NonZeroUsize,
    pub sequence: ObservationSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainTransition {
    pub from: ObservationId,
    pub to: ObservationId,
}

/// Movement evidence for one materialized product interaction.
///
/// The score is the mean of `sign(delta_left) * sign(delta_right)` over every
/// complete consecutive transition. It is scale-free and bounded to [-1, 1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainAxisMovement {
    pub left: UnitId,
    pub right: UnitId,
    pub directional_concordance: f64,
    pub transition_count: usize,
    pub concordant_transitions: usize,
    pub discordant_transitions: usize,
    pub left_only_transitions: usize,
    pub right_only_transitions: usize,
    pub stationary_transitions: usize,
    pub excluded_transitions: Vec<CrossDomainTransition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnassessedCrossDomainAxisMovement {
    pub left: UnitId,
    pub right: UnitId,
    pub have: usize,
    pub need: usize,
    pub excluded_transitions: Vec<CrossDomainTransition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainInteractionMovement {
    pub binding: ProductMeasurementBinding,
    pub sequence: ObservationSequence,
    pub axes: Vec<CrossDomainAxisMovement>,
    pub unassessed_axes: Vec<UnassessedCrossDomainAxisMovement>,
    pub observation_count: usize,
    pub transition_count: usize,
    pub minimum_transitions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CrossDomainInteractionMovementOutcome {
    Value {
        movement: Box<CrossDomainInteractionMovement>,
    },
    InsufficientEvidence {
        have: usize,
        need: usize,
        unassessed_axes: Vec<UnassessedCrossDomainAxisMovement>,
    },
    NoAxes {
        observation_count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossDomainInteractionMovementError {
    InvalidConfiguration,
    InvalidAxis {
        left: UnitId,
        right: UnitId,
    },
    DuplicateObservation(ObservationId),
    SequenceMismatch,
    UnexpectedUnit(UnitId),
    NonFiniteValue {
        observation: ObservationId,
        unit: UnitId,
    },
}

impl std::fmt::Display for CrossDomainInteractionMovementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration => write!(
                f,
                "cross-domain interaction movement requires valid product binding and a nonzero transition floor"
            ),
            Self::InvalidAxis { left, right } => {
                write!(f, "invalid or duplicate product axis {} x {}", left.0, right.0)
            }
            Self::DuplicateObservation(observation) => {
                write!(f, "duplicate cross-domain sample {}", observation.0)
            }
            Self::SequenceMismatch => write!(
                f,
                "cross-domain samples must exactly match the explicit observation sequence"
            ),
            Self::UnexpectedUnit(unit) => {
                write!(f, "cross-domain sample contains unexpected unit {}", unit.0)
            }
            Self::NonFiniteValue { observation, unit } => write!(
                f,
                "cross-domain sample {} contains a nonfinite value for {}",
                observation.0, unit.0
            ),
        }
    }
}

impl std::error::Error for CrossDomainInteractionMovementError {}

/// Measure direction agreement on every explicit product-frame interaction.
///
/// Only adjacent entries in the declared sequence form transitions. An incomplete
/// transition is excluded as that exact pair; later observations never close the gap.
pub fn cross_domain_interaction_movement(
    binding: ProductMeasurementBinding,
    axes: &[ProductFrameAxis],
    samples: &[CrossDomainSample],
    config: CrossDomainInteractionMovementConfig,
) -> Result<CrossDomainInteractionMovementOutcome, CrossDomainInteractionMovementError> {
    validate_binding(&binding)?;

    let mut coordinates = BTreeSet::new();
    let mut left_units = BTreeSet::new();
    let mut right_units = BTreeSet::new();
    for axis in axes {
        if axis.left.0.trim().is_empty()
            || axis.right.0.trim().is_empty()
            || axis
                .label
                .as_ref()
                .is_some_and(|label| label.trim().is_empty())
            || !coordinates.insert((&axis.left, &axis.right))
        {
            return Err(CrossDomainInteractionMovementError::InvalidAxis {
                left: axis.left.clone(),
                right: axis.right.clone(),
            });
        }
        left_units.insert(&axis.left);
        right_units.insert(&axis.right);
    }

    let mut by_observation = BTreeMap::new();
    for sample in samples {
        if by_observation.insert(&sample.observation, sample).is_some() {
            return Err(CrossDomainInteractionMovementError::DuplicateObservation(
                sample.observation.clone(),
            ));
        }
        validate_values(sample, &left_units, &right_units)?;
    }
    let sequence_ids = config
        .sequence
        .observations()
        .iter()
        .map(|entry| &entry.observation)
        .collect::<BTreeSet<_>>();
    if sequence_ids.len() != samples.len()
        || by_observation.len() != samples.len()
        || by_observation.keys().copied().collect::<BTreeSet<_>>() != sequence_ids
    {
        return Err(CrossDomainInteractionMovementError::SequenceMismatch);
    }

    if axes.is_empty() {
        return Ok(CrossDomainInteractionMovementOutcome::NoAxes {
            observation_count: samples.len(),
        });
    }

    let transitions = config
        .sequence
        .observations()
        .windows(2)
        .collect::<Vec<_>>();
    let mut measured = Vec::new();
    let mut unassessed = Vec::new();
    for axis in axes {
        let mut complete = 0;
        let mut concordant = 0;
        let mut discordant = 0;
        let mut left_only = 0;
        let mut right_only = 0;
        let mut stationary = 0;
        let mut excluded = Vec::new();
        for transition in &transitions {
            let from = by_observation[&transition[0].observation];
            let to = by_observation[&transition[1].observation];
            let values = (
                from.left.get(&axis.left),
                to.left.get(&axis.left),
                from.right.get(&axis.right),
                to.right.get(&axis.right),
            );
            let (Some(&left_from), Some(&left_to), Some(&right_from), Some(&right_to)) = values
            else {
                excluded.push(CrossDomainTransition {
                    from: transition[0].observation.clone(),
                    to: transition[1].observation.clone(),
                });
                continue;
            };
            complete += 1;
            let left_direction = direction(left_from, left_to);
            let right_direction = direction(right_from, right_to);
            match (left_direction, right_direction) {
                (0, 0) => stationary += 1,
                (0, _) => right_only += 1,
                (_, 0) => left_only += 1,
                (left, right) if left == right => concordant += 1,
                _ => discordant += 1,
            }
        }
        if complete < config.minimum_transitions.get() {
            unassessed.push(UnassessedCrossDomainAxisMovement {
                left: axis.left.clone(),
                right: axis.right.clone(),
                have: complete,
                need: config.minimum_transitions.get(),
                excluded_transitions: excluded,
            });
        } else {
            measured.push(CrossDomainAxisMovement {
                left: axis.left.clone(),
                right: axis.right.clone(),
                directional_concordance: (concordant as f64 - discordant as f64) / complete as f64,
                transition_count: complete,
                concordant_transitions: concordant,
                discordant_transitions: discordant,
                left_only_transitions: left_only,
                right_only_transitions: right_only,
                stationary_transitions: stationary,
                excluded_transitions: excluded,
            });
        }
    }

    if measured.is_empty() {
        return Ok(
            CrossDomainInteractionMovementOutcome::InsufficientEvidence {
                have: unassessed.iter().map(|axis| axis.have).max().unwrap_or(0),
                need: config.minimum_transitions.get(),
                unassessed_axes: unassessed,
            },
        );
    }
    Ok(CrossDomainInteractionMovementOutcome::Value {
        movement: Box::new(CrossDomainInteractionMovement {
            binding,
            sequence: config.sequence.clone(),
            axes: measured,
            unassessed_axes: unassessed,
            observation_count: samples.len(),
            transition_count: transitions.len(),
            minimum_transitions: config.minimum_transitions.get(),
        }),
    })
}

fn direction(from: f64, to: f64) -> i8 {
    if to > from {
        1
    } else if to < from {
        -1
    } else {
        0
    }
}

fn validate_binding(
    binding: &ProductMeasurementBinding,
) -> Result<(), CrossDomainInteractionMovementError> {
    if binding.product.0.trim().is_empty()
        || binding.product_version.0.trim().is_empty()
        || binding.frame.0.trim().is_empty()
        || binding.frame_version.0.trim().is_empty()
        || binding.left.domain.0.trim().is_empty()
        || binding.left.version.0.trim().is_empty()
        || binding.right.domain.0.trim().is_empty()
        || binding.right.version.0.trim().is_empty()
        || binding.left.domain == binding.right.domain
    {
        return Err(CrossDomainInteractionMovementError::InvalidConfiguration);
    }
    Ok(())
}

fn validate_values(
    sample: &CrossDomainSample,
    left_units: &BTreeSet<&UnitId>,
    right_units: &BTreeSet<&UnitId>,
) -> Result<(), CrossDomainInteractionMovementError> {
    for (unit, value) in &sample.left {
        if !left_units.contains(unit) {
            return Err(CrossDomainInteractionMovementError::UnexpectedUnit(
                unit.clone(),
            ));
        }
        if !value.is_finite() {
            return Err(CrossDomainInteractionMovementError::NonFiniteValue {
                observation: sample.observation.clone(),
                unit: unit.clone(),
            });
        }
    }
    for (unit, value) in &sample.right {
        if !right_units.contains(unit) {
            return Err(CrossDomainInteractionMovementError::UnexpectedUnit(
                unit.clone(),
            ));
        }
        if !value.is_finite() {
            return Err(CrossDomainInteractionMovementError::NonFiniteValue {
                observation: sample.observation.clone(),
                unit: unit.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrderedObservation;
    use unclip_domain::{
        DomainId, ProductDomainId, ProductDomainInput, ProductDomainVersion, ProductFrameId,
        ProductFrameVersion,
    };
    use unclip_epistemic::DomainVersion;

    fn binding() -> ProductMeasurementBinding {
        ProductMeasurementBinding {
            product: ProductDomainId::new("left-x-right"),
            product_version: ProductDomainVersion::new("1"),
            frame: ProductFrameId::new("frame"),
            frame_version: ProductFrameVersion::new("2"),
            left: ProductDomainInput {
                domain: DomainId::new("left"),
                version: DomainVersion::new("3"),
            },
            right: ProductDomainInput {
                domain: DomainId::new("right"),
                version: DomainVersion::new("4"),
            },
        }
    }

    fn axis(left: &str, right: &str) -> ProductFrameAxis {
        ProductFrameAxis {
            left: UnitId::new(left),
            right: UnitId::new(right),
            label: None,
        }
    }

    fn sample(observation: &str, left: &[(&str, f64)], right: &[(&str, f64)]) -> CrossDomainSample {
        CrossDomainSample {
            observation: ObservationId::new(observation),
            left: left
                .iter()
                .map(|(unit, value)| (UnitId::new(*unit), *value))
                .collect(),
            right: right
                .iter()
                .map(|(unit, value)| (UnitId::new(*unit), *value))
                .collect(),
        }
    }

    fn config(ids: &[&str], minimum: usize) -> CrossDomainInteractionMovementConfig {
        CrossDomainInteractionMovementConfig {
            minimum_transitions: NonZeroUsize::new(minimum).unwrap(),
            sequence: ObservationSequence::new(
                ids.iter()
                    .enumerate()
                    .map(|(position, observation)| OrderedObservation {
                        observation: ObservationId::new(*observation),
                        position: position as i64,
                    })
                    .collect(),
            )
            .unwrap(),
        }
    }

    #[test]
    fn measures_every_transition_class_and_is_scale_free() {
        let ids = ["a", "b", "c", "d", "e", "f"];
        let samples = [
            sample("a", &[("l", 0.0)], &[("r", 10.0)]),
            sample("b", &[("l", 1.0)], &[("r", 20.0)]),
            sample("c", &[("l", 2.0)], &[("r", 15.0)]),
            sample("d", &[("l", 2.0)], &[("r", 15.0)]),
            sample("e", &[("l", 3.0)], &[("r", 15.0)]),
            sample("f", &[("l", 3.0)], &[("r", 25.0)]),
        ];
        let calculate = |samples: &[CrossDomainSample]| {
            cross_domain_interaction_movement(
                binding(),
                &[axis("l", "r")],
                samples,
                config(&ids, 1),
            )
            .unwrap()
        };
        let result = calculate(&samples);
        assert_eq!(calculate(&samples), result);
        let scaled = [
            sample("a", &[("l", 100.0)], &[("r", -500.0)]),
            sample("b", &[("l", 110.0)], &[("r", -300.0)]),
            sample("c", &[("l", 120.0)], &[("r", -400.0)]),
            sample("d", &[("l", 120.0)], &[("r", -400.0)]),
            sample("e", &[("l", 130.0)], &[("r", -400.0)]),
            sample("f", &[("l", 130.0)], &[("r", -200.0)]),
        ];
        assert_eq!(calculate(&scaled), result);
        let CrossDomainInteractionMovementOutcome::Value { movement } = result else {
            panic!("expected movement")
        };
        let measured = &movement.axes[0];
        assert_eq!(measured.directional_concordance, 0.0);
        assert_eq!(measured.concordant_transitions, 1);
        assert_eq!(measured.discordant_transitions, 1);
        assert_eq!(measured.left_only_transitions, 1);
        assert_eq!(measured.right_only_transitions, 1);
        assert_eq!(measured.stationary_transitions, 1);
        assert_eq!(measured.transition_count, 5);
    }

    #[test]
    fn preserves_sparse_transition_gaps_without_closing_them() {
        let samples = [
            sample("a", &[("l", 0.0)], &[("r", 0.0)]),
            sample("b", &[], &[("r", 1.0)]),
            sample("c", &[("l", 2.0)], &[("r", 2.0)]),
            sample("d", &[("l", 3.0)], &[("r", 3.0)]),
        ];
        let CrossDomainInteractionMovementOutcome::Value { movement } =
            cross_domain_interaction_movement(
                binding(),
                &[axis("l", "r")],
                &samples,
                config(&["a", "b", "c", "d"], 1),
            )
            .unwrap()
        else {
            panic!("one complete transition should be measured")
        };
        assert_eq!(movement.axes[0].transition_count, 1);
        assert_eq!(movement.axes[0].excluded_transitions.len(), 2);
        assert_eq!(movement.axes[0].excluded_transitions[0].from.0, "a");
        assert_eq!(movement.axes[0].excluded_transitions[1].to.0, "c");

        assert!(matches!(
            cross_domain_interaction_movement(
                binding(),
                &[axis("l", "r")],
                &samples,
                config(&["a", "b", "c", "d"], 2),
            )
            .unwrap(),
            CrossDomainInteractionMovementOutcome::InsufficientEvidence {
                have: 1,
                need: 2,
                ..
            }
        ));
    }

    #[test]
    fn rejects_implicit_or_ambiguous_order_and_invalid_values() {
        let samples = [
            sample("a", &[("l", 0.0)], &[("r", 0.0)]),
            sample("b", &[("l", 1.0)], &[("r", 1.0)]),
        ];
        assert_eq!(
            cross_domain_interaction_movement(
                binding(),
                &[axis("l", "r")],
                &samples,
                config(&["a", "other"], 1),
            ),
            Err(CrossDomainInteractionMovementError::SequenceMismatch)
        );
        let duplicate = [samples[0].clone(), samples[0].clone()];
        assert!(matches!(
            cross_domain_interaction_movement(
                binding(),
                &[axis("l", "r")],
                &duplicate,
                config(&["a", "b"], 1),
            ),
            Err(CrossDomainInteractionMovementError::DuplicateObservation(_))
        ));
        let nonfinite = [
            samples[0].clone(),
            sample("b", &[("l", f64::INFINITY)], &[("r", 1.0)]),
        ];
        assert!(matches!(
            cross_domain_interaction_movement(
                binding(),
                &[axis("l", "r")],
                &nonfinite,
                config(&["a", "b"], 1),
            ),
            Err(CrossDomainInteractionMovementError::NonFiniteValue { .. })
        ));
    }
}
