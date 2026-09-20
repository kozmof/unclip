use serde_json::{json, Value};
use std::collections::BTreeMap;
use unclip_domain::CandidateKind;
use unclip_engine::{CandidateInputs, Engine, MeasurementRun};
use unclip_epistemic::{DerivedId, PluginId, Timestamp, Tracked};
use unclip_measure::{Measurement, MeasurementContext, MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};

fn measured(value: f64, samples: usize) -> Value {
    json!({"status":"value","value":value,"sample_count":samples})
}
fn input(id: &str, metric: &str, cell: Value) -> Tracked<Measurement> {
    let matrix=serde_json::from_value(json!({"metric":metric,"units":["a","b"],"cells":[[measured(1.0,4),cell.clone()],[cell,measured(1.0,4)]]})).unwrap();
    Tracked::from_recorded(
        DerivedId::new(id),
        Measurement {
            sensor: PluginId::new(format!("sensor.{metric}")),
            sensor_version: "0.1.0".parse().unwrap(),
            reading: Reading::Value {
                value: MeasurementValue::PairwiseMatrix(matrix),
            },
            confidence: None,
            sample_count: Some(4),
            context: MeasurementContext {
                values: BTreeMap::from([("fixture".into(), json!(id))]),
            },
        },
    )
}
fn generate(
    inputs: &[Tracked<Measurement>],
    params: Value,
) -> unclip_plugin::Result<Vec<unclip_epistemic::Calculated<unclip_domain::CandidateProposal>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any("generate.pairwise-coupling")],
            ..Default::default()
        })
        .unwrap();
    engine.generate_candidates(
        &plan,
        CandidateInputs {
            structures: &[],
            domain_version_id: "domain@1",
            measurements: inputs,
            observations: &[],
        },
        MeasurementRun {
            id: "discover",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("generate.pairwise-coupling"), params)]),
        },
    )
}
fn params(metric: &str, threshold: f64) -> Value {
    json!({"metric":metric,"threshold":threshold,"minimum_samples":2})
}

#[test]
fn metrics_use_their_own_threshold_direction_and_keep_measured_zero() {
    for (metric, value, threshold, expected) in [
        ("relative_rank_variance", 0.2, 0.3, 1),
        ("relative_rank_variance", 0.4, 0.3, 0),
        ("spearman", -0.9, 0.8, 0),
        ("spearman", 0.9, 0.8, 1),
        ("spearman", -0.9, -1.0, 1),
        ("kendall", 0.7, 0.7, 1),
        ("kendall", 0.6, 0.7, 0),
        ("mutual_information", 0.0, 0.0, 1),
        ("mutual_information", 0.0, 0.1, 0),
        ("relative_rank_variance", 0.0, 0.0, 1),
    ] {
        let outputs = generate(
            &[input("m", metric, measured(value, 4))],
            params(metric, threshold),
        )
        .unwrap();
        assert_eq!(outputs.len(), expected, "{metric} {value} {threshold}");
        if let Some(output) = outputs.first() {
            assert_eq!(output.value().kind, CandidateKind::DynamicCoupling);
            assert_eq!(output.value().value["evidence"]["cell"]["value"], value);
            assert_eq!(output.value().value["evidence"]["cell"]["sample_count"], 4);
            assert_eq!(output.value().value["causal_claim"], false);
            assert_eq!(output.value().value["pattern"]["units"], json!(["a", "b"]));
            assert!(!output.value().value.contains_key("label"));
        }
    }
}
#[test]
fn source_profiles_stay_separate_and_reordering_replays_identically() {
    let mut inputs = vec![
        input("second", "spearman", measured(0.8, 4)),
        input("first", "spearman", measured(0.9, 4)),
        input("other-metric", "mutual_information", measured(0.9, 4)),
    ];
    let outputs = generate(&inputs, params("spearman", 0.7)).unwrap();
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].value().value["evidence"]["measurement"], "first");
    assert_eq!(
        outputs[1].value().value["evidence"]["measurement"],
        "second"
    );
    assert_eq!(
        outputs[0].value().value["evidence"]["context"]["values"]["fixture"],
        "first"
    );
    for output in &outputs {
        assert_eq!(output.provenance().inputs.len(), 3);
    }
    inputs.reverse();
    assert_eq!(generate(&inputs, params("spearman", 0.7)).unwrap(), outputs);
    assert!(generate(&inputs, params("spearman", 0.95))
        .unwrap()
        .is_empty());
}
#[test]
fn sparse_undefined_and_low_sample_cells_never_become_coupling_proposals() {
    for cell in [
        json!({"status":"insufficient_evidence","have":1,"need":2}),
        json!({"status":"undefined","sample_count":4}),
        measured(0.9, 2),
    ] {
        let mut selection = params("spearman", 0.0);
        selection["minimum_samples"] = json!(3);
        assert!(generate(&[input("sparse", "spearman", cell)], selection)
            .unwrap()
            .is_empty());
    }
    let source = input("sparse", "spearman", measured(0.9, 4));
    let reader = unclip_epistemic::DependencyCollector::default();
    let mut measurement = reader.read(&source).clone();
    measurement.reading = Reading::NotMeasured;
    assert!(generate(
        &[Tracked::from_recorded(
            DerivedId::new("sparse"),
            measurement
        )],
        params("spearman", 0.0)
    )
    .unwrap()
    .is_empty());
}
#[test]
fn invalid_configuration_and_duplicate_source_ids_are_rejected() {
    let inputs = vec![input("m", "spearman", measured(0.9, 4))];
    for selection in [
        params("spearman", 1.1),
        params("kendall", -1.1),
        params("relative_rank_variance", -0.1),
        params("mutual_information", -0.1),
        params("unknown", 0.0),
        json!({"metric":"spearman","threshold":0.0,"minimum_samples":1}),
        json!({"metric":"spearman","threshold":0.0,"minimum_samples":2,"label":"invented"}),
    ] {
        assert!(generate(&inputs, selection).is_err());
    }
    assert!(generate(
        &[inputs[0].clone(), inputs[0].clone()],
        params("spearman", 0.0)
    )
    .is_err());
    let engine = Engine::with_builtins().unwrap();
    for generator in engine.registry().candidate_generators() {
        let _: Value = serde_json::from_str(generator.descriptor().params_schema).unwrap();
    }
}
