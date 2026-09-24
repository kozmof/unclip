//! Anonymous connected components of thresholded product interactions.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;

use crate::{CrossDomainMutualInformation, ProductMeasurementBinding, UnassessedCrossDomainAxis};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainCommunityConfig {
    pub minimum_mutual_information_bits: f64,
    pub minimum_samples: NonZeroUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductSide {
    Left,
    Right,
}

/// Side qualification prevents equal unit IDs from distinct domains collapsing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainCommunityMember {
    pub side: ProductSide,
    pub unit: UnitId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnassessedCrossDomainInteraction {
    pub left: UnitId,
    pub right: UnitId,
    pub have: usize,
    pub need: usize,
    pub excluded_observations: Vec<unclip_observe::ObservationId>,
}

/// Bipartite connected components without semantic labels.
///
/// Only materialized interaction coordinates present in the source profile are
/// considered. Absent Cartesian pairs are neither zero-valued nor unassessed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainCommunityDetection {
    pub binding: ProductMeasurementBinding,
    pub minimum_mutual_information_bits: f64,
    pub minimum_samples: usize,
    pub communities: Vec<Vec<CrossDomainCommunityMember>>,
    pub interaction_count: usize,
    pub assessed_interactions: usize,
    pub qualifying_interactions: usize,
    pub unassessed_interactions: Vec<UnassessedCrossDomainInteraction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CrossDomainCommunityOutcome {
    Value {
        detection: Box<CrossDomainCommunityDetection>,
    },
    InsufficientEvidence {
        have: usize,
        need: usize,
        unassessed_interactions: Vec<UnassessedCrossDomainInteraction>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossDomainCommunityError {
    InvalidConfiguration,
    InvalidInteractionEvidence,
}

impl std::fmt::Display for CrossDomainCommunityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration => write!(
                f,
                "cross-domain communities require a finite nonnegative MI threshold and at least two samples"
            ),
            Self::InvalidInteractionEvidence => {
                write!(f, "cross-domain community input is malformed or ambiguous")
            }
        }
    }
}

impl std::error::Error for CrossDomainCommunityError {}

/// Find deterministic connected components in a thresholded bipartite MI graph.
///
/// A measured interaction qualifies when its mutual information is at least the
/// configured threshold and its sample count meets the configured floor. A zero
/// threshold intentionally includes measured zero. Connectivity is transitive;
/// groups and members use stable side-qualified ordering.
pub fn detect_cross_domain_communities(
    profile: &CrossDomainMutualInformation,
    config: CrossDomainCommunityConfig,
) -> Result<CrossDomainCommunityOutcome, CrossDomainCommunityError> {
    if !config.minimum_mutual_information_bits.is_finite()
        || config.minimum_mutual_information_bits < 0.0
        || config.minimum_samples.get() < 2
    {
        return Err(CrossDomainCommunityError::InvalidConfiguration);
    }
    validate_profile(profile)?;

    let mut vertices = BTreeSet::new();
    let mut unassessed = Vec::new();
    for axis in &profile.unassessed_axes {
        vertices.insert(member(ProductSide::Left, &axis.left));
        vertices.insert(member(ProductSide::Right, &axis.right));
        unassessed.push(unassessed_axis(
            axis,
            axis.need.max(config.minimum_samples.get()),
        ));
    }
    for axis in &profile.axes {
        vertices.insert(member(ProductSide::Left, &axis.left));
        vertices.insert(member(ProductSide::Right, &axis.right));
        if axis.sample_count < config.minimum_samples.get() {
            unassessed.push(UnassessedCrossDomainInteraction {
                left: axis.left.clone(),
                right: axis.right.clone(),
                have: axis.sample_count,
                need: config.minimum_samples.get(),
                excluded_observations: axis.excluded_observations.clone(),
            });
        }
    }
    unassessed.sort_by(|left, right| (&left.left, &left.right).cmp(&(&right.left, &right.right)));

    let vertices = vertices.into_iter().collect::<Vec<_>>();
    let indexes = vertices
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, member)| (member, index))
        .collect::<BTreeMap<_, _>>();
    let mut parents = (0..vertices.len()).collect::<Vec<_>>();
    let mut assessed = 0;
    let mut qualifying = 0;
    for axis in &profile.axes {
        if axis.sample_count < config.minimum_samples.get() {
            continue;
        }
        assessed += 1;
        if axis.mutual_information_bits >= config.minimum_mutual_information_bits {
            qualifying += 1;
            let left = indexes[&member(ProductSide::Left, &axis.left)];
            let right = indexes[&member(ProductSide::Right, &axis.right)];
            let left_root = root(&mut parents, left);
            let right_root = root(&mut parents, right);
            parents[left_root.max(right_root)] = left_root.min(right_root);
        }
    }
    if assessed == 0 {
        return Ok(CrossDomainCommunityOutcome::InsufficientEvidence {
            have: unassessed
                .iter()
                .map(|interaction| interaction.have)
                .max()
                .unwrap_or(0),
            need: config.minimum_samples.get(),
            unassessed_interactions: unassessed,
        });
    }

    let mut groups = BTreeMap::<usize, Vec<CrossDomainCommunityMember>>::new();
    for (index, member) in vertices.into_iter().enumerate() {
        groups
            .entry(root(&mut parents, index))
            .or_default()
            .push(member);
    }
    Ok(CrossDomainCommunityOutcome::Value {
        detection: Box::new(CrossDomainCommunityDetection {
            binding: profile.binding.clone(),
            minimum_mutual_information_bits: config.minimum_mutual_information_bits,
            minimum_samples: config.minimum_samples.get(),
            communities: groups.into_values().collect(),
            interaction_count: profile.axes.len() + profile.unassessed_axes.len(),
            assessed_interactions: assessed,
            qualifying_interactions: qualifying,
            unassessed_interactions: unassessed,
        }),
    })
}

