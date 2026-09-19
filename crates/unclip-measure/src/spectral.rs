//! Deterministic symmetric eigendecomposition of a complete pairwise matrix.

use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;

use crate::{MatrixCell, PairwiseMatrix, PairwiseMetric};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Eigenpair {
    pub eigenvalue: f64,
    /// Unit loadings in the decomposition's unit-ID order, with Euclidean norm 1.
    pub loadings: Vec<f64>,
}

/// Anonymous empirical axes, not interpreted semantic factors. Negative
/// eigenvalues are retained: pairwise estimates need not be positive definite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpectralDecomposition {
    pub metric: PairwiseMetric,
    pub units: Vec<UnitId>,
    pub eigenpairs: Vec<Eigenpair>,
    /// Smallest sample count across all input cells, including the diagonal.
    pub minimum_cell_samples: usize,
    pub tolerance: f64,
    pub sweeps: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectralError {
    InvalidTolerance,
    DidNotConverge { sweeps: usize },
    NonFiniteResult,
}

impl std::fmt::Display for SpectralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTolerance => write!(
                f,
                "spectral tolerance must be finite and strictly between zero and one"
            ),
            Self::DidNotConverge { sweeps } => write!(
                f,
                "spectral decomposition did not converge after {sweeps} sweeps"
            ),
            Self::NonFiniteResult => {
                write!(f, "spectral eigenvalue is outside the finite numeric range")
            }
        }
    }
}
impl std::error::Error for SpectralError {}

/// Cyclic Jacobi eigendecomposition with fixed pivot and reduction order.
/// `None` means empty input, an undefined/sparse cell, or a cell below the
/// configured sample floor. Missing values are never filled with zero.
///
/// The matrix is scaled by its largest absolute entry. Convergence requires all
/// normalized off-diagonal entries to be <= `tolerance`; failure within
/// `max_sweeps` is an error, never an apparently successful partial result.
/// Eigenvalues are sorted descending; equal values retain solver-column order.
/// The first largest-magnitude loading in each vector is made nonnegative.
/// Repeated eigenspaces have a deterministic basis for identical input, but do
/// not imply uniquely identifiable factors. No variance ratios are inferred.
pub fn spectral_decomposition(
    matrix: &PairwiseMatrix,
    minimum_samples: NonZeroUsize,
    tolerance: f64,
    max_sweeps: NonZeroUsize,
) -> Result<Option<SpectralDecomposition>, SpectralError> {
    if !tolerance.is_finite() || tolerance <= 0.0 || tolerance >= 1.0 {
        return Err(SpectralError::InvalidTolerance);
    }
    let n = matrix.units().len();
    if n == 0 {
        return Ok(None);
    }
    let mut a = vec![vec![0.0; n]; n];
    let mut scale = 0.0_f64;
    let mut minimum_cell_samples = usize::MAX;
    for (i, row) in matrix.cells().iter().enumerate() {
        for (j, cell) in row.iter().enumerate() {
            let MatrixCell::Value {
                value,
                sample_count,
            } = cell
            else {
                return Ok(None);
            };
            if *sample_count < minimum_samples.get() {
                return Ok(None);
            }
            minimum_cell_samples = minimum_cell_samples.min(*sample_count);
            scale = scale.max(value.abs());
            a[i][j] = *value;
        }
    }
    if scale > 0.0 {
        for row in &mut a {
            for value in row {
                *value /= scale;
            }
        }
    }
    let mut vectors = vec![vec![0.0; n]; n];
    for (i, row) in vectors.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    let mut sweeps = 0;
    while max_off_diagonal(&a) > tolerance {
        if sweeps == max_sweeps.get() {
            return Err(SpectralError::DidNotConverge { sweeps });
        }
        for p in 0..n {
            for q in p + 1..n {
                if a[p][q].abs() <= tolerance {
                    continue;
                }
                rotate(&mut a, &mut vectors, p, q);
            }
        }
        sweeps += 1;
    }
    let mut eigenpairs = Vec::with_capacity(n);
    for (column, row) in a.iter().enumerate() {
        let eigenvalue = row[column] * scale;
        if !eigenvalue.is_finite() {
            return Err(SpectralError::NonFiniteResult);
        }
        let mut loadings = vectors.iter().map(|row| row[column]).collect::<Vec<_>>();
        let mut pivot = 0;
        for i in 1..n {
            if loadings[i].abs() > loadings[pivot].abs() {
                pivot = i;
            }
        }
        if loadings[pivot].is_sign_negative() {
            for value in &mut loadings {
                *value = -*value;
            }
        }
        for value in &mut loadings {
            if *value == 0.0 {
                *value = 0.0;
            }
        }
        eigenpairs.push(Eigenpair {
            eigenvalue,
            loadings,
        });
    }
    eigenpairs.sort_by(|left, right| right.eigenvalue.total_cmp(&left.eigenvalue));
    Ok(Some(SpectralDecomposition {
        metric: matrix.metric(),
        units: matrix.units().to_vec(),
        eigenpairs,
        minimum_cell_samples,
        tolerance,
        sweeps,
    }))
}

