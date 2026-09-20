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

fn ranking(units: &[&str], unknown: &[&str]) -> Reading {
    Reading::Value {
        value: MeasurementValue::Ranking(unclip_measure::RankedState {
            tiers: units
                .iter()
                .map(|id| vec![unclip_domain::UnitId::new(*id)])
                .collect(),
            unknown: unknown
                .iter()
                .map(|id| unclip_domain::UnitId::new(*id))
                .collect(),
            unresolved: vec![],
        }),
    }
}
fn compare_rank(
    id: &str,
    a: Reading,
    b: Reading,
    params: serde_json::Value,
) -> unclip_plugin::Result<Vec<Calculated<Delta>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            comparators: vec![PluginSelection::any(id)],
            ..Default::default()
        })
        .unwrap();
    engine.compare_measurements(
        &plan,
        &Tracked::from_recorded(DerivedId::new("before"), measurement(a)),
        &Tracked::from_recorded(DerivedId::new("after"), measurement(b)),
        MeasurementRun {
            id: "ranks",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new(id), params)]),
        },
    )
}
fn ranking_payload(results: &[Calculated<Delta>]) -> unclip_engine::RankingComparison {
    let MeasurementValue::Structured(value) = &results[0].value().value else {
        panic!()
    };
    serde_json::from_value(value.clone()).unwrap()
}
#[test]
fn kendall_counts_inversions_while_rbo_measures_prefix_agreement() {
    use unclip_engine::RankingComparison;
    for (order, expected) in [
        (vec!["a", "b", "c"], 0),
        (vec!["b", "a", "c"], 1),
        (vec!["c", "b", "a"], 3),
    ] {
        let a = ranking(&["a", "b", "c"], &[]);
        let b = ranking(&order, &[]);
        let results = compare_rank("compare.kendall", a.clone(), b.clone(), json!({})).unwrap();
        let RankingComparison::Kendall {
            distance,
            discordant_pairs,
            pairs,
            ..
        } = ranking_payload(&results)
        else {
            panic!()
        };
        assert_eq!(discordant_pairs, expected);
        assert_eq!(pairs, 3);
        assert_eq!(distance, expected as f64 / 3.0);
        assert_eq!(
            results[0].provenance().inputs,
            vec![DerivedId::new("after"), DerivedId::new("before")]
        );
        assert_eq!(
            compare_rank("compare.kendall", a, b, json!({})).unwrap(),
            results
        );
    }
    for (right, expected) in [
        (vec!["a", "b"], 1.0),
        (vec!["b", "a"], 0.5),
        (vec!["c", "d"], 0.0),
    ] {
        let results = compare_rank(
            "compare.rbo",
            ranking(&["a", "b"], &["x"]),
            ranking(&right, &["y"]),
            json!({"p":0.5}),
        )
        .unwrap();
        let RankingComparison::Rbo {
            similarity,
            p,
            depth,
            before,
            after,
        } = ranking_payload(&results)
        else {
            panic!()
        };
        assert_eq!(similarity, expected);
        assert_eq!(p, 0.5);
        assert_eq!(depth, 2);
        assert_eq!(before.unknown, vec![unclip_domain::UnitId::new("x")]);
        assert_eq!(after.unknown, vec![unclip_domain::UnitId::new("y")]);
        assert_eq!(
            compare_rank(
                "compare.rbo",
                ranking(&["a", "b"], &["x"]),
                ranking(&right, &["y"]),
                json!({"p":0.5})
            )
            .unwrap(),
            results
        );
    }
}
#[test]
fn ranking_comparators_preserve_sparse_and_unsupported_shapes() {
    use unclip_engine::RankingComparison;
    let tied = Reading::Value {
        value: MeasurementValue::Ranking(unclip_measure::RankedState {
            tiers: vec![vec![
                unclip_domain::UnitId::new("a"),
                unclip_domain::UnitId::new("b"),
            ]],
            unknown: vec![],
            unresolved: vec![],
        }),
    };
    for (id, params) in [
        ("compare.kendall", json!({})),
        ("compare.rbo", json!({"p":0.9})),
    ] {
        let results = compare_rank(
            id,
            Reading::NotMeasured,
            ranking(&["a", "b"], &[]),
            params.clone(),
        )
        .unwrap();
        assert!(matches!(
            ranking_payload(&results),
            RankingComparison::Unavailable {
                before: Reading::NotMeasured,
                ..
            }
        ));
        let results =
            compare_rank(id, tied.clone(), ranking(&["a", "b"], &[]), params.clone()).unwrap();
        assert!(matches!(
            ranking_payload(&results),
            RankingComparison::NotApplicable { .. }
        ));
        assert!(compare_rank(
            id,
            ranking(&["a", "a"], &[]),
            ranking(&["a", "b"], &[]),
            params.clone()
        )
        .is_err());
        assert!(compare_rank(
            id,
            ranking(&["a"], &["a"]),
            ranking(&["a", "b"], &[]),
            params
        )
        .is_err());
    }
    for (id, a, b, params) in [
        (
            "compare.kendall",
            ranking(&["a"], &["b"]),
            ranking(&["a", "b"], &[]),
            json!({}),
        ),
        (
            "compare.kendall",
            ranking(&["a", "b"], &[]),
            ranking(&["a", "c"], &[]),
            json!({}),
        ),
        (
            "compare.rbo",
            ranking(&["a"], &[]),
            ranking(&["a", "b"], &[]),
            json!({"p":0.9}),
        ),
    ] {
        assert!(matches!(
            ranking_payload(&compare_rank(id, a, b, params).unwrap()),
            RankingComparison::NotApplicable { .. }
        ));
    }
    for params in [
        json!({}),
        json!({"p":0}),
        json!({"p":1}),
        json!({"p":0.9,"weighted":true}),
    ] {
        assert!(compare_rank(
            "compare.rbo",
            ranking(&["a"], &[]),
            ranking(&["a"], &[]),
            params
        )
        .is_err());
    }
}
