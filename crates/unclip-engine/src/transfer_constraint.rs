//! Scalar stability across explicit disjoint observation sets.
use super::ConstraintStatus;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use unclip_measure::{Measurement, MeasurementValue, Reading};
use unclip_observe::ObservationId;
use unclip_plugin::{PluginError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferAssessment {
    pub source: Reading,
    pub target: Reading,
    pub absolute_difference: Option<f64>,
}

pub(super) fn assess(
    source: &Measurement,
    target: &Measurement,
    minimum: usize,
    tolerance: f64,
) -> Result<(ConstraintStatus, TransferAssessment)> {
    let invalid = |message: &str| PluginError::Message(message.into());
    if minimum == 0 || !tolerance.is_finite() || tolerance < 0.0 {
        return Err(invalid(
            "transfer requires a positive sample floor and finite nonnegative tolerance",
        ));
    }
    let mut left = source.context.values.clone();
    let mut right = target.context.values.clone();
    let parse = |value: Option<serde_json::Value>| -> Result<BTreeSet<ObservationId>> {
        let ids: Vec<ObservationId> = serde_json::from_value(value.ok_or_else(|| {
            invalid("transfer requires explicit observation identities on both measurements")
        })?)
        .map_err(|_| invalid("invalid transfer observation identities"))?;
        let set = ids.iter().cloned().collect::<BTreeSet<_>>();
        if ids.is_empty() || ids.len() != set.len() || ids.iter().any(|id| id.0.trim().is_empty()) {
            return Err(invalid(
                "transfer observation identities must be nonempty and unique",
            ));
        }
        Ok(set)
    };
    let source_ids = parse(left.remove("observations"))?;
    let target_ids = parse(right.remove("observations"))?;
    if !source_ids.is_disjoint(&target_ids) {
        return Err(invalid(
            "transfer evidence observation sets must be disjoint",
        ));
    }
    if source.sensor != target.sensor
        || source.sensor_version != target.sensor_version
        || left != right
    {
        return Err(invalid("transfer requires the same sensor, version and settings apart from observation selection"));
    }
    for (value, available) in [(source, source_ids.len()), (target, target_ids.len())] {
        if value.sample_count.is_some_and(|count| count > available) {
            return Err(invalid(
                "transfer sample count exceeds its explicit observation set",
            ));
        }
        if let Reading::Value { value } = &value.reading {
            if !matches!(value, MeasurementValue::Scalar(number) if number.is_finite()) {
                return Err(invalid(
                    "transfer stability requires finite scalar readings",
                ));
            }
        }
    }
    let difference = match (&source.reading, &target.reading) {
        (
            Reading::Value {
                value: MeasurementValue::Scalar(a),
            },
            Reading::Value {
                value: MeasurementValue::Scalar(b),
            },
        ) => {
            let value = (b - a).abs();
            if !value.is_finite() {
                return Err(invalid("transfer difference is not finite"));
            }
            Some(value)
        }
        _ => None,
    };
    let status = match (source.sample_count, target.sample_count, difference) {
        (Some(a), Some(b), Some(delta)) => {
            if a >= minimum && b >= minimum && delta <= tolerance {
                ConstraintStatus::Satisfied
            } else {
                ConstraintStatus::Violated
            }
        }
        _ => ConstraintStatus::Unavailable,
    };
    Ok((
        status,
        TransferAssessment {
            source: source.reading.clone(),
            target: target.reading.clone(),
            absolute_difference: difference,
        },
    ))
}
