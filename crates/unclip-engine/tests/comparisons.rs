use serde_json::json;
use std::collections::BTreeMap;
use unclip_engine::{Engine, MeasurementRun, ScalarDifference};
use unclip_epistemic::{Calculated, DerivedId, PluginId, Timestamp, Tracked};
use unclip_measure::{Delta, Measurement, MeasurementContext, MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};
fn measurement(reading: Reading) -> Measurement {
    Measurement {
        sensor: PluginId::new("sensor.fixture"),
        sensor_version: "1.0.0".parse().unwrap(),
        reading,
        confidence: None,
        sample_count: Some(4),
        context: MeasurementContext::default(),
    }
}
fn scalar(v: f64) -> Reading {
    Reading::Value {
        value: MeasurementValue::Scalar(v),
    }
}
fn compare(
    before: Measurement,
    after: Measurement,
    params: serde_json::Value,
) -> unclip_plugin::Result<Vec<Calculated<Delta>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            comparators: vec![PluginSelection::any("compare.scalar-difference")],
            ..Default::default()
        })
        .unwrap();
    engine.compare_measurements(
        &plan,
        &Tracked::from_recorded(DerivedId::new("before"), before),
        &Tracked::from_recorded(DerivedId::new("after"), after),
        MeasurementRun {
            id: "compare",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("compare.scalar-difference"), params)]),
        },
    )
}
fn payload(result: &[Calculated<Delta>]) -> ScalarDifference {
    let MeasurementValue::Structured(value) = &result[0].value().value else {
        panic!()
    };
    serde_json::from_value(value.clone()).unwrap()
}
#[test]
fn signed_differences_and_zero_replay_with_both_dependencies() {
    for (a, b) in [(3.0, 1.0), (0.0, 0.0), (-2.0, 3.0)] {
        let results = compare(measurement(scalar(a)), measurement(scalar(b)), json!({})).unwrap();
        assert_eq!(
            payload(&results),
            ScalarDifference::Value {
                before: a,
                after: b,
                difference: b - a
            }
        );
        assert_eq!(
            results[0].value().comparator,
            PluginId::new("compare.scalar-difference")
        );
        assert_eq!(
            results[0].provenance().inputs,
            vec![DerivedId::new("after"), DerivedId::new("before")]
        );
        assert_eq!(
            compare(measurement(scalar(a)), measurement(scalar(b)), json!({})).unwrap(),
            results
        );
    }
}
#[test]
fn unavailable_readings_are_preserved_and_structures_are_not_subtracted() {
    for sparse in [
        Reading::NotMeasured,
        Reading::NotApplicable {
            reason: "no axis".into(),
        },
        Reading::InsufficientEvidence { have: 1, need: 3 },
    ] {
        let result = compare(
            measurement(sparse.clone()),
            measurement(scalar(0.0)),
            json!({}),
        )
        .unwrap();
        assert_eq!(
            payload(&result),
            ScalarDifference::Unavailable {
                before: sparse,
                after: scalar(0.0)
            }
        );
    }
    let result = compare(
        measurement(Reading::Value {
            value: MeasurementValue::Structured(json!({"score":1})),
        }),
        measurement(scalar(2.0)),
        json!({}),
    )
    .unwrap();
    assert!(matches!(
        payload(&result),
        ScalarDifference::NotApplicable { .. }
    ));
}
#[test]
fn mismatched_semantics_invalid_configuration_and_nonfinite_results_fail() {
    let before = measurement(scalar(1.0));
    let mut sensor = before.clone();
    sensor.sensor = PluginId::new("other");
    let mut version = before.clone();
    version.sensor_version = "2.0.0".parse().unwrap();
    let mut context = before.clone();
    context.context.values.insert("unit".into(), json!("other"));
    for after in [sensor, version, context] {
        assert!(compare(before.clone(), after, json!({})).is_err());
    }
    for invalid in [f64::NAN, f64::INFINITY] {
        assert!(compare(
            measurement(scalar(invalid)),
            measurement(Reading::NotMeasured),
            json!({})
        )
        .is_err());
    }
    assert!(compare(
        measurement(scalar(-f64::MAX)),
        measurement(scalar(f64::MAX)),
        json!({})
    )
    .is_err());
    assert!(compare(before.clone(), before, json!({"weight":1})).is_err());
}
#[test]
fn explicit_selection_enforces_versions_and_records_comparator_configuration() {
    let engine = Engine::with_builtins().unwrap();
    let empty = engine.plan(&EngineProfile::default()).unwrap();
    assert!(empty.comparators.is_empty());
    let plan = engine
        .plan(&EngineProfile {
            comparators: vec![PluginSelection::any("compare.scalar-difference")],
            ..Default::default()
        })
        .unwrap();
    let record = engine.run_record(
        &plan,
        &BTreeMap::new(),
        "comparison",
        Timestamp::new("now"),
        json!({}),
    );
    assert_eq!(
        record.resolved_plan["comparators"][0]["id"],
        "compare.scalar-difference"
    );
    assert_eq!(record.resolved_plan["comparators"][0]["version"], "0.1.0");
    assert_eq!(record.resolved_plan["comparators"][0]["params"], json!({}));
    let mut selection = PluginSelection::any("compare.scalar-difference");
    selection.version = ">=1".parse().unwrap();
    assert!(engine
        .plan(&EngineProfile {
            comparators: vec![selection],
            ..Default::default()
        })
        .is_err());
}
