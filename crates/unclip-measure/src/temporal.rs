//! Temporal calculations over explicitly ordered observations.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use unclip_observe::ObservationId;

use crate::{pearson_correlation, Correlation, RankPosition, RankTrajectory};

/// An explicit sequence coordinate, not an inferred timestamp. Coordinates need
/// not be consecutive; lag is measured in observation steps, not elapsed time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderedObservation {
    pub observation: ObservationId,
    pub position: i64,
}

/// Immutable validated order. Deserialization uses the same validation as `new`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<OrderedObservation>", into = "Vec<OrderedObservation>")]
pub struct ObservationSequence(Vec<OrderedObservation>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemporalError {
    InvalidChangeThreshold,
    NonIncreasingPosition { index: usize },
    DuplicateObservation { index: usize },
    TrajectoryMismatch { index: usize },
}

impl std::fmt::Display for TemporalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidChangeThreshold => {
                write!(f, "change threshold must be finite and strictly positive")
            }
            Self::NonIncreasingPosition { index } => {
                write!(f, "sequence position must increase at index {index}")
            }
            Self::DuplicateObservation { index } => {
                write!(f, "duplicate observation at index {index}")
            }
            Self::TrajectoryMismatch { index } => {
                write!(f, "trajectory does not match sequence at index {index}")
            }
        }
    }
}

impl std::error::Error for TemporalError {}

impl ObservationSequence {
    pub fn new(observations: Vec<OrderedObservation>) -> Result<Self, TemporalError> {
        let mut seen = BTreeSet::new();
        for (index, observation) in observations.iter().enumerate() {
            if !seen.insert(&observation.observation) {
                return Err(TemporalError::DuplicateObservation { index });
            }
            if index > 0 && observations[index - 1].position >= observation.position {
                return Err(TemporalError::NonIncreasingPosition { index });
            }
        }
        Ok(Self(observations))
    }

    pub fn observations(&self) -> &[OrderedObservation] {
        &self.0
    }

    fn validate(&self, trajectory: &RankTrajectory) -> Result<(), TemporalError> {
        for (index, (expected, actual)) in self.0.iter().zip(&trajectory.samples).enumerate() {
            if expected.observation != actual.observation {
                return Err(TemporalError::TrajectoryMismatch { index });
            }
        }
        if self.0.len() != trajectory.samples.len() {
            return Err(TemporalError::TrajectoryMismatch {
                index: self.0.len().min(trajectory.samples.len()),
            });
        }
        Ok(())
    }
}

