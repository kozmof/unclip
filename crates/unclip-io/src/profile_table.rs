//! Side-by-side display of independent sensor readings.

use std::collections::{BTreeMap, BTreeSet};

use unclip_measure::{Measurement, MeasurementProfile, Reading};

/// Render a Markdown table with one column per sensor/version and one row per
/// complete measurement context. Distinct contexts are never silently aligned.
/// Multiple readings in a cell are retained, including duplicates. A missing
/// entry is displayed as `no result`, not as a sensor's `NotMeasured` reading.
/// Columns, contexts, and readings use stable order without scalar aggregation.
pub fn render_measurement_profile_table(profile: &MeasurementProfile) -> anyhow::Result<String> {
    let mut columns = BTreeSet::new();
    let mut rows = BTreeMap::<String, BTreeMap<String, Vec<String>>>::new();
    for measurement in &profile.measurements {
        let sensor = format!("{}@{}", measurement.sensor.0, measurement.sensor_version);
        columns.insert(sensor.clone());
        let context = serde_json::to_string(&measurement.context.values)?;
        rows.entry(context)
            .or_default()
            .entry(sensor)
            .or_default()
            .push(render_reading(measurement)?);
    }
    let mut output = String::from("CALCULATED SENSOR RESULTS\n\n");
    if columns.is_empty() {
        output.push_str("No sensor results.\n");
        return Ok(output);
    }
    output.push_str("Each column retains its own metric and scale. No result means no entry for that context.\n\n| Context |");
    for column in &columns {
        output.push_str(&format!(" {} |", escape_cell(column)));
    }
    output.push_str("\n| --- |");
    for _ in &columns {
        output.push_str(" --- |");
    }
    output.push('\n');
    for (context, mut readings) in rows {
        output.push_str(&format!("| {} |", escape_cell(&context)));
        for column in &columns {
            let cell = match readings.get_mut(column) {
                Some(values) => {
                    values.sort();
                    values
                        .iter()
                        .map(|value| escape_cell(value))
                        .collect::<Vec<_>>()
                        .join("<br>")
                }
                None => "no result".into(),
            };
            output.push_str(&format!(" {cell} |"));
        }
        output.push('\n');
    }
    Ok(output)
}

fn render_reading(measurement: &Measurement) -> anyhow::Result<String> {
    let reading = match &measurement.reading {
        Reading::Value { value } => serde_json::to_string(value)?,
        Reading::NotApplicable { reason } => {
            format!("not applicable: {}", serde_json::to_string(reason)?)
        }
        Reading::NotMeasured => "not measured".into(),
        Reading::InsufficientEvidence { have, need } => {
            format!("insufficient evidence: {have}/{need}")
        }
    };
    let samples = measurement
        .sample_count
        .map_or_else(|| "unknown".into(), |value| value.to_string());
    let confidence = measurement
        .confidence
        .map_or_else(|| "unknown".into(), |value| value.to_string());
    Ok(format!(
        "{reading}; samples={samples}; confidence={confidence}"
    ))
}

fn escape_cell(value: &str) -> String {
    // Escape user-supplied Markdown/HTML and control characters before inserting
    // the renderer's own line breaks. This also keeps terminal output one line.
    let mut result = String::new();
    for character in value.chars() {
        match character {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '|' => result.push_str("&#124;"),
            '\\' => result.push_str("&#92;"),
            '`' => result.push_str("&#96;"),
            '*' => result.push_str("&#42;"),
            '_' => result.push_str("&#95;"),
            '[' => result.push_str("&#91;"),
            ']' => result.push_str("&#93;"),
            value if value.is_control() => result.extend(value.escape_default()),
            value => result.push(value),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use unclip_epistemic::PluginId;
    use unclip_measure::{MeasurementContext, MeasurementValue};

    fn reading(sensor: &str, value: Reading) -> Measurement {
        Measurement {
            sensor: PluginId::new(sensor),
            sensor_version: semver::Version::new(1, 0, 0),
            reading: value,
            sample_count: Some(4),
            confidence: Some(0.9),
            context: MeasurementContext {
                values: BTreeMap::from([("pair".into(), serde_json::json!(["a", "b"]))]),
            },
        }
    }

    #[test]
    fn disagreements_and_all_sparse_states_are_preserved_side_by_side() {
        let mut profile = MeasurementProfile {
            measurements: vec![
                reading(
                    "sensor.spearman",
                    Reading::Value {
                        value: MeasurementValue::Scalar(-1.0),
                    },
                ),
                reading(
                    "sensor.mi",
                    Reading::Value {
                        value: MeasurementValue::Scalar(1.0),
                    },
                ),
                reading(
                    "sensor.zero",
                    Reading::Value {
                        value: MeasurementValue::Scalar(0.0),
                    },
                ),
                reading("sensor.skipped", Reading::NotMeasured),
                reading(
                    "sensor.sparse",
                    Reading::InsufficientEvidence { have: 1, need: 4 },
                ),
                reading(
                    "sensor.unsupported",
                    Reading::NotApplicable {
                        reason: "no ranking".into(),
                    },
                ),
            ],
        };
        let rendered = render_measurement_profile_table(&profile).unwrap();
        assert!(rendered.contains(
            "sensor.mi@1.0.0 | sensor.skipped@1.0.0 | sensor.sparse@1.0.0 | sensor.spearman@1.0.0"
        ));
        let data = rendered
            .lines()
            .find(|line| line.starts_with("| {"))
            .unwrap();
        for expected in [
            "\"value\":-1.0",
            "\"value\":1.0",
            "\"value\":0.0",
            "not measured",
            "insufficient evidence: 1/4",
            "not applicable",
            "samples=4",
            "confidence=0.9",
        ] {
            assert!(data.contains(expected), "missing {expected}");
        }
        profile.measurements.reverse();
        assert_eq!(
            render_measurement_profile_table(&profile).unwrap(),
            rendered
        );
    }

    #[test]
    fn duplicate_readings_versions_and_contexts_are_not_collapsed() {
        let first = reading("sensor.a", Reading::NotMeasured);
        let mut different_context = first.clone();
        different_context
            .context
            .values
            .insert("pair".into(), serde_json::json!(["b", "c"]));
        different_context.sample_count = None;
        different_context.confidence = None;
        let mut different_version = first.clone();
        different_version.sensor_version = semver::Version::new(2, 0, 0);
        let rendered = render_measurement_profile_table(&MeasurementProfile {
            measurements: vec![first.clone(), first, different_context, different_version],
        })
        .unwrap();
        assert!(rendered.contains("sensor.a@1.0.0 | sensor.a@2.0.0"));
        assert!(rendered.contains("<br>"));
        assert!(rendered.contains("samples=unknown; confidence=unknown | no result |"));
        assert_eq!(rendered.matches("not measured").count(), 4);
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.starts_with("| {"))
                .count(),
            2
        );
    }

    #[test]
    fn escapes_labels_and_preserves_structured_readings() {
        let measurement = reading(
            "sensor.|\n\u{1b}[31m",
            Reading::Value {
                value: MeasurementValue::Structured(
                    serde_json::json!({"note":"<br>|*label*", "values":[0, 1]}),
                ),
            },
        );
        let rendered = render_measurement_profile_table(&MeasurementProfile {
            measurements: vec![measurement],
        })
        .unwrap();
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains("<br>"));
        assert!(rendered.contains("&lt;br&gt;&#124;&#42;label&#42;"));
        assert!(rendered.contains("&#91;0,1&#93;"));
        assert!(
            render_measurement_profile_table(&MeasurementProfile::default())
                .unwrap()
                .contains("No sensor results.")
        );
    }
}
