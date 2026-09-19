//! Anonymous components of an explicitly thresholded pairwise graph.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;

use crate::{MatrixCell, PairwiseMatrix, PairwiseMetric};

/// A pair that could not be assessed. Its original cell retains the distinction
/// between undefined correlation, sparse evidence, and too few usable samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnassessedPair {
    pub left: UnitId,
    pub right: UnitId,
    pub evidence: MatrixCell,
}

/// Connected components of the observed threshold graph, not semantic labels.
/// Missing edges can hide links between components; `unassessed` must remain
/// alongside the partition. Connectivity is transitive: members need not all
/// meet the threshold pairwise. Singleton groups are retained.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommunityDetection {
    pub metric: PairwiseMetric,
    pub threshold: f64,
    pub minimum_samples: NonZeroUsize,
    pub communities: Vec<Vec<UnitId>>,
    pub assessed_pairs: usize,
    pub qualifying_pairs: usize,
    pub unassessed: Vec<UnassessedPair>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCommunityThreshold;

impl std::fmt::Display for InvalidCommunityThreshold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "community threshold must be finite and within the metric's range"
        )
    }
}
impl std::error::Error for InvalidCommunityThreshold {}

/// Finds deterministic connected components of a thresholded pairwise graph.
/// For Spearman, Kendall, and mutual information an edge qualifies at or above
/// `threshold`. Relative-rank variance qualifies at or below it. Correlation
/// signs are preserved; no absolute values or cross-metric aggregation occur.
/// Correlation thresholds must lie in [-1, 1]; other thresholds must be >= 0.
/// A zero mutual-information threshold intentionally includes measured zero.
///
/// Diagonals never supply inter-unit evidence. Only measured off-diagonal cells
/// meeting `minimum_samples` are assessed. `None` means no pair could be
/// assessed, including empty and singleton inputs. A result with no qualifying
/// edges is a measured partition into singletons, distinct from `None`.
/// Groups and members follow stable unit-ID order; the algorithm uses no RNG.
pub fn detect_communities(
    matrix: &PairwiseMatrix,
    threshold: f64,
    minimum_samples: NonZeroUsize,
) -> Result<Option<CommunityDetection>, InvalidCommunityThreshold> {
    let valid_range = match matrix.metric() {
        PairwiseMetric::Spearman | PairwiseMetric::Kendall => (-1.0..=1.0).contains(&threshold),
        PairwiseMetric::RelativeRankVariance | PairwiseMetric::MutualInformation => {
            threshold >= 0.0
        }
    };
    if !threshold.is_finite() || !valid_range {
        return Err(InvalidCommunityThreshold);
    }
    let units = matrix.units();
    let mut parents = (0..units.len()).collect::<Vec<_>>();
    let mut result = CommunityDetection {
        metric: matrix.metric(),
        threshold,
        minimum_samples,
        communities: Vec::new(),
        assessed_pairs: 0,
        qualifying_pairs: 0,
        unassessed: Vec::new(),
    };
    for (left, row) in matrix.cells().iter().enumerate() {
        for (right, cell) in row.iter().enumerate().skip(left + 1) {
            let value = match cell {
                MatrixCell::Value {
                    value,
                    sample_count,
                } if *sample_count >= minimum_samples.get() => *value,
                _ => {
                    result.unassessed.push(UnassessedPair {
                        left: units[left].clone(),
                        right: units[right].clone(),
                        evidence: cell.clone(),
                    });
                    continue;
                }
            };
            result.assessed_pairs += 1;
            let qualifies = match matrix.metric() {
                PairwiseMetric::RelativeRankVariance => value <= threshold,
                _ => value >= threshold,
            };
            if qualifies {
                result.qualifying_pairs += 1;
                let left_root = root(&mut parents, left);
                let right_root = root(&mut parents, right);
                // Stable roots make output independent of union-tree shape.
                parents[left_root.max(right_root)] = left_root.min(right_root);
            }
        }
    }
    if result.assessed_pairs == 0 {
        return Ok(None);
    }
    let mut groups = BTreeMap::<usize, Vec<UnitId>>::new();
    for (index, unit) in units.iter().enumerate() {
        groups
            .entry(root(&mut parents, index))
            .or_default()
            .push(unit.clone());
    }
    result.communities = groups.into_values().collect();
    Ok(Some(result))
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
    use crate::{pairwise_matrix, RankPosition, RankSample, RankTrajectory};
    use unclip_observe::ObservationId;

    fn trajectory(unit: &str, ranks: &[usize]) -> RankTrajectory {
        RankTrajectory {
            unit: UnitId::new(unit),
            samples: ranks
                .iter()
                .enumerate()
                .map(|(index, &rank)| RankSample {
                    observation: ObservationId::new(index.to_string()),
                    position: RankPosition::Ranked { rank },
                })
                .collect(),
        }
    }
    fn detect(matrix: &PairwiseMatrix, threshold: f64) -> Option<CommunityDetection> {
        detect_communities(matrix, threshold, NonZeroUsize::new(2).unwrap()).unwrap()
    }
    fn groups(groups: &[&[&str]]) -> Vec<Vec<UnitId>> {
        groups
            .iter()
            .map(|group| group.iter().map(|id| UnitId::new(*id)).collect())
            .collect()
    }

    #[test]
    fn finds_anonymous_groups_and_preserves_metric_disagreement() {
        let trajectories = [
            trajectory("a", &[1, 2, 3]),
            trajectory("b", &[1, 2, 3]),
            trajectory("c", &[3, 2, 1]),
        ];
        for metric in [PairwiseMetric::Spearman, PairwiseMetric::Kendall] {
            let matrix = pairwise_matrix(&trajectories, metric).unwrap();
            let result = detect(&matrix, 1.0).unwrap();
            assert_eq!(result.communities, groups(&[&["a", "b"], &["c"]]));
            assert_eq!((result.assessed_pairs, result.qualifying_pairs), (3, 1));
            assert!(result.unassessed.is_empty());
            let mut reversed = trajectories.clone();
            reversed.reverse();
            assert_eq!(
                detect(&pairwise_matrix(&reversed, metric).unwrap(), 1.0),
                Some(result.clone())
            );
            assert_eq!(
                serde_json::from_str::<CommunityDetection>(
                    &serde_json::to_string(&result).unwrap()
                )
                .unwrap(),
                result
            );
        }
        let movement =
            pairwise_matrix(&trajectories, PairwiseMetric::RelativeRankVariance).unwrap();
        assert_eq!(
            detect(&movement, 0.0).unwrap().communities,
            groups(&[&["a", "b"], &["c"]])
        );
        let dependency = pairwise_matrix(&trajectories, PairwiseMetric::MutualInformation).unwrap();
        assert_eq!(
            detect(&dependency, 1.0).unwrap().communities,
            groups(&[&["a", "b", "c"]])
        );
    }

    #[test]
    fn connectivity_is_transitive_and_does_not_require_a_clique() {
        // A-B and B-C meet .5; A-C does not. D is measured but isolated.
        let matrix: PairwiseMatrix = serde_json::from_value(serde_json::json!({
            "metric":"spearman", "units":["a","b","c","d"],
            "cells": ([
                [1.0, 0.6, 0.1, -0.2],
                [0.6, 1.0, 0.7, -0.3],
                [0.1, 0.7, 1.0, -0.1],
                [-0.2, -0.3, -0.1, 1.0]
            ].map(|row| row.map(|value| serde_json::json!({"status":"value","value":value,"sample_count":10}))))
        })).unwrap();
        let result = detect(&matrix, 0.5).unwrap();
        assert_eq!(result.communities, groups(&[&["a", "b", "c"], &["d"]]));
        assert_eq!((result.assessed_pairs, result.qualifying_pairs), (6, 2));
    }

    #[test]
    fn missing_undefined_and_low_sample_edges_are_not_measured_absence() {
        let a = trajectory("a", &[1, 2, 3]);
        let b = trajectory("b", &[3, 2, 1]);
        let mut sparse = trajectory("c", &[1, 2, 3]);
        sparse.samples[0].position = RankPosition::Unknown;
        let constant = trajectory("d", &[1, 1, 1]);
        let matrix = pairwise_matrix(&[a, b, sparse, constant], PairwiseMetric::Spearman).unwrap();
        let result = detect_communities(&matrix, 0.5, NonZeroUsize::new(3).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            result.communities,
            groups(&[&["a"], &["b"], &["c"], &["d"]])
        );
        assert_eq!(
            (
                result.assessed_pairs,
                result.qualifying_pairs,
                result.unassessed.len()
            ),
            (1, 0, 5)
        );
        assert!(result
            .unassessed
            .iter()
            .any(|pair| matches!(pair.evidence, MatrixCell::Undefined { .. })));
        assert!(result.unassessed.iter().any(|pair| matches!(
            pair.evidence,
            MatrixCell::Value {
                sample_count: 2,
                ..
            }
        )));
        assert_eq!(
            detect_communities(&matrix, 0.5, NonZeroUsize::new(4).unwrap()).unwrap(),
            None
        );
        let mut missing = trajectory("e", &[1, 2, 3]);
        for sample in &mut missing.samples {
            sample.position = RankPosition::Missing;
        }
        let matrix = pairwise_matrix(
            &[trajectory("a", &[1, 2, 3]), missing],
            PairwiseMetric::Spearman,
        )
        .unwrap();
        assert_eq!(detect(&matrix, 0.5), None);
    }

    #[test]
    fn validates_thresholds_and_distinguishes_measured_zero() {
        let trajectories = [
            trajectory("a", &[1, 1, 2, 2]),
            trajectory("b", &[1, 2, 1, 2]),
        ];
        let matrix = pairwise_matrix(&trajectories, PairwiseMetric::MutualInformation).unwrap();
        assert_eq!(
            detect(&matrix, 0.1).unwrap().communities,
            groups(&[&["a"], &["b"]])
        );
        assert_eq!(
            detect(&matrix, 0.0).unwrap().communities,
            groups(&[&["a", "b"]])
        );
        for threshold in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                detect_communities(&matrix, threshold, NonZeroUsize::new(2).unwrap()),
                Err(InvalidCommunityThreshold)
            );
        }
        let correlation = pairwise_matrix(&trajectories, PairwiseMetric::Spearman).unwrap();
        for threshold in [-1.1, 1.1] {
            assert_eq!(
                detect_communities(&correlation, threshold, NonZeroUsize::new(2).unwrap()),
                Err(InvalidCommunityThreshold)
            );
        }
        assert_eq!(
            detect(
                &pairwise_matrix(&[], PairwiseMetric::Spearman).unwrap(),
                0.5
            ),
            None
        );
        assert_eq!(
            detect(
                &pairwise_matrix(&trajectories[..1], PairwiseMetric::Spearman).unwrap(),
                0.5
            ),
            None
        );
        assert!(InvalidCommunityThreshold.to_string().contains("finite"));
    }
}
