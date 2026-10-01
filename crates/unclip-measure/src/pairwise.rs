//! Labeled pairwise statistics, without reconciling different metrics.

use std::{collections::BTreeSet, sync::Arc};

use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;

use crate::{
    kendall_association, mutual_information, pairwise_rank_values, spearman_correlation,
    validate_trajectory_alignment, RankTrajectory, TrajectoryAlignmentError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairwiseMetric {
    /// Population variance of rank(left) - rank(right).
    RelativeRankVariance,
    Spearman,
    Kendall,
    /// Empirical discrete-rank mutual information in bits.
    MutualInformation,
}

/// Undefined correlation (constant ranks) is distinct from too few samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum MatrixCell {
    Value { value: f64, sample_count: usize },
    InsufficientEvidence { have: usize, need: usize },
    Undefined { sample_count: usize },
}

/// A square symmetric matrix whose row and column axes share stable unit IDs.
/// Construction and deserialization validate shape, symmetry, and finite values.
///
/// The validated data is held behind an [`Arc`]. A matrix is immutable once
/// built — no method hands out a mutable reference to one — and the same matrix
/// is routinely carried in a reading, tracked as an input, compared, stored, and
/// reported within a single run. Owning the rows inline made every one of those
/// hops copy an n×n cell grid and its unit axis; sharing the allocation makes
/// copying a matrix a refcount bump.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "MatrixData")]
pub struct PairwiseMatrix(Arc<MatrixData>);

