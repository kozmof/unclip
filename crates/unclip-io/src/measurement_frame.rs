//! Versioned measurement-frame import and export in YAML and JSON.

use std::path::Path;

use anyhow::bail;
use serde::{Deserialize, Serialize};
use unclip_domain::{DomainId, MeasurementFrame};
use unclip_epistemic::DomainVersion;

use crate::{read_text_file, Format};

/// A measurement frame together with the immutable domain version it targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementFrameDocument {
    pub domain_id: DomainId,
    pub domain_version: DomainVersion,
    pub frame: MeasurementFrame,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrappedFrameIn {
    measurement_frame: MeasurementFrameDocument,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FrameIn {
    Wrapped(WrappedFrameIn),
    Bare(MeasurementFrameDocument),
}

#[derive(Debug, Serialize)]
struct WrappedFrameOut<'a> {
    measurement_frame: &'a MeasurementFrameDocument,
}

/// Parse one bare or `measurement_frame:`-wrapped document.
pub fn parse_measurement_frame(text: &str) -> anyhow::Result<MeasurementFrameDocument> {
    let parsed: FrameIn = serde_norway::from_str(text)?;
    Ok(match parsed {
        FrameIn::Wrapped(value) => value.measurement_frame,
        FrameIn::Bare(value) => value,
    })
}

/// Load and parse a measurement-frame document from disk.
pub fn load_measurement_frame(path: &Path) -> anyhow::Result<MeasurementFrameDocument> {
    let text = read_text_file(path, "measurement frame file")?;
    parse_measurement_frame(&text)
}

/// Render a frame document with a stable `measurement_frame:` wrapper.
pub fn render_measurement_frame(
    document: &MeasurementFrameDocument,
    format: Format,
) -> anyhow::Result<String> {
    let wrapped = WrappedFrameOut {
        measurement_frame: document,
    };
    match format {
        Format::Yaml => Ok(serde_norway::to_string(&wrapped)?),
        Format::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&wrapped)?)),
        Format::Jsonl => bail!("JSONL is not supported for measurement frames"),
    }
}

#[cfg(test)]
mod tests {
    use unclip_domain::{FrameAxis, FrameId, UnitId};
    use unclip_epistemic::FrameVersion;

    use super::*;

    fn sample() -> MeasurementFrameDocument {
        MeasurementFrameDocument {
            domain_id: DomainId::new("example"),
            domain_version: DomainVersion::new("3"),
            frame: MeasurementFrame {
                id: FrameId::new("example.general"),
                version: FrameVersion::new("2"),
                axes: vec![
                    FrameAxis {
                        unit: UnitId::new("source"),
                        label: Some("Source".into()),
                    },
                    FrameAxis {
                        unit: UnitId::new("target"),
                        label: None,
                    },
                ],
            },
        }
    }

    #[test]
    fn yaml_and_json_round_trip_stably() {
        let expected = sample();
        for format in [Format::Yaml, Format::Json] {
            let rendered = render_measurement_frame(&expected, format).unwrap();
            assert_eq!(parse_measurement_frame(&rendered).unwrap(), expected);
            assert_eq!(
                render_measurement_frame(&parse_measurement_frame(&rendered).unwrap(), format)
                    .unwrap(),
                rendered
            );
        }
    }

    #[test]
    fn accepts_a_bare_document() {
        let text = serde_norway::to_string(&sample()).unwrap();
        assert_eq!(parse_measurement_frame(&text).unwrap(), sample());
    }

    #[test]
    fn rejects_unknown_fields_at_every_level() {
        let wrapper = r#"
measurement_frame:
  domain_id: example
  domain_version: "3"
  frame:
    id: example.general
    version: "2"
    axes: []
extra: true
"#;
        assert!(parse_measurement_frame(wrapper).is_err());

        let document = r#"
measurement_frame:
  domain_id: example
  domain_version: "3"
  typo: true
  frame:
    id: example.general
    version: "2"
    axes: []
"#;
        assert!(parse_measurement_frame(document).is_err());

        let axis = r#"
measurement_frame:
  domain_id: example
  domain_version: "3"
  frame:
    id: example.general
    version: "2"
    axes:
      - unit: source
        typo: true
"#;
        assert!(parse_measurement_frame(axis).is_err());
    }

    #[test]
    fn rejects_jsonl_output() {
        assert!(render_measurement_frame(&sample(), Format::Jsonl).is_err());
    }
}