fn member(side: ProductSide, unit: &UnitId) -> CrossDomainCommunityMember {
    CrossDomainCommunityMember {
        side,
        unit: unit.clone(),
    }
}

fn unassessed_axis(
    axis: &UnassessedCrossDomainAxis,
    need: usize,
) -> UnassessedCrossDomainInteraction {
    UnassessedCrossDomainInteraction {
        left: axis.left.clone(),
        right: axis.right.clone(),
        have: axis.have,
        need,
        excluded_observations: axis.excluded_observations.clone(),
    }
}

fn validate_profile(
    profile: &CrossDomainMutualInformation,
) -> Result<(), CrossDomainCommunityError> {
    let binding = &profile.binding;
    if binding.product.0.trim().is_empty()
        || binding.product_version.0.trim().is_empty()
        || binding.frame.0.trim().is_empty()
        || binding.frame_version.0.trim().is_empty()
        || binding.left.domain.0.trim().is_empty()
        || binding.left.version.0.trim().is_empty()
        || binding.right.domain.0.trim().is_empty()
        || binding.right.version.0.trim().is_empty()
        || binding.left.domain == binding.right.domain
        || profile.minimum_samples < 2
        || profile.requested_bins == 0
        || profile.requested_bins > 1_024
        || profile.axes.is_empty()
    {
        return Err(CrossDomainCommunityError::InvalidInteractionEvidence);
    }
    let mut coordinates = BTreeSet::new();
    for axis in &profile.axes {
        if axis.left.0.trim().is_empty()
            || axis.right.0.trim().is_empty()
            || !axis.mutual_information_bits.is_finite()
            || axis.mutual_information_bits < 0.0
            || axis.sample_count < profile.minimum_samples
            || axis
                .sample_count
                .checked_add(axis.excluded_observations.len())
                != Some(profile.observation_count)
            || axis
                .left_boundaries
                .iter()
                .chain(&axis.right_boundaries)
                .any(|boundary| !boundary.is_finite())
            || axis
                .left_boundaries
                .windows(2)
                .chain(axis.right_boundaries.windows(2))
                .any(|pair| pair[0] > pair[1])
            || !unique_observations(&axis.excluded_observations)
            || !coordinates.insert((&axis.left, &axis.right))
        {
            return Err(CrossDomainCommunityError::InvalidInteractionEvidence);
        }
    }
    for axis in &profile.unassessed_axes {
        if axis.left.0.trim().is_empty()
            || axis.right.0.trim().is_empty()
            || axis.need != profile.minimum_samples
            || axis.have >= axis.need
            || axis.have.checked_add(axis.excluded_observations.len())
                != Some(profile.observation_count)
            || !unique_observations(&axis.excluded_observations)
            || !coordinates.insert((&axis.left, &axis.right))
        {
            return Err(CrossDomainCommunityError::InvalidInteractionEvidence);
        }
    }
    Ok(())
}

