//! Deterministic canonical correlation over sparse cross-domain samples.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;
use unclip_observe::ObservationId;

/// One observation of numeric variables from each side of a product domain.
///
/// An absent unit is missing evidence. It is never filled with zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainSample {
    pub observation: ObservationId,
    #[serde(default)]
    pub left: BTreeMap<UnitId, f64>,
    #[serde(default)]
    pub right: BTreeMap<UnitId, f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalCorrelationConfig {
    pub minimum_samples: NonZeroUsize,
    /// Nonnegative diagonal covariance regularization.
    pub regularization: f64,
    pub tolerance: f64,
    pub max_sweeps: NonZeroUsize,
}

impl Default for CanonicalCorrelationConfig {
    fn default() -> Self {
        Self {
            minimum_samples: NonZeroUsize::new(3).expect("three is nonzero"),
            regularization: 1e-9,
            tolerance: 1e-12,
            max_sweeps: NonZeroUsize::new(100).expect("one hundred is nonzero"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalCorrelationMode {
    pub correlation: f64,
    /// Coefficients in `CanonicalCorrelationAnalysis::left_units` order.
    pub left_weights: Vec<f64>,
    /// Coefficients in `CanonicalCorrelationAnalysis::right_units` order.
    pub right_weights: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalCorrelationAnalysis {
    pub left_units: Vec<UnitId>,
    pub right_units: Vec<UnitId>,
    pub modes: Vec<CanonicalCorrelationMode>,
    pub sample_count: usize,
    pub excluded_observations: Vec<ObservationId>,
    pub regularization: f64,
    pub tolerance: f64,
    /// Total cyclic Jacobi sweeps across the three symmetric decompositions.
    pub sweeps: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalCorrelationUndefined {
    NoVariables,
    NoVariation,
}

/// Sparse outcomes are explicit so callers can map them to measurement states.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CanonicalCorrelationOutcome {
    Value {
        analysis: CanonicalCorrelationAnalysis,
    },
    InsufficientEvidence {
        have: usize,
        need: usize,
        excluded_observations: Vec<ObservationId>,
    },
    Undefined {
        sample_count: usize,
        excluded_observations: Vec<ObservationId>,
        reason: CanonicalCorrelationUndefined,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalCorrelationError {
    InvalidConfiguration,
    DuplicateUnit {
        side: &'static str,
        unit: UnitId,
    },
    DuplicateObservation(ObservationId),
    InvalidSample(ObservationId),
    UnexpectedUnit {
        observation: ObservationId,
        side: &'static str,
        unit: UnitId,
    },
    DidNotConverge {
        sweeps: usize,
    },
    NonFiniteResult,
}

impl std::fmt::Display for CanonicalCorrelationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration => write!(
                f,
                "CCA requires at least two samples, nonnegative finite regularization, and a finite tolerance strictly between zero and one"
            ),
            Self::DuplicateUnit { side, unit } => {
                write!(f, "duplicate {side} CCA unit {}", unit.0)
            }
            Self::DuplicateObservation(observation) => {
                write!(f, "duplicate CCA observation {}", observation.0)
            }
            Self::InvalidSample(observation) => {
                write!(f, "CCA observation {} contains an invalid value", observation.0)
            }
            Self::UnexpectedUnit {
                observation,
                side,
                unit,
            } => write!(
                f,
                "CCA observation {} contains unexpected {side} unit {}",
                observation.0, unit.0
            ),
            Self::DidNotConverge { sweeps } => {
                write!(f, "CCA eigensolver did not converge after {sweeps} sweeps")
            }
            Self::NonFiniteResult => write!(f, "CCA produced a non-finite numeric result"),
        }
    }
}

impl std::error::Error for CanonicalCorrelationError {}

/// Calculate regularized canonical correlations in fixed iteration order.
///
/// Only rows containing every selected left and right unit participate. Empty
/// sides and data with no variance are reported separately from an evidence
/// shortfall. The solver uses covariance whitening and a cyclic Jacobi
/// eigendecomposition; modes are sorted by decreasing correlation and their
/// signs are canonicalized from the first largest-magnitude left coefficient.
pub fn canonical_correlation(
    left_units: &[UnitId],
    right_units: &[UnitId],
    samples: &[CrossDomainSample],
    config: CanonicalCorrelationConfig,
) -> Result<CanonicalCorrelationOutcome, CanonicalCorrelationError> {
    validate_config(config)?;
    validate_units("left", left_units)?;
    validate_units("right", right_units)?;

    if left_units.is_empty() || right_units.is_empty() {
        return Ok(CanonicalCorrelationOutcome::Undefined {
            sample_count: 0,
            excluded_observations: vec![],
            reason: CanonicalCorrelationUndefined::NoVariables,
        });
    }

    let left_set = left_units.iter().collect::<BTreeSet<_>>();
    let right_set = right_units.iter().collect::<BTreeSet<_>>();
    let mut ordered = samples.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|sample| &sample.observation);
    for pair in ordered.windows(2) {
        if pair[0].observation == pair[1].observation {
            return Err(CanonicalCorrelationError::DuplicateObservation(
                pair[0].observation.clone(),
            ));
        }
    }

    let mut retained = Vec::new();
    let mut excluded_observations = Vec::new();
    for sample in ordered {
        if sample.observation.0.trim().is_empty()
            || sample
                .left
                .values()
                .chain(sample.right.values())
                .any(|value| !value.is_finite())
        {
            return Err(CanonicalCorrelationError::InvalidSample(
                sample.observation.clone(),
            ));
        }
        if let Some(unit) = sample.left.keys().find(|unit| !left_set.contains(unit)) {
            return Err(CanonicalCorrelationError::UnexpectedUnit {
                observation: sample.observation.clone(),
                side: "left",
                unit: unit.clone(),
            });
        }
        if let Some(unit) = sample.right.keys().find(|unit| !right_set.contains(unit)) {
            return Err(CanonicalCorrelationError::UnexpectedUnit {
                observation: sample.observation.clone(),
                side: "right",
                unit: unit.clone(),
            });
        }
        let complete = left_units.iter().all(|unit| sample.left.contains_key(unit))
            && right_units
                .iter()
                .all(|unit| sample.right.contains_key(unit));
        if complete {
            retained.push((
                left_units
                    .iter()
                    .map(|unit| sample.left[unit])
                    .collect::<Vec<_>>(),
                right_units
                    .iter()
                    .map(|unit| sample.right[unit])
                    .collect::<Vec<_>>(),
            ));
        } else {
            excluded_observations.push(sample.observation.clone());
        }
    }

    let sample_count = retained.len();
    if sample_count < config.minimum_samples.get() {
        return Ok(CanonicalCorrelationOutcome::InsufficientEvidence {
            have: sample_count,
            need: config.minimum_samples.get(),
            excluded_observations,
        });
    }

    let (mut left, mut right): (Vec<_>, Vec<_>) = retained.into_iter().unzip();
    center(&mut left);
    center(&mut right);
    let denominator = (sample_count - 1) as f64;
    let mut left_covariance = cross_product(&left, &left, denominator);
    let mut right_covariance = cross_product(&right, &right, denominator);
    let cross_covariance = cross_product(&left, &right, denominator);
    let left_variation = diagonal_sum(&left_covariance);
    let right_variation = diagonal_sum(&right_covariance);
    if left_variation == 0.0 || right_variation == 0.0 {
        return Ok(CanonicalCorrelationOutcome::Undefined {
            sample_count,
            excluded_observations,
            reason: CanonicalCorrelationUndefined::NoVariation,
        });
    }
    for (index, row) in left_covariance.iter_mut().enumerate() {
        row[index] += config.regularization;
    }
    for (index, row) in right_covariance.iter_mut().enumerate() {
        row[index] += config.regularization;
    }

    let (left_eigen, left_sweeps) =
        symmetric_eigen(left_covariance, config.tolerance, config.max_sweeps.get())?;
    let (right_eigen, right_sweeps) =
        symmetric_eigen(right_covariance, config.tolerance, config.max_sweeps.get())?;
    let left_whitening = inverse_square_root(&left_eigen, config.tolerance)?;
    let right_whitening = inverse_square_root(&right_eigen, config.tolerance)?;
    let whitened = multiply(
        &multiply(&left_whitening, &cross_covariance),
        &right_whitening,
    );
    let gram = multiply(&whitened, &transpose(&whitened));
    let (gram_eigen, gram_sweeps) =
        symmetric_eigen(gram, config.tolerance, config.max_sweeps.get())?;

    let gram_scale = gram_eigen.iter().fold(0.0_f64, |largest, component| {
        largest.max(component.value.abs())
    });
    let negative_cutoff = config.tolerance * gram_scale;
    let mut modes = Vec::new();
    for component in gram_eigen
        .into_iter()
        .take(left_units.len().min(right_units.len()))
    {
        if component.value < -negative_cutoff {
            return Err(CanonicalCorrelationError::NonFiniteResult);
        }
        let correlation = component.value.max(0.0).sqrt();
        if correlation <= config.tolerance {
            continue;
        }
        if correlation > 1.0 + config.tolerance {
            return Err(CanonicalCorrelationError::NonFiniteResult);
        }
        let mut left_weights = matrix_vector(&left_whitening, &component.vector);
        let right_direction = matrix_vector(&transpose(&whitened), &component.vector)
            .into_iter()
            .map(|value| value / correlation)
            .collect::<Vec<_>>();
        let mut right_weights = matrix_vector(&right_whitening, &right_direction);
        if left_weights
            .iter()
            .chain(&right_weights)
            .any(|value| !value.is_finite())
        {
            return Err(CanonicalCorrelationError::NonFiniteResult);
        }
        canonicalize_paired_sign(&mut left_weights, &mut right_weights);
        clean_zeroes(&mut left_weights);
        clean_zeroes(&mut right_weights);
        modes.push(CanonicalCorrelationMode {
            correlation: correlation.clamp(0.0, 1.0),
            left_weights,
            right_weights,
        });
    }
    Ok(CanonicalCorrelationOutcome::Value {
        analysis: CanonicalCorrelationAnalysis {
            left_units: left_units.to_vec(),
            right_units: right_units.to_vec(),
            modes,
            sample_count,
            excluded_observations,
            regularization: config.regularization,
            tolerance: config.tolerance,
            sweeps: left_sweeps + right_sweeps + gram_sweeps,
        },
    })
}

fn validate_config(config: CanonicalCorrelationConfig) -> Result<(), CanonicalCorrelationError> {
    if config.minimum_samples.get() < 2
        || !config.regularization.is_finite()
        || config.regularization < 0.0
        || !config.tolerance.is_finite()
        || config.tolerance <= 0.0
        || config.tolerance >= 1.0
    {
        return Err(CanonicalCorrelationError::InvalidConfiguration);
    }
    Ok(())
}

fn validate_units(side: &'static str, units: &[UnitId]) -> Result<(), CanonicalCorrelationError> {
    let mut seen = BTreeSet::new();
    for unit in units {
        if unit.0.trim().is_empty() || !seen.insert(unit) {
            return Err(CanonicalCorrelationError::DuplicateUnit {
                side,
                unit: unit.clone(),
            });
        }
    }
    Ok(())
}

fn center(rows: &mut [Vec<f64>]) {
    for column in 0..rows[0].len() {
        let mean = rows.iter().map(|row| row[column]).sum::<f64>() / rows.len() as f64;
        for row in rows.iter_mut() {
            row[column] -= mean;
        }
    }
}

fn cross_product(left: &[Vec<f64>], right: &[Vec<f64>], denominator: f64) -> Vec<Vec<f64>> {
    let mut result = vec![vec![0.0; right[0].len()]; left[0].len()];
    for row in 0..left[0].len() {
        for column in 0..right[0].len() {
            result[row][column] = left
                .iter()
                .zip(right)
                .map(|(left, right)| left[row] * right[column])
                .sum::<f64>()
                / denominator;
        }
    }
    result
}

fn diagonal_sum(matrix: &[Vec<f64>]) -> f64 {
    matrix.iter().enumerate().map(|(i, row)| row[i]).sum()
}

#[derive(Debug)]
struct EigenComponent {
    value: f64,
    vector: Vec<f64>,
}

fn symmetric_eigen(
    mut matrix: Vec<Vec<f64>>,
    tolerance: f64,
    max_sweeps: usize,
) -> Result<(Vec<EigenComponent>, usize), CanonicalCorrelationError> {
    let n = matrix.len();
    let scale = matrix
        .iter()
        .flatten()
        .fold(0.0_f64, |largest, value| largest.max(value.abs()));
    if scale > 0.0 {
        for value in matrix.iter_mut().flatten() {
            *value /= scale;
        }
    }
    let mut vectors = vec![vec![0.0; n]; n];
    for (index, row) in vectors.iter_mut().enumerate() {
        row[index] = 1.0;
    }
    let mut sweeps = 0;
    while max_off_diagonal(&matrix) > tolerance {
        if sweeps == max_sweeps {
            return Err(CanonicalCorrelationError::DidNotConverge { sweeps });
        }
        for left in 0..n {
            for right in left + 1..n {
                if matrix[left][right].abs() > tolerance {
                    rotate(&mut matrix, &mut vectors, left, right);
                }
            }
        }
        sweeps += 1;
    }
    let mut components = (0..n)
        .map(|column| EigenComponent {
            value: matrix[column][column] * scale,
            vector: vectors.iter().map(|row| row[column]).collect(),
        })
        .collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| !component.value.is_finite())
    {
        return Err(CanonicalCorrelationError::NonFiniteResult);
    }
    components.sort_by(|left, right| right.value.total_cmp(&left.value));
    Ok((components, sweeps))
}

fn max_off_diagonal(matrix: &[Vec<f64>]) -> f64 {
    matrix
        .iter()
        .enumerate()
        .flat_map(|(row, values)| values.iter().skip(row + 1))
        .fold(0.0_f64, |largest, value| largest.max(value.abs()))
}

fn rotate(matrix: &mut [Vec<f64>], vectors: &mut [Vec<f64>], left: usize, right: usize) {
    let off_diagonal = matrix[left][right];
    let delta = (matrix[right][right] - matrix[left][left]) / 2.0;
    let tangent = off_diagonal / (delta + delta.hypot(off_diagonal).copysign(delta));
    let cosine = 1.0 / 1.0_f64.hypot(tangent);
    let sine = tangent * cosine;
    matrix[left][left] -= tangent * off_diagonal;
    matrix[right][right] += tangent * off_diagonal;
    matrix[left][right] = 0.0;
    matrix[right][left] = 0.0;
    #[allow(
        clippy::needless_range_loop,
        reason = "Jacobi rotation updates symmetric rows and columns"
    )]
    for index in 0..matrix.len() {
        if index != left && index != right {
            let index_left = matrix[index][left];
            let index_right = matrix[index][right];
            matrix[index][left] = cosine * index_left - sine * index_right;
            matrix[left][index] = matrix[index][left];
            matrix[index][right] = sine * index_left + cosine * index_right;
            matrix[right][index] = matrix[index][right];
        }
    }
    for row in vectors {
        let vector_left = row[left];
        let vector_right = row[right];
        row[left] = cosine * vector_left - sine * vector_right;
        row[right] = sine * vector_left + cosine * vector_right;
    }
}

