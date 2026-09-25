//! Explicit independence expectations bound to a versioned composition profile.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp, Tracked,
};
use unclip_measure::{ExpectedIndependentBehavior, MeasurementKind, Reading};
use unclip_plugin::{PluginError, Result};

use crate::CompositionMeasurementProfile;

/// One explicit rule for the expected behavior of a product measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependenceDefinition {
    pub product_measurement: DerivedId,
    pub left_measurements: Vec<DerivedId>,
    pub right_measurements: Vec<DerivedId>,
    pub rule: String,
    pub parameters: serde_json::Value,
    pub expected: ExpectedIndependentBehavior,
}

/// A validated rule attached to the exact sensor and input-profile evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependenceExpectation {
    pub product_measurement: DerivedId,
    pub sensor: PluginId,
    pub sensor_version: semver::Version,
    pub measurement_kind: MeasurementKind,
    pub left_measurements: Vec<DerivedId>,
    pub right_measurements: Vec<DerivedId>,
    pub rule: String,
    pub parameters: serde_json::Value,
    pub expected: ExpectedIndependentBehavior,
}

/// Expected independent behavior for every measurement in one product profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependenceExpectationProfile {
    pub composition_profile: DerivedId,
    pub expectations: Vec<IndependenceExpectation>,
}

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

fn canonical_sources(
    sources: &[DerivedId],
    available: &BTreeSet<DerivedId>,
    side: &str,
) -> Result<Vec<DerivedId>> {
    if sources.is_empty() {
        return Err(invalid(format!(
            "independence definitions require selected {side} measurements"
        )));
    }
    let mut result = sources.to_vec();
    result.sort();
    if result
        .iter()
        .any(|identity| identity.0.trim().is_empty() || !available.contains(identity))
        || result.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(invalid(format!(
            "independence definitions require unique measurements from the selected {side} profile"
        )));
    }
    Ok(result)
}

impl crate::Engine {
    /// Validate explicit, kind-preserving independence rules for a composition.
    ///
    /// Every product measurement receives exactly one rule based on selected
    /// evidence from both input profiles. This stage does not compare the rule
    /// with observed product behavior or choose a conventional zero baseline.
    pub fn define_independent_behavior(
        &self,
        composition: &Calculated<CompositionMeasurementProfile>,
        definitions: &[IndependenceDefinition],
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<IndependenceExpectationProfile>> {
        if run_id.trim().is_empty() {
            return Err(invalid(
                "independence expectation requires a nonempty run ID",
            ));
        }
        let provenance = composition.provenance();
        if composition.id().0.trim().is_empty()
            || provenance.producer != PluginId::new("calculate.composition-profile")
            || provenance.algorithm != "versioned_composition_measurement_profiles"
            || provenance.params_hash != hash_params(&provenance.params)
        {
            return Err(invalid(
                "independence expectations require a valid calculated composition profile",
            ));
        }

        let output_id = DerivedId::new(format!("{run_id}/independence-expectations"));
        if composition.id() == &output_id {
            return Err(invalid(
                "independence expectation output identity collides with its composition input",
            ));
        }
        let dependencies = DependencyCollector::default();
        let tracked = Tracked::from(composition);
        let composition_value = dependencies.read(&tracked);
        let left = composition_value
            .left
            .measurements
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<BTreeSet<_>>();
        let right = composition_value
            .right
            .measurements
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<BTreeSet<_>>();
        let product = composition_value
            .product
            .measurements
            .iter()
            .map(|entry| (entry.id.clone(), &entry.measurement))
            .collect::<BTreeMap<_, _>>();
        if left.len() != composition_value.left.measurements.len()
            || right.len() != composition_value.right.measurements.len()
            || product.len() != composition_value.product.measurements.len()
        {
            return Err(invalid(
                "composition profiles require unique measurement identities",
            ));
        }
        if definitions.len() != product.len() {
            return Err(invalid(
                "independence definitions must cover every product measurement exactly once",
            ));
        }

        let mut definitions = definitions.to_vec();
        definitions.sort_by(|left, right| left.product_measurement.cmp(&right.product_measurement));
        let mut selected = BTreeSet::new();
        let mut expectations = Vec::with_capacity(definitions.len());
        for definition in definitions {
            if definition.product_measurement.0.trim().is_empty()
                || !selected.insert(definition.product_measurement.clone())
            {
                return Err(invalid(
                    "independence definitions require unique nonempty product measurement identities",
                ));
            }
            let measurement = product
                .get(&definition.product_measurement)
                .ok_or_else(|| {
                    invalid("independence definition references an unselected product measurement")
                })?;
            if definition.rule.trim().is_empty() || !definition.parameters.is_object() {
                return Err(invalid(
                    "independence definitions require a named rule and object parameters",
                ));
            }
            definition
                .expected
                .validate()
                .map_err(|error| invalid(error.to_string()))?;
            let measurement_kind = definition.expected.kind();
            let sensor = self
                .registry()
                .product_sensor(&measurement.sensor)
                .ok_or_else(|| PluginError::MissingPlugin(measurement.sensor.clone()))?;
            let descriptor = sensor.descriptor();
            if descriptor.version != measurement.sensor_version
                || !descriptor.produces.contains(&measurement_kind)
                || matches!(
                    &measurement.reading,
                    Reading::Value { value } if value.kind() != measurement_kind
                )
            {
                return Err(invalid(
                    "independence behavior kind must match the exact product sensor and measured value kind",
                ));
            }
            expectations.push(IndependenceExpectation {
                product_measurement: definition.product_measurement,
                sensor: measurement.sensor.clone(),
                sensor_version: measurement.sensor_version.clone(),
                measurement_kind,
                left_measurements: canonical_sources(&definition.left_measurements, &left, "left")?,
                right_measurements: canonical_sources(
                    &definition.right_measurements,
                    &right,
                    "right",
                )?,
                rule: definition.rule,
                parameters: definition.parameters,
                expected: definition.expected,
            });
        }
        if selected != product.keys().cloned().collect() {
            return Err(invalid(
                "independence definitions must cover every product measurement exactly once",
            ));
        }

        let value = IndependenceExpectationProfile {
            composition_profile: composition.id().clone(),
            expectations,
        };
        let params = serde_json::json!({
            "composition_profile": composition.id(),
            "binding": &composition_value.product.binding,
            "definitions": &value.expectations,
        });
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output_id,
                producer: PluginId::new("calculate.independence-expectations"),
                algorithm: "explicit_typed_independence_rules".into(),
                version: semver::Version::new(0, 1, 0),
                params_hash: hash_params(&params),
                params,
                source: None,
                timestamp,
                domain_version: None,
                frame_version: None,
                model: None,
            },
            dependencies,
        );
        Ok(token.emit(value))
    }
}
