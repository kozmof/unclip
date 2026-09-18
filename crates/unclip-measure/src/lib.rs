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
use unclip_observe::{ObservationId, ObservedUnitId};

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

/// A unit's rank at one observation in a trajectory.
///
/// `Missing` means the unit was absent from the partial state, while `Unknown`
/// means the state explicitly included the unit without assigning it a rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RankPosition {
    Ranked { rank: usize },
    Unknown,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankSample {
    pub observation: ObservationId,
    pub position: RankPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankTrajectory {
    pub unit: UnitId,
    pub samples: Vec<RankSample>,
}

/// The relative rank of an ordered unit pair at one observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RelativeRankPosition {
    Difference { value: isize },
    Unknown,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelativeRankSample {
    pub observation: ObservationId,
    pub position: RelativeRankPosition,
}

/// The trajectory `rank(left) - rank(right)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelativeRankTrajectory {
    pub left: UnitId,
    pub right: UnitId,
    pub samples: Vec<RelativeRankSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correlation {
    pub coefficient: f64,
    pub sample_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrajectoryAlignmentError {
    pub index: usize,
    pub left: Option<ObservationId>,
    pub right: Option<ObservationId>,
}

/// Constructs per-unit trajectories from states in caller-supplied order.
///
/// Ranks are one-based dense ranks, so every unit in a tied tier receives the
/// same rank. Units mentioned by a state but absent from `frame_units` are
/// retained rather than silently discarded.
pub fn construct_rank_trajectories(
    frame_units: &[UnitId],
    states: &[(ObservationId, RankedState)],
) -> Vec<RankTrajectory> {
    let mut units = frame_units
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for (_, state) in states {
        units.extend(state.tiers.iter().flatten().cloned());
        units.extend(state.unknown.iter().cloned());
    }

    units
        .into_iter()
        .map(|unit| RankTrajectory {
            samples: states
                .iter()
                .map(|(observation, state)| RankSample {
                    observation: observation.clone(),
                    position: rank_position(state, &unit),
                })
                .collect(),
            unit,
        })
        .collect()
}

/// Constructs a trajectory for every unordered pair of observed frame units.
///
/// Pair orientation follows the stable unit ordering. A difference is emitted
/// only when both units have known ranks; sparse states remain sparse.
pub fn construct_relative_rank_trajectories(
    frame_units: &[UnitId],
    states: &[(ObservationId, RankedState)],
) -> Vec<RelativeRankTrajectory> {
    let trajectories = construct_rank_trajectories(frame_units, states);
    let mut relative = Vec::new();

    for (left_index, left) in trajectories.iter().enumerate() {
        for right in trajectories.iter().skip(left_index + 1) {
            relative.push(RelativeRankTrajectory {
                left: left.unit.clone(),
                right: right.unit.clone(),
                samples: left
                    .samples
                    .iter()
                    .zip(&right.samples)
                    .map(|(left_sample, right_sample)| RelativeRankSample {
                        observation: left_sample.observation.clone(),
                        position: relative_rank_position(
                            left_sample.position,
                            right_sample.position,
                        ),
                    })
                    .collect(),
            });
        }
    }

    relative
}

/// Calculates Spearman's rank correlation over pairwise-complete samples.
///
/// Explicitly unknown and missing positions are excluded. Ties receive their
/// average rank. `None` indicates fewer than two comparable samples or zero
/// variance in either retained series.
pub fn spearman_correlation(
    left: &RankTrajectory,
    right: &RankTrajectory,
) -> Result<Option<Correlation>, TrajectoryAlignmentError> {
    let mut left_values = Vec::new();
    let mut right_values = Vec::new();

    for (index, (left_sample, right_sample)) in left.samples.iter().zip(&right.samples).enumerate()
    {
        if left_sample.observation != right_sample.observation {
            return Err(TrajectoryAlignmentError {
                index,
                left: Some(left_sample.observation.clone()),
                right: Some(right_sample.observation.clone()),
            });
        }
        if let (
            RankPosition::Ranked { rank: left_rank },
            RankPosition::Ranked { rank: right_rank },
        ) = (left_sample.position, right_sample.position)
        {
            left_values.push(left_rank);
            right_values.push(right_rank);
        }
    }

    if left.samples.len() != right.samples.len() {
        let index = left.samples.len().min(right.samples.len());
        return Err(TrajectoryAlignmentError {
            index,
            left: left
                .samples
                .get(index)
                .map(|sample| sample.observation.clone()),
            right: right
                .samples
                .get(index)
                .map(|sample| sample.observation.clone()),
        });
    }

    let sample_count = left_values.len();
    if sample_count < 2 {
        return Ok(None);
    }
    let left_ranks = average_ranks(&left_values);
    let right_ranks = average_ranks(&right_values);
    Ok(
        pearson_correlation(&left_ranks, &right_ranks).map(|coefficient| Correlation {
            coefficient,
            sample_count,
        }),
    )
}

fn average_ranks(values: &[usize]) -> Vec<f64> {
    let mut ordered = values.iter().copied().enumerate().collect::<Vec<_>>();
    ordered.sort_by_key(|(_, value)| *value);
    let mut ranks = vec![0.0; values.len()];
    let mut start = 0;
    while start < ordered.len() {
        let mut end = start + 1;
        while end < ordered.len() && ordered[end].1 == ordered[start].1 {
            end += 1;
        }
        let average = ((start + 1 + end) as f64) / 2.0;
        for &(original, _) in &ordered[start..end] {
            ranks[original] = average;
        }
        start = end;
    }
    ranks
}

fn pearson_correlation(left: &[f64], right: &[f64]) -> Option<f64> {
    let count = left.len() as f64;
    let left_mean = left.iter().sum::<f64>() / count;
    let right_mean = right.iter().sum::<f64>() / count;
    let mut covariance = 0.0;
    let mut left_variance = 0.0;
    let mut right_variance = 0.0;
    for (&left, &right) in left.iter().zip(right) {
        let left_delta = left - left_mean;
        let right_delta = right - right_mean;
        covariance += left_delta * right_delta;
        left_variance += left_delta * left_delta;
        right_variance += right_delta * right_delta;
    }
    let denominator = (left_variance * right_variance).sqrt();
    (denominator > 0.0).then_some(covariance / denominator)
}

fn rank_position(state: &RankedState, unit: &UnitId) -> RankPosition {
    if let Some(rank) = state
        .tiers
        .iter()
        .position(|tier| tier.iter().any(|candidate| candidate == unit))
    {
        RankPosition::Ranked { rank: rank + 1 }
    } else if state.unknown.contains(unit) {
        RankPosition::Unknown
    } else {
        RankPosition::Missing
    }
}

fn relative_rank_position(left: RankPosition, right: RankPosition) -> RelativeRankPosition {
    match (left, right) {
        (RankPosition::Missing, _) | (_, RankPosition::Missing) => RelativeRankPosition::Missing,
        (RankPosition::Unknown, _) | (_, RankPosition::Unknown) => RelativeRankPosition::Unknown,
        (RankPosition::Ranked { rank: left }, RankPosition::Ranked { rank: right }) => {
            RelativeRankPosition::Difference {
                // Vec lengths cannot exceed isize::MAX, so ranks obtained from
                // tier indexes are representable as isize.
                value: left as isize - right as isize,
            }
        }
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

    fn unit(id: &str) -> UnitId {
        UnitId::new(id)
    }

    #[test]
    fn constructs_rank_trajectories_in_observation_order() {
        let states = vec![
            (
                ObservationId::new("first"),
                RankedState {
                    tiers: vec![vec![unit("a"), unit("b")], vec![unit("c")]],
                    unknown: vec![],
                    unresolved: vec![],
                },
            ),
            (
                ObservationId::new("second"),
                RankedState {
                    tiers: vec![vec![unit("c")], vec![unit("a")]],
                    unknown: vec![unit("b")],
                    unresolved: vec![],
                },
            ),
        ];

        let trajectories = construct_rank_trajectories(&[unit("a"), unit("b"), unit("c")], &states);

        assert_eq!(
            trajectories,
            vec![
                RankTrajectory {
                    unit: unit("a"),
                    samples: vec![
                        RankSample {
                            observation: ObservationId::new("first"),
                            position: RankPosition::Ranked { rank: 1 },
                        },
                        RankSample {
                            observation: ObservationId::new("second"),
                            position: RankPosition::Ranked { rank: 2 },
                        },
                    ],
                },
                RankTrajectory {
                    unit: unit("b"),
                    samples: vec![
                        RankSample {
                            observation: ObservationId::new("first"),
                            position: RankPosition::Ranked { rank: 1 },
                        },
                        RankSample {
                            observation: ObservationId::new("second"),
                            position: RankPosition::Unknown,
                        },
                    ],
                },
                RankTrajectory {
                    unit: unit("c"),
                    samples: vec![
                        RankSample {
                            observation: ObservationId::new("first"),
                            position: RankPosition::Ranked { rank: 2 },
                        },
                        RankSample {
                            observation: ObservationId::new("second"),
                            position: RankPosition::Ranked { rank: 1 },
                        },
                    ],
                },
            ]
        );
    }

    #[test]
    fn preserves_missing_units_and_units_discovered_in_states() {
        let states = vec![
            (
                ObservationId::new("first"),
                RankedState {
                    tiers: vec![vec![unit("outside-frame")]],
                    unknown: vec![],
                    unresolved: vec![],
                },
            ),
            (
                ObservationId::new("second"),
                RankedState {
                    tiers: vec![],
                    unknown: vec![unit("frame-unit")],
                    unresolved: vec![],
                },
            ),
        ];

        let trajectories = construct_rank_trajectories(&[unit("frame-unit")], &states);

        assert_eq!(trajectories.len(), 2);
        assert_eq!(trajectories[0].samples[0].position, RankPosition::Missing);
        assert_eq!(trajectories[0].samples[1].position, RankPosition::Unknown);
        assert_eq!(
            trajectories[1].samples[0].position,
            RankPosition::Ranked { rank: 1 }
        );
        assert_eq!(trajectories[1].samples[1].position, RankPosition::Missing);
    }

    #[test]
    fn constructs_sparse_relative_rank_trajectories() {
        let states = vec![
            (
                ObservationId::new("known"),
                RankedState {
                    tiers: vec![vec![unit("a")], vec![unit("b")], vec![unit("c")]],
                    unknown: vec![],
                    unresolved: vec![],
                },
            ),
            (
                ObservationId::new("unknown"),
                RankedState {
                    tiers: vec![vec![unit("a")]],
                    unknown: vec![unit("b"), unit("c")],
                    unresolved: vec![],
                },
            ),
            (
                ObservationId::new("missing"),
                RankedState {
                    tiers: vec![vec![unit("a")], vec![unit("b")]],
                    unknown: vec![],
                    unresolved: vec![],
                },
            ),
        ];

        let trajectories =
            construct_relative_rank_trajectories(&[unit("a"), unit("b"), unit("c")], &states);

        assert_eq!(trajectories.len(), 3);
        assert_eq!(
            trajectories[0],
            RelativeRankTrajectory {
                left: unit("a"),
                right: unit("b"),
                samples: vec![
                    RelativeRankSample {
                        observation: ObservationId::new("known"),
                        position: RelativeRankPosition::Difference { value: -1 },
                    },
                    RelativeRankSample {
                        observation: ObservationId::new("unknown"),
                        position: RelativeRankPosition::Unknown,
                    },
                    RelativeRankSample {
                        observation: ObservationId::new("missing"),
                        position: RelativeRankPosition::Difference { value: -1 },
                    },
                ],
            }
        );
        assert_eq!(
            trajectories[1].samples[2].position,
            RelativeRankPosition::Missing
        );
        assert_eq!(
            trajectories[2].samples[0].position,
            RelativeRankPosition::Difference { value: -1 }
        );
    }

    fn trajectory(unit_id: &str, positions: &[(&str, RankPosition)]) -> RankTrajectory {
        RankTrajectory {
            unit: unit(unit_id),
            samples: positions
                .iter()
                .map(|(observation, position)| RankSample {
                    observation: ObservationId::new(*observation),
                    position: *position,
                })
                .collect(),
        }
    }

    #[test]
    fn spearman_uses_pairwise_complete_samples_and_average_ties() {
        let left = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Unknown),
                ("three", RankPosition::Ranked { rank: 2 }),
                ("four", RankPosition::Ranked { rank: 2 }),
                ("five", RankPosition::Ranked { rank: 4 }),
            ],
        );
        let right = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 4 }),
                ("two", RankPosition::Ranked { rank: 3 }),
                ("three", RankPosition::Ranked { rank: 2 }),
                ("four", RankPosition::Ranked { rank: 2 }),
                ("five", RankPosition::Ranked { rank: 1 }),
            ],
        );

        let correlation = spearman_correlation(&left, &right).unwrap().unwrap();

        assert_eq!(correlation.sample_count, 4);
        assert!((correlation.coefficient + 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn spearman_reports_undefined_and_misaligned_trajectories() {
        let constant = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 1 }),
            ],
        );
        let varying = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
            ],
        );
        assert_eq!(spearman_correlation(&constant, &varying).unwrap(), None);

        let misaligned = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("other", RankPosition::Ranked { rank: 2 }),
            ],
        );
        assert_eq!(
            spearman_correlation(&constant, &misaligned),
            Err(TrajectoryAlignmentError {
                index: 1,
                left: Some(ObservationId::new("two")),
                right: Some(ObservationId::new("other")),
            })
        );
    }

    #[test]
    fn spearman_rejects_different_trajectory_lengths() {
        let short = trajectory("a", &[("one", RankPosition::Ranked { rank: 1 })]);
        let long = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
            ],
        );

        assert_eq!(
            spearman_correlation(&short, &long),
            Err(TrajectoryAlignmentError {
                index: 1,
                left: None,
                right: Some(ObservationId::new("two")),
            })
        );
    }

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
