//! Explicit, tracked comparisons; scalar subtraction is one comparator only.
use serde::{Deserialize, Serialize};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Tracked,
};
use unclip_measure::{Delta, Measurement, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result, RunPlan};

/// Payload carried in a scalar comparator's structured Delta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScalarDifference {
    Value {
        before: f64,
        after: f64,
        difference: f64,
    },
    Unavailable {
        before: Reading,
        after: Reading,
    },
    NotApplicable {
        reason: String,
    },
}
pub struct ScalarDifferenceComparator {
    descriptor: ComparatorDescriptor,
}
impl Default for ScalarDifferenceComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.scalar-difference"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Scalar],
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}
fn invalid(message: &str) -> PluginError {
    PluginError::Message(message.into())
}
impl Comparator for ScalarDifferenceComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let _: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
        let before = ctx.before();
        let after = ctx.after();
        if before.sensor != after.sensor
            || before.sensor_version != after.sensor_version
            || before.context != after.context
        {
            return Err(invalid(
                "scalar comparison requires the same sensor, version, and measurement context",
            ));
        }
        let unsupported = |reading: &Reading| matches!(reading,Reading::Value {value} if !matches!(value,MeasurementValue::Scalar(_)));
        // Validate measured scalars even if the other side is unavailable.
        for reading in [&before.reading, &after.reading] {
            if matches!(reading,Reading::Value {value:MeasurementValue::Scalar(v)} if !v.is_finite())
            {
                return Err(invalid("scalar comparison requires finite readings"));
            }
        }
        let result = if unsupported(&before.reading) || unsupported(&after.reading) {
            ScalarDifference::NotApplicable {
                reason: "requires scalar readings; structured values are not scalarized".into(),
            }
        } else if let (
            Reading::Value {
                value: MeasurementValue::Scalar(a),
            },
            Reading::Value {
                value: MeasurementValue::Scalar(b),
            },
        ) = (&before.reading, &after.reading)
        {
            let difference = b - a;
            if !difference.is_finite() {
                return Err(invalid("scalar difference exceeds finite numeric range"));
            }
            ScalarDifference::Value {
                before: *a,
                after: *b,
                difference,
            }
        } else {
            ScalarDifference::Unavailable {
                before: before.reading.clone(),
                after: after.reading.clone(),
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(result).map_err(|e| PluginError::Message(e.to_string()))?,
            ),
        }))
    }
}
impl super::Engine {
    /// Compare one explicitly paired measurement using only selected comparators.
    pub fn compare_measurements(
        &self,
        plan: &RunPlan,
        before: &Tracked<Measurement>,
        after: &Tracked<Measurement>,
        run: super::MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<Delta>>> {
        super::require_calculated_evidence(before, "comparison input measurement")?;
        super::require_calculated_evidence(after, "comparison input measurement")?;
        let mut comparators = plan.comparators.iter().collect::<Vec<_>>();
        comparators.sort_by_key(|p| &p.descriptor().id);
        let mut results = Vec::new();
        let empty = serde_json::json!({});
        for comparator in comparators {
            let descriptor = comparator.descriptor();
            let params = run.params.get(&descriptor.id).unwrap_or(&empty);
            let ctx = CompareCtx::new(before, after, params, DependencyCollector::default());
            let token = ctx.calculation_token(EmitMetadata {
                id: DerivedId::new(format!("{}/{}", run.id, descriptor.id)),
                producer: descriptor.id.clone(),
                algorithm: descriptor.id.0.clone(),
                version: descriptor.version.clone(),
                params: params.clone(),
                params_hash: hash_params(params),
                source: None,
                timestamp: run.timestamp.clone(),
                domain_version: None,
                frame_version: None,
                model: None,
            });
            results.push(comparator.compare(&ctx, token)?);
        }
        Ok(results)
    }
}
