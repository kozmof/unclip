//! Contract checks that every built-in sensor must satisfy.
//!
//! These run against the registry rather than against a hand-written list, so
//! a newly added sensor is covered the moment it is registered. They assert the
//! parts of the sensor contract that hold regardless of what a sensor
//! calculates: a usable descriptor, a declared stage, honest evidence
//! requirements, and determinism.

use std::collections::BTreeSet;

use unclip_plugin::{EvidenceRequirement, Registry, SensorDescriptor, SensorStage};

/// Every descriptor in the registry, across all three sensor kinds.
fn descriptors() -> Vec<SensorDescriptor> {
    let mut registry = Registry::default();
    unclip_sensors::register_all(&mut registry).expect("built-in sensors register cleanly");
    registry
        .sensors()
        .map(|plugin| plugin.descriptor().clone())
        .chain(
            registry
                .product_sensors()
                .map(|plugin| plugin.descriptor().clone()),
        )
        .chain(
            registry
                .cross_product_sensors()
                .map(|plugin| plugin.descriptor().clone()),
        )
        .collect()
}

#[test]
fn the_registry_is_not_empty() {
    assert!(
        descriptors().len() >= 10,
        "these contract checks are worthless if the registry is empty"
    );
}

#[test]
fn every_sensor_has_a_namespaced_nonempty_id() {
    for descriptor in descriptors() {
        assert!(
            descriptor.id.as_str().starts_with("sensor."),
            "sensor id {} is not namespaced under `sensor.`",
            descriptor.id
        );
        assert!(
            !descriptor
                .id
                .as_str()
                .trim_start_matches("sensor.")
                .is_empty(),
            "sensor id {} has an empty name",
            descriptor.id
        );
    }
}

#[test]
fn sensor_ids_are_unique_across_every_sensor_kind() {
    let mut seen = BTreeSet::new();
    for descriptor in descriptors() {
        assert!(
            seen.insert(descriptor.id.clone()),
            "sensor id {} is registered more than once",
            descriptor.id
        );
    }
}

/// A schema that does not parse cannot validate anything, and the engine hashes
/// parameters against it for reproducibility.
#[test]
fn every_params_schema_is_a_json_object_schema() {
    for descriptor in descriptors() {
        let schema: serde_json::Value = serde_json::from_str(descriptor.params_schema)
            .unwrap_or_else(|error| {
                panic!(
                    "sensor {} has an unparseable params schema: {error}",
                    descriptor.id
                )
            });
        assert_eq!(
            schema.get("type").and_then(|value| value.as_str()),
            Some("object"),
            "sensor {} must declare an object params schema",
            descriptor.id
        );
    }
}

/// Every sensor emits something, or the engine has no reason to call it.
#[test]
fn every_sensor_declares_at_least_one_produced_kind() {
    for descriptor in descriptors() {
        assert!(
            !descriptor.produces.is_empty(),
            "sensor {} declares no produced measurement kind",
            descriptor.id
        );
    }
}

/// A floor of zero is not a requirement at all: the engine would let every
/// input through, including none.
///
/// The floor is deliberately not asserted to be two or more. A co-occurrence
/// count is well defined at a single observation, so the right floor depends
/// on what the sensor calculates, not on a blanket rule.
#[test]
fn evidence_requirements_declare_nonzero_floors() {
    for descriptor in descriptors() {
        for requirement in descriptor.evidence {
            let (kind, minimum) = match requirement {
                EvidenceRequirement::MinSamples(minimum) => ("sample", *minimum),
                EvidenceRequirement::MinConditioningVariables(minimum) => {
                    ("conditioning-variable", *minimum)
                }
                _ => continue,
            };
            assert!(
                minimum >= 1,
                "sensor {} declares a zero {kind} floor, which requires nothing",
                descriptor.id
            );
        }
    }
}

/// Statistics that need more than one observation say so in their descriptor.
///
/// These floors are what stop the engine from handing a sensor evidence it
/// cannot support a result from, so they are pinned rather than left to drift.
/// `co-foreground` counts co-occurrences and is meaningful at one observation;
/// conditional mutual information needs a pair; partial correlation needs
/// enough residual degrees of freedom.
#[test]
fn conditioned_pair_sensors_pin_their_known_sample_floors() {
    let floor = |id: &str| {
        descriptors()
            .into_iter()
            .find(|descriptor| descriptor.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} is not registered"))
            .evidence
            .iter()
            .find_map(|requirement| match requirement {
                EvidenceRequirement::MinSamples(minimum) => Some(*minimum),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{id} declares no sample floor"))
    };
    assert_eq!(floor("sensor.co-foreground"), 1);
    assert_eq!(floor("sensor.conditional-mutual-information"), 2);
    assert_eq!(floor("sensor.partial-correlation"), 4);
}

/// Exactly one sensor establishes what the domain explains, and exactly one
/// reports what it does not. The stages are a chain: two of either, or none,
/// would leave the residual stage without a well-defined input.
#[test]
fn the_explanation_and_residual_stages_have_exactly_one_sensor_each() {
    let staged = |stage: SensorStage| {
        descriptors()
            .into_iter()
            .filter(|descriptor| descriptor.stage == stage)
            .map(|descriptor| descriptor.id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(staged(SensorStage::Explanation), vec!["sensor.coverage"]);
    assert_eq!(staged(SensorStage::Residual), vec!["sensor.residual"]);
    assert!(
        !staged(SensorStage::Measurement).is_empty(),
        "no measurement-stage sensors registered"
    );
}

/// Descriptors are read on every run and must not depend on call order or
/// interior mutation.
#[test]
fn descriptors_are_stable_across_registrations() {
    let first = descriptors();
    let second = descriptors();
    assert_eq!(first.len(), second.len());
    for (left, right) in first.iter().zip(second.iter()) {
        assert_eq!(left.id, right.id);
        assert_eq!(left.version, right.version);
        assert_eq!(left.stage, right.stage);
        assert_eq!(left.applicability, right.applicability);
        assert_eq!(left.evidence, right.evidence);
        assert_eq!(left.produces, right.produces);
        assert_eq!(left.params_schema, right.params_schema);
    }
}

/// Registering the built-ins twice into one registry must fail rather than
/// silently replace a plugin, which would make the resolved plan ambiguous.
#[test]
fn registering_the_built_ins_twice_is_rejected() {
    let mut registry = Registry::default();
    unclip_sensors::register_all(&mut registry).expect("first registration succeeds");
    assert!(
        unclip_sensors::register_all(&mut registry).is_err(),
        "re-registering the built-in sensors must be rejected"
    );
}
