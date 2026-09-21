//! Chronological change-point alignment on an explicit observation sequence.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    ChangePoint, Delta, Measurement, MeasurementKind, MeasurementValue, ObservationSequence,
    Reading,
};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result};
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
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["max_shift_steps"],"properties":{"max_shift_steps":{"type":"integer","minimum":0}}}"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    max_shift_steps: usize,
}
fn invalid(s: impl ToString) -> PluginError {
    PluginError::Message(s.to_string())
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
        .any(|o| o.observation.0.is_empty())
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
        let event: ChangePoint = serde_json::from_value(value.clone()).map_err(invalid)?;
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
        let params: Parameters = serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
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
        let result = if before.sensor.0 != "sensor.change-points"
            || unsupported(&before.reading)
            || unsupported(&after.reading)
        {
            EventComparison::NotApplicable {
                reason: "requires change-point sensor events; other event schemas are not inferred"
                    .into(),
            }
        } else {
            let parse =
                |m: &Measurement| -> Result<Option<(ObservationSequence, Vec<ChangePoint>)>> {
                    if let Reading::Value {
                        value: MeasurementValue::Events(v),
                    } = &m.reading
                    {
                        Ok(Some(events(m, v)?))
                    } else {
                        Ok(None)
                    }
                };
            let a = parse(before)?;
            let b = parse(after)?;
            if let (Some((sequence, a)), Some((_, b))) = (a, b) {
                let (mut i, mut j) = (0, 0);
                let mut matched = Vec::new();
                let mut removed = Vec::new();
                let mut added = Vec::new();
                while i < a.len() && j < b.len() {
                    if a[i].index.abs_diff(b[j].index) <= params.max_shift_steps {
                        let shift = i64::try_from(b[j].index).map_err(invalid)?
                            - i64::try_from(a[i].index).map_err(invalid)?;
                        matched.push(EventMatch {
                            before: a[i].clone(),
                            after: b[j].clone(),
                            shift_steps: shift,
                        });
                        i += 1;
                        j += 1;
                    } else if a[i].index < b[j].index {
                        removed.push(a[i].clone());
                        i += 1;
                    } else {
                        added.push(b[j].clone());
                        j += 1;
                    }
                }
                removed.extend_from_slice(&a[i..]);
                added.extend_from_slice(&b[j..]);
                EventComparison::Value {
                    sequence,
                    max_shift_steps: params.max_shift_steps,
                    matching:
                        "chronological earliest feasible one-to-one match; not minimum displacement"
                            .into(),
                    matched,
                    removed,
                    added,
                }
            } else {
                EventComparison::Unavailable {
                    before: before.reading.clone(),
                    after: after.reading.clone(),
                }
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(serde_json::to_value(result).map_err(invalid)?),
        }))
    }
}
