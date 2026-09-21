//! Ordered-spectrum changes, without treating eigenvectors as identified factors.
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    spectral_decomposition, Delta, MeasurementKind, MeasurementValue, Reading,
    SpectralDecomposition,
};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result};
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
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["minimum_samples","tolerance","max_sweeps"],"properties":{"minimum_samples":{"type":"integer","minimum":2},"tolerance":{"type":"number","exclusiveMinimum":0,"exclusiveMaximum":1},"max_sweeps":{"type":"integer","minimum":1}}}"#,
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
fn invalid(s: impl ToString) -> PluginError {
    PluginError::Message(s.to_string())
}
impl Comparator for SpectrumComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
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
            SpectralComparison::NotApplicable {reason:"requires labeled pairwise matrices; no spectral meaning is inferred for other values".into()}
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
                SpectralComparison::Value {before:left,after:right,eigenvalue_differences:differences,matching:"descending signed eigenvalue order; not matched factors or loading distances".into()}
            } else {
                SpectralComparison::Unavailable {reason:"both matrices must be nonempty and every cell must be measured at the sample floor".into(),before:before.reading.clone(),after:after.reading.clone()}
            }
        } else {
            SpectralComparison::Unavailable {
                reason: "both matrix readings must be measured".into(),
                before: before.reading.clone(),
                after: after.reading.clone(),
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(serde_json::to_value(result).map_err(invalid)?),
        }))
    }
}
