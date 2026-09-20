use serde_json::{json, Value};
use std::collections::BTreeMap;
use unclip_domain::CandidateKind;
use unclip_engine::{CandidateInputs, Engine, MeasurementRun};
use unclip_epistemic::{DerivedId, PluginId, Timestamp, Tracked};
use unclip_measure::{Measurement, MeasurementContext, MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};

fn measurement(coefficient: f64) -> Measurement {
    Measurement {sensor:PluginId::new("sensor.lagged-dependency"),sensor_version:"0.1.0".parse().unwrap(),reading:Reading::Value {value:MeasurementValue::Scalar(coefficient)},confidence:None,sample_count:Some(3),context:MeasurementContext {values:serde_json::from_value(json!({"source":"a","target":"b","lag_steps":1,"sequence":[{"observation":"z","position":0},{"observation":"a","position":10},{"observation":"m","position":30},{"observation":"b","position":40}],"evidence":"directional association, not causality"})).unwrap()}}
}
fn generate(
    inputs: &[Tracked<Measurement>],
    params: Value,
) -> unclip_plugin::Result<Vec<unclip_epistemic::Calculated<unclip_domain::CandidateProposal>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any("generate.temporal-coupling")],
            ..Default::default()
        })
        .unwrap();
    engine.generate_candidates(
        &plan,
        CandidateInputs {
            structures: &[],
            domain_version_id: "d1",
            measurements: inputs,
            observations: &[],
        },
        MeasurementRun {
            id: "discovery",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("generate.temporal-coupling"), params)]),
        },
    )
}
fn input(id: &str, value: Measurement) -> Tracked<Measurement> {
    Tracked::from_recorded(DerivedId::new(id), value)
}
fn params(threshold: f64) -> Value {
    json!({"threshold":threshold,"minimum_samples":2})
}
#[test]
fn preserves_direction_lag_order_and_independent_profiles_without_causal_claims() {
    let forward = measurement(0.9);
    let mut reverse = measurement(0.8);
    reverse.context.values.insert("source".into(), json!("b"));
    reverse.context.values.insert("target".into(), json!("a"));
    let mut inputs = vec![input("reverse", reverse), input("forward", forward.clone())];
    let candidates = generate(&inputs, params(0.7)).unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].value().kind, CandidateKind::DynamicCoupling);
    assert_eq!(candidates[0].value().value["pattern"]["source"], "a");
    assert_eq!(candidates[1].value().value["pattern"]["source"], "b");
    assert_eq!(
        candidates[0].value().value["pattern"]["sequence"],
        forward.context.values["sequence"]
    );
    assert_eq!(candidates[0].value().value["pattern"]["lag_steps"], 1);
    assert_eq!(candidates[0].value().value["evidence"]["sample_count"], 3);
    for candidate in &candidates {
        assert_eq!(candidate.value().value["causal_claim"], false);
        assert!(!candidate.value().value.contains_key("label"));
        assert_eq!(candidate.provenance().inputs.len(), 2);
    }
    inputs.reverse();
    assert_eq!(generate(&inputs, params(0.7)).unwrap(), candidates);
}
#[test]
fn signed_thresholds_and_sample_floors_keep_zero_and_sparse_evidence_distinct() {
    for (coefficient, threshold, expected) in [
        (-0.9, 0.8, 0),
        (-0.9, -1.0, 1),
        (0.0, 0.0, 1),
        (0.0, 0.1, 0),
    ] {
        let candidates =
            generate(&[input("m", measurement(coefficient))], params(threshold)).unwrap();
        assert_eq!(candidates.len(), expected);
        if let Some(candidate) = candidates.first() {
            assert_eq!(
                candidate.value().value["evidence"]["coefficient"],
                coefficient
            );
        }
    }
    assert!(generate(
        &[input("m", measurement(0.9))],
        json!({"threshold":0.8,"minimum_samples":4})
    )
    .unwrap()
    .is_empty());
    for reading in [
        Reading::NotMeasured,
        Reading::InsufficientEvidence { have: 1, need: 2 },
        Reading::NotApplicable {
            reason: "no order".into(),
        },
    ] {
        let mut value = measurement(0.9);
        value.reading = reading;
        value.context.values.clear();
        assert!(generate(&[input("sparse", value)], params(0.8))
            .unwrap()
            .is_empty());
    }
    let mut unrelated = measurement(0.9);
    unrelated.sensor = PluginId::new("sensor.dtw");
    assert!(generate(&[input("dtw", unrelated)], params(0.8))
        .unwrap()
        .is_empty());
}
#[test]
fn malformed_temporal_evidence_is_rejected_even_below_selection_threshold() {
    for change in 0..11 {
        let mut value = measurement(0.0);
        match change {
            0 => {
                value.context.values.remove("sequence");
            }
            1 => {
                value.context.values.insert("lag_steps".into(), json!(0));
            }
            2 => value.context.values.get_mut("sequence").unwrap()[1]["observation"] = json!("z"),
            3 => value.context.values.get_mut("sequence").unwrap()[1]["position"] = json!(0),
            4 => value.sample_count = None,
            5 => value.sample_count = Some(4),
            6 => value.sample_count = Some(1),
            7 => {
                value.context.values.insert("source".into(), json!(""));
            }
            8 => {
                value.reading = Reading::Value {
                    value: MeasurementValue::Scalar(f64::NAN),
                }
            }
            9 => {
                value.reading = Reading::Value {
                    value: MeasurementValue::Scalar(1.1),
                }
            }
            _ => {
                value.reading = Reading::Value {
                    value: MeasurementValue::Vector(vec![0.0]),
                }
            }
        }
        assert!(
            generate(&[input("bad", value)], params(0.9)).is_err(),
            "case {change}"
        );
    }
}
#[test]
fn invalid_parameters_and_duplicate_measurement_ids_are_rejected() {
    let value = input("m", measurement(0.9));
    for config in [
        params(1.1),
        params(-1.1),
        json!({"threshold":0.8,"minimum_samples":1}),
        json!({"threshold":0.8,"minimum_samples":2,"causal":true}),
    ] {
        assert!(generate(std::slice::from_ref(&value), config).is_err());
    }
    assert!(generate(&[value.clone(), value], params(0.8)).is_err());
}