fn unique_observations(values: &[unclip_observe::ObservationId]) -> bool {
    values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CrossDomainAxisMutualInformation, ProductMeasurementBinding};
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
                domain: DomainId::new("left-domain"),
                version: DomainVersion::new("3"),
            },
            right: ProductDomainInput {
                domain: DomainId::new("right-domain"),
                version: DomainVersion::new("4"),
            },
        }
    }

    fn axis(
        left: &str,
        right: &str,
        value: f64,
        samples: usize,
    ) -> CrossDomainAxisMutualInformation {
        CrossDomainAxisMutualInformation {
            left: UnitId::new(left),
            right: UnitId::new(right),
            mutual_information_bits: value,
            sample_count: samples,
            left_boundaries: vec![0.5],
            right_boundaries: vec![0.5],
            excluded_observations: (samples..4)
                .map(|index| unclip_observe::ObservationId::new(format!("missing-{index}")))
                .collect(),
        }
    }

    fn profile(axes: Vec<CrossDomainAxisMutualInformation>) -> CrossDomainMutualInformation {
        CrossDomainMutualInformation {
            binding: binding(),
            axes,
            unassessed_axes: vec![],
            observation_count: 4,
            requested_bins: 2,
            minimum_samples: 2,
        }
    }

    fn config(threshold: f64, samples: usize) -> CrossDomainCommunityConfig {
        CrossDomainCommunityConfig {
            minimum_mutual_information_bits: threshold,
            minimum_samples: NonZeroUsize::new(samples).unwrap(),
        }
    }

    fn member_value(side: ProductSide, unit: &str) -> CrossDomainCommunityMember {
        CrossDomainCommunityMember {
            side,
            unit: UnitId::new(unit),
        }
    }

    #[test]
    fn finds_stable_transitive_bipartite_components() {
        let input = profile(vec![
            axis("a", "x", 1.0, 4),
            axis("b", "x", 0.8, 4),
            axis("b", "y", 0.1, 4),
        ]);
        let calculate = || detect_cross_domain_communities(&input, config(0.5, 2)).unwrap();
        let result = calculate();
        assert_eq!(calculate(), result);
        let CrossDomainCommunityOutcome::Value { detection } = result else {
            panic!("expected measured communities")
        };
        assert_eq!(
            detection.communities,
            vec![
                vec![
                    member_value(ProductSide::Left, "a"),
                    member_value(ProductSide::Left, "b"),
                    member_value(ProductSide::Right, "x"),
                ],
                vec![member_value(ProductSide::Right, "y")],
            ]
        );
        assert_eq!(detection.assessed_interactions, 3);
        assert_eq!(detection.qualifying_interactions, 2);
        assert_eq!(
            serde_json::from_value::<CrossDomainCommunityDetection>(
                serde_json::to_value(&detection).unwrap()
            )
            .unwrap(),
            *detection
        );
    }

    #[test]
    fn measured_zero_qualifies_only_at_an_explicit_zero_threshold() {
        let input = profile(vec![axis("same", "same", 0.0, 4)]);
        let CrossDomainCommunityOutcome::Value { detection } =
            detect_cross_domain_communities(&input, config(0.0, 2)).unwrap()
        else {
            panic!("zero threshold should be measured")
        };
        assert_eq!(detection.communities.len(), 1);
        assert_eq!(detection.communities[0].len(), 2);
        assert_ne!(
            detection.communities[0][0].side,
            detection.communities[0][1].side
        );

        let CrossDomainCommunityOutcome::Value { detection } =
            detect_cross_domain_communities(&input, config(0.1, 2)).unwrap()
        else {
            panic!("nonqualifying measured edge should retain singleton partition")
        };
        assert_eq!(detection.communities.len(), 2);
    }

    #[test]
    fn higher_sample_floor_preserves_unassessed_interactions() {
        let input = profile(vec![axis("a", "x", 1.0, 4), axis("b", "y", 1.0, 3)]);
        let CrossDomainCommunityOutcome::Value { detection } =
            detect_cross_domain_communities(&input, config(0.5, 4)).unwrap()
        else {
            panic!("one assessed interaction should yield communities")
        };
        assert_eq!(detection.assessed_interactions, 1);
        assert_eq!(detection.unassessed_interactions.len(), 1);
        assert_eq!(detection.unassessed_interactions[0].have, 3);

        assert!(matches!(
            detect_cross_domain_communities(&input, config(0.5, 5)).unwrap(),
            CrossDomainCommunityOutcome::InsufficientEvidence {
                have: 4,
                need: 5,
                ..
            }
        ));
    }

    #[test]
    fn rejects_invalid_thresholds_and_ambiguous_coordinates() {
        let input = profile(vec![axis("a", "x", 1.0, 4)]);
        for threshold in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                detect_cross_domain_communities(&input, config(threshold, 2)),
                Err(CrossDomainCommunityError::InvalidConfiguration)
            );
        }
        let duplicate = profile(vec![axis("a", "x", 1.0, 4), axis("a", "x", 0.5, 4)]);
        assert_eq!(
            detect_cross_domain_communities(&duplicate, config(0.5, 2)),
            Err(CrossDomainCommunityError::InvalidInteractionEvidence)
        );
    }
}
