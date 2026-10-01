//! Cellwise comparison of labeled pairwise matrices without aggregate scores.
use crate::support::{invalid, invalid_params};
use serde::{Deserialize, Serialize};
use unclip_domain::UnitId;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    Delta, MatrixCell, MeasurementKind, MeasurementValue, PairwiseMetric, Reading,
};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum MatrixCellDifference {
    Value {
        before: MatrixCell,
        after: MatrixCell,
        difference: f64,
    },
    Unavailable {
        before: MatrixCell,
        after: MatrixCell,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum MatrixComparison {
    Value {
        metric: PairwiseMetric,
        units: Vec<UnitId>,
        minimum_samples: usize,
        cells: Vec<Vec<MatrixCellDifference>>,
    },
    Unavailable {
        before: Reading,
        after: Reading,
    },
    NotApplicable {
        reason: String,
    },
}
/// The write side of [`MatrixComparison`], borrowing what it serializes.
///
/// The `Value` arm reports the shared unit axis, which it reads straight off
/// the matrix it is comparing, and the `Unavailable` arms report two whole
/// readings that are reached with both matrices in hand. The payload is built
/// only to become JSON, so neither needs its own copy. The round-trip test
/// below pins this shape to the owning one.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum MatrixComparisonRef<'a> {
    Value {
        metric: PairwiseMetric,
        units: &'a [UnitId],
        minimum_samples: usize,
        cells: Vec<Vec<MatrixCellDifference>>,
    },
    Unavailable {
        before: &'a Reading,
        after: &'a Reading,
    },
    NotApplicable {
        reason: &'a str,
    },
}
pub struct PairwiseMatrixComparator {
    descriptor: ComparatorDescriptor,
}
impl Default for PairwiseMatrixComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.pairwise-matrix"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Matrix],
                params_schema: r#"{
                    "type":"object",
                    "additionalProperties":false,
                    "required":["minimum_samples"],
                    "properties":{"minimum_samples":{"type":"integer","minimum":2}}
                }"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    minimum_samples: usize,
}
impl Comparator for PairwiseMatrixComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let params: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
        if params.minimum_samples < 2 {
            return Err(invalid(
                "matrix comparisons require at least two samples per measured cell",
            ));
        }
        let before = ctx.before();
        let after = ctx.after();
        if before.sensor != after.sensor
            || before.sensor_version != after.sensor_version
            || before.context != after.context
        {
            return Err(invalid(
                "matrix comparison requires the same sensor, version, and measurement context",
            ));
        }
        let unsupported = |reading: &Reading| matches!(reading,Reading::Value {value} if !matches!(value,MeasurementValue::PairwiseMatrix(_)));
        let result = if unsupported(&before.reading) || unsupported(&after.reading) {
            MatrixComparisonRef::NotApplicable {reason:"requires labeled pairwise matrices; unlabeled axes and other values are not inferred"}
        } else if let (
            Reading::Value {
                value: MeasurementValue::PairwiseMatrix(a),
            },
            Reading::Value {
                value: MeasurementValue::PairwiseMatrix(b),
            },
        ) = (&before.reading, &after.reading)
        {
            if a.metric() != b.metric() || a.units() != b.units() {
                return Err(invalid(
                    "matrix comparison requires identical metric and labeled unit axes",
                ));
            }
            if a.units().is_empty() {
                MatrixComparisonRef::Unavailable {
                    before: &before.reading,
                    after: &after.reading,
                }
            } else {
                let mut cells = Vec::new();
                for (left, right) in a.cells().iter().zip(b.cells()) {
                    let mut row = Vec::new();
                    for (a, b) in left.iter().zip(right) {
                        let cell = match (a, b) {
                            (
                                MatrixCell::Value {
                                    value: x,
                                    sample_count: n,
                                },
                                MatrixCell::Value {
                                    value: y,
                                    sample_count: m,
                                },
                            ) if *n >= params.minimum_samples && *m >= params.minimum_samples => {
                                let difference = y - x;
                                if !difference.is_finite() {
                                    return Err(invalid(
                                        "matrix cell difference exceeds finite numeric range",
                                    ));
                                }
                                MatrixCellDifference::Value {
                                    before: a.clone(),
                                    after: b.clone(),
                                    difference,
                                }
                            }
                            _ => MatrixCellDifference::Unavailable {
                                before: a.clone(),
                                after: b.clone(),
                            },
                        };
                        row.push(cell);
                    }
                    cells.push(row);
                }
                MatrixComparisonRef::Value {
                    metric: a.metric(),
                    units: a.units(),
                    minimum_samples: params.minimum_samples,
                    cells,
                }
            }
        } else {
            MatrixComparisonRef::Unavailable {
                before: &before.reading,
                after: &after.reading,
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(result)
                    .map_err(|e| PluginError::Message(e.to_string()))?
                    .into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MatrixComparisonRef` is what the comparator writes and
    /// `MatrixComparison` is what consumers read. They are one wire format, so
    /// this pins them to each other over all three arms.
    #[test]
    fn the_borrowed_payload_writes_what_the_owned_one_reads() {
        let units = vec![UnitId::new("a"), UnitId::new("b")];
        let cells = vec![vec![MatrixCellDifference::Unavailable {
            before: MatrixCell::Undefined { sample_count: 2 },
            after: MatrixCell::Undefined { sample_count: 2 },
        }]];
        let scalar = Reading::Value {
            value: MeasurementValue::Scalar(1.0),
        };
        let missing = Reading::NotMeasured;
        let cases = [
            (
                MatrixComparisonRef::Value {
                    metric: PairwiseMetric::Spearman,
                    units: &units,
                    minimum_samples: 2,
                    cells: cells.clone(),
                },
                MatrixComparison::Value {
                    metric: PairwiseMetric::Spearman,
                    units: units.clone(),
                    minimum_samples: 2,
                    cells,
                },
            ),
            (
                MatrixComparisonRef::Unavailable {
                    before: &scalar,
                    after: &missing,
                },
                MatrixComparison::Unavailable {
                    before: scalar.clone(),
                    after: missing.clone(),
                },
            ),
            (
                MatrixComparisonRef::NotApplicable { reason: "why" },
                MatrixComparison::NotApplicable {
                    reason: "why".to_owned(),
                },
            ),
        ];
        for (borrowed, owned) in cases {
            let written = serde_json::to_value(&borrowed).expect("borrowed payload serializes");
            assert_eq!(
                written,
                serde_json::to_value(&owned).expect("owned payload serializes")
            );
            let read: MatrixComparison =
                serde_json::from_value(written).expect("a written delta reads back");
            assert_eq!(read, owned);
        }
    }
}