fn inverse_square_root(
    components: &[EigenComponent],
    tolerance: f64,
) -> Result<Vec<Vec<f64>>, CanonicalCorrelationError> {
    let n = components.len();
    let scale = components.iter().fold(0.0_f64, |largest, component| {
        largest.max(component.value.abs())
    });
    let cutoff = tolerance * scale;
    if components.iter().any(|component| component.value < -cutoff) {
        return Err(CanonicalCorrelationError::NonFiniteResult);
    }
    let mut result = vec![vec![0.0; n]; n];
    for component in components {
        if component.value <= cutoff {
            continue;
        }
        let factor = component.value.sqrt().recip();
        for (row, result_row) in result.iter_mut().enumerate() {
            for (column, value) in result_row.iter_mut().enumerate() {
                *value += factor * component.vector[row] * component.vector[column];
            }
        }
    }
    Ok(result)
}

fn transpose(matrix: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let mut result = vec![vec![0.0; matrix.len()]; matrix[0].len()];
    for (row, values) in matrix.iter().enumerate() {
        for (column, value) in values.iter().enumerate() {
            result[column][row] = *value;
        }
    }
    result
}

fn multiply(left: &[Vec<f64>], right: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let mut result = vec![vec![0.0; right[0].len()]; left.len()];
    for (row, result_row) in result.iter_mut().enumerate() {
        for (column, value) in result_row.iter_mut().enumerate() {
            *value = (0..right.len())
                .map(|inner| left[row][inner] * right[inner][column])
                .sum();
        }
    }
    result
}

