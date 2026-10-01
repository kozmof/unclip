//! Chronological change-point alignment on an explicit observation sequence.
use crate::support::{invalid, invalid_params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    ChangePoint, Delta, Measurement, MeasurementKind, MeasurementValue, ObservationSequence,
    Reading,
};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, Result};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventMatch {
    pub before: ChangePoint,
    pub after: ChangePoint,
    pub shift_steps: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventComparison {
    Value {
        sequence: ObservationSequence,
        max_shift_steps: usize,
        matching: String,
        matched: Vec<EventMatch>,
        removed: Vec<ChangePoint>,
        added: Vec<ChangePoint>,
    },
    Unavailable {
        before: Reading,
        after: Reading,
    },
    NotApplicable {
        reason: String,
    },
}
/// The write side of [`EventMatch`], borrowing the two aligned events.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct EventMatchRef<'a> {
    before: &'a ChangePoint,
    after: &'a ChangePoint,
    shift_steps: i64,
}
/// The write side of [`EventComparison`], borrowing what it serializes.
///
/// Every event the alignment reports is already held by the parsed sequences,
/// which outlive the payload, so matching no longer copies a change point into
/// each of the three result lists. The `Unavailable` arm borrows its readings
/// for the same reason the other comparators do. The round-trip test below
/// pins this shape to the owning one.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum EventComparisonRef<'a> {
    Value {
        sequence: &'a ObservationSequence,
        max_shift_steps: usize,
        matching: &'a str,
        matched: Vec<EventMatchRef<'a>>,
        removed: Vec<&'a ChangePoint>,
        added: Vec<&'a ChangePoint>,
    },
    Unavailable {
        before: &'a Reading,
        after: &'a Reading,
    },
    NotApplicable {
        reason: &'a str,
    },
}
pub struct ChangePointAlignmentComparator {
    descriptor: ComparatorDescriptor,
}
impl Default for ChangePointAlignmentComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.change-point-alignment"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Events],
                params_schema: r#"{
                    "type":"object",
                    "additionalProperties":false,
                    "required":["max_shift_steps"],
                    "properties":{"max_shift_steps":{"type":"integer","minimum":0}}
                }"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    max_shift_steps: usize,
}
fn events(
    measurement: &Measurement,
    values: &[serde_json::Value],
) -> Result<(ObservationSequence, Vec<ChangePoint>)> {
    let context = &measurement.context.values;
    let sequence: ObservationSequence = serde_json::from_value(
        context
            .get("sequence")
            .ok_or_else(|| invalid("change-point comparison requires explicit sequence"))?
            .clone(),
    )
    .map_err(invalid)?;
    if sequence
        .observations()
        .iter()
        .any(|o| o.observation.is_empty())
    {
        return Err(invalid("empty sequence observation identity"));
    }
    let window = context
        .get("window")
        .and_then(|v| v.as_u64())
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| invalid("change-point window must be positive"))?;
    let count = window
        .checked_mul(2)
        .ok_or_else(|| invalid("change-point window overflow"))?;
    if sequence.observations().len() < count {
        return Err(invalid(
            "measured change-point events require a sequence long enough for two windows",
        ));
    }
    let minimum_shift = context
        .get("minimum_shift")
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| invalid("invalid change-point threshold"))?;
    if context
        .get("unit")
        .and_then(|v| v.as_str())
        .is_none_or(|v| v.trim().is_empty())
    {
        return Err(invalid("change-point unit must be explicit"));
    }
    let mut events = Vec::new();
    let mut indices = BTreeSet::new();
    for value in values {
        let event: ChangePoint = serde::Deserialize::deserialize(value).map_err(invalid)?;
        if !indices.insert(event.index)
            || event.index < window
            || event
                .index
                .checked_add(window)
                .is_none_or(|end| end > sequence.observations().len())
            || sequence
                .observations()
                .get(event.index)
                .is_none_or(|o| o.observation != event.observation)
            || event.sample_count != count
            || !event.before_mean.is_finite()
            || !event.after_mean.is_finite()
            || !(event.after_mean - event.before_mean).is_finite()
            || (event.after_mean - event.before_mean).abs() < minimum_shift
        {
            return Err(invalid(
                "change-point event conflicts with its sequence, window, or detection threshold",
            ));
        }
        events.push(event);
    }
    events.sort_by_key(|e| e.index);
    Ok((sequence, events))
}
impl Comparator for ChangePointAlignmentComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let params: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
        let before = ctx.before();
        let after = ctx.after();
        if before.sensor != after.sensor
            || before.sensor_version != after.sensor_version
            || before.context != after.context
        {
            return Err(invalid(
                "event comparison requires the same sensor, version, and sequence context",
            ));
        }
        let unsupported = |r: &Reading| matches!(r,Reading::Value {value} if !matches!(value,MeasurementValue::Events(_)));
        let not_applicable = before.sensor.as_str() != "sensor.change-points"
            || unsupported(&before.reading)
            || unsupported(&after.reading);
        let parse = |m: &Measurement| -> Result<Option<(ObservationSequence, Vec<ChangePoint>)>> {
            if let Reading::Value {
                value: MeasurementValue::Events(v),
            } = &m.reading
            {
                Ok(Some(events(m, v)?))
            } else {
                Ok(None)
            }
        };
        // The parsed sequences outlive the payload so it can borrow their events.
        let parsed = if not_applicable {
            (None, None)
        } else {
            (parse(before)?, parse(after)?)
        };
        let result = if not_applicable {
            EventComparisonRef::NotApplicable {
                reason: "requires change-point sensor events; other event schemas are not inferred",
            }
        } else if let (Some((sequence, a)), Some((_, b))) = (&parsed.0, &parsed.1) {
            let (mut i, mut j) = (0, 0);
            let mut matched = Vec::new();
            let mut removed = Vec::new();
            let mut added = Vec::new();
            while i < a.len() && j < b.len() {
                if a[i].index.abs_diff(b[j].index) <= params.max_shift_steps {
                    let shift = i64::try_from(b[j].index).map_err(invalid)?
                        - i64::try_from(a[i].index).map_err(invalid)?;
                    matched.push(EventMatchRef {
                        before: &a[i],
                        after: &b[j],
                        shift_steps: shift,
                    });
                    i += 1;
                    j += 1;
                } else if a[i].index < b[j].index {
                    removed.push(&a[i]);
                    i += 1;
                } else {
                    added.push(&b[j]);
                    j += 1;
                }
            }
            removed.extend(a[i..].iter());
            added.extend(b[j..].iter());
            EventComparisonRef::Value {
                sequence,
                max_shift_steps: params.max_shift_steps,
                matching:
                    "chronological earliest feasible one-to-one match; not minimum displacement",
                matched,
                removed,
                added,
            }
        } else {
            EventComparisonRef::Unavailable {
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
    use unclip_measure::OrderedObservation;

    /// `EventComparisonRef` is what the comparator writes and
    /// `EventComparison` is what consumers read. They are one wire format, so
    /// this pins them to each other over all three arms.
    #[test]
    fn the_borrowed_payload_writes_what_the_owned_one_reads() {
        let sequence = ObservationSequence::new(vec![OrderedObservation {
            observation: unclip_observe::ObservationId::new("first"),
            position: 0,
        }])
        .expect("a single-entry sequence is valid");
        let event = ChangePoint {
            observation: unclip_observe::ObservationId::new("first"),
            index: 0,
            before_mean: 0.0,
            after_mean: 1.0,
            sample_count: 2,
        };
        let scalar = Reading::Value {
            value: MeasurementValue::Scalar(1.0),
        };
        let missing = Reading::NotMeasured;
        let cases = [
            (
                EventComparisonRef::Value {
                    sequence: &sequence,
                    max_shift_steps: 1,
                    matching: "how",
                    matched: vec![EventMatchRef {
                        before: &event,
                        after: &event,
                        shift_steps: 0,
                    }],
                    removed: vec![&event],
                    added: Vec::new(),
                },
                EventComparison::Value {
                    sequence: sequence.clone(),
                    max_shift_steps: 1,
                    matching: "how".to_owned(),
                    matched: vec![EventMatch {
                        before: event.clone(),
                        after: event.clone(),
                        shift_steps: 0,
                    }],
                    removed: vec![event.clone()],
                    added: Vec::new(),
                },
            ),
            (
                EventComparisonRef::Unavailable {
                    before: &scalar,
                    after: &missing,
                },
                EventComparison::Unavailable {
                    before: scalar.clone(),
                    after: missing.clone(),
                },
            ),
            (
                EventComparisonRef::NotApplicable { reason: "why" },
                EventComparison::NotApplicable {
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
            let read: EventComparison =
                serde_json::from_value(written).expect("a written delta reads back");
            assert_eq!(read, owned);
        }
    }
}
