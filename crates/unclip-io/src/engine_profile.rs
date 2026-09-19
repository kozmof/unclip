//! Engine-profile parsing and validation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::ensure;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use unclip_epistemic::PluginId;
use unclip_plugin::{EngineProfile, PluginSelection};

use crate::read_text_file;

fn any_version() -> VersionReq {
    VersionReq::STAR
}

/// One configured plugin and its version/parameter constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    pub id: PluginId,
    #[serde(default = "any_version")]
    pub version: VersionReq,
    #[serde(default = "default_params")]
    pub params: serde_json::Value,
}

fn default_params() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Serializable engine profile used by YAML and JSON configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EngineProfileDocument {
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub frame: Option<String>,
    #[serde(default)]
    pub sensors: Vec<PluginConfig>,
    #[serde(default)]
    pub inferrers: Vec<PluginConfig>,
    #[serde(default)]
    pub comparators: Vec<PluginConfig>,
    #[serde(default)]
    pub candidate_generators: Vec<PluginConfig>,
    #[serde(default)]
    pub null_models: Vec<PluginConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrappedProfile {
    engine_profile: EngineProfileDocument,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProfileIn {
    Wrapped(WrappedProfile),
    Bare(EngineProfileDocument),
}

/// Validated runtime selections and their per-plugin parameters.
#[derive(Debug, Clone)]
pub struct ParsedEngineProfile {
    pub profile: EngineProfile,
    pub params: BTreeMap<PluginId, serde_json::Value>,
}

fn validate(document: &EngineProfileDocument) -> anyhow::Result<()> {
    let mut ids = BTreeSet::new();
    for plugin in document
        .sensors
        .iter()
        .chain(&document.inferrers)
        .chain(&document.comparators)
        .chain(&document.candidate_generators)
        .chain(&document.null_models)
    {
        ensure!(!plugin.id.0.is_empty(), "plugin id must not be empty");
        ensure!(
            ids.insert(plugin.id.clone()),
            "duplicate plugin id in engine profile: {}",
            plugin.id.0
        );
        ensure!(
            plugin.params.is_object(),
            "parameters for {} must be a JSON object",
            plugin.id.0
        );
    }
    Ok(())
}

fn selections(configs: &[PluginConfig]) -> Vec<PluginSelection> {
    configs
        .iter()
        .map(|plugin| PluginSelection {
            id: plugin.id.clone(),
            version: plugin.version.clone(),
        })
        .collect()
}

impl EngineProfileDocument {
    /// Validate and convert this document to runtime selections and parameters.
    pub fn resolve(&self) -> anyhow::Result<ParsedEngineProfile> {
        validate(self)?;
        let params = self
            .sensors
            .iter()
            .chain(&self.inferrers)
            .chain(&self.comparators)
            .chain(&self.candidate_generators)
            .chain(&self.null_models)
            .map(|plugin| (plugin.id.clone(), plugin.params.clone()))
            .collect();
        Ok(ParsedEngineProfile {
            profile: EngineProfile {
                sensors: selections(&self.sensors),
                inferrers: selections(&self.inferrers),
                comparators: selections(&self.comparators),
                candidate_generators: selections(&self.candidate_generators),
                null_models: selections(&self.null_models),
            },
            params,
        })
    }
}

/// Parse and validate a bare or `engine_profile:`-wrapped YAML/JSON document.
pub fn parse_engine_profile(text: &str) -> anyhow::Result<EngineProfileDocument> {
    let parsed: ProfileIn = serde_norway::from_str(text)?;
    let document = match parsed {
        ProfileIn::Wrapped(value) => value.engine_profile,
        ProfileIn::Bare(value) => value,
    };
    validate(&document)?;
    Ok(document)
}

/// Load and validate an engine profile from disk.
pub fn load_engine_profile(path: &Path) -> anyhow::Result<EngineProfileDocument> {
    let text = read_text_file(path, "engine profile file")?;
    parse_engine_profile(&text)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_versions_parameters_and_defaults() {
        let document = parse_engine_profile(
            r#"
sensors:
  - id: sensor.coverage
  - id: sensor.rbo
    version: ^1.2
    params:
      p: 0.9
inferrers:
  - id: infer.manual
comparators: []
"#,
        )
        .unwrap();
        let parsed = document.resolve().unwrap();

        assert_eq!(parsed.profile.sensors.len(), 2);
        assert_eq!(parsed.profile.sensors[0].version, VersionReq::STAR);
        assert!(parsed.profile.sensors[1]
            .version
            .matches(&semver::Version::new(1, 3, 0)));
        assert_eq!(
            parsed.params.get(&PluginId::new("sensor.rbo")),
            Some(&json!({"p": 0.9}))
        );
        assert_eq!(
            parsed.params.get(&PluginId::new("sensor.coverage")),
            Some(&json!({}))
        );
    }

    #[test]
    fn accepts_wrapped_json() {
        let document =
            parse_engine_profile(r#"{"engine_profile":{"sensors":[{"id":"sensor.coverage"}]}}"#)
                .unwrap();
        assert_eq!(document.sensors[0].id.0, "sensor.coverage");
    }

    #[test]
    fn rejects_duplicates_non_object_params_and_unknown_fields() {
        let duplicate = r#"
sensors:
  - id: sensor.coverage
comparators:
  - id: sensor.coverage
"#;
        assert!(parse_engine_profile(duplicate).is_err());

        let params = r#"
sensors:
  - id: sensor.coverage
    params: [1, 2]
"#;
        assert!(parse_engine_profile(params).is_err());

        let unknown = r#"
sensors:
  - id: sensor.coverage
    typo: true
"#;
        assert!(parse_engine_profile(unknown).is_err());
    }
}

#[cfg(test)]
mod discovery_profile_tests {
    use super::*;
    #[test]
    fn discovery_selections_round_trip_and_old_profiles_default_to_empty() {
        let document = parse_engine_profile(
            r#"
candidate_generators:
  - id: generate.fixture
    version: ^1.2
    params: {minimum_samples: 4}
null_models:
  - id: null.fixture
    version: '=1.0.0'
    params: {seed: 7}
"#,
        )
        .unwrap();
        let parsed = document.resolve().unwrap();
        assert_eq!(
            parsed.profile.candidate_generators[0].id.0,
            "generate.fixture"
        );
        assert_eq!(parsed.profile.null_models[0].id.0, "null.fixture");
        assert!(parsed.profile.candidate_generators[0]
            .version
            .matches(&semver::Version::new(1, 3, 0)));
        assert_eq!(parsed.params[&PluginId::new("null.fixture")]["seed"], 7);
        let encoded = serde_json::to_string(&document).unwrap();
        assert_eq!(parse_engine_profile(&encoded).unwrap(), document);
        let old = parse_engine_profile("sensors: []")
            .unwrap()
            .resolve()
            .unwrap();
        assert!(old.profile.candidate_generators.is_empty() && old.profile.null_models.is_empty());
    }
    #[test]
    fn discovery_configuration_rejects_duplicate_ids_and_invalid_parameters() {
        for text in [
            "candidate_generators: [{id: same}]\nnull_models: [{id: same}]",
            "candidate_generators: [{id: generator, params: []}]",
            "null_models: [{id: null.fixture, params: null}]",
            "candidate_generators: [{id: '', params: {}}]",
            "null_models: [{id: null.fixture, version: not-semver}]",
            "candidate_generators: [{id: generator, typo: true}]",
        ] {
            assert!(parse_engine_profile(text).is_err(), "accepted {text}");
        }
    }
}
