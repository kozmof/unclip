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

#[test]
fn conditional_requirements_validate_context_counts_and_retained_readings() {
    use unclip_measure::MeasurementValue;
    let engine = Engine::with_builtins().unwrap();
    let requirement = ExperimentConstraint::ConditionalDependency {
        measurement: DerivedId::new("conditional"),
        left: UnitId::new("a"),
        right: UnitId::new("b"),
        conditioning: UnitId::new("z"),
        minimum_samples: 4,
        minimum_information: 0.2,
    };
    let input = |value: f64, samples: Option<usize>, condition: &str| Measurement {
        sensor: PluginId::new("sensor.conditional-mutual-information"),
        sensor_version: semver::Version::new(0, 1, 0),
        reading: Reading::Value {
            value: MeasurementValue::Scalar(value),
        },
        confidence: None,
        sample_count: samples,
        context: MeasurementContext {
            values: BTreeMap::from([
                ("pair".into(), serde_json::json!(["a", "b"])),
                (
                    "conditioning_variables".into(),
                    serde_json::json!([condition]),
                ),
            ]),
        },
    };
    let assess = |value: Measurement, constraint: &ExperimentConstraint| {
        engine.assess_experiment_constraints(
            std::slice::from_ref(constraint),
            &[Tracked::from_recorded(DerivedId::new("conditional"), value)],
            &application(),
            "run",
            Timestamp::new("now"),
        )
    };
    for (value, samples, status) in [
        (0.2, Some(4), ConstraintStatus::Satisfied),
        (0.1, Some(4), ConstraintStatus::Violated),
        (0.4, Some(3), ConstraintStatus::Violated),
        (0.4, None, ConstraintStatus::Unavailable),
    ] {
        let measurement = input(value, samples, "z");
        let result = assess(measurement.clone(), &requirement).unwrap();
        assert_eq!(result.value()[0].status, status);
        assert_eq!(result.value()[0].reading, Some(measurement.reading));
        assert_eq!(
            result.provenance().inputs,
            vec![DerivedId::new("conditional")]
        );
    }
    assert!(assess(input(0.3, Some(4), "other"), &requirement).is_err());
    assert!(assess(input(f64::NAN, Some(4), "z"), &requirement).is_err());
    assert!(assess(input(-0.1, Some(4), "z"), &requirement).is_err());
    let mut unavailable = input(0.3, Some(4), "z");
    unavailable.reading = Reading::InsufficientEvidence { have: 1, need: 2 };
    unavailable.context = MeasurementContext::default();
    assert_eq!(
        assess(unavailable, &requirement).unwrap().value()[0].status,
        ConstraintStatus::Unavailable
    );
    let mut wrong_sensor = input(0.3, Some(4), "z");
    wrong_sensor.sensor = PluginId::new("sensor.mutual-information");
    assert!(assess(wrong_sensor, &requirement).is_err());
    let mut invalid = requirement.clone();
    if let ExperimentConstraint::ConditionalDependency {
        minimum_information,
        ..
    } = &mut invalid
    {
        *minimum_information = f64::INFINITY;
    }
    assert!(assess(input(0.3, Some(4), "z"), &invalid).is_err());
}

#[test]
fn scalar_transfer_requires_disjoint_comparable_evidence_and_preserves_failures() {
    use unclip_measure::MeasurementValue;
    let engine = Engine::with_builtins().unwrap();
    let requirement = ExperimentConstraint::ScalarTransfer {
        source: DerivedId::new("s"),
        target: DerivedId::new("t"),
        minimum_samples: 2,
        maximum_absolute_difference: 0.25,
    };
    let input = |id: &str, observations: &[&str], value: f64, samples: Option<usize>| {
        Tracked::from_recorded(
            DerivedId::new(id),
            Measurement {
                sensor: PluginId::new("fixture"),
                sensor_version: semver::Version::new(1, 0, 0),
                reading: Reading::Value {
                    value: MeasurementValue::Scalar(value),
                },
                confidence: None,
                sample_count: samples,
                context: MeasurementContext {
                    values: BTreeMap::from([
                        ("observations".into(), serde_json::json!(observations)),
                        ("pair".into(), serde_json::json!(["a", "b"])),
                    ]),
                },
            },
        )
    };
    let assess = |target| {
        engine.assess_experiment_constraints(
            std::slice::from_ref(&requirement),
            &[input("s", &["a", "b"], 0.5, Some(2)), target],
            &application(),
            "run",
            Timestamp::new("now"),
        )
    };
    for (value, count, expected) in [
        (0.75, Some(2), ConstraintStatus::Satisfied),
        (0.8, Some(2), ConstraintStatus::Violated),
        (0.5, Some(1), ConstraintStatus::Violated),
        (0.5, None, ConstraintStatus::Unavailable),
    ] {
        let result = assess(input("t", &["c", "d"], value, count)).unwrap();
        assert_eq!(result.value()[0].status, expected);
        assert_eq!(
            result.value()[0]
                .transfer
                .as_ref()
                .unwrap()
                .absolute_difference,
            Some((value - 0.5).abs())
        );
        assert_eq!(
            result.provenance().inputs,
            vec![DerivedId::new("s"), DerivedId::new("t")]
        );
    }
    assert!(assess(input("t", &["b", "c"], 0.5, Some(2))).is_err());
    assert!(assess(input("t", &["c", "c"], 0.5, Some(2))).is_err());
    assert!(assess(input("t", &["c", "d"], 0.5, Some(3))).is_err());
    assert!(assess(input("t", &["c", "d"], f64::NAN, Some(2))).is_err());
}