impl TryFrom<Vec<OrderedObservation>> for ObservationSequence {
    type Error = TemporalError;
    fn try_from(value: Vec<OrderedObservation>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ObservationSequence> for Vec<OrderedObservation> {
    fn from(value: ObservationSequence) -> Self {
        value.0
    }
}

/// Pearson association between `source[t]` and `target[t + lag]` rank positions.
/// This is directional temporal evidence, not a causal or predictive model.
/// Sparse endpoints are excluded without closing gaps or shifting the lag.
/// `None` means fewer than two complete pairs or zero variance.
pub fn lagged_dependency(
    sequence: &ObservationSequence,
    source: &RankTrajectory,
    target: &RankTrajectory,
    lag: NonZeroUsize,
) -> Result<Option<Correlation>, TemporalError> {
    sequence.validate(source)?;
    sequence.validate(target)?;
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (source, target) in source
        .samples
        .iter()
        .zip(target.samples.iter().skip(lag.get()))
    {
        if let (RankPosition::Ranked { rank: x }, RankPosition::Ranked { rank: y }) =
            (source.position, target.position)
        {
            xs.push(x as f64);
            ys.push(y as f64);
        }
    }
    if xs.len() < 2 {
        return Ok(None);
    }
    Ok(
        pearson_correlation(&xs, &ys).map(|coefficient| Correlation {
            coefficient: coefficient.clamp(-1.0, 1.0),
            sample_count: xs.len(),
        }),
    )
}

/// Unnormalized Dynamic Time Warping cost using absolute rank differences.
/// Both trajectories require explicit validated order, but may have different
/// lengths and observation identities. Missing or unknown ranks yield `None`;
/// they are never removed to manufacture an alignment. Uses O(right length)
/// memory and O(left length * right length) time with fixed reduction order.
pub fn dynamic_time_warping(
    left_sequence: &ObservationSequence,
    left: &RankTrajectory,
    right_sequence: &ObservationSequence,
    right: &RankTrajectory,
) -> Result<Option<f64>, TemporalError> {
    left_sequence.validate(left)?;
    right_sequence.validate(right)?;
    let ranks = |trajectory: &RankTrajectory| {
        trajectory
            .samples
            .iter()
            .map(|sample| match sample.position {
                RankPosition::Ranked { rank } => Some(rank),
                RankPosition::Unknown | RankPosition::Missing => None,
            })
            .collect::<Option<Vec<_>>>()
    };
    let (Some(xs), Some(ys)) = (ranks(left), ranks(right)) else {
        return Ok(None);
    };
    if xs.is_empty() || ys.is_empty() {
        return Ok(None);
    }
    let mut previous = vec![f64::INFINITY; ys.len() + 1];
    previous[0] = 0.0;
    let mut current = vec![f64::INFINITY; ys.len() + 1];
    for x in xs {
        current[0] = f64::INFINITY;
        for (index, y) in ys.iter().enumerate() {
            let cost = x.abs_diff(*y) as f64;
            current[index + 1] =
                cost + previous[index].min(previous[index + 1]).min(current[index]);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    Ok(Some(previous[ys.len()]))
}

/// A boundary whose adjacent windows differ by the configured rank threshold.
/// The observation and index identify the first sample in the right window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePoint {
    pub observation: ObservationId,
    pub index: usize,
    pub before_mean: f64,
    pub after_mean: f64,
    pub sample_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePointDetection {
    pub events: Vec<ChangePoint>,
    pub evaluated_boundaries: usize,
    pub window: NonZeroUsize,
    pub minimum_shift: f64,
}

/// Detects rank-mean changes using two adjacent windows of `window` samples.
/// A boundary is flagged when the absolute mean difference is at least the
/// finite, strictly positive `minimum_shift` (in rank units). Every qualifying
/// boundary is retained; adjacent flags may describe the same transition.
/// This is descriptive threshold detection, not a significance or causal test.
///
/// Windows containing unknown or missing ranks are skipped without closing
/// gaps. `None` means no complete boundary could be evaluated. An empty event
/// list with a positive evaluated count means no threshold crossing was found.
/// Window size counts observations, not elapsed time. Reduction order is fixed.
pub fn detect_change_points(
    sequence: &ObservationSequence,
    trajectory: &RankTrajectory,
    window: NonZeroUsize,
    minimum_shift: f64,
) -> Result<Option<ChangePointDetection>, TemporalError> {
    sequence.validate(trajectory)?;
    if !minimum_shift.is_finite() || minimum_shift <= 0.0 {
        return Err(TemporalError::InvalidChangeThreshold);
    }
    let width = window.get();
    let count = trajectory.samples.len();
    if width > count / 2 {
        return Ok(None);
    }
    let mean = |samples: &[crate::RankSample]| {
        samples
            .iter()
            .try_fold(0.0, |sum, sample| match sample.position {
                RankPosition::Ranked { rank } => Some(sum + rank as f64),
                RankPosition::Unknown | RankPosition::Missing => None,
            })
            .map(|sum| sum / width as f64)
    };
    let mut result = ChangePointDetection {
        events: Vec::new(),
        evaluated_boundaries: 0,
        window,
        minimum_shift,
    };
    for index in width..=count - width {
        let (Some(before_mean), Some(after_mean)) = (
            mean(&trajectory.samples[index - width..index]),
            mean(&trajectory.samples[index..index + width]),
        ) else {
            continue;
        };
        result.evaluated_boundaries += 1;
        if (after_mean - before_mean).abs() >= minimum_shift {
            result.events.push(ChangePoint {
                observation: sequence.observations()[index].observation.clone(),
                index,
                before_mean,
                after_mean,
                sample_count: width * 2,
            });
        }
    }
    Ok((result.evaluated_boundaries > 0).then_some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RankSample;
    use unclip_domain::UnitId;

    fn sequence() -> ObservationSequence {
        ObservationSequence::new(
            (0..5)
                .map(|i| OrderedObservation {
                    observation: ObservationId::new(i.to_string()),
                    position: i * 10,
                })
                .collect(),
        )
        .unwrap()
    }

    fn trajectory(values: &[usize]) -> RankTrajectory {
        RankTrajectory {
            unit: UnitId::new("unit"),
            samples: values
                .iter()
                .enumerate()
                .map(|(i, &rank)| RankSample {
                    observation: ObservationId::new(i.to_string()),
                    position: RankPosition::Ranked { rank },
                })
                .collect(),
        }
    }

    fn change_sequence(count: usize) -> ObservationSequence {
        ObservationSequence::new(
            (0..count)
                .map(|i| OrderedObservation {
                    observation: ObservationId::new(i.to_string()),
                    position: (i * 10) as i64,
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn change_points_detect_both_directions_with_exact_boundaries() {
        let sequence = change_sequence(9);
        let ranks = trajectory(&[1, 1, 1, 5, 5, 5, 1, 1, 1]);
        let window = NonZeroUsize::new(3).unwrap();
        let result = detect_change_points(&sequence, &ranks, window, 4.0)
            .unwrap()
            .unwrap();
        assert_eq!(result.evaluated_boundaries, 4);
        assert_eq!(
            result.events,
            vec![
                ChangePoint {
                    observation: ObservationId::new("3"),
                    index: 3,
                    before_mean: 1.0,
                    after_mean: 5.0,
                    sample_count: 6
                },
                ChangePoint {
                    observation: ObservationId::new("6"),
                    index: 6,
                    before_mean: 5.0,
                    after_mean: 1.0,
                    sample_count: 6
                },
            ]
        );
        assert_eq!(
            detect_change_points(&sequence, &ranks, window, 4.0).unwrap(),
            Some(result.clone())
        );
        let encoded = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serde_json::from_str::<ChangePointDetection>(&encoded).unwrap(),
            result
        );
        assert!(detect_change_points(&sequence, &ranks, window, 4.1)
            .unwrap()
            .unwrap()
            .events
            .is_empty());
    }

    #[test]
    fn change_points_distinguish_no_change_from_insufficient_evidence() {
        let sequence = change_sequence(6);
        let window = NonZeroUsize::new(2).unwrap();
        let constant = trajectory(&[3; 6]);
        let result = detect_change_points(&sequence, &constant, window, 0.5)
            .unwrap()
            .unwrap();
        assert!(result.events.is_empty());
        assert_eq!(result.evaluated_boundaries, 3);
        for position in [RankPosition::Unknown, RankPosition::Missing] {
            let mut sparse = trajectory(&[1, 1, 4, 4, 4, 4]);
            sparse.samples[0].position = position;
            let result = detect_change_points(&sequence, &sparse, window, 3.0)
                .unwrap()
                .unwrap();
            assert_eq!(result.evaluated_boundaries, 2);
            assert!(result.events.is_empty());
            sparse.samples[3].position = position;
            assert_eq!(
                detect_change_points(&sequence, &sparse, window, 3.0).unwrap(),
                None
            );
        }
        assert_eq!(
            detect_change_points(
                &sequence,
                &constant,
                NonZeroUsize::new(usize::MAX).unwrap(),
                1.0
            )
            .unwrap(),
            None
        );
        assert_eq!(
            detect_change_points(&change_sequence(0), &trajectory(&[]), window, 1.0).unwrap(),
            None
        );
        let adjacent = detect_change_points(
            &change_sequence(4),
            &trajectory(&[1, 3, 1, 3]),
            NonZeroUsize::new(1).unwrap(),
            2.0,
        )
        .unwrap()
        .unwrap();
        assert_eq!(adjacent.events.len(), 3);
    }

    #[test]
    fn change_points_reject_invalid_thresholds_and_order_mismatches() {
        let sequence = change_sequence(4);
        let ranks = trajectory(&[1, 1, 3, 3]);
        let window = NonZeroUsize::new(2).unwrap();
        for threshold in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                detect_change_points(&sequence, &ranks, window, threshold),
                Err(TemporalError::InvalidChangeThreshold)
            );
        }
        assert_eq!(
            TemporalError::InvalidChangeThreshold.to_string(),
            "change threshold must be finite and strictly positive"
        );
        assert!(detect_change_points(&change_sequence(3), &ranks, window, 1.0).is_err());
        let mut reordered = ranks.clone();
        reordered.samples.swap(0, 1);
        assert!(detect_change_points(&sequence, &reordered, window, 1.0).is_err());
    }

    #[test]
    fn dtw_handles_warping_distance_and_sparse_evidence() {
        let make_sequence = |count| {
            ObservationSequence::new(
                (0..count)
                    .map(|i| OrderedObservation {
                        observation: ObservationId::new(i.to_string()),
                        position: i,
                    })
                    .collect(),
            )
            .unwrap()
        };
        let left_sequence = make_sequence(3);
        let right_sequence = make_sequence(4);
        let left = trajectory(&[1, 2, 3]);
        let right = trajectory(&[1, 2, 2, 3]);
        assert_eq!(
            dynamic_time_warping(&left_sequence, &left, &right_sequence, &right).unwrap(),
            Some(0.0)
        );
        let shifted = trajectory(&[2, 3, 4]);
        assert_eq!(
            dynamic_time_warping(&left_sequence, &left, &left_sequence, &shifted).unwrap(),
            Some(2.0)
        );
        assert_eq!(
            dynamic_time_warping(&left_sequence, &shifted, &left_sequence, &left).unwrap(),
            Some(2.0)
        );
        for position in [RankPosition::Missing, RankPosition::Unknown] {
            let mut sparse = right.clone();
            sparse.samples[1].position = position;
            assert_eq!(
                dynamic_time_warping(&left_sequence, &left, &right_sequence, &sparse).unwrap(),
                None
            );
        }
        assert_eq!(
            dynamic_time_warping(&make_sequence(0), &trajectory(&[]), &right_sequence, &right)
                .unwrap(),
            None
        );
        assert!(dynamic_time_warping(&left_sequence, &right, &right_sequence, &right).is_err());
        assert!(dynamic_time_warping(&left_sequence, &left, &left_sequence, &right).is_err());
    }

    #[test]
    fn validates_order_identity_and_serialized_input() {
        let sequence = sequence();
        let json = serde_json::to_string(&sequence).unwrap();
        assert_eq!(
            serde_json::from_str::<ObservationSequence>(&json).unwrap(),
            sequence
        );
        for position in [0, -1] {
            let mut values = sequence.observations().to_vec();
            values[1].position = position;
            assert_eq!(
                ObservationSequence::new(values.clone()),
                Err(TemporalError::NonIncreasingPosition { index: 1 })
            );
            assert!(serde_json::from_value::<ObservationSequence>(
                serde_json::to_value(values).unwrap()
            )
            .is_err());
        }
        let mut values = sequence.observations().to_vec();
        values[1].observation = values[0].observation.clone();
        assert_eq!(
            ObservationSequence::new(values),
            Err(TemporalError::DuplicateObservation { index: 1 })
        );
        assert!(ObservationSequence::new(vec![]).is_ok());
        assert!(serde_json::from_str::<ObservationSequence>(
            r#"[{"observation":"a","position":0,"extra":1}]"#
        )
        .is_err());
    }

    #[test]
    fn lag_is_directional_and_does_not_close_sparse_gaps() {
        let sequence = sequence();
        let source = trajectory(&[1, 3, 2, 4, 1]);
        let target = trajectory(&[4, 1, 3, 2, 4]);
        let lag = NonZeroUsize::new(1).unwrap();
        let result = lagged_dependency(&sequence, &source, &target, lag)
            .unwrap()
            .unwrap();
        assert_eq!(
            result,
            Correlation {
                coefficient: 1.0,
                sample_count: 4
            }
        );
        assert_ne!(
            lagged_dependency(&sequence, &target, &source, lag).unwrap(),
            Some(result)
        );
        let mut sparse_source = source.clone();
        let mut sparse_target = target.clone();
        sparse_source.samples[1].position = RankPosition::Missing;
        sparse_target.samples[3].position = RankPosition::Unknown;
        assert_eq!(
            lagged_dependency(&sequence, &sparse_source, &sparse_target, lag).unwrap(),
            Some(Correlation {
                coefficient: 1.0,
                sample_count: 2
            })
        );
        sparse_source.samples[0].position = RankPosition::Unknown;
        assert_eq!(
            lagged_dependency(&sequence, &sparse_source, &sparse_target, lag).unwrap(),
            None
        );
        assert_eq!(
            lagged_dependency(
                &sequence,
                &source,
                &target,
                NonZeroUsize::new(usize::MAX).unwrap()
            )
            .unwrap(),
            None
        );
        assert_eq!(
            lagged_dependency(&sequence, &source, &trajectory(&[1; 5]), lag).unwrap(),
            None
        );
        assert_eq!(
            lagged_dependency(&sequence, &trajectory(&[1; 4]), &target, lag),
            Err(TemporalError::TrajectoryMismatch { index: 4 })
        );
        let mut reordered = target.clone();
        reordered.samples.swap(0, 1);
        assert_eq!(
            lagged_dependency(&sequence, &source, &reordered, lag),
            Err(TemporalError::TrajectoryMismatch { index: 0 })
        );
    }
}
