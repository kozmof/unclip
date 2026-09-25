//! Exact identity comparison for explicitly structured measurement values.

use serde::{Deserialize, Serialize};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Delta, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuredIdentityComparison {
    Value {
        identical: bool,
        expected: serde_json::Value,
        observed: serde_json::Value,
    },
    Unavailable {
        expected: Reading,
        observed: Reading,
    },
    NotApplicable {
        reason: String,
    },
}

pub struct StructuredIdentityComparator {
    descriptor: ComparatorDescriptor,
}

impl Default for StructuredIdentityComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.structured-identity"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Structured],
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

impl Comparator for StructuredIdentityComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }

    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let _: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|error| invalid(error.to_string()))?;
        let expected = ctx.before();
        let observed = ctx.after();
        if expected.sensor != observed.sensor
            || expected.sensor_version != observed.sensor_version
            || expected.context != observed.context
        {
            return Err(invalid(
                "structured comparison requires the same sensor, version, and measurement context",
            ));
        }
        let unsupported = |reading: &Reading| matches!(reading, Reading::Value { value } if !matches!(value, MeasurementValue::Structured(_)));
        let comparison = if unsupported(&expected.reading) || unsupported(&observed.reading) {
            StructuredIdentityComparison::NotApplicable {
                reason:
                    "requires explicitly structured values; no fields are inferred or scalarized"
                        .into(),
            }
        } else if let (
            Reading::Value {
                value: MeasurementValue::Structured(expected),
            },
            Reading::Value {
                value: MeasurementValue::Structured(observed),
            },
        ) = (&expected.reading, &observed.reading)
        {
            StructuredIdentityComparison::Value {
                identical: expected == observed,
                expected: expected.clone(),
                observed: observed.clone(),
            }
        } else {
            StructuredIdentityComparison::Unavailable {
                expected: expected.reading.clone(),
                observed: observed.reading.clone(),
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(comparison).map_err(|error| invalid(error.to_string()))?,
            ),
        }))
    }
}
