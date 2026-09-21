use std::collections::BTreeMap;
use unclip_domain::{DomainId, DomainSnapshot, UnitId};
use unclip_engine::{ConstraintStatus, CounterfactualSnapshot, Engine, ExperimentConstraint};
use unclip_epistemic::{DerivedId, DomainVersion, PluginId, Timestamp, Tracked};
use unclip_measure::{Measurement, MeasurementContext, Reading};
fn application() -> Tracked<CounterfactualSnapshot> {
    Tracked::from_recorded(
        DerivedId::new("application"),
        CounterfactualSnapshot {
            baseline_domain_version_id: "baseline".into(),
            candidate: DerivedId::new("candidate"),
            added_units: vec![UnitId::new("new")],
            added_relations: vec![],
            property_changes: vec![],
            domain: DomainSnapshot {
                id: DomainId::new("d"),
                version: DomainVersion::new("2"),
                units: BTreeMap::new(),
                relations: BTreeMap::new(),
            },
        },
    )
}
fn measurement(id: &str, count: Option<usize>) -> Tracked<Measurement> {
    Tracked::from_recorded(
        DerivedId::new(id),
        Measurement {
            sensor: PluginId::new("fixture"),
            sensor_version: semver::Version::new(1, 0, 0),
            reading: Reading::InsufficientEvidence { have: 0, need: 2 },
            confidence: None,
            sample_count: count,
            context: MeasurementContext::default(),
        },
    )
}
fn floor(id: &str, minimum: usize) -> ExperimentConstraint {
    ExperimentConstraint::MinimumSamples {
        measurement: DerivedId::new(id),
        minimum,
    }
}
fn budget(maximum_added_units: usize) -> ExperimentConstraint {
    ExperimentConstraint::ComplexityBudget {
        maximum_added_units,
        maximum_added_relations: 0,
        maximum_property_changes: 0,
    }
}
#[test]
fn explicit_constraints_preserve_failures_missing_counts_and_provenance() {
    let engine = Engine::with_builtins().unwrap();
    let inputs = [
        measurement("enough", Some(2)),
        measurement("short", Some(1)),
        measurement("unknown", None),
    ];
    let constraints = [
        floor("enough", 2),
        floor("short", 2),
        floor("unknown", 2),
        budget(0),
    ];
    let result = engine
        .assess_experiment_constraints(
            &constraints,
            &inputs,
            &application(),
            "run",
            Timestamp::new("now"),
        )
        .unwrap();
    assert_eq!(
        result
            .value()
            .iter()
            .map(|v| v.status.clone())
            .collect::<Vec<_>>(),
        vec![
            ConstraintStatus::Satisfied,
            ConstraintStatus::Violated,
            ConstraintStatus::Unavailable,
            ConstraintStatus::Violated
        ]
    );
    assert_eq!(result.value()[3].observed["added_units"], 1);
    assert_eq!(
        result.provenance().inputs,
        ["application", "enough", "short", "unknown"].map(DerivedId::new)
    );
    assert_eq!(
        result,
        engine
            .assess_experiment_constraints(
                &constraints,
                &inputs,
                &application(),
                "run",
                Timestamp::new("now")
            )
            .unwrap()
    );
    let accepted = engine
        .assess_experiment_constraints(
            &[budget(1)],
            &[],
            &application(),
            "run",
            Timestamp::new("now"),
        )
        .unwrap();
    assert_eq!(accepted.value()[0].status, ConstraintStatus::Satisfied);
}
#[test]
fn ambiguous_or_missing_requirements_fail() {
    let engine = Engine::with_builtins().unwrap();
    for constraints in [
        vec![],
        vec![floor("missing", 2)],
        vec![floor("m", 0)],
        vec![floor("m", 2), floor("m", 3)],
        vec![budget(1), budget(2)],
    ] {
        assert!(engine
            .assess_experiment_constraints(
                &constraints,
                &[measurement("m", Some(2))],
                &application(),
                "run",
                Timestamp::new("now")
            )
            .is_err());
    }
    for inputs in [
        vec![measurement("m", Some(2)), measurement("m", Some(3))],
        vec![measurement("run/constraints", Some(2))],
        vec![measurement("application", Some(2))],
    ] {
        assert!(engine
            .assess_experiment_constraints(
                &[budget(1)],
                &inputs,
                &application(),
                "run",
                Timestamp::new("now")
            )
            .is_err());
    }
}
