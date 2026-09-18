//! Measurement-profile interchange and output formats.

use serde::{Deserialize, Serialize};
use unclip_measure::MeasurementProfile;

use crate::Format;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrappedProfileIn {
    measurement_profile: MeasurementProfile,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProfileIn {
    Wrapped(WrappedProfileIn),
    Bare(MeasurementProfile),
}

#[derive(Debug, Serialize)]
struct WrappedProfileOut<'a> {
    measurement_profile: &'a MeasurementProfile,
}

/// Parse one bare or `measurement_profile:`-wrapped YAML/JSON document.
pub fn parse_measurement_profile(text: &str) -> anyhow::Result<MeasurementProfile> {
    let parsed: ProfileIn = serde_norway::from_str(text)?;
    Ok(match parsed {
        ProfileIn::Wrapped(value) => value.measurement_profile,
        ProfileIn::Bare(value) => value,
    })
}

/// Parse JSONL containing one measurement per non-empty line.
pub fn parse_measurement_profile_jsonl(text: &str) -> anyhow::Result<MeasurementProfile> {
    let measurements = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MeasurementProfile { measurements })
}

/// Render a profile in YAML, JSON, or one-measurement-per-line JSONL.
pub fn render_measurement_profile(
    profile: &MeasurementProfile,
    format: Format,
) -> anyhow::Result<String> {
    let wrapped = WrappedProfileOut {
        measurement_profile: profile,
    };
    Ok(match format {
        Format::Yaml => serde_norway::to_string(&wrapped)?,
        Format::Json => format!("{}\n", serde_json::to_string_pretty(&wrapped)?),
        Format::Jsonl => {
            let mut output = String::new();
            for measurement in &profile.measurements {
                output.push_str(&serde_json::to_string(measurement)?);
                output.push('\n');
            }
            output
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use unclip_epistemic::PluginId;
    use unclip_measure::{Measurement, MeasurementContext, MeasurementValue, Reading};

    use super::*;

    fn sample() -> MeasurementProfile {
        MeasurementProfile {
            measurements: vec![
                Measurement {
                    sensor: PluginId::new("sensor.scalar"),
                    sensor_version: semver::Version::new(1, 2, 3),
                    reading: Reading::Value {
                        value: MeasurementValue::Scalar(0.0),
                    },
                    confidence: Some(0.9),
                    sample_count: Some(4),
                    context: MeasurementContext {
                        values: [("axis".into(), json!("source"))].into_iter().collect(),
                    },
                },
                Measurement {
                    sensor: PluginId::new("sensor.ranking"),
                    sensor_version: semver::Version::new(2, 0, 0),
                    reading: Reading::InsufficientEvidence { have: 2, need: 5 },
                    confidence: None,
                    sample_count: Some(2),
                    context: MeasurementContext {
                        values: BTreeMap::new(),
                    },
                },
                Measurement {
                    sensor: PluginId::new("sensor.optional"),
                    sensor_version: semver::Version::new(1, 0, 0),
                    reading: Reading::NotApplicable {
                        reason: "no ranking".into(),
                    },
                    confidence: None,
                    sample_count: None,
                    context: MeasurementContext::default(),
                },
            ],
        }
    }

    #[test]
    fn yaml_json_and_jsonl_preserve_values_and_sparse_states() {
        let expected = sample();
        for format in [Format::Yaml, Format::Json] {
            let rendered = render_measurement_profile(&expected, format).unwrap();
            assert_eq!(parse_measurement_profile(&rendered).unwrap(), expected);
        }

        let jsonl = render_measurement_profile(&expected, Format::Jsonl).unwrap();
        assert_eq!(jsonl.lines().count(), expected.measurements.len());
        assert_eq!(parse_measurement_profile_jsonl(&jsonl).unwrap(), expected);
    }

    #[test]
    fn rejects_unknown_measurement_fields() {
        let text = r#"
measurement_profile:
  measurements:
    - sensor: sensor.scalar
      sensor_version: 1.0.0
      reading:
        status: not_measured
      confidence: null
      sample_count: null
      context:
        values: {}
      typo: true
"#;
        assert!(parse_measurement_profile(text).is_err());
    }

    #[test]
    fn tagged_values_expose_their_kind() {
        let output = render_measurement_profile(&sample(), Format::Json).unwrap();
        assert!(output.contains(r#""kind": "scalar""#));
        assert!(output.contains(r#""status": "insufficient_evidence""#));
    }
}
