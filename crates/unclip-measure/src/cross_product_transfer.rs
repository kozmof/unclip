//! Transfer of measured interaction movement across explicitly mapped products.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;

use crate::{
    CrossDomainAxisMovement, CrossDomainInteractionMovement, CrossDomainTransition,
    ObservationSequence, ProductMeasurementBinding, UnassessedCrossDomainAxisMovement,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossProductAxisMapping {
    pub source_left: UnitId,
    pub source_right: UnitId,
    pub target_left: UnitId,
    pub target_right: UnitId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossProductTransferConfig {
    pub mappings: Vec<CrossProductAxisMapping>,
    pub minimum_transitions: NonZeroUsize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossProductAxisTransfer {
    pub mapping: CrossProductAxisMapping,
    pub source_directional_concordance: f64,
    pub target_directional_concordance: f64,
    pub directional_concordance_change: f64,
    pub absolute_change: f64,
    pub source_transitions: usize,
    pub target_transitions: usize,
    pub source_excluded_transitions: Vec<CrossDomainTransition>,
    pub target_excluded_transitions: Vec<CrossDomainTransition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnassessedCrossProductAxisTransfer {
    pub mapping: CrossProductAxisMapping,
    pub source_have: usize,
    pub target_have: usize,
    pub need: usize,
    pub source_excluded_transitions: Vec<CrossDomainTransition>,
    pub target_excluded_transitions: Vec<CrossDomainTransition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossProductTransfer {
    pub source_binding: ProductMeasurementBinding,
    pub target_binding: ProductMeasurementBinding,
    pub source_sequence: ObservationSequence,
    pub target_sequence: ObservationSequence,
    pub minimum_transitions: usize,
    pub transfers: Vec<CrossProductAxisTransfer>,
    pub unassessed_transfers: Vec<UnassessedCrossProductAxisTransfer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CrossProductTransferOutcome {
    Value {
        transfer: Box<CrossProductTransfer>,
    },
    InsufficientEvidence {
        have: usize,
        need: usize,
        unassessed_transfers: Vec<UnassessedCrossProductAxisTransfer>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossProductTransferError {
    InvalidConfiguration,
    InvalidMovementEvidence,
    InvalidMapping,
}

impl std::fmt::Display for CrossProductTransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration => write!(
                f,
                "cross-product transfer requires distinct product evidence, explicit one-to-one mappings, and a nonzero transition floor"
            ),
            Self::InvalidMovementEvidence => {
                write!(f, "cross-product transfer movement evidence is malformed")
            }
            Self::InvalidMapping => write!(
                f,
                "cross-product transfer mapping is empty, ambiguous, or does not name materialized movement axes"
            ),
        }
    }
}

impl std::error::Error for CrossProductTransferError {}

/// Compare signed movement concordance across explicitly mapped product axes.
///
/// This measures stability of an interaction signature. It does not infer mappings,
/// causal transfer, or an acceptance threshold. Exact zero absolute change remains a
/// measured result; sparse source or target evidence remains explicitly unassessed.
pub fn cross_product_transfer(
    source: &CrossDomainInteractionMovement,
    target: &CrossDomainInteractionMovement,
    config: CrossProductTransferConfig,
) -> Result<CrossProductTransferOutcome, CrossProductTransferError> {
    validate_movement(source)?;
    validate_movement(target)?;
    if source.binding == target.binding || config.mappings.is_empty() {
        return Err(CrossProductTransferError::InvalidConfiguration);
    }

    let source_axes = movement_axes(source);
    let target_axes = movement_axes(target);
    let mut mappings = config.mappings;
    mappings.sort();
    let mut source_coordinates = BTreeSet::new();
    let mut target_coordinates = BTreeSet::new();
    for mapping in &mappings {
        if mapping.source_left.0.trim().is_empty()
            || mapping.source_right.0.trim().is_empty()
            || mapping.target_left.0.trim().is_empty()
            || mapping.target_right.0.trim().is_empty()
            || !source_coordinates.insert((&mapping.source_left, &mapping.source_right))
            || !target_coordinates.insert((&mapping.target_left, &mapping.target_right))
            || !source_axes.contains_key(&(&mapping.source_left, &mapping.source_right))
            || !target_axes.contains_key(&(&mapping.target_left, &mapping.target_right))
        {
            return Err(CrossProductTransferError::InvalidMapping);
        }
    }

    let mut transfers = Vec::new();
    let mut unassessed = Vec::new();
    for mapping in mappings {
        let source_axis = source_axes[&(&mapping.source_left, &mapping.source_right)];
        let target_axis = target_axes[&(&mapping.target_left, &mapping.target_right)];
        let (source_have, source_excluded) = evidence_count(source_axis);
        let (target_have, target_excluded) = evidence_count(target_axis);
        let need = evidence_need(source_axis, config.minimum_transitions.get())
            .max(evidence_need(target_axis, config.minimum_transitions.get()));
        let (MovementEvidence::Measured(source_axis), MovementEvidence::Measured(target_axis)) =
            (source_axis, target_axis)
        else {
            unassessed.push(UnassessedCrossProductAxisTransfer {
                mapping,
                source_have,
                target_have,
                need,
                source_excluded_transitions: source_excluded.to_vec(),
                target_excluded_transitions: target_excluded.to_vec(),
            });
            continue;
        };
        if source_have < need || target_have < need {
            unassessed.push(UnassessedCrossProductAxisTransfer {
                mapping,
                source_have,
                target_have,
                need,
                source_excluded_transitions: source_excluded.to_vec(),
                target_excluded_transitions: target_excluded.to_vec(),
            });
            continue;
        }
        let change = target_axis.directional_concordance - source_axis.directional_concordance;
        transfers.push(CrossProductAxisTransfer {
            mapping,
            source_directional_concordance: source_axis.directional_concordance,
            target_directional_concordance: target_axis.directional_concordance,
            directional_concordance_change: change,
            absolute_change: change.abs(),
            source_transitions: source_axis.transition_count,
            target_transitions: target_axis.transition_count,
            source_excluded_transitions: source_axis.excluded_transitions.clone(),
            target_excluded_transitions: target_axis.excluded_transitions.clone(),
        });
    }

    if transfers.is_empty() {
        return Ok(CrossProductTransferOutcome::InsufficientEvidence {
            have: unassessed
                .iter()
                .map(|entry| entry.source_have.min(entry.target_have))
                .max()
                .unwrap_or(0),
            need: unassessed
                .iter()
                .map(|entry| entry.need)
                .max()
                .unwrap_or(config.minimum_transitions.get()),
            unassessed_transfers: unassessed,
        });
    }
    Ok(CrossProductTransferOutcome::Value {
        transfer: Box::new(CrossProductTransfer {
            source_binding: source.binding.clone(),
            target_binding: target.binding.clone(),
            source_sequence: source.sequence.clone(),
            target_sequence: target.sequence.clone(),
            minimum_transitions: config.minimum_transitions.get(),
            transfers,
            unassessed_transfers: unassessed,
        }),
    })
}

#[derive(Clone, Copy)]
enum MovementEvidence<'a> {
    Measured(&'a CrossDomainAxisMovement),
    Unassessed(&'a UnassessedCrossDomainAxisMovement),
}

fn evidence_need(evidence: MovementEvidence<'_>, transfer_minimum: usize) -> usize {
    match evidence {
        MovementEvidence::Measured(_) => transfer_minimum,
        MovementEvidence::Unassessed(axis) => transfer_minimum.max(axis.need),
    }
}

fn movement_axes(
    movement: &CrossDomainInteractionMovement,
) -> BTreeMap<(&UnitId, &UnitId), MovementEvidence<'_>> {
    movement
        .axes
        .iter()
        .map(|axis| ((&axis.left, &axis.right), MovementEvidence::Measured(axis)))
        .chain(movement.unassessed_axes.iter().map(|axis| {
            (
                (&axis.left, &axis.right),
                MovementEvidence::Unassessed(axis),
            )
        }))
        .collect()
}

fn evidence_count(evidence: MovementEvidence<'_>) -> (usize, &[CrossDomainTransition]) {
    match evidence {
        MovementEvidence::Measured(axis) => (axis.transition_count, &axis.excluded_transitions),
        MovementEvidence::Unassessed(axis) => (axis.have, &axis.excluded_transitions),
    }
}

fn validate_movement(
    movement: &CrossDomainInteractionMovement,
) -> Result<(), CrossProductTransferError> {
    let binding = &movement.binding;
    let sequence = movement.sequence.observations();
    if binding.product.0.trim().is_empty()
        || binding.product_version.0.trim().is_empty()
        || binding.frame.0.trim().is_empty()
        || binding.frame_version.0.trim().is_empty()
        || binding.left.domain.0.trim().is_empty()
        || binding.left.version.0.trim().is_empty()
        || binding.right.domain.0.trim().is_empty()
        || binding.right.version.0.trim().is_empty()
        || binding.left.domain == binding.right.domain
        || movement.minimum_transitions == 0
        || movement.observation_count != sequence.len()
        || movement.transition_count != sequence.len().saturating_sub(1)
        || movement.axes.is_empty()
    {
        return Err(CrossProductTransferError::InvalidMovementEvidence);
    }
    let valid_transitions = sequence
        .windows(2)
        .map(|pair| (&pair[0].observation, &pair[1].observation))
        .collect::<BTreeSet<_>>();
    let mut coordinates = BTreeSet::new();
    for axis in &movement.axes {
        let classified = axis
            .concordant_transitions
            .checked_add(axis.discordant_transitions)
            .and_then(|count| count.checked_add(axis.left_only_transitions))
            .and_then(|count| count.checked_add(axis.right_only_transitions))
            .and_then(|count| count.checked_add(axis.stationary_transitions));
        let expected = (axis.concordant_transitions as f64 - axis.discordant_transitions as f64)
            / axis.transition_count as f64;
        if axis.left.0.trim().is_empty()
            || axis.right.0.trim().is_empty()
            || axis.transition_count < movement.minimum_transitions
            || classified != Some(axis.transition_count)
            || axis
                .transition_count
                .checked_add(axis.excluded_transitions.len())
                != Some(movement.transition_count)
            || !axis.directional_concordance.is_finite()
            || !(-1.0..=1.0).contains(&axis.directional_concordance)
            || axis.directional_concordance != expected
            || !valid_exclusions(&axis.excluded_transitions, &valid_transitions)
            || !coordinates.insert((&axis.left, &axis.right))
        {
            return Err(CrossProductTransferError::InvalidMovementEvidence);
        }
    }
    for axis in &movement.unassessed_axes {
        if axis.left.0.trim().is_empty()
            || axis.right.0.trim().is_empty()
            || axis.need != movement.minimum_transitions
            || axis.have >= axis.need
            || axis.have.checked_add(axis.excluded_transitions.len())
                != Some(movement.transition_count)
            || !valid_exclusions(&axis.excluded_transitions, &valid_transitions)
            || !coordinates.insert((&axis.left, &axis.right))
        {
            return Err(CrossProductTransferError::InvalidMovementEvidence);
        }
    }
    Ok(())
}

fn valid_exclusions(
    exclusions: &[CrossDomainTransition],
    valid: &BTreeSet<(
        &unclip_observe::ObservationId,
        &unclip_observe::ObservationId,
    )>,
) -> bool {
    let unique = exclusions
        .iter()
        .map(|transition| (&transition.from, &transition.to))
        .collect::<BTreeSet<_>>();
    unique.len() == exclusions.len() && unique.is_subset(valid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unclip_domain::{
        DomainId, ProductDomainId, ProductDomainInput, ProductDomainVersion, ProductFrameId,
        ProductFrameVersion,
    };
    use unclip_epistemic::DomainVersion;
    use unclip_observe::ObservationId;

    fn binding(product: &str) -> ProductMeasurementBinding {
        ProductMeasurementBinding {
            product: ProductDomainId::new(product),
            product_version: ProductDomainVersion::new("1"),
            frame: ProductFrameId::new(format!("{product}-frame")),
            frame_version: ProductFrameVersion::new("1"),
            left: ProductDomainInput {
                domain: DomainId::new(format!("{product}-left")),
                version: DomainVersion::new("1"),
            },
            right: ProductDomainInput {
                domain: DomainId::new(format!("{product}-right")),
                version: DomainVersion::new("1"),
            },
        }
    }

    fn sequence(prefix: &str) -> ObservationSequence {
        ObservationSequence::new(
            (0..5)
                .map(|position| crate::OrderedObservation {
                    observation: ObservationId::new(format!("{prefix}-{position}")),
                    position,
                })
                .collect(),
        )
        .unwrap()
    }

    fn transition(prefix: &str, from: i64) -> CrossDomainTransition {
        CrossDomainTransition {
            from: ObservationId::new(format!("{prefix}-{from}")),
            to: ObservationId::new(format!("{prefix}-{}", from + 1)),
        }
    }

    fn axis(
        left: &str,
        right: &str,
        concordant: usize,
        discordant: usize,
    ) -> CrossDomainAxisMovement {
        CrossDomainAxisMovement {
            left: UnitId::new(left),
            right: UnitId::new(right),
            directional_concordance: (concordant as f64 - discordant as f64) / 4.0,
            transition_count: 4,
            concordant_transitions: concordant,
            discordant_transitions: discordant,
            left_only_transitions: 4 - concordant - discordant,
            right_only_transitions: 0,
            stationary_transitions: 0,
            excluded_transitions: vec![],
        }
    }

    fn sparse(left: &str, right: &str, prefix: &str) -> UnassessedCrossDomainAxisMovement {
        UnassessedCrossDomainAxisMovement {
            left: UnitId::new(left),
            right: UnitId::new(right),
            have: 2,
            need: 3,
            excluded_transitions: vec![transition(prefix, 0), transition(prefix, 1)],
        }
    }

    fn movement(
        product: &str,
        axes: Vec<CrossDomainAxisMovement>,
        sparse_axes: Vec<UnassessedCrossDomainAxisMovement>,
    ) -> CrossDomainInteractionMovement {
        CrossDomainInteractionMovement {
            binding: binding(product),
            sequence: sequence(product),
            axes,
            unassessed_axes: sparse_axes,
            observation_count: 5,
            transition_count: 4,
            minimum_transitions: 3,
        }
    }

    fn mapping(source: (&str, &str), target: (&str, &str)) -> CrossProductAxisMapping {
        CrossProductAxisMapping {
            source_left: UnitId::new(source.0),
            source_right: UnitId::new(source.1),
            target_left: UnitId::new(target.0),
            target_right: UnitId::new(target.1),
        }
    }

    fn config(
        mappings: Vec<CrossProductAxisMapping>,
        minimum: usize,
    ) -> CrossProductTransferConfig {
        CrossProductTransferConfig {
            mappings,
            minimum_transitions: NonZeroUsize::new(minimum).unwrap(),
        }
    }

    #[test]
    fn measures_signed_transfer_and_preserves_exact_zero() {
        let source = movement(
            "source",
            vec![axis("a", "x", 3, 1), axis("b", "y", 1, 3)],
            vec![],
        );
        let target = movement(
            "target",
            vec![axis("p", "u", 3, 1), axis("q", "v", 3, 1)],
            vec![],
        );
        let mappings = vec![
            mapping(("b", "y"), ("q", "v")),
            mapping(("a", "x"), ("p", "u")),
        ];
        let calculate =
            || cross_product_transfer(&source, &target, config(mappings.clone(), 3)).unwrap();
        let result = calculate();
        assert_eq!(calculate(), result);
        let CrossProductTransferOutcome::Value { transfer } = result else {
            panic!("expected transfer")
        };
        assert_eq!(transfer.transfers[0].absolute_change, 0.0);
        assert_eq!(transfer.transfers[1].directional_concordance_change, 1.0);
        assert_eq!(transfer.transfers[1].absolute_change, 1.0);
        assert!(transfer.unassessed_transfers.is_empty());
    }

    #[test]
    fn retains_sparse_mappings_and_higher_evidence_floors() {
        let source = movement(
            "source",
            vec![axis("a", "x", 3, 1)],
            vec![sparse("b", "y", "source")],
        );
        let target = movement(
            "target",
            vec![axis("p", "u", 3, 1), axis("q", "v", 3, 1)],
            vec![],
        );
        let mappings = vec![
            mapping(("a", "x"), ("p", "u")),
            mapping(("b", "y"), ("q", "v")),
        ];
        let CrossProductTransferOutcome::Value { transfer } =
            cross_product_transfer(&source, &target, config(mappings.clone(), 3)).unwrap()
        else {
            panic!("one mapping should remain measured")
        };
        assert_eq!(transfer.transfers.len(), 1);
        assert_eq!(transfer.unassessed_transfers[0].source_have, 2);
        let CrossProductTransferOutcome::Value { transfer } =
            cross_product_transfer(&source, &target, config(mappings.clone(), 1)).unwrap()
        else {
            panic!("a lower transfer floor must not promote an unmeasured source axis")
        };
        assert_eq!(transfer.unassessed_transfers[0].need, 3);

        assert!(matches!(
            cross_product_transfer(&source, &target, config(mappings, 5)).unwrap(),
            CrossProductTransferOutcome::InsufficientEvidence {
                have: 4,
                need: 5,
                ..
            }
        ));
    }

    #[test]
    fn rejects_inferred_duplicate_or_unmaterialized_mappings() {
        let source = movement("source", vec![axis("a", "x", 3, 1)], vec![]);
        let target = movement("target", vec![axis("p", "u", 3, 1)], vec![]);
        let valid = mapping(("a", "x"), ("p", "u"));
        assert_eq!(
            cross_product_transfer(&source, &source, config(vec![valid.clone()], 3)),
            Err(CrossProductTransferError::InvalidConfiguration)
        );
        assert_eq!(
            cross_product_transfer(&source, &target, config(vec![valid.clone(), valid], 3)),
            Err(CrossProductTransferError::InvalidMapping)
        );
        assert_eq!(
            cross_product_transfer(
                &source,
                &target,
                config(vec![mapping(("missing", "x"), ("p", "u"))], 3)
            ),
            Err(CrossProductTransferError::InvalidMapping)
        );
    }
}
