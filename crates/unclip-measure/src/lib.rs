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
mod empirical;

mod community;
pub use community::{
    detect_communities, CommunityDetection, InvalidCommunityThreshold, UnassessedPair,
};

mod canonical_correlation;
pub use canonical_correlation::{
    canonical_correlation, CanonicalCorrelationAnalysis, CanonicalCorrelationConfig,
    CanonicalCorrelationError, CanonicalCorrelationMode, CanonicalCorrelationOutcome,
    CanonicalCorrelationUndefined, CrossDomainSample,
};

mod cross_domain_interaction_movement;
mod cross_domain_mutual_information;
pub use cross_domain_interaction_movement::{
    cross_domain_interaction_movement, CrossDomainAxisMovement, CrossDomainInteractionMovement,
    CrossDomainInteractionMovementConfig, CrossDomainInteractionMovementError,
    CrossDomainInteractionMovementOutcome, CrossDomainTransition,
    UnassessedCrossDomainAxisMovement,
};

pub use cross_domain_mutual_information::{
    cross_domain_mutual_information, CrossDomainAxisMutualInformation,
    CrossDomainMutualInformation, CrossDomainMutualInformationConfig,
    CrossDomainMutualInformationError, CrossDomainMutualInformationOutcome,
    ProductMeasurementBinding, UnassessedCrossDomainAxis,
};

mod cross_domain_community;
pub use cross_domain_community::{
    detect_cross_domain_communities, CrossDomainCommunityConfig, CrossDomainCommunityDetection,
    CrossDomainCommunityError, CrossDomainCommunityMember, CrossDomainCommunityOutcome,
    ProductSide, UnassessedCrossDomainInteraction,
};

mod pairwise;
pub use pairwise::{
    pairwise_matrix, MatrixCell, PairwiseMatrix, PairwiseMatrixError, PairwiseMetric,
};

mod regime;
pub use regime::{InvalidRegimePartition, RegimePartition};

mod spectral;
pub use spectral::{spectral_decomposition, Eigenpair, SpectralDecomposition, SpectralError};

