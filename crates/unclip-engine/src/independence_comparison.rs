//! Typed comparison of measured product behavior with explicit independence rules.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Tracked,
};
use unclip_measure::{Delta, Measurement, ProductMeasurementBinding, Reading};
use unclip_plugin::{PluginError, Result, RunPlan};

use crate::{CompositionMeasurementProfile, IndependenceExpectationProfile, MeasurementRun};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependenceComparisonEntry {
    pub product_measurement: DerivedId,
    pub expectation_measurement: DerivedId,
    pub comparator: PluginId,
    pub delta_id: DerivedId,
    pub expected: Reading,
    pub observed: Reading,
    pub delta: Delta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependenceComparisonProfile {
    pub composition_profile: DerivedId,
    pub expectation_profile: DerivedId,
    pub binding: ProductMeasurementBinding,
    pub comparisons: Vec<IndependenceComparisonEntry>,
}

#[derive(Debug)]
pub struct IndependenceComparisonResult {
    pub profile: Calculated<IndependenceComparisonProfile>,
    pub expectations: Vec<Calculated<Measurement>>,
    pub deltas: Vec<Calculated<Delta>>,
}

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

fn valid_calculated_profile<T>(value: &Calculated<T>, producer: &str, algorithm: &str) -> bool {
    let provenance = value.provenance();
    !value.id().0.trim().is_empty()
        && provenance.producer == PluginId::new(producer)
        && provenance.algorithm == algorithm
        && provenance.params_hash == hash_params(&provenance.params)
}

fn valid_sources(sources: &[DerivedId], available: &BTreeSet<DerivedId>) -> bool {
    !sources.is_empty()
        && sources
            .iter()
            .all(|identity| !identity.0.trim().is_empty() && available.contains(identity))
        && sources.iter().collect::<BTreeSet<_>>().len() == sources.len()
}

impl crate::Engine {
    /// Compare explicit typed independence expectations with observed product behavior.
    ///
    /// The expectation is always the comparator's `before` input and the measured
    /// product value is its `after` input. Only explicitly selected comparators that
    /// declare support for the expectation kind run; every expectation must have at
    /// least one such comparator.
    pub fn compare_product_with_independence(
        &self,
        plan: &RunPlan,
        composition: &Calculated<CompositionMeasurementProfile>,
        expectations: &Calculated<IndependenceExpectationProfile>,
        run: MeasurementRun<'_>,
    ) -> Result<IndependenceComparisonResult> {
        if run.id.trim().is_empty() {
            return Err(invalid(
                "independence comparison requires a nonempty run ID",
            ));
        }
        if plan.comparators.is_empty() {
            return Err(invalid(
                "independence comparison requires explicitly selected comparators",
            ));
        }
        if !valid_calculated_profile(
            composition,
            "calculate.composition-profile",
            "versioned_composition_measurement_profiles",
        ) || !valid_calculated_profile(
            expectations,
            "calculate.independence-expectations",
            "explicit_typed_independence_rules",
        ) {
            return Err(invalid(
                "independence comparison requires valid calculated composition and expectation profiles",
            ));
        }
        if expectations.value().composition_profile != *composition.id() {
            return Err(invalid(
                "independence expectation profile belongs to another composition",
            ));
        }

        let mut comparators = plan.comparators.iter().collect::<Vec<_>>();
        comparators.sort_by(|left, right| left.descriptor().id.cmp(&right.descriptor().id));
        let mut comparator_ids = BTreeSet::new();
        for comparator in &comparators {
            if !comparator_ids.insert(comparator.descriptor().id.clone()) {
                return Err(invalid(
                    "independence comparison requires unique comparator identities",
                ));
            }
        }

        let composition_value = composition.value();
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
        let all_measurements = composition_value
            .left
            .measurements
            .iter()
            .chain(&composition_value.right.measurements)
            .chain(&composition_value.product.measurements)
            .map(|entry| (entry.id.clone(), &entry.measurement))
            .collect::<BTreeMap<_, _>>();
        if product.len() != composition_value.product.measurements.len()
            || all_measurements.len()
                != composition_value.left.measurements.len()
                    + composition_value.right.measurements.len()
                    + composition_value.product.measurements.len()
            || expectations.value().expectations.len() != product.len()
        {
            return Err(invalid(
                "independence comparison requires unique, fully covered product measurements",
            ));
        }

        let profile_id = DerivedId::new(format!("{}/independence-comparison", run.id));
        let mut identities = all_measurements.keys().cloned().collect::<BTreeSet<_>>();
        identities.insert(composition.id().clone());
        identities.insert(expectations.id().clone());
        if profile_id.0.trim().is_empty() || !identities.insert(profile_id.clone()) {
            return Err(invalid(
                "independence comparison output identity collides with an input",
            ));
        }

        let aggregate_dependencies = DependencyCollector::default();
        aggregate_dependencies.read(&Tracked::from(composition));
        aggregate_dependencies.read(&Tracked::from(expectations));
        let mut used_products = BTreeSet::new();
        let mut used_comparators = BTreeSet::new();
        let mut baseline_measurements = Vec::new();
        let mut deltas = Vec::new();
        let mut entries = Vec::new();

        for (index, expectation) in expectations.value().expectations.iter().enumerate() {
            if !used_products.insert(expectation.product_measurement.clone())
                || expectation.expected.kind() != expectation.measurement_kind
                || expectation.expected.validate().is_err()
                || expectation.rule.trim().is_empty()
                || !expectation.parameters.is_object()
                || !valid_sources(&expectation.left_measurements, &left)
                || !valid_sources(&expectation.right_measurements, &right)
            {
                return Err(invalid(
                    "independence comparison requires one internally consistent expectation per product measurement",
                ));
            }
            let observed = product
                .get(&expectation.product_measurement)
                .ok_or_else(|| {
                    invalid("independence expectation references an unselected product measurement")
                })?;
            if observed.sensor != expectation.sensor
                || observed.sensor_version != expectation.sensor_version
                || matches!(
                    &observed.reading,
                    Reading::Value { value } if value.kind() != expectation.measurement_kind
                )
            {
                return Err(invalid(
                    "independence expectation does not match its exact product measurement",
                ));
            }
            let compatible = comparators
                .iter()
                .filter(|comparator| {
                    comparator
                        .descriptor()
                        .supports
                        .contains(&expectation.measurement_kind)
                })
                .copied()
                .collect::<Vec<_>>();
            if compatible.is_empty() {
                return Err(invalid(
                    "every independence expectation requires a selected comparator for its measurement kind",
                ));
            }

            let expectation_id = DerivedId::new(format!("{}/expectations/{index}", run.id));
            if !identities.insert(expectation_id.clone()) {
                return Err(invalid(
                    "independence expectation output identity collides with an input or output",
                ));
            }
            let dependencies = DependencyCollector::default();
            dependencies.read(&Tracked::from(composition));
            dependencies.read(&Tracked::from(expectations));
            let observed_tracked = Tracked::from_recorded(
                expectation.product_measurement.clone(),
                (*observed).clone(),
            );
            dependencies.read(&observed_tracked);
            aggregate_dependencies.read(&observed_tracked);
            for source in expectation
                .left_measurements
                .iter()
                .chain(&expectation.right_measurements)
            {
                let measurement = all_measurements.get(source).ok_or_else(|| {
                    invalid("independence expectation references unavailable source evidence")
                })?;
                let tracked = Tracked::from_recorded(source.clone(), (*measurement).clone());
                dependencies.read(&tracked);
                aggregate_dependencies.read(&tracked);
            }
            let expected_reading = expectation.expected.reading();
            let expected_params = serde_json::json!({
                "binding": &composition_value.product.binding,
                "product_measurement": &expectation.product_measurement,
                "left_measurements": &expectation.left_measurements,
                "right_measurements": &expectation.right_measurements,
                "rule": &expectation.rule,
                "rule_parameters": &expectation.parameters,
                "expected": &expectation.expected,
            });
            let token = CalculationToken::from_harness(
                EmitMetadata {
                    id: expectation_id,
                    producer: PluginId::new("calculate.independence-baseline"),
                    algorithm: "typed_independence_expectation".into(),
                    version: semver::Version::new(0, 1, 0),
                    params_hash: hash_params(&expected_params),
                    params: expected_params,
                    source: None,
                    timestamp: run.timestamp.clone(),
                    domain_version: None,
                    frame_version: None,
                    model: None,
                },
                dependencies,
            );
            let baseline = token.emit(Measurement {
                sensor: observed.sensor.clone(),
                sensor_version: observed.sensor_version.clone(),
                reading: expected_reading.clone(),
                confidence: None,
                sample_count: None,
                context: observed.context.clone(),
            });
            let baseline_tracked = Tracked::from(&baseline);
            aggregate_dependencies.read(&baseline_tracked);

            for comparator in compatible {
                let descriptor = comparator.descriptor();
                used_comparators.insert(descriptor.id.clone());
                let subplan = RunPlan {
                    sensors: vec![],
                    inferrers: vec![],
                    comparators: vec![(*comparator).clone()],
                    interpreters: vec![],
                    candidate_generators: vec![],
                    null_models: vec![],
                };
                let comparison_id = format!("{}/comparisons/{index}", run.id);
                let mut results = self.compare_measurements(
                    &subplan,
                    &baseline_tracked,
                    &observed_tracked,
                    MeasurementRun {
                        id: &comparison_id,
                        timestamp: run.timestamp.clone(),
                        params: run.params,
                    },
                )?;
                let delta = results
                    .pop()
                    .ok_or_else(|| invalid("typed comparator emitted no independence delta"))?;
                if !results.is_empty() || !identities.insert(delta.id().clone()) {
                    return Err(invalid(
                        "independence delta identity collides with an input or output",
                    ));
                }
                let delta_tracked = Tracked::from(&delta);
                aggregate_dependencies.read(&delta_tracked);
                entries.push(IndependenceComparisonEntry {
                    product_measurement: expectation.product_measurement.clone(),
                    expectation_measurement: baseline.id().clone(),
                    comparator: descriptor.id.clone(),
                    delta_id: delta.id().clone(),
                    expected: expected_reading.clone(),
                    observed: observed.reading.clone(),
                    delta: delta.value().clone(),
                });
                deltas.push(delta);
            }
            baseline_measurements.push(baseline);
        }
        if used_products != product.keys().cloned().collect() {
            return Err(invalid(
                "independence comparison requires every product measurement exactly once",
            ));
        }
        if used_comparators != comparator_ids {
            return Err(invalid(
                "every selected independence comparator must support at least one expectation kind",
            ));
        }

        entries.sort_by(|left, right| {
            (&left.product_measurement, &left.comparator)
                .cmp(&(&right.product_measurement, &right.comparator))
        });
        let mut resolved_comparators = comparators
            .iter()
            .map(|comparator| {
                let descriptor = comparator.descriptor();
                let params = run
                    .params
                    .get(&descriptor.id)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                serde_json::json!({
                    "id": &descriptor.id,
                    "version": &descriptor.version,
                    "params": &params,
                    "params_hash": hash_params(&params),
                })
            })
            .collect::<Vec<_>>();
        resolved_comparators.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
        let value = IndependenceComparisonProfile {
            composition_profile: composition.id().clone(),
            expectation_profile: expectations.id().clone(),
            binding: composition_value.product.binding.clone(),
            comparisons: entries,
        };
        let params = serde_json::json!({
            "composition_profile": composition.id(),
            "expectation_profile": expectations.id(),
            "binding": &value.binding,
            "comparators": resolved_comparators,
            "pairs": value.comparisons.iter().map(|entry| serde_json::json!({
                "product_measurement": &entry.product_measurement,
                "expectation_measurement": &entry.expectation_measurement,
                "comparator": &entry.comparator,
                "delta": &entry.delta_id,
            })).collect::<Vec<_>>(),
        });
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: profile_id,
                producer: PluginId::new("compare.product-independence"),
                algorithm: "explicit_typed_product_independence_comparison".into(),
                version: semver::Version::new(0, 1, 0),
                params_hash: hash_params(&params),
                params,
                source: None,
                timestamp: run.timestamp,
                domain_version: None,
                frame_version: None,
                model: None,
            },
            aggregate_dependencies,
        );
        Ok(IndependenceComparisonResult {
            profile: token.emit(value),
            expectations: baseline_measurements,
            deltas,
        })
    }
}
