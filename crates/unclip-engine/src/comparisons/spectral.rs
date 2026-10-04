//! Ordered-spectrum changes, without treating eigenvectors as identified factors.
use crate::support::{invalid, invalid_params};
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    spectral_decomposition, Delta, MeasurementKind, MeasurementValue, Reading,
    SpectralDecomposition,
};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, Result};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpectralComparison {
    Value {
        before: SpectralDecomposition,
        after: SpectralDecomposition,
        eigenvalue_differences: Vec<f64>,
        matching: String,
    },
    Unavailable {
        reason: String,
        before: Reading,
        after: Reading,
    },
    NotApplicable {
        reason: String,
    },
}
/// The write side of [`SpectralComparison`], borrowing what it serializes.
///
/// The `Value` arm owns the two decompositions it just computed, so it keeps
/// them; the arms that report whole readings borrow, because each is reached
/// with the matrices still in hand and copying both to serialize them once was
/// the expensive path. The round-trip test below pins the two shapes together.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum SpectralComparisonRef<'a> {
    Value {
        before: SpectralDecomposition,
        after: SpectralDecomposition,
        eigenvalue_differences: Vec<f64>,
        matching: &'a str,
    },
    Unavailable {
        reason: &'a str,
        before: &'a Reading,
        after: &'a Reading,
    },
    NotApplicable {
        reason: &'a str,
    },
}
pub struct SpectrumComparator {
    descriptor: ComparatorDescriptor,
}
impl Default for SpectrumComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.spectrum"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Matrix],
                params_schema: r#"{
                    "type":"object",
                    "additionalProperties":false,
                    "required":["minimum_samples","tolerance","max_sweeps"],
                    "properties":{"minimum_samples":{"type":"integer","minimum":2},"tolerance":{"type":"number","exclusiveMinimum":0,"exclusiveMaximum":1},"max_sweeps":{"type":"integer","minimum":1}}
                }"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    minimum_samples: NonZeroUsize,
    tolerance: f64,
    max_sweeps: NonZeroUsize,
}
impl Comparator for SpectrumComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let params: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
        if params.minimum_samples.get() < 2
            || !params.tolerance.is_finite()
            || params.tolerance <= 0.0
            || params.tolerance >= 1.0
        {
            return Err(invalid("spectral comparison requires at least two samples and a finite tolerance strictly between zero and one"));
        }
        let before = ctx.before();
        let after = ctx.after();
        if before.sensor != after.sensor
            || before.sensor_version != after.sensor_version
            || before.context != after.context
        {
            return Err(invalid(
                "spectral comparison requires the same sensor, version, and measurement context",
            ));
        }
        let unsupported = |reading: &Reading| matches!(reading,Reading::Value {value} if !matches!(value,MeasurementValue::PairwiseMatrix(_)));
        let result = if unsupported(&before.reading) || unsupported(&after.reading) {
            SpectralComparisonRef::NotApplicable {reason:"requires labeled pairwise matrices; no spectral meaning is inferred for other values"}
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
                    "spectral comparison requires identical metric and labeled unit axes",
                ));
            }
            let left = spectral_decomposition(
                a,
                params.minimum_samples,
                params.tolerance,
                params.max_sweeps,
            )
            .map_err(invalid)?;
            let right = spectral_decomposition(
                b,
                params.minimum_samples,
                params.tolerance,
                params.max_sweeps,
            )
            .map_err(invalid)?;
            if let (Some(left), Some(right)) = (left, right) {
                let differences = left
                    .eigenpairs
                    .iter()
                    .zip(&right.eigenpairs)
                    .map(|(a, b)| {
                        let difference = b.eigenvalue - a.eigenvalue;
                        if difference.is_finite() {
                            Ok(difference)
                        } else {
                            Err(invalid("spectral difference exceeds finite numeric range"))
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                SpectralComparisonRef::Value {before:left,after:right,eigenvalue_differences:differences,matching:"descending signed eigenvalue order; not matched factors or loading distances"}
            } else {
                SpectralComparisonRef::Unavailable {reason:"both matrices must be nonempty and every cell must be measured at the sample floor",before:&before.reading,after:&after.reading}
            }
        } else {
            SpectralComparisonRef::Unavailable {
                reason: "both matrix readings must be measured",
                before: &before.reading,
                after: &after.reading,
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(result).map_err(invalid)?.into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SpectralComparisonRef` is what the comparator writes and
    /// `SpectralComparison` is what consumers read. They are one wire format,
    /// so this pins them to each other over all three arms.
    #[test]
    fn the_borrowed_payload_writes_what_the_owned_one_reads() {
        let decomposition = SpectralDecomposition {
            metric: unclip_measure::PairwiseMetric::Spearman,
            eigenpairs: Vec::new(),
            minimum_cell_samples: 2,
            units: Vec::new().into(),
            sweeps: 0,
            tolerance: 0.5,
        };
        let scalar = Reading::Value {
            value: MeasurementValue::Scalar(1.0),
        };
        let missing = Reading::NotMeasured;
        let cases = [
            (
                SpectralComparisonRef::Value {
                    before: decomposition.clone(),
                    after: decomposition.clone(),
                    eigenvalue_differences: Vec::new(),
                    matching: "how",
                },
                SpectralComparison::Value {
                    before: decomposition.clone(),
                    after: decomposition,
                    eigenvalue_differences: Vec::new(),
                    matching: "how".to_owned(),
                },
            ),
            (
                SpectralComparisonRef::Unavailable {
                    reason: "why",
                    before: &scalar,
                    after: &missing,
                },
                SpectralComparison::Unavailable {
                    reason: "why".to_owned(),
                    before: scalar.clone(),
                    after: missing.clone(),
                },
            ),
            (
                SpectralComparisonRef::NotApplicable { reason: "why" },
                SpectralComparison::NotApplicable {
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
            let read: SpectralComparison =
                serde_json::from_value(written).expect("a written delta reads back");
            assert_eq!(read, owned);
        }
    }
}
