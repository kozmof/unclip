//! Deterministic empirical mutual information across product-domain axes.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use unclip_domain::{ProductFrameAxis, UnitId};
use unclip_observe::ObservationId;

use crate::CrossDomainSample;

const MAXIMUM_BINS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainMutualInformationConfig {
    pub minimum_samples: NonZeroUsize,
    /// Requested equal-width bins per side. Constant variables use one bin.
    pub bins: NonZeroUsize,
}

impl Default for CrossDomainMutualInformationConfig {
    fn default() -> Self {
        Self {
            minimum_samples: NonZeroUsize::new(2).expect("two is nonzero"),
            bins: NonZeroUsize::new(4).expect("four is nonzero"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainAxisMutualInformation {
    pub left: UnitId,
    pub right: UnitId,
    pub mutual_information_bits: f64,
    pub sample_count: usize,
    /// Interior equal-width boundaries; empty means the retained side is constant.
    pub left_boundaries: Vec<f64>,
    pub right_boundaries: Vec<f64>,
    pub excluded_observations: Vec<ObservationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnassessedCrossDomainAxis {
    pub left: UnitId,
    pub right: UnitId,
    pub have: usize,
    pub need: usize,
    pub excluded_observations: Vec<ObservationId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainMutualInformation {
    pub axes: Vec<CrossDomainAxisMutualInformation>,
    pub unassessed_axes: Vec<UnassessedCrossDomainAxis>,
    pub observation_count: usize,
    pub requested_bins: usize,
    pub minimum_samples: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CrossDomainMutualInformationOutcome {
    Value {
        analysis: CrossDomainMutualInformation,
    },
    InsufficientEvidence {
        have: usize,
        need: usize,
        unassessed_axes: Vec<UnassessedCrossDomainAxis>,
    },
    NoAxes {
        observation_count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossDomainMutualInformationError {
    InvalidConfiguration,
    InvalidAxis {
        left: UnitId,
        right: UnitId,
    },
    DuplicateAxis {
        left: UnitId,
        right: UnitId,
    },
    DuplicateObservation(ObservationId),
    InvalidSample(ObservationId),
    UnexpectedUnit {
        observation: ObservationId,
        side: &'static str,
        unit: UnitId,
    },
    NonFiniteResult,
}

impl std::fmt::Display for CrossDomainMutualInformationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                write!(
                    f,
                    "cross-domain MI requires at least two complete samples and at most 1024 bins"
                )
            }
            Self::InvalidAxis { left, right } => write!(
                f,
                "cross-domain MI axis has an empty unit: {} x {}",
                left.0, right.0
            ),
            Self::DuplicateAxis { left, right } => write!(
                f,
                "duplicate cross-domain MI axis: {} x {}",
                left.0, right.0
            ),
            Self::DuplicateObservation(observation) => {
                write!(f, "duplicate cross-domain MI observation {}", observation.0)
            }
            Self::InvalidSample(observation) => write!(
                f,
                "cross-domain MI observation {} contains an invalid value",
                observation.0
            ),
            Self::UnexpectedUnit {
                observation,
                side,
                unit,
            } => write!(
                f,
                "cross-domain MI observation {} contains unexpected {side} unit {}",
                observation.0, unit.0
            ),
            Self::NonFiniteResult => write!(f, "cross-domain MI produced a non-finite result"),
        }
    }
}

impl std::error::Error for CrossDomainMutualInformationError {}

/// Estimate per-axis discrete mutual information after explicit equal-width binning.
///
/// Each axis uses pairwise-complete samples. Bin boundaries are derived only from
/// its retained pairs, stored in the result, and applied with half-open intervals
/// whose boundary values enter the upper bin. No missing value is filled with zero.
pub fn cross_domain_mutual_information(
    axes: &[ProductFrameAxis],
    samples: &[CrossDomainSample],
    config: CrossDomainMutualInformationConfig,
) -> Result<CrossDomainMutualInformationOutcome, CrossDomainMutualInformationError> {
    if config.minimum_samples.get() < 2 || config.bins.get() > MAXIMUM_BINS {
        return Err(CrossDomainMutualInformationError::InvalidConfiguration);
    }
    let mut coordinates = BTreeSet::new();
    let mut left_units = BTreeSet::new();
    let mut right_units = BTreeSet::new();
    for axis in axes {
        if axis.left.0.trim().is_empty() || axis.right.0.trim().is_empty() {
            return Err(CrossDomainMutualInformationError::InvalidAxis {
                left: axis.left.clone(),
                right: axis.right.clone(),
            });
        }
        if !coordinates.insert((&axis.left, &axis.right)) {
            return Err(CrossDomainMutualInformationError::DuplicateAxis {
                left: axis.left.clone(),
                right: axis.right.clone(),
            });
        }
        left_units.insert(&axis.left);
        right_units.insert(&axis.right);
    }

    if axes.is_empty() {
        return Ok(CrossDomainMutualInformationOutcome::NoAxes {
            observation_count: samples.len(),
        });
    }

    let mut ordered = samples.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|sample| &sample.observation);
    for pair in ordered.windows(2) {
        if pair[0].observation == pair[1].observation {
            return Err(CrossDomainMutualInformationError::DuplicateObservation(
                pair[0].observation.clone(),
            ));
        }
    }
    for sample in &ordered {
        if sample.observation.0.trim().is_empty()
            || sample
                .left
                .values()
                .chain(sample.right.values())
                .any(|value| !value.is_finite())
        {
            return Err(CrossDomainMutualInformationError::InvalidSample(
                sample.observation.clone(),
            ));
        }
        if let Some(unit) = sample.left.keys().find(|unit| !left_units.contains(unit)) {
            return Err(CrossDomainMutualInformationError::UnexpectedUnit {
                observation: sample.observation.clone(),
                side: "left",
                unit: unit.clone(),
            });
        }
        if let Some(unit) = sample.right.keys().find(|unit| !right_units.contains(unit)) {
            return Err(CrossDomainMutualInformationError::UnexpectedUnit {
                observation: sample.observation.clone(),
                side: "right",
                unit: unit.clone(),
            });
        }
    }
    let mut measured = Vec::new();
    let mut unassessed = Vec::new();
    for axis in axes {
        let mut pairs = Vec::new();
        let mut excluded_observations = Vec::new();
        for sample in &ordered {
            match (sample.left.get(&axis.left), sample.right.get(&axis.right)) {
                (Some(left), Some(right)) => pairs.push((*left, *right)),
                _ => excluded_observations.push(sample.observation.clone()),
            }
        }
        if pairs.len() < config.minimum_samples.get() {
            unassessed.push(UnassessedCrossDomainAxis {
                left: axis.left.clone(),
                right: axis.right.clone(),
                have: pairs.len(),
                need: config.minimum_samples.get(),
                excluded_observations,
            });
            continue;
        }
        let left_boundaries =
            equal_width_boundaries(pairs.iter().map(|(left, _)| *left), config.bins.get());
        let right_boundaries =
            equal_width_boundaries(pairs.iter().map(|(_, right)| *right), config.bins.get());
        if left_boundaries
            .iter()
            .chain(&right_boundaries)
            .any(|boundary| !boundary.is_finite())
        {
            return Err(CrossDomainMutualInformationError::NonFiniteResult);
        }
        let mut mutual_information_bits =
            empirical_mutual_information(&pairs, &left_boundaries, &right_boundaries);
        if !mutual_information_bits.is_finite() || mutual_information_bits < -1e-12 {
            return Err(CrossDomainMutualInformationError::NonFiniteResult);
        }
        mutual_information_bits = mutual_information_bits.max(0.0);
        measured.push(CrossDomainAxisMutualInformation {
            left: axis.left.clone(),
            right: axis.right.clone(),
            mutual_information_bits,
            sample_count: pairs.len(),
            left_boundaries,
            right_boundaries,
            excluded_observations,
        });
    }

    if measured.is_empty() {
        let have = unassessed.iter().map(|axis| axis.have).max().unwrap_or(0);
        return Ok(CrossDomainMutualInformationOutcome::InsufficientEvidence {
            have,
            need: config.minimum_samples.get(),
            unassessed_axes: unassessed,
        });
    }
    Ok(CrossDomainMutualInformationOutcome::Value {
        analysis: CrossDomainMutualInformation {
            axes: measured,
            unassessed_axes: unassessed,
            observation_count: ordered.len(),
            requested_bins: config.bins.get(),
            minimum_samples: config.minimum_samples.get(),
        },
    })
}

fn equal_width_boundaries(values: impl Iterator<Item = f64>, bins: usize) -> Vec<f64> {
    let (minimum, maximum) = values.fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
    );
    if minimum == maximum {
        return Vec::new();
    }
    (1..bins)
        .map(|index| {
            let fraction = index as f64 / bins as f64;
            minimum * (1.0 - fraction) + maximum * fraction
        })
        .collect()
}

fn bin(value: f64, boundaries: &[f64]) -> usize {
    boundaries.partition_point(|boundary| value >= *boundary)
}

fn empirical_mutual_information(
    pairs: &[(f64, f64)],
    left_boundaries: &[f64],
    right_boundaries: &[f64],
) -> f64 {
    let mut left_counts = BTreeMap::<usize, usize>::new();
    let mut right_counts = BTreeMap::<usize, usize>::new();
    let mut joint_counts = BTreeMap::<(usize, usize), usize>::new();
    for (left, right) in pairs {
        let left = bin(*left, left_boundaries);
        let right = bin(*right, right_boundaries);
        *left_counts.entry(left).or_default() += 1;
        *right_counts.entry(right).or_default() += 1;
        *joint_counts.entry((left, right)).or_default() += 1;
    }
    let count = pairs.len() as f64;
    joint_counts
        .into_iter()
        .map(|((left, right), joint_count)| {
            let joint_probability = joint_count as f64 / count;
            let left_probability = left_counts[&left] as f64 / count;
            let right_probability = right_counts[&right] as f64 / count;
            joint_probability * (joint_probability / (left_probability * right_probability)).log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(left: &str, right: &str) -> ProductFrameAxis {
        ProductFrameAxis {
            left: UnitId::new(left),
            right: UnitId::new(right),
            label: None,
        }
    }

    fn sample(id: &str, left: &[(&str, f64)], right: &[(&str, f64)]) -> CrossDomainSample {
        CrossDomainSample {
            observation: ObservationId::new(id),
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

    fn config(bins: usize) -> CrossDomainMutualInformationConfig {
        CrossDomainMutualInformationConfig {
            minimum_samples: NonZeroUsize::new(2).unwrap(),
            bins: NonZeroUsize::new(bins).unwrap(),
        }
    }

    #[test]
    fn measures_dependency_and_exact_zero_in_bits() {
        let dependent = [
            sample("a", &[("left", 0.0)], &[("right", 0.0)]),
            sample("b", &[("left", 0.0)], &[("right", 0.0)]),
            sample("c", &[("left", 1.0)], &[("right", 1.0)]),
            sample("d", &[("left", 1.0)], &[("right", 1.0)]),
        ];
        let calculate = |samples: &[CrossDomainSample]| {
            cross_domain_mutual_information(&[axis("left", "right")], samples, config(2)).unwrap()
        };
        let result = calculate(&dependent);
        assert_eq!(calculate(&dependent), result);
        let CrossDomainMutualInformationOutcome::Value { analysis } = result else {
            panic!("expected measured MI")
        };
        assert_eq!(analysis.axes[0].mutual_information_bits, 1.0);
        assert_eq!(analysis.axes[0].left_boundaries, vec![0.5]);

        let independent = [
            sample("a", &[("left", 0.0)], &[("right", 0.0)]),
            sample("b", &[("left", 0.0)], &[("right", 1.0)]),
            sample("c", &[("left", 1.0)], &[("right", 0.0)]),
            sample("d", &[("left", 1.0)], &[("right", 1.0)]),
        ];
        let CrossDomainMutualInformationOutcome::Value { analysis } = calculate(&independent)
        else {
            panic!("zero MI must remain measured")
        };
        assert_eq!(analysis.axes[0].mutual_information_bits, 0.0);
    }

    #[test]
    fn uses_pairwise_complete_rows_and_retains_sparse_axes() {
        let samples = [
            sample(
                "a",
                &[("left-a", 0.0), ("left-b", 0.0)],
                &[("right-a", 0.0), ("right-b", 0.0)],
            ),
            sample(
                "b",
                &[("left-a", 1.0)],
                &[("right-a", 1.0), ("right-b", 1.0)],
            ),
        ];
        let CrossDomainMutualInformationOutcome::Value { analysis } =
            cross_domain_mutual_information(
                &[axis("left-a", "right-a"), axis("left-b", "right-b")],
                &samples,
                config(2),
            )
            .unwrap()
        else {
            panic!("one assessed axis should yield a value")
        };
        assert_eq!(analysis.axes.len(), 1);
        assert_eq!(analysis.axes[0].sample_count, 2);
        assert_eq!(analysis.unassessed_axes.len(), 1);
        assert_eq!(analysis.unassessed_axes[0].have, 1);
        assert_eq!(
            analysis.unassessed_axes[0].excluded_observations,
            vec![ObservationId::new("b")]
        );
    }

    #[test]
    fn all_sparse_axes_and_no_axes_are_distinct() {
        let sparse = [sample("only", &[("left", 0.0)], &[("right", 0.0)])];
        assert!(matches!(
            cross_domain_mutual_information(&[axis("left", "right")], &sparse, config(2)).unwrap(),
            CrossDomainMutualInformationOutcome::InsufficientEvidence {
                have: 1,
                need: 2,
                ..
            }
        ));
        assert_eq!(
            cross_domain_mutual_information(
                &[],
                &[sample(
                    "ignored",
                    &[("unselected-left", f64::NAN)],
                    &[("unselected-right", 1.0)],
                )],
                config(2),
            )
            .unwrap(),
            CrossDomainMutualInformationOutcome::NoAxes {
                observation_count: 1
            }
        );
    }

    #[test]
    fn rejects_an_unbounded_bin_request() {
        assert_eq!(
            cross_domain_mutual_information(
                &[axis("left", "right")],
                &[],
                CrossDomainMutualInformationConfig {
                    minimum_samples: NonZeroUsize::new(2).unwrap(),
                    bins: NonZeroUsize::new(MAXIMUM_BINS + 1).unwrap(),
                },
            )
            .unwrap_err(),
            CrossDomainMutualInformationError::InvalidConfiguration
        );
    }

    #[test]
    fn rejects_duplicate_observations_unexpected_units_and_nonfinite_values() {
        let duplicate = sample("same", &[("left", 0.0)], &[("right", 0.0)]);
        assert!(matches!(
            cross_domain_mutual_information(
                &[axis("left", "right")],
                &[duplicate.clone(), duplicate],
                config(2),
            ),
            Err(CrossDomainMutualInformationError::DuplicateObservation(_))
        ));
        assert!(matches!(
            cross_domain_mutual_information(
                &[axis("left", "right")],
                &[sample("extra", &[("other", 0.0)], &[("right", 0.0)])],
                config(2),
            ),
            Err(CrossDomainMutualInformationError::UnexpectedUnit { .. })
        ));
        assert!(matches!(
            cross_domain_mutual_information(
                &[axis("left", "right")],
                &[sample("nan", &[("left", f64::NAN)], &[("right", 0.0)])],
                config(2),
            ),
            Err(CrossDomainMutualInformationError::InvalidSample(_))
        ));
    }
}