fn matrix_vector(matrix: &[Vec<f64>], vector: &[f64]) -> Vec<f64> {
    matrix
        .iter()
        .map(|row| {
            row.iter()
                .zip(vector)
                .map(|(left, right)| left * right)
                .sum()
        })
        .collect()
}

fn canonicalize_paired_sign(left: &mut [f64], right: &mut [f64]) {
    let mut pivot = 0;
    for index in 1..left.len() {
        if left[index].abs() > left[pivot].abs() {
            pivot = index;
        }
    }
    if left
        .get(pivot)
        .is_some_and(|value| value.is_sign_negative())
    {
        for value in left.iter_mut().chain(right.iter_mut()) {
            *value = -*value;
        }
    }
}

fn clean_zeroes(values: &mut [f64]) {
    for value in values {
        if *value == 0.0 {
            *value = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(values: &[&str]) -> Vec<UnitId> {
        values.iter().map(|value| UnitId::new(*value)).collect()
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

    #[test]
    fn recovers_deterministic_perfect_linear_modes() {
        let samples = (-3..=3)
            .map(|x| {
                let value = f64::from(x);
                let alternating = if x % 2 == 0 { 1.0 } else { -1.0 };
                sample(
                    &format!("sample-{x:+02}"),
                    &[("presentation", value), ("social", alternating)],
                    &[
                        ("composition", 2.0 * value + alternating),
                        ("sharing", -value + 3.0 * alternating),
                    ],
                )
            })
            .collect::<Vec<_>>();
        let config = CanonicalCorrelationConfig {
            regularization: 0.0,
            ..CanonicalCorrelationConfig::default()
        };
        let calculate = || {
            canonical_correlation(
                &units(&["presentation", "social"]),
                &units(&["composition", "sharing"]),
                &samples,
                config,
            )
            .unwrap()
        };
        let result = calculate();
        assert_eq!(calculate(), result);
        let CanonicalCorrelationOutcome::Value { analysis } = result else {
            panic!("expected CCA value")
        };
        assert_eq!(analysis.modes.len(), 2);
        assert!(analysis
            .modes
            .iter()
            .all(|mode| (mode.correlation - 1.0).abs() < 1e-10));
        assert_eq!(analysis.sample_count, 7);
        assert!(analysis.excluded_observations.is_empty());
        assert_eq!(
            serde_json::from_value::<CanonicalCorrelationAnalysis>(
                serde_json::to_value(&analysis).unwrap()
            )
            .unwrap(),
            analysis
        );
    }

    #[test]
    fn empty_variable_basis_is_not_an_evidence_shortfall() {
        assert_eq!(
            canonical_correlation(
                &[],
                &units(&["right"]),
                &[],
                CanonicalCorrelationConfig::default(),
            )
            .unwrap(),
            CanonicalCorrelationOutcome::Undefined {
                sample_count: 0,
                excluded_observations: vec![],
                reason: CanonicalCorrelationUndefined::NoVariables,
            }
        );
    }

    #[test]
    fn excludes_incomplete_rows_and_reports_the_complete_sample_floor() {
        let samples = vec![
            sample("complete", &[("left", 1.0)], &[("right", 2.0)]),
            sample("missing-left", &[], &[("right", 3.0)]),
            sample("missing-right", &[("left", 2.0)], &[]),
        ];
        assert_eq!(
            canonical_correlation(
                &units(&["left"]),
                &units(&["right"]),
                &samples,
                CanonicalCorrelationConfig::default(),
            )
            .unwrap(),
            CanonicalCorrelationOutcome::InsufficientEvidence {
                have: 1,
                need: 3,
                excluded_observations: vec![
                    ObservationId::new("missing-left"),
                    ObservationId::new("missing-right"),
                ],
            }
        );
    }

    #[test]
    fn separates_no_variation_from_insufficient_evidence_and_measured_zero() {
        let constant = [
            sample("a", &[("left", 1.0)], &[("right", 1.0)]),
            sample("b", &[("left", 1.0)], &[("right", 2.0)]),
            sample("c", &[("left", 1.0)], &[("right", 3.0)]),
        ];
        assert_eq!(
            canonical_correlation(
                &units(&["left"]),
                &units(&["right"]),
                &constant,
                CanonicalCorrelationConfig::default(),
            )
            .unwrap(),
            CanonicalCorrelationOutcome::Undefined {
                sample_count: 3,
                excluded_observations: vec![],
                reason: CanonicalCorrelationUndefined::NoVariation,
            }
        );

        let uncoupled = [
            sample("a", &[("left", -1.0)], &[("right", 1.0)]),
            sample("b", &[("left", 0.0)], &[("right", -2.0)]),
            sample("c", &[("left", 1.0)], &[("right", 1.0)]),
        ];
        let CanonicalCorrelationOutcome::Value { analysis } = canonical_correlation(
            &units(&["left"]),
            &units(&["right"]),
            &uncoupled,
            CanonicalCorrelationConfig::default(),
        )
        .unwrap() else {
            panic!("zero association must remain a measured value")
        };
        assert!(analysis.modes.is_empty());
    }

    #[test]
    fn unregularized_result_is_invariant_to_tiny_finite_units() {
        let samples = [
            sample("a", &[("left", -1e-100)], &[("right", -2.0)]),
            sample("b", &[("left", 0.0)], &[("right", 0.0)]),
            sample("c", &[("left", 1e-100)], &[("right", 2.0)]),
        ];
        let config = CanonicalCorrelationConfig {
            regularization: 0.0,
            ..CanonicalCorrelationConfig::default()
        };
        let CanonicalCorrelationOutcome::Value { analysis } =
            canonical_correlation(&units(&["left"]), &units(&["right"]), &samples, config).unwrap()
        else {
            panic!("tiny finite scale must remain measurable")
        };
        assert_eq!(analysis.modes.len(), 1);
        assert!((analysis.modes[0].correlation - 1.0).abs() < 1e-10);
    }

    #[test]
    fn rejects_duplicate_ids_unexpected_units_and_nonfinite_values() {
        let duplicate = sample("same", &[("left", 1.0)], &[("right", 1.0)]);
        assert!(matches!(
            canonical_correlation(
                &units(&["left"]),
                &units(&["right"]),
                &[duplicate.clone(), duplicate],
                CanonicalCorrelationConfig::default(),
            ),
            Err(CanonicalCorrelationError::DuplicateObservation(_))
        ));
        assert!(matches!(
            canonical_correlation(
                &units(&["left"]),
                &units(&["right"]),
                &[sample("extra", &[("other", 1.0)], &[("right", 1.0)])],
                CanonicalCorrelationConfig::default(),
            ),
            Err(CanonicalCorrelationError::UnexpectedUnit { .. })
        ));
        assert!(matches!(
            canonical_correlation(
                &units(&["left"]),
                &units(&["right"]),
                &[sample("nan", &[("left", f64::NAN)], &[("right", 1.0)])],
                CanonicalCorrelationConfig::default(),
            ),
            Err(CanonicalCorrelationError::InvalidSample(_))
        ));
    }
}
