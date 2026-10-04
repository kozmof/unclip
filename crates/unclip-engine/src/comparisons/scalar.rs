//! Explicit, tracked comparisons; scalar subtraction is one comparator only.
use crate::support::{invalid, invalid_params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use unclip_epistemic::{
    Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata, PluginId,
    SharedParams, Tracked,
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

/// The write side of [`ScalarDifference`], borrowing what it serializes.
///
/// The comparator builds this payload only to turn it straight into JSON, and
/// it never retains it. The `Unavailable` arm is where that matters: it is
/// reached precisely when the readings are *not* scalars, so the two readings
/// it reports can be matrices or graphs, and copying both of them per
/// comparison to serialize them once is the expensive path through this
/// comparator. `ScalarDifference` stays as the readable, owning shape that
/// consumers deserialize into; the round-trip test below pins the two
/// together.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ScalarDifferenceRef<'a> {
    Value {
        before: f64,
        after: f64,
        difference: f64,
    },
    Unavailable {
        before: &'a Reading,
        after: &'a Reading,
    },
    NotApplicable {
        reason: &'a str,
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
impl Comparator for ScalarDifferenceComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let _: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
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
            ScalarDifferenceRef::NotApplicable {
                reason: "requires scalar readings; structured values are not scalarized",
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
            ScalarDifferenceRef::Value {
                before: *a,
                after: *b,
                difference,
            }
        } else {
            ScalarDifferenceRef::Unavailable {
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

/// Compare one explicitly paired measurement using only selected comparators.
pub fn compare_measurements(
    plan: &RunPlan,
    before: &Tracked<Measurement>,
    after: &Tracked<Measurement>,
    run: crate::MeasurementRun<'_>,
) -> Result<Vec<Calculated<Delta>>> {
    compare_with_comparators(&plan.comparators, before, after, run)
}

/// Run exactly these comparators over one before/after pair.
///
/// A comparison reads nothing from a plan but its comparators, so a caller that
/// has selected some — the independence stage runs them one at a time — passes
/// them directly instead of building a plan around them.
pub(crate) fn compare_with_comparators(
    comparators: &[Arc<dyn Comparator>],
    before: &Tracked<Measurement>,
    after: &Tracked<Measurement>,
    run: crate::MeasurementRun<'_>,
) -> Result<Vec<Calculated<Delta>>> {
    crate::support::require_run_id("comparison", run.id)?;
    crate::require_calculated_evidence(before, "comparison input measurement")?;
    crate::require_calculated_evidence(after, "comparison input measurement")?;
    let mut comparators = comparators.iter().collect::<Vec<_>>();
    comparators.sort_by_key(|p| &p.descriptor().id);
    let mut results = Vec::new();
    let empty = SharedParams::new(serde_json::json!({}));
    for comparator in comparators {
        let descriptor = comparator.descriptor();
        let params = run.params.get(&descriptor.id).unwrap_or(&empty);
        crate::support::require_declared_params(&descriptor.id, descriptor.params_schema, params)?;
        let ctx = CompareCtx::new(before, after, params, DependencyCollector::default());
        let token = ctx.calculation_token(EmitMetadata::new(
            DerivedId::new(format!("{}/{}", run.id, descriptor.id)),
            descriptor.id.clone(),
            descriptor.version.clone(),
            ctx.shared_params(),
            run.timestamp.clone(),
        ));
        let delta = comparator.compare(&ctx, token)?;
        crate::support::require_producer(&descriptor.id, &delta)?;
        results.push(delta);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ScalarDifferenceRef` is what the comparator writes and
    /// `ScalarDifference` is what consumers read. They are one wire format, so
    /// a drift between them would silently change every recorded delta. This
    /// pins them to each other over all three arms.
    #[test]
    fn the_borrowed_payload_writes_what_the_owned_one_reads() {
        let matrix = Reading::Value {
            value: MeasurementValue::Matrix(vec![vec![1.0, 0.5], vec![0.5, 1.0]].into()),
        };
        let missing = Reading::NotMeasured;
        let cases = [
            (
                ScalarDifferenceRef::Value {
                    before: 1.0,
                    after: 2.5,
                    difference: 1.5,
                },
                ScalarDifference::Value {
                    before: 1.0,
                    after: 2.5,
                    difference: 1.5,
                },
            ),
            (
                ScalarDifferenceRef::Unavailable {
                    before: &matrix,
                    after: &missing,
                },
                ScalarDifference::Unavailable {
                    before: matrix.clone(),
                    after: missing.clone(),
                },
            ),
            (
                ScalarDifferenceRef::NotApplicable { reason: "why" },
                ScalarDifference::NotApplicable {
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
            let read: ScalarDifference =
                serde_json::from_value(written).expect("a written delta reads back");
            assert_eq!(read, owned);
        }
    }
}
