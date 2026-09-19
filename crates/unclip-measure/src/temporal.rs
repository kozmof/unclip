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
    NonIncreasingPosition { index: usize },
    DuplicateObservation { index: usize },
    TrajectoryMismatch { index: usize },
}

impl std::fmt::Display for TemporalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