fn max_off_diagonal(matrix: &[Vec<f64>]) -> f64 {
    matrix
        .iter()
        .enumerate()
        .flat_map(|(i, row)| row.iter().skip(i + 1))
        .fold(0.0, |largest, value| largest.max(value.abs()))
}

fn rotate(a: &mut [Vec<f64>], vectors: &mut [Vec<f64>], p: usize, q: usize) {
    let off_diagonal = a[p][q];
    let delta = (a[q][q] - a[p][p]) / 2.0;
    // The denominator adds equal-sign terms, avoiding cancellation and a
    // potentially overflowing ratio delta / off_diagonal.
    let t = off_diagonal / (delta + delta.hypot(off_diagonal).copysign(delta));
    let c = 1.0 / 1.0_f64.hypot(t);
    let s = t * c;
    a[p][p] -= t * off_diagonal;
    a[q][q] += t * off_diagonal;
    a[p][q] = 0.0;
    a[q][p] = 0.0;
    #[allow(
        clippy::needless_range_loop,
        reason = "Jacobi rotation updates both row and column entries of the same matrix"
    )]
    for k in 0..a.len() {
        if k != p && k != q {
            let kp = a[k][p];
            let kq = a[k][q];
            a[k][p] = c * kp - s * kq;
            a[p][k] = a[k][p];
            a[k][q] = s * kp + c * kq;
            a[q][k] = a[k][q];
        }
    }
    for row in vectors {
        let vp = row[p];
        let vq = row[q];
        row[p] = c * vp - s * vq;
        row[q] = s * vp + c * vq;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(values: &[&[f64]]) -> PairwiseMatrix {
        serde_json::from_value(serde_json::json!({
            "metric": "relative_rank_variance",
            "units": (0..values.len()).map(|i| format!("unit-{i}")).collect::<Vec<_>>(),
            "cells": values.iter().map(|row| row.iter().map(|value| serde_json::json!({
                "status": "value", "value": value, "sample_count": 5,
            })).collect::<Vec<_>>()).collect::<Vec<_>>(),
        }))
        .unwrap()
    }
    fn decompose(matrix: &PairwiseMatrix) -> SpectralDecomposition {
        spectral_decomposition(
            matrix,
            NonZeroUsize::new(2).unwrap(),
            1e-12,
            NonZeroUsize::new(100).unwrap(),
        )
        .unwrap()
        .unwrap()
    }
    fn check_reconstruction(values: &[&[f64]], result: &SpectralDecomposition) {
        for (i, row) in values.iter().enumerate() {
            for (j, original) in row.iter().enumerate() {
                let reconstructed = result
                    .eigenpairs
                    .iter()
                    .map(|pair| pair.eigenvalue * pair.loadings[i] * pair.loadings[j])
                    .sum::<f64>();
                assert!(
                    (reconstructed - original).abs() < 1e-10,
                    "{i},{j}: {reconstructed} != {original}"
                );
            }
        }
        for (i, left) in result.eigenpairs.iter().enumerate() {
            for (j, right) in result.eigenpairs.iter().enumerate() {
                let dot = left
                    .loadings
                    .iter()
                    .zip(&right.loadings)
                    .map(|(a, b)| a * b)
                    .sum::<f64>();
                assert!((dot - if i == j { 1.0 } else { 0.0 }).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn analytic_eigenvalues_and_negative_modes_are_preserved() {
        let values: &[&[f64]] = &[&[0.0, 2.0], &[2.0, 0.0]];
        let result = decompose(&matrix(values));
        assert_eq!(
            result
                .eigenpairs
                .iter()
                .map(|pair| pair.eigenvalue)
                .collect::<Vec<_>>(),
            vec![2.0, -2.0]
        );
        assert!(result.eigenpairs.iter().all(|pair| pair.loadings[0] > 0.0));
        assert_eq!(result.minimum_cell_samples, 5);
        check_reconstruction(values, &result);
    }

    #[test]
    fn reconstructs_a_dense_matrix_with_orthonormal_deterministic_modes() {
        let values: &[&[f64]] = &[
            &[0.0, 1.0, 3.0, 2.0],
            &[1.0, 0.0, 2.0, 4.0],
            &[3.0, 2.0, 0.0, 1.0],
            &[2.0, 4.0, 1.0, 0.0],
        ];
        let input = matrix(values);
        let result = decompose(&input);
        check_reconstruction(values, &result);
        assert_eq!(decompose(&input), result);
        assert!(result
            .eigenpairs
            .windows(2)
            .all(|pair| pair[0].eigenvalue >= pair[1].eigenvalue));
        assert_eq!(
            serde_json::from_str::<SpectralDecomposition>(&serde_json::to_string(&result).unwrap())
                .unwrap(),
            result
        );
        assert!(matches!(
            spectral_decomposition(
                &input,
                NonZeroUsize::new(2).unwrap(),
                1e-15,
                NonZeroUsize::new(1).unwrap()
            ),
            Err(SpectralError::DidNotConverge { sweeps: 1 })
        ));
    }

    #[test]
    fn handles_zero_singleton_repeated_and_large_eigenvalues() {
        for values in [
            vec![vec![0.0, 0.0], vec![0.0, 0.0]],
            vec![vec![1.0, 0.0], vec![0.0, 1.0]],
            vec![vec![7.0]],
        ] {
            let rows = values.iter().map(Vec::as_slice).collect::<Vec<_>>();
            let result = decompose(&matrix(&rows));
            assert_eq!(result.sweeps, 0);
            check_reconstruction(&rows, &result);
        }
        for magnitude in [1e-250, 1e250] {
            let input = matrix(&[&[0.0, magnitude], &[magnitude, 0.0]]);
            let result = decompose(&input);
            assert!((result.eigenpairs[0].eigenvalue / magnitude - 1.0).abs() < 1e-12);
            assert!((result.eigenpairs[1].eigenvalue / magnitude + 1.0).abs() < 1e-12);
        }
        let overflow = matrix(&[&[f64::MAX, f64::MAX], &[f64::MAX, f64::MAX]]);
        assert_eq!(
            spectral_decomposition(
                &overflow,
                NonZeroUsize::new(2).unwrap(),
                1e-12,
                NonZeroUsize::new(10).unwrap()
            ),
            Err(SpectralError::NonFiniteResult)
        );
    }

    #[test]
    fn rejects_missing_or_insufficient_cells_and_invalid_tolerances() {
        let input = matrix(&[&[1.0, 0.5], &[0.5, 1.0]]);
        let samples = NonZeroUsize::new(2).unwrap();
        let sweeps = NonZeroUsize::new(10).unwrap();
        for tolerance in [0.0, -1.0, 1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                spectral_decomposition(&input, samples, tolerance, sweeps),
                Err(SpectralError::InvalidTolerance)
            );
        }
        for cell in [
            serde_json::json!({"status":"undefined", "sample_count":5}),
            serde_json::json!({"status":"insufficient_evidence", "have":1,"need":2}),
        ] {
            let mut json = serde_json::to_value(&input).unwrap();
            json["cells"][0][1] = cell.clone();
            json["cells"][1][0] = cell;
            let sparse = serde_json::from_value::<PairwiseMatrix>(json).unwrap();
            assert_eq!(
                spectral_decomposition(&sparse, samples, 1e-12, sweeps).unwrap(),
                None
            );
        }
        assert_eq!(
            spectral_decomposition(&input, NonZeroUsize::new(6).unwrap(), 1e-12, sweeps).unwrap(),
            None
        );
        assert_eq!(
            spectral_decomposition(&matrix(&[]), samples, 1e-12, sweeps).unwrap(),
            None
        );
    }
}