mod temporal;
pub use temporal::{
    detect_change_points, dynamic_time_warping, lagged_dependency, ChangePoint,
    ChangePointDetection, ObservationSequence, OrderedObservation, TemporalError,
};

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScalarStatistic {
    pub value: f64,
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
    let (left_values, right_values) = pairwise_rank_values(left, right)?;
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

/// Calculates Kendall's tau-b association over pairwise-complete samples.
///
/// Tau-b corrects for ties in either trajectory. `None` indicates fewer than
/// two comparable samples or no comparable variation.
pub fn kendall_association(
    left: &RankTrajectory,
    right: &RankTrajectory,
) -> Result<Option<Correlation>, TrajectoryAlignmentError> {
    let (left_values, right_values) = pairwise_rank_values(left, right)?;
    let sample_count = left_values.len();
    if sample_count < 2 {
        return Ok(None);
    }

    let mut concordant = 0_usize;
    let mut discordant = 0_usize;
    let mut left_ties = 0_usize;
    let mut right_ties = 0_usize;
    for first in 0..sample_count {
        for second in first + 1..sample_count {
            use std::cmp::Ordering;
            match (
                left_values[first].cmp(&left_values[second]),
                right_values[first].cmp(&right_values[second]),
            ) {
                (Ordering::Equal, Ordering::Equal) => {}
                (Ordering::Equal, _) => left_ties += 1,
                (_, Ordering::Equal) => right_ties += 1,
                (left_order, right_order) if left_order == right_order => concordant += 1,
                _ => discordant += 1,
            }
        }
    }

    let untied = concordant + discordant;
    let denominator = (((untied + left_ties) as f64) * ((untied + right_ties) as f64)).sqrt();
    if denominator == 0.0 {
        return Ok(None);
    }
    Ok(Some(Correlation {
        coefficient: (concordant as f64 - discordant as f64) / denominator,
        sample_count,
    }))
}

/// Calculates population variance over known relative-rank differences.
///
/// Unknown and missing samples are excluded. `None` indicates fewer than two
/// known differences; zero variance is retained as a measured value.
pub fn relative_rank_variance(trajectory: &RelativeRankTrajectory) -> Option<ScalarStatistic> {
    let values = trajectory
        .samples
        .iter()
        .filter_map(|sample| match sample.position {
            RelativeRankPosition::Difference { value } => Some(value as f64),
            RelativeRankPosition::Unknown | RelativeRankPosition::Missing => None,
        })
        .collect::<Vec<_>>();
    let sample_count = values.len();
    if sample_count < 2 {
        return None;
    }
    let mean = values.iter().sum::<f64>() / sample_count as f64;
    let variance = values
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / sample_count as f64;
    Some(ScalarStatistic {
        value: variance,
        sample_count,
    })
}

/// Calculates how often two units are jointly within a foreground rank band.
///
/// The denominator contains only observations where both ranks are known.
/// `None` indicates that there are no pairwise-complete observations.
pub fn co_foreground_frequency(
    left: &RankTrajectory,
    right: &RankTrajectory,
    foreground_rank: NonZeroUsize,
) -> Result<Option<ScalarStatistic>, TrajectoryAlignmentError> {
    let (left_values, right_values) = pairwise_rank_values(left, right)?;
    let sample_count = left_values.len();
    if sample_count == 0 {
        return Ok(None);
    }
    let cutoff = foreground_rank.get();
    let co_foreground = left_values
        .iter()
        .zip(&right_values)
        .filter(|(left, right)| **left <= cutoff && **right <= cutoff)
        .count();
    Ok(Some(ScalarStatistic {
        value: co_foreground as f64 / sample_count as f64,
        sample_count,
    }))
}

/// Calculates empirical mutual information between two rank trajectories.
///
/// The plug-in estimate is reported in bits. Unknown and missing samples are
/// excluded pairwise; `None` indicates fewer than two comparable samples.
pub fn mutual_information(
    left: &RankTrajectory,
    right: &RankTrajectory,
) -> Result<Option<ScalarStatistic>, TrajectoryAlignmentError> {
    let (left_values, right_values) = pairwise_rank_values(left, right)?;
    let sample_count = left_values.len();
    if sample_count < 2 {
        return Ok(None);
    }

    let mut left_counts = BTreeMap::<usize, usize>::new();
    let mut right_counts = BTreeMap::<usize, usize>::new();
    let mut joint_counts = BTreeMap::<(usize, usize), usize>::new();
    for (&left, &right) in left_values.iter().zip(&right_values) {
        *left_counts.entry(left).or_default() += 1;
        *right_counts.entry(right).or_default() += 1;
        *joint_counts.entry((left, right)).or_default() += 1;
    }

    let count = sample_count as f64;
    let value = joint_counts
        .into_iter()
        .map(|((left, right), joint_count)| {
            let joint_probability = joint_count as f64 / count;
            let left_probability = left_counts[&left] as f64 / count;
            let right_probability = right_counts[&right] as f64 / count;
            joint_probability * (joint_probability / (left_probability * right_probability)).log2()
        })
        .sum();
    Ok(Some(ScalarStatistic {
        value,
        sample_count,
    }))
}

/// Calculates empirical mutual information conditioned on a third trajectory.
///
/// The plug-in estimate is reported in bits over triple-complete samples.
/// `None` indicates fewer than two comparable triples.
pub fn conditional_mutual_information(
    left: &RankTrajectory,
    right: &RankTrajectory,
    conditioning: &RankTrajectory,
) -> Result<Option<ScalarStatistic>, TrajectoryAlignmentError> {
    validate_trajectory_alignment(left, right)?;
    validate_trajectory_alignment(left, conditioning)?;

    let values = left
        .samples
        .iter()
        .zip(&right.samples)
        .zip(&conditioning.samples)
        .filter_map(|((left, right), conditioning)| {
            match (left.position, right.position, conditioning.position) {
                (
                    RankPosition::Ranked { rank: left },
                    RankPosition::Ranked { rank: right },
                    RankPosition::Ranked { rank: conditioning },
                ) => Some((left, right, conditioning)),
                _ => None,
            }
        })
        .collect::<Vec<_>>();
    let sample_count = values.len();
    if sample_count < 2 {
        return Ok(None);
    }

    let mut conditioning_counts = BTreeMap::<usize, usize>::new();
    let mut left_conditioning_counts = BTreeMap::<(usize, usize), usize>::new();
    let mut right_conditioning_counts = BTreeMap::<(usize, usize), usize>::new();
    let mut joint_counts = BTreeMap::<(usize, usize, usize), usize>::new();
    for &(left, right, conditioning) in &values {
        *conditioning_counts.entry(conditioning).or_default() += 1;
        *left_conditioning_counts
            .entry((left, conditioning))
            .or_default() += 1;
        *right_conditioning_counts
            .entry((right, conditioning))
            .or_default() += 1;
        *joint_counts.entry((left, right, conditioning)).or_default() += 1;
    }

    let count = sample_count as f64;
    let value = joint_counts
        .into_iter()
        .map(|((left, right, conditioning), joint_count)| {
            let joint_probability = joint_count as f64 / count;
            let ratio = (joint_count * conditioning_counts[&conditioning]) as f64
                / (left_conditioning_counts[&(left, conditioning)]
                    * right_conditioning_counts[&(right, conditioning)]) as f64;
            joint_probability * ratio.log2()
        })
        .sum();
    Ok(Some(ScalarStatistic {
        value,
        sample_count,
    }))
}

/// First-order partial Pearson correlation of rank positions, controlling for
/// one trajectory. Only triple-complete samples participate. Returns `None`
/// for fewer than four samples, constant conditioning, or zero residual
/// variance. The result measures association, not causality.
pub fn partial_correlation(
    left: &RankTrajectory,
    right: &RankTrajectory,
    conditioning: &RankTrajectory,
) -> Result<Option<Correlation>, TrajectoryAlignmentError> {
    validate_trajectory_alignment(left, right)?;
    validate_trajectory_alignment(left, conditioning)?;
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let mut zs = Vec::new();
    for ((x, y), z) in left
        .samples
        .iter()
        .zip(&right.samples)
        .zip(&conditioning.samples)
    {
        if let (
            RankPosition::Ranked { rank: x },
            RankPosition::Ranked { rank: y },
            RankPosition::Ranked { rank: z },
        ) = (x.position, y.position, z.position)
        {
            xs.push(x as f64);
            ys.push(y as f64);
            zs.push(z as f64);
        }
    }
    let sample_count = zs.len();
    if sample_count < 4 {
        return Ok(None);
    }
    for values in [&mut xs, &mut ys, &mut zs] {
        let mean = values.iter().sum::<f64>() / sample_count as f64;
        for value in values {
            *value -= mean;
        }
    }
    let variance = zs.iter().map(|z| z * z).sum::<f64>();
    if variance == 0.0 {
        return Ok(None);
    }
    for values in [&mut xs, &mut ys] {
        let original = values.iter().map(|v| v * v).sum::<f64>();
        let slope = values.iter().zip(&zs).map(|(v, z)| v * z).sum::<f64>() / variance;
        for (value, z) in values.iter_mut().zip(&zs) {
            *value -= slope * z;
        }
        let residual = values.iter().map(|v| v * v).sum::<f64>();
        // Do not interpret floating-point residue as unexplained variation.
        if residual <= original * (64.0 * f64::EPSILON).powi(2) {
            return Ok(None);
        }
    }
    Ok(
        pearson_correlation(&xs, &ys).map(|coefficient| Correlation {
            coefficient: coefficient.clamp(-1.0, 1.0),
            sample_count,
        }),
    )
}

fn pairwise_rank_values(
    left: &RankTrajectory,
    right: &RankTrajectory,
) -> Result<(Vec<usize>, Vec<usize>), TrajectoryAlignmentError> {
    validate_trajectory_alignment(left, right)?;
    let mut left_values = Vec::new();
    let mut right_values = Vec::new();

    for (left_sample, right_sample) in left.samples.iter().zip(&right.samples) {
        if let (
            RankPosition::Ranked { rank: left_rank },
            RankPosition::Ranked { rank: right_rank },
        ) = (left_sample.position, right_sample.position)
        {
            left_values.push(left_rank);
            right_values.push(right_rank);
        }
    }
    Ok((left_values, right_values))
}

fn validate_trajectory_alignment(
    left: &RankTrajectory,
    right: &RankTrajectory,
) -> Result<(), TrajectoryAlignmentError> {
    for (index, (left_sample, right_sample)) in left.samples.iter().zip(&right.samples).enumerate()
    {
        if left_sample.observation != right_sample.observation {
            return Err(TrajectoryAlignmentError {
                index,
                left: Some(left_sample.observation.clone()),
                right: Some(right_sample.observation.clone()),
            });
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
    Ok(())
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
    PairwiseMatrix(PairwiseMatrix),
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
            Self::Matrix(_) | Self::PairwiseMatrix(_) => MeasurementKind::Matrix,
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
    fn kendall_tau_b_corrects_for_ties_and_sparse_samples() {
        let left = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
                ("three", RankPosition::Unknown),
                ("four", RankPosition::Ranked { rank: 2 }),
                ("five", RankPosition::Ranked { rank: 3 }),
            ],
        );
        let right = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
                ("three", RankPosition::Ranked { rank: 1 }),
                ("four", RankPosition::Ranked { rank: 3 }),
                ("five", RankPosition::Ranked { rank: 3 }),
            ],
        );

        let association = kendall_association(&left, &right).unwrap().unwrap();

        assert_eq!(association.sample_count, 4);
        assert!((association.coefficient - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn kendall_reports_inverse_and_undefined_associations() {
        let ascending = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
                ("three", RankPosition::Ranked { rank: 3 }),
            ],
        );
        let descending = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 3 }),
                ("two", RankPosition::Ranked { rank: 2 }),
                ("three", RankPosition::Ranked { rank: 1 }),
            ],
        );
        let constant = trajectory(
            "c",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 1 }),
                ("three", RankPosition::Ranked { rank: 1 }),
            ],
        );

        assert_eq!(
            kendall_association(&ascending, &descending)
                .unwrap()
                .unwrap()
                .coefficient,
            -1.0
        );
        assert_eq!(kendall_association(&ascending, &constant).unwrap(), None);
    }

    #[test]
    fn relative_rank_variance_excludes_sparse_samples_and_retains_zero() {
        let moving = RelativeRankTrajectory {
            left: unit("a"),
            right: unit("b"),
            samples: vec![
                RelativeRankSample {
                    observation: ObservationId::new("one"),
                    position: RelativeRankPosition::Difference { value: -1 },
                },
                RelativeRankSample {
                    observation: ObservationId::new("two"),
                    position: RelativeRankPosition::Unknown,
                },
                RelativeRankSample {
                    observation: ObservationId::new("three"),
                    position: RelativeRankPosition::Difference { value: 1 },
                },
                RelativeRankSample {
                    observation: ObservationId::new("four"),
                    position: RelativeRankPosition::Missing,
                },
            ],
        };
        assert_eq!(
            relative_rank_variance(&moving),
            Some(ScalarStatistic {
                value: 1.0,
                sample_count: 2,
            })
        );

        let stationary = RelativeRankTrajectory {
            left: unit("a"),
            right: unit("b"),
            samples: vec![
                RelativeRankSample {
                    observation: ObservationId::new("one"),
                    position: RelativeRankPosition::Difference { value: 2 },
                },
                RelativeRankSample {
                    observation: ObservationId::new("two"),
                    position: RelativeRankPosition::Difference { value: 2 },
                },
            ],
        };
        assert_eq!(
            relative_rank_variance(&stationary),
            Some(ScalarStatistic {
                value: 0.0,
                sample_count: 2,
            })
        );
    }

    #[test]
    fn relative_rank_variance_requires_two_known_differences() {
        let trajectory = RelativeRankTrajectory {
            left: unit("a"),
            right: unit("b"),
            samples: vec![
                RelativeRankSample {
                    observation: ObservationId::new("one"),
                    position: RelativeRankPosition::Difference { value: 1 },
                },
                RelativeRankSample {
                    observation: ObservationId::new("two"),
                    position: RelativeRankPosition::Missing,
                },
            ],
        };

        assert_eq!(relative_rank_variance(&trajectory), None);
    }

    #[test]
    fn co_foreground_frequency_uses_pairwise_complete_denominator() {
        let left = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
                ("three", RankPosition::Unknown),
                ("four", RankPosition::Ranked { rank: 3 }),
            ],
        );
        let right = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 2 }),
                ("two", RankPosition::Ranked { rank: 3 }),
                ("three", RankPosition::Ranked { rank: 1 }),
                ("four", RankPosition::Ranked { rank: 1 }),
            ],
        );

        assert_eq!(
            co_foreground_frequency(
                &left,
                &right,
                NonZeroUsize::new(2).expect("nonzero fixture cutoff"),
            )
            .unwrap(),
            Some(ScalarStatistic {
                value: 1.0 / 3.0,
                sample_count: 3,
            })
        );
    }

    #[test]
    fn co_foreground_frequency_retains_zero_and_requires_comparable_samples() {
        let foreground = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Missing),
            ],
        );
        let background = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 3 }),
                ("two", RankPosition::Ranked { rank: 1 }),
            ],
        );
        let unknown = trajectory(
            "c",
            &[
                ("one", RankPosition::Unknown),
                ("two", RankPosition::Missing),
            ],
        );
        let cutoff = NonZeroUsize::new(1).expect("nonzero fixture cutoff");

        assert_eq!(
            co_foreground_frequency(&foreground, &background, cutoff).unwrap(),
            Some(ScalarStatistic {
                value: 0.0,
                sample_count: 1,
            })
        );
        assert_eq!(
            co_foreground_frequency(&foreground, &unknown, cutoff).unwrap(),
            None
        );
    }

    #[test]
    fn mutual_information_measures_discrete_rank_dependency_in_bits() {
        let left = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 1 }),
                ("three", RankPosition::Ranked { rank: 2 }),
                ("four", RankPosition::Ranked { rank: 2 }),
            ],
        );
        let same_partition = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 3 }),
                ("two", RankPosition::Ranked { rank: 3 }),
                ("three", RankPosition::Ranked { rank: 4 }),
                ("four", RankPosition::Ranked { rank: 4 }),
            ],
        );
        let independent = trajectory(
            "c",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
                ("three", RankPosition::Ranked { rank: 1 }),
                ("four", RankPosition::Ranked { rank: 2 }),
            ],
        );

        assert_eq!(
            mutual_information(&left, &same_partition).unwrap(),
            Some(ScalarStatistic {
                value: 1.0,
                sample_count: 4,
            })
        );
        assert_eq!(
            mutual_information(&left, &independent).unwrap(),
            Some(ScalarStatistic {
                value: 0.0,
                sample_count: 4,
            })
        );
    }

    #[test]
    fn mutual_information_excludes_sparse_samples_and_requires_two() {
        let left = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Unknown),
                ("three", RankPosition::Missing),
            ],
        );
        let right = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 2 }),
                ("two", RankPosition::Ranked { rank: 1 }),
                ("three", RankPosition::Ranked { rank: 2 }),
            ],
        );

        assert_eq!(mutual_information(&left, &right).unwrap(), None);
    }

    #[test]
    fn conditional_mutual_information_detects_xor_dependency() {
        let left = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 1 }),
                ("three", RankPosition::Ranked { rank: 2 }),
                ("four", RankPosition::Ranked { rank: 2 }),
            ],
        );
        let right = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
                ("three", RankPosition::Ranked { rank: 1 }),
                ("four", RankPosition::Ranked { rank: 2 }),
            ],
        );
        let xor = trajectory(
            "condition",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
                ("three", RankPosition::Ranked { rank: 2 }),
                ("four", RankPosition::Ranked { rank: 1 }),
            ],
        );

        assert_eq!(
            mutual_information(&left, &right).unwrap(),
            Some(ScalarStatistic {
                value: 0.0,
                sample_count: 4,
            })
        );
        assert_eq!(
            conditional_mutual_information(&left, &right, &xor).unwrap(),
            Some(ScalarStatistic {
                value: 1.0,
                sample_count: 4,
            })
        );
    }

    #[test]
    fn conditional_mutual_information_uses_triple_complete_samples() {
        let left = trajectory(
            "a",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
            ],
        );
        let right = trajectory(
            "b",
            &[
                ("one", RankPosition::Ranked { rank: 1 }),
                ("two", RankPosition::Ranked { rank: 2 }),
            ],
        );
        let sparse_condition = trajectory(
            "condition",
            &[
                ("one", RankPosition::Unknown),
                ("two", RankPosition::Ranked { rank: 1 }),
            ],
        );

        assert_eq!(
            conditional_mutual_information(&left, &right, &sparse_condition).unwrap(),
            None
        );
    }

    #[test]
    fn partial_correlation_controls_shared_variation_and_preserves_sparse_states() {
        let make = |ranks: &[usize]| RankTrajectory {
            unit: UnitId::new("unit"),
            samples: ranks
                .iter()
                .enumerate()
                .map(|(i, &rank)| RankSample {
                    observation: ObservationId::new(i.to_string()),
                    position: RankPosition::Ranked { rank },
                })
                .collect(),
        };
        let x = make(&[2, 2, 4, 4]);
        let y = make(&[2, 4, 2, 4]);
        let z = make(&[1, 2, 3, 4]);
        let result = partial_correlation(&x, &y, &z).unwrap().unwrap();
        assert!((result.coefficient + 1.0).abs() < 1e-12);
        assert_eq!(result.sample_count, 4);
        assert_eq!(Some(result), partial_correlation(&x, &y, &z).unwrap());
        assert!(
            (partial_correlation(&x, &x, &z)
                .unwrap()
                .unwrap()
                .coefficient
                - 1.0)
                .abs()
                < 1e-12
        );
        assert_eq!(
            partial_correlation(&x, &y, &make(&[1, 2, 2, 1])).unwrap(),
            Some(Correlation {
                coefficient: 0.0,
                sample_count: 4
            })
        );
        assert_eq!(partial_correlation(&x, &y, &x).unwrap(), None);
        assert_eq!(
            partial_correlation(&x, &y, &make(&[1, 1, 1, 1])).unwrap(),
            None
        );
        assert_eq!(
            partial_correlation(&make(&[1, 1, 1, 1]), &y, &z).unwrap(),
            None
        );
        let mut sparse = z.clone();
        sparse.samples[0].position = RankPosition::Unknown;
        sparse.samples[1].position = RankPosition::Missing;
        assert_eq!(partial_correlation(&x, &y, &sparse).unwrap(), None);
        let mut longer_x = x.clone();
        let mut longer_y = y.clone();
        let mut longer_z = z.clone();
        for trajectory in [&mut longer_x, &mut longer_y, &mut longer_z] {
            trajectory.samples.push(RankSample {
                observation: ObservationId::new("extra"),
                position: RankPosition::Missing,
            });
        }
        assert_eq!(
            partial_correlation(&longer_x, &longer_y, &longer_z).unwrap(),
            Some(result)
        );
        assert!(partial_correlation(&x, &y, &longer_z).is_err());
        assert!(partial_correlation(&x, &longer_y, &z).is_err());
        sparse.samples.swap(0, 1);
        assert!(partial_correlation(&x, &y, &sparse).is_err());
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
