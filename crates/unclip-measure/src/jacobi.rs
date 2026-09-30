//! Cyclic Jacobi eigendecomposition of a real symmetric matrix.
//!
//! Two callers need this: [`crate::spectral`] decomposes a pairwise matrix into
//! anonymous empirical axes, and [`crate::canonical_correlation`] whitens its
//! covariance blocks. They had a rotation, a convergence measure, and a sweep
//! loop each, identical line for line under different identifiers — and the
//! copies had already begun to drift: only one of them still carried the
//! reasoning for the rotation's numerically-stable tangent.
//!
//! What the callers keep is what actually differs between them: how a matrix is
//! assembled from their own inputs, which errors they raise, and what they do
//! with the eigenvectors afterwards. Sign canonicalization in particular stays
//! with the callers, because they canonicalize different things — `spectral`
//! fixes the sign of each eigenvector, while `canonical_correlation` fixes the
//! sign of a *pair* of weight vectors much further downstream, and neither
//! substitutes for the other.

/// One eigenvalue with its eigenvector, in solver-column order.
///
/// The eigenvector's sign is whatever the rotation sequence produced. A caller
/// that needs a reproducible sign across equivalent inputs canonicalizes it
/// itself; see the module docs for why this is not done here.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Eigen {
    pub value: f64,
    pub vector: Vec<f64>,
}

/// The matrix did not reach `tolerance` within the sweep budget.
///
/// Returned rather than approximated: a partially-diagonalized matrix still
/// yields plausible-looking eigenvalues, and the callers both treat silent
/// non-convergence as the failure worth being loudest about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DidNotConverge {
    pub sweeps: usize,
}

/// Largest absolute value strictly above the diagonal.
///
/// The convergence measure: the matrix is diagonal to within `tolerance` when
/// this falls below it. Only the upper triangle is scanned because the input is
/// symmetric and every rotation preserves that.
fn max_off_diagonal(matrix: &[Vec<f64>]) -> f64 {
    matrix
        .iter()
        .enumerate()
        .flat_map(|(row, values)| values.iter().skip(row + 1))
        .fold(0.0_f64, |largest, value| largest.max(value.abs()))
}

/// Zero one symmetric off-diagonal pair, accumulating the rotation into
/// `vectors`.
fn rotate(matrix: &mut [Vec<f64>], vectors: &mut [Vec<f64>], p: usize, q: usize) {
    let off_diagonal = matrix[p][q];
    let delta = (matrix[q][q] - matrix[p][p]) / 2.0;
    // The denominator adds equal-sign terms, avoiding cancellation and a
    // potentially overflowing ratio delta / off_diagonal.
    let tangent = off_diagonal / (delta + delta.hypot(off_diagonal).copysign(delta));
    let cosine = 1.0 / 1.0_f64.hypot(tangent);
    let sine = tangent * cosine;
    matrix[p][p] -= tangent * off_diagonal;
    matrix[q][q] += tangent * off_diagonal;
    matrix[p][q] = 0.0;
    matrix[q][p] = 0.0;
    #[allow(
        clippy::needless_range_loop,
        reason = "Jacobi rotation updates both row and column entries of the same matrix"
    )]
    for k in 0..matrix.len() {
        if k != p && k != q {
            let kp = matrix[k][p];
            let kq = matrix[k][q];
            matrix[k][p] = cosine * kp - sine * kq;
            matrix[p][k] = matrix[k][p];
            matrix[k][q] = sine * kp + cosine * kq;
            matrix[q][k] = matrix[k][q];
        }
    }
    for row in vectors {
        let vp = row[p];
        let vq = row[q];
        row[p] = cosine * vp - sine * vq;
        row[q] = sine * vp + cosine * vq;
    }
}

