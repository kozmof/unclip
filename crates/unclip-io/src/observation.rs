//! Manual observation documents with optional partial rankings.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};
use unclip_observe::{Observation, ObservedUnitId, PartialRanking};

use crate::{read_text_file, Format};

/// A manually authored observation and its optional partial ordering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualObservationDocument {
    pub observation: Observation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking: Option<PartialRanking>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrappedObservationIn {
    manual_observation: ManualObservationDocument,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ObservationIn {
    Wrapped(WrappedObservationIn),
    Bare(ManualObservationDocument),
}

#[derive(Debug, Serialize)]
struct WrappedObservationOut<'a> {
    manual_observation: &'a ManualObservationDocument,
}

fn validate(document: &ManualObservationDocument) -> anyhow::Result<()> {
    let mut unit_ids = BTreeSet::new();
    for unit in &document.observation.units {
        ensure!(
            unit_ids.insert(unit.id.clone()),
            "duplicate observed unit id: {}",
            unit.id.0
        );
        if let Some(value) = unit.salience {
            ensure!(value.is_finite(), "unit salience must be finite");
        }
        if let Some(value) = unit.uncertainty {
            ensure!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "unit uncertainty must be between zero and one"
            );
        }
    }

    let mut relation_ids = BTreeSet::new();
    for relation in &document.observation.relations {
        ensure!(
            relation_ids.insert(relation.id.clone()),
            "duplicate observed relation id: {}",
            relation.id.0
        );
        ensure!(
            unit_ids.contains(&relation.source),
            "relation {} has an unknown source unit",
            relation.id.0
        );
        ensure!(
            unit_ids.contains(&relation.target),
            "relation {} has an unknown target unit",
            relation.id.0
        );
        ensure!(!relation.kind.is_empty(), "relation kind must not be empty");
        if let Some(value) = relation.uncertainty {
            ensure!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "relation uncertainty must be between zero and one"
            );
        }
    }

    if let Some(ranking) = &document.ranking {
        ensure!(
            ranking.observation == document.observation.id,
            "ranking observation id does not match the observation"
        );
        let mut ranked = BTreeSet::<ObservedUnitId>::new();
        for unit in ranking
            .tiers
            .iter()
            .flat_map(|tier| &tier.units)
            .chain(&ranking.unknown)
        {
            ensure!(
                unit_ids.contains(unit),
                "ranking refers to unknown observed unit: {}",
                unit.0
            );
            ensure!(
                ranked.insert(unit.clone()),
                "observed unit appears more than once in ranking: {}",
                unit.0
            );
        }
        ensure!(
            ranking.tiers.iter().all(|tier| !tier.units.is_empty()),
            "ranking tiers must not be empty"
        );
    }
    Ok(())
}

/// Parse and validate one bare or `manual_observation:`-wrapped document.
pub fn parse_manual_observation(text: &str) -> anyhow::Result<ManualObservationDocument> {
    let parsed: ObservationIn = serde_norway::from_str(text)?;
    let document = match parsed {
        ObservationIn::Wrapped(value) => value.manual_observation,
        ObservationIn::Bare(value) => value,
    };
    validate(&document)?;
    Ok(document)
}

/// Load and validate a manual observation document from disk.
pub fn load_manual_observation(path: &Path) -> anyhow::Result<ManualObservationDocument> {
    let text = read_text_file(path, "manual observation file")?;
    parse_manual_observation(&text)
}

/// Validate and render a manual observation document.
pub fn render_manual_observation(
    document: &ManualObservationDocument,
    format: Format,
) -> anyhow::Result<String> {
    validate(document)?;
    let wrapped = WrappedObservationOut {
        manual_observation: document,
    };
    match format {
        Format::Yaml => Ok(serde_norway::to_string(&wrapped)?),
        Format::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&wrapped)?)),
        Format::Jsonl => bail!("JSONL is not supported for manual observations"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use unclip_epistemic::SourceRef;
    use unclip_observe::{
        ObservationId, ObservedRelation, ObservedRelationId, ObservedUnit, RankTier,
    };

    use super::*;

    fn sample() -> ManualObservationDocument {
        let units = ["a", "b", "c"]
            .into_iter()
            .map(|id| ObservedUnit {
                id: ObservedUnitId::new(id),
                label: id.to_uppercase(),
                salience: Some(0.5),
                uncertainty: Some(0.1),
                context: BTreeMap::new(),
            })
            .collect();
        ManualObservationDocument {
            observation: Observation {
                id: ObservationId::new("manual"),
                source: SourceRef::new("notes.txt"),
                observed_at: None,
                units,
                relations: vec![ObservedRelation {
                    id: ObservedRelationId::new("r"),
                    source: ObservedUnitId::new("a"),
                    target: ObservedUnitId::new("b"),
                    kind: "before".into(),
                    uncertainty: None,
                }],
                context: BTreeMap::new(),
            },
            ranking: Some(PartialRanking {
                observation: ObservationId::new("manual"),
                tiers: vec![RankTier {
                    units: vec![ObservedUnitId::new("a"), ObservedUnitId::new("b")],
                }],
                unknown: vec![ObservedUnitId::new("c")],
            }),
        }
    }

    #[test]
    fn yaml_and_json_round_trip_ties_and_unknown_tail() {
        let expected = sample();
        for format in [Format::Yaml, Format::Json] {
            let rendered = render_manual_observation(&expected, format).unwrap();
            assert_eq!(parse_manual_observation(&rendered).unwrap(), expected);
        }
    }

    #[test]
    fn ranking_is_optional() {
        let mut expected = sample();
        expected.ranking = None;
        let rendered = render_manual_observation(&expected, Format::Yaml).unwrap();
        assert_eq!(parse_manual_observation(&rendered).unwrap(), expected);
    }

    #[test]
    fn rejects_inconsistent_rankings_and_relations() {
        let mut duplicate = sample();
        duplicate
            .ranking
            .as_mut()
            .unwrap()
            .unknown
            .push(ObservedUnitId::new("a"));
        assert!(render_manual_observation(&duplicate, Format::Yaml).is_err());

        let mut unknown = sample();
        unknown.observation.relations[0].target = ObservedUnitId::new("missing");
        assert!(render_manual_observation(&unknown, Format::Yaml).is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let text = r#"
manual_observation:
  observation:
    id: manual
    source: notes.txt
    units:
      - id: a
        label: A
        typo: true
    relations: []
"#;
        assert!(parse_manual_observation(text).is_err());
    }
}
