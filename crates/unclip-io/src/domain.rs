//! Versioned semantic-domain import and export in YAML and JSON.

use std::path::Path;

use anyhow::bail;
use serde::{Deserialize, Serialize};
use unclip_domain::DomainSnapshot;

use crate::{read_text_file, Format};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrappedDomainIn {
    domain: DomainSnapshot,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DomainIn {
    Wrapped(WrappedDomainIn),
    Bare(DomainSnapshot),
}

#[derive(Debug, Serialize)]
struct WrappedDomainOut<'a> {
    domain: &'a DomainSnapshot,
}

/// Parse one bare or `domain:`-wrapped snapshot from YAML or JSON.
pub fn parse_domain(text: &str) -> anyhow::Result<DomainSnapshot> {
    let parsed: DomainIn = serde_norway::from_str(text)?;
    Ok(match parsed {
        DomainIn::Wrapped(value) => value.domain,
        DomainIn::Bare(value) => value,
    })
}

/// Load and parse one domain snapshot from disk.
pub fn load_domain(path: &Path) -> anyhow::Result<DomainSnapshot> {
    let text = read_text_file(path, "domain file")?;
    parse_domain(&text)
}

/// Render one snapshot using a stable `domain:` wrapper.
pub fn render_domain(domain: &DomainSnapshot, format: Format) -> anyhow::Result<String> {
    let wrapped = WrappedDomainOut { domain };
    match format {
        Format::Yaml => Ok(serde_norway::to_string(&wrapped)?),
        Format::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&wrapped)?)),
        Format::Jsonl => bail!("JSONL is not supported for domain snapshots"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use unclip_domain::{DomainId, PropertyValue, Relation, RelationId, Unit, UnitId, UnitKind};
    use unclip_epistemic::DomainVersion;

    use super::*;

    fn sample() -> DomainSnapshot {
        let source = UnitId::new("source");
        let target = UnitId::new("target");
        let units = [
            (
                source.clone(),
                Unit {
                    id: source.clone(),
                    kind: UnitKind::AtomicMeaning,
                    label: Some("Source".into()),
                    properties: [
                        ("enabled".into(), PropertyValue::Boolean(true)),
                        ("count".into(), PropertyValue::Integer(2)),
                        ("weight".into(), PropertyValue::Number(0.5)),
                        ("note".into(), PropertyValue::Text("stable".into())),
                        (
                            "metadata".into(),
                            PropertyValue::Structured(json!({"tags": ["a", "b"]})),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
            ),
            (
                target.clone(),
                Unit {
                    id: target.clone(),
                    kind: UnitKind::SemanticRole,
                    label: None,
                    properties: BTreeMap::new(),
                },
            ),
        ]
        .into_iter()
        .collect();
        let relation_id = RelationId::new("connects");
        let relations = [(
            relation_id.clone(),
            Relation {
                id: relation_id,
                source,
                target,
                kind: "association".into(),
                properties: BTreeMap::new(),
            },
        )]
        .into_iter()
        .collect();
        DomainSnapshot {
            id: DomainId::new("example"),
            version: DomainVersion::new("1"),
            units,
            relations,
        }
    }

    #[test]
    fn yaml_and_json_round_trip_stably() {
        let expected = sample();
        for format in [Format::Yaml, Format::Json] {
            let rendered = render_domain(&expected, format).unwrap();
            assert_eq!(parse_domain(&rendered).unwrap(), expected);
            assert_eq!(
                render_domain(&parse_domain(&rendered).unwrap(), format).unwrap(),
                rendered
            );
        }
    }

    #[test]
    fn accepts_a_bare_domain() {
        let text = serde_norway::to_string(&sample()).unwrap();
        assert_eq!(parse_domain(&text).unwrap(), sample());
    }

    #[test]
    fn rejects_unknown_wrapper_and_nested_fields() {
        let wrapper =
            "domain:\n  id: example\n  version: '1'\n  units: {}\n  relations: {}\nextra: true\n";
        assert!(parse_domain(wrapper).is_err());

        let nested = "domain:\n  id: example\n  version: '1'\n  units:\n    source:\n      id: source\n      kind: atomic_meaning\n      typo: true\n  relations: {}\n";
        assert!(parse_domain(nested).is_err());
    }

    #[test]
    fn rejects_jsonl_output() {
        assert!(render_domain(&sample(), Format::Jsonl).is_err());
    }
}