/// Diagonalize `matrix` in place by cyclic Jacobi rotations.
///
/// `matrix` must be square and symmetric; callers assemble it, so that is their
/// invariant to hold rather than one re-checked per call.
///
/// The matrix is normalized by its largest absolute entry before sweeping and
/// the eigenvalues are rescaled afterwards, so `tolerance` means the same thing
/// whatever the inputs' magnitude. Pivot order is fixed (`p` ascending, then
/// `q`), which is what makes a repeated run reproduce its result exactly.
///
/// Returns the eigenpairs in solver-column order — **not** sorted, because the
/// callers sort by different keys — together with the number of sweeps taken.
pub(crate) fn symmetric_eigen(
    mut matrix: Vec<Vec<f64>>,
    tolerance: f64,
    max_sweeps: usize,
) -> Result<(Vec<Eigen>, usize), DidNotConverge> {
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
            return Err(DidNotConverge { sweeps });
        }
        for p in 0..n {
            for q in p + 1..n {
                if matrix[p][q].abs() > tolerance {
                    rotate(&mut matrix, &mut vectors, p, q);
                }
            }
        }
        sweeps += 1;
    }

    let eigen = (0..n)
        .map(|column| Eigen {
            value: matrix[column][column] * scale,
            vector: vectors.iter().map(|row| row[column]).collect(),
        })
        .collect();
    Ok((eigen, sweeps))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix with known eigenvalues is recovered, signs aside.
    #[test]
    fn recovers_analytic_eigenvalues() {
        let matrix = vec![vec![2.0, 1.0], vec![1.0, 2.0]];
        let (mut eigen, sweeps) = symmetric_eigen(matrix, 1e-12, 100).unwrap();
        eigen.sort_by(|left, right| right.value.total_cmp(&left.value));

        assert!(sweeps > 0);
        assert!((eigen[0].value - 3.0).abs() < 1e-10, "{:?}", eigen[0].value);
        assert!((eigen[1].value - 1.0).abs() < 1e-10, "{:?}", eigen[1].value);
    }

    /// Eigenvectors come back orthonormal, which is what both callers rely on.
    #[test]
    fn returns_an_orthonormal_basis() {
        let matrix = vec![
            vec![4.0, 1.0, 0.5],
            vec![1.0, 3.0, 0.25],
            vec![0.5, 0.25, 2.0],
        ];
        let (eigen, _) = symmetric_eigen(matrix, 1e-12, 100).unwrap();

        for component in &eigen {
            let norm = component
                .vector
                .iter()
                .map(|value| value * value)
                .sum::<f64>()
                .sqrt();
            assert!((norm - 1.0).abs() < 1e-10, "norm {norm}");
        }
        for (index, left) in eigen.iter().enumerate() {
            for right in eigen.iter().skip(index + 1) {
                let dot = left
                    .vector
                    .iter()
                    .zip(&right.vector)
                    .map(|(a, b)| a * b)
                    .sum::<f64>();
                assert!(dot.abs() < 1e-10, "dot {dot}");
            }
        }
    }

    /// Scaling the input scales the eigenvalues by the same factor.
    ///
    /// The internal normalization is invisible from outside, which is the
    /// property that lets both callers state `tolerance` without knowing their
    /// inputs' magnitude.
    #[test]
    fn normalization_does_not_leak_into_eigenvalues() {
        let base = vec![vec![2.0, 1.0], vec![1.0, 2.0]];
        let scaled = vec![vec![2.0e6, 1.0e6], vec![1.0e6, 2.0e6]];

        let (mut small, _) = symmetric_eigen(base, 1e-12, 100).unwrap();
        let (mut large, _) = symmetric_eigen(scaled, 1e-12, 100).unwrap();
        small.sort_by(|l, r| r.value.total_cmp(&l.value));
        large.sort_by(|l, r| r.value.total_cmp(&l.value));

        for (small, large) in small.iter().zip(&large) {
            assert!((small.value * 1.0e6 - large.value).abs() < 1e-4);
        }
    }

    /// An exhausted sweep budget is an error, never a partial result.
    #[test]
    fn refuses_to_return_an_unconverged_matrix() {
        let matrix = vec![vec![2.0, 1.0], vec![1.0, 2.0]];
        assert_eq!(
            symmetric_eigen(matrix, 1e-12, 0),
            Err(DidNotConverge { sweeps: 0 })
        );
    }

    /// An already-diagonal matrix needs no sweeps and is returned untouched.
    #[test]
    fn a_diagonal_matrix_converges_immediately() {
        let matrix = vec![vec![3.0, 0.0], vec![0.0, 1.0]];
        let (eigen, sweeps) = symmetric_eigen(matrix, 1e-12, 100).unwrap();

        assert_eq!(sweeps, 0);
        assert_eq!(eigen[0].value, 3.0);
        assert_eq!(eigen[1].value, 1.0);
    }

    /// The zero matrix has scale zero, which must not divide.
    #[test]
    fn an_all_zero_matrix_does_not_divide_by_its_scale() {
        let (eigen, sweeps) = symmetric_eigen(vec![vec![0.0, 0.0], vec![0.0, 0.0]], 1e-12, 100)
            .expect("a zero matrix is already diagonal");

        assert_eq!(sweeps, 0);
        assert!(eigen.iter().all(|component| component.value == 0.0));
    }
}
