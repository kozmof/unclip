//! Exact identity comparison for explicitly structured measurement values.

use crate::support::{invalid, invalid_params};
use serde::{Deserialize, Serialize};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Delta, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, Result};

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

/// The write side of [`StructuredIdentityComparison`], borrowing what it
/// serializes.
///
/// This comparator is the one where the owning shape cost the most: its
/// `Value` arm reports both measured values verbatim, and those are arbitrary
/// structured trees, so building the payload deep-copied two of them per
/// comparison only to serialize them once. The round-trip test below pins the
/// two shapes together.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum StructuredIdentityComparisonRef<'a> {
    Value {
        identical: bool,
        expected: &'a serde_json::Value,
        observed: &'a serde_json::Value,
    },
    Unavailable {
        expected: &'a Reading,
        observed: &'a Reading,
    },
    NotApplicable {
        reason: &'a str,
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

impl Comparator for StructuredIdentityComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }

    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let _: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
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
            StructuredIdentityComparisonRef::NotApplicable {
                reason:
                    "requires explicitly structured values; no fields are inferred or scalarized",
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
            StructuredIdentityComparisonRef::Value {
                identical: expected == observed,
                expected,
                observed,
            }
        } else {
            StructuredIdentityComparisonRef::Unavailable {
                expected: &expected.reading,
                observed: &observed.reading,
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(comparison).map_err(invalid)?.into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_borrowed_payload_writes_what_the_owned_one_reads() {
        let tree = serde_json::json!({"a": [1, 2, {"b": "c"}]});
        let other = serde_json::json!({"a": []});
        let missing = Reading::NotMeasured;
        let cases = [
            (
                StructuredIdentityComparisonRef::Value {
                    identical: false,
                    expected: &tree,
                    observed: &other,
                },
                StructuredIdentityComparison::Value {
                    identical: false,
                    expected: tree.clone(),
                    observed: other.clone(),
                },
            ),
            (
                StructuredIdentityComparisonRef::Unavailable {
                    expected: &missing,
                    observed: &missing,
                },
                StructuredIdentityComparison::Unavailable {
                    expected: missing.clone(),
                    observed: missing.clone(),
                },
            ),
            (
                StructuredIdentityComparisonRef::NotApplicable { reason: "why" },
                StructuredIdentityComparison::NotApplicable {
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
            let read: StructuredIdentityComparison =
                serde_json::from_value(written).expect("a written delta reads back");
            assert_eq!(read, owned);
        }
    }
}