// Written out rather than declared with `into = "MatrixData"`, which serializes
// by converting `self.clone()` into the inner data and so copied the whole
// matrix every time one was written. Borrowing the shared data produces the
// identical bytes and copies nothing.
impl Serialize for PairwiseMatrix {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatrixData {
    metric: PairwiseMetric,
    units: Vec<UnitId>,
    cells: Vec<Vec<MatrixCell>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairwiseMatrixError {
    #[error("unit {0} appears more than once in the matrix frame")]
    DuplicateUnit(UnitId),
    #[error("duplicate observation at index {index}")]
    DuplicateObservation { index: usize },
    #[error(transparent)]
    Alignment(#[from] TrajectoryAlignmentError),
    #[error("matrix must be square, symmetric, and ordered by unit id")]
    InvalidMatrix,
}

impl TryFrom<MatrixData> for PairwiseMatrix {
    type Error = PairwiseMatrixError;
    fn try_from(data: MatrixData) -> Result<Self, Self::Error> {
        let n = data.units.len();
        if data.units.windows(2).any(|pair| pair[0] >= pair[1])
            || data.cells.len() != n
            || data.cells.iter().any(|row| row.len() != n)
        {
            return Err(PairwiseMatrixError::InvalidMatrix);
        }
        for (i, row) in data.cells.iter().enumerate() {
            for (j, cell) in row.iter().enumerate() {
                let valid = match cell {
                    MatrixCell::Value {
                        value,
                        sample_count,
                    } => value.is_finite() && *sample_count >= 2,
                    MatrixCell::InsufficientEvidence { have, need } => *need == 2 && have < need,
                    MatrixCell::Undefined { sample_count } => *sample_count >= 2,
                };
                if !valid || *cell != data.cells[j][i] {
                    return Err(PairwiseMatrixError::InvalidMatrix);
                }
            }
        }
        Ok(Self(Arc::new(data)))
    }
}
impl PairwiseMatrix {
    pub fn metric(&self) -> PairwiseMetric {
        self.0.metric
    }
    pub fn units(&self) -> &[UnitId] {
        &self.0.units
    }
    pub fn cells(&self) -> &[Vec<MatrixCell>] {
        &self.0.cells
    }
}

/// Computes one explicitly selected metric per unit pair, including diagonals.
/// Axes use sorted unit IDs, independently of input trajectory order. Every
/// trajectory must name the same unique observations in the same order. Each
/// cell uses only pairwise-complete ranks and retains its own sample count.
/// Results are associations, not causal evidence or universal profile scores.
pub fn pairwise_matrix(
    trajectories: &[RankTrajectory],
    metric: PairwiseMetric,
) -> Result<PairwiseMatrix, PairwiseMatrixError> {
    let mut ordered = trajectories.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.unit.cmp(&right.unit));
    for pair in ordered.windows(2) {
        if pair[0].unit == pair[1].unit {
            return Err(PairwiseMatrixError::DuplicateUnit(pair[0].unit.clone()));
        }
    }
    if let Some(first) = ordered.first() {
        let mut seen = BTreeSet::new();
        for (index, sample) in first.samples.iter().enumerate() {
            if !seen.insert(&sample.observation) {
                return Err(PairwiseMatrixError::DuplicateObservation { index });
            }
        }
        for other in &ordered {
            validate_trajectory_alignment(first, other)?;
        }
    }
    let n = ordered.len();
    let mut cells = vec![vec![MatrixCell::InsufficientEvidence { have: 0, need: 2 }; n]; n];
    for (i, left) in ordered.iter().enumerate() {
        for (j, right) in ordered.iter().enumerate().skip(i) {
            let (xs, ys) = pairwise_rank_values(left, right)?;
            let sample_count = xs.len();
            let cell = if sample_count < 2 {
                MatrixCell::InsufficientEvidence {
                    have: sample_count,
                    need: 2,
                }
            } else {
                let value = match metric {
                    PairwiseMetric::Spearman => {
                        spearman_correlation(left, right)?.map(|v| v.coefficient)
                    }
                    PairwiseMetric::Kendall => {
                        kendall_association(left, right)?.map(|v| v.coefficient)
                    }
                    PairwiseMetric::MutualInformation => {
                        mutual_information(left, right)?.map(|v| v.value)
                    }
                    PairwiseMetric::RelativeRankVariance => {
                        let differences = xs
                            .iter()
                            .zip(&ys)
                            .map(|(x, y)| {
                                let magnitude = x.abs_diff(*y) as f64;
                                if x >= y {
                                    magnitude
                                } else {
                                    -magnitude
                                }
                            })
                            .collect::<Vec<_>>();
                        let mean = differences.iter().sum::<f64>() / sample_count as f64;
                        Some(
                            differences.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
                                / sample_count as f64,
                        )
                    }
                };
                value.map_or(MatrixCell::Undefined { sample_count }, |value| {
                    MatrixCell::Value {
                        value,
                        sample_count,
                    }
                })
            };
            cells[i][j] = cell.clone();
            cells[j][i] = cell;
        }
    }
    PairwiseMatrix::try_from(MatrixData {
        metric,
        units: ordered
            .iter()
            .map(|trajectory| trajectory.unit.clone())
            .collect(),
        cells,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MeasurementKind, MeasurementValue, RankPosition, RankSample};
    use unclip_observe::ObservationId;

    fn ranks(unit: &str, values: &[usize]) -> RankTrajectory {
        RankTrajectory {
            unit: UnitId::new(unit),
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

    /// A matrix is carried in a reading, tracked as an input, compared, stored,
    /// and reported in one run, so copying one shares the cell grid and writing
    /// one borrows it. The owning shape it used to declare serialized
    /// `self.clone()` — an n×n copy per write — for the identical bytes, so
    /// only pointer identity catches a regression back to it.
    #[test]
    fn copying_a_matrix_shares_one_cell_grid() {
        let matrix = pairwise_matrix(
            &[ranks("a", &[1, 2, 3]), ranks("b", &[3, 2, 1])],
            PairwiseMetric::Spearman,
        )
        .expect("two aligned trajectories");
        let copied = matrix.clone();
        assert_eq!(matrix, copied);
        assert!(std::ptr::eq(matrix.cells(), copied.cells()));
        assert!(std::ptr::eq(matrix.units(), copied.units()));
        // Writing one is unchanged, and still round-trips into the same matrix.
        let json = serde_json::to_string(&matrix).expect("a matrix serializes");
        assert_eq!(
            serde_json::from_str::<PairwiseMatrix>(&json).expect("a matrix deserializes"),
            matrix
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()["units"],
            serde_json::json!(["a", "b"])
        );
    }

    #[test]
    fn preserves_metric_disagreement_labels_and_permutation_invariance() {
        let a = ranks("a", &[1, 2, 3]);
        let b = ranks("b", &[3, 2, 1]);
        for (metric, expected) in [
            (PairwiseMetric::Spearman, -1.0),
            (PairwiseMetric::Kendall, -1.0),
            (PairwiseMetric::RelativeRankVariance, 8.0 / 3.0),
            (PairwiseMetric::MutualInformation, 3.0_f64.log2()),
        ] {
            let matrix = pairwise_matrix(&[b.clone(), a.clone()], metric).unwrap();
            assert_eq!(matrix.units(), &[a.unit.clone(), b.unit.clone()]);
            assert_eq!(matrix.metric(), metric);
            let MatrixCell::Value {
                value,
                sample_count,
            } = matrix.cells()[0][1]
            else {
                panic!("expected measured value")
            };
            assert!((value - expected).abs() < 1e-12);
            assert_eq!(sample_count, 3);
            assert_eq!(
                matrix,
                pairwise_matrix(&[a.clone(), b.clone()], metric).unwrap()
            );
            let measurement = MeasurementValue::PairwiseMatrix(matrix);
            assert_eq!(measurement.kind(), MeasurementKind::Matrix);
            assert_eq!(
                serde_json::from_str::<MeasurementValue>(
                    &serde_json::to_string(&measurement).unwrap()
                )
                .unwrap(),
                measurement
            );
        }
    }

    #[test]
    fn sparse_cells_constant_correlations_and_measured_zero_stay_distinct() {
        let a = ranks("a", &[1, 1, 1]);
        let mut b = ranks("b", &[1, 2, 3]);
        b.samples[0].position = RankPosition::Missing;
        let matrix = pairwise_matrix(&[a.clone(), b.clone()], PairwiseMetric::Spearman).unwrap();
        assert_eq!(
            matrix.cells()[0][1],
            MatrixCell::Undefined { sample_count: 2 }
        );
        b.samples[1].position = RankPosition::Unknown;
        let matrix = pairwise_matrix(&[a, b], PairwiseMetric::RelativeRankVariance).unwrap();
        assert_eq!(
            matrix.cells()[0][0],
            MatrixCell::Value {
                value: 0.0,
                sample_count: 3
            }
        );
        assert_eq!(
            matrix.cells()[0][1],
            MatrixCell::InsufficientEvidence { have: 1, need: 2 }
        );
        assert!(pairwise_matrix(&[], PairwiseMetric::Kendall)
            .unwrap()
            .cells()
            .is_empty());
    }

    #[test]
    fn rejects_ambiguous_axes_and_misaligned_observations() {
        let a = ranks("a", &[1, 2]);
        assert!(matches!(
            pairwise_matrix(&[a.clone(), a.clone()], PairwiseMetric::Spearman),
            Err(PairwiseMatrixError::DuplicateUnit(_))
        ));
        assert!(matches!(
            pairwise_matrix(&[a.clone(), ranks("b", &[1])], PairwiseMetric::Spearman),
            Err(PairwiseMatrixError::Alignment(_))
        ));
        let mut duplicate = a.clone();
        duplicate.samples[1].observation = duplicate.samples[0].observation.clone();
        assert_eq!(
            pairwise_matrix(&[duplicate], PairwiseMetric::Spearman),
            Err(PairwiseMatrixError::DuplicateObservation { index: 1 })
        );
        let mut b = ranks("b", &[1, 2]);
        b.samples.swap(0, 1);
        assert!(pairwise_matrix(&[a, b], PairwiseMetric::Spearman).is_err());
    }

    #[test]
    fn deserialization_rejects_invalid_matrix_shapes_and_cells() {
        let matrix = pairwise_matrix(
            &[ranks("a", &[1, 2]), ranks("b", &[2, 1])],
            PairwiseMetric::Spearman,
        )
        .unwrap();
        let valid = serde_json::to_value(&matrix).unwrap();
        for path in ["shape", "symmetry", "count", "units", "unknown"] {
            let mut value = valid.clone();
            match path {
                "shape" => value["cells"][0] = serde_json::json!([]),
                "symmetry" => value["cells"][0][1]["value"] = serde_json::json!(0.0),
                "count" => value["cells"][0][0]["sample_count"] = serde_json::json!(0),
                "units" => value["units"][0] = value["units"][1].clone(),
                _ => value["extra"] = serde_json::json!(true),
            }
            assert!(
                serde_json::from_value::<PairwiseMatrix>(value).is_err(),
                "{path}"
            );
        }
    }
}
