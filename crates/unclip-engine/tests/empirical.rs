use std::num::NonZeroUsize;
use unclip_engine::{EmpiricalMethod, Engine};
use unclip_epistemic::{
    hash_params, DependencyCollector, DerivedId, EmitMetadata, InterpretationToken, ModelRef,
    Operation, PluginId, Timestamp, Tracked,
};
use unclip_measure::{Measurement, MeasurementContext, MeasurementValue, Reading};

fn input(id: &str, correlation: f64) -> Tracked<Measurement> {
    let cell = |value| serde_json::json!({"status":"value","value":value,"sample_count":4});
    let matrix = serde_json::from_value(serde_json::json!({
        "metric":"spearman", "units":["a","b"],
        "cells":[[cell(1.0),cell(correlation)],[cell(correlation),cell(1.0)]]
    }))
    .unwrap();
    Tracked::from_recorded(
        DerivedId::new(id),
        Measurement {
            sensor: PluginId::new("sensor.spearman"),
            sensor_version: semver::Version::new(0, 1, 0),
            reading: Reading::Value {
                value: MeasurementValue::PairwiseMatrix(matrix),
            },
            confidence: None,
            sample_count: Some(4),
            context: MeasurementContext::default(),
        },
    )
}
fn methods() -> [EmpiricalMethod; 2] {
    [
        EmpiricalMethod::Communities {
            threshold: 0.5,
            minimum_samples: NonZeroUsize::new(2).unwrap(),
        },
        EmpiricalMethod::Spectral {
            minimum_samples: NonZeroUsize::new(2).unwrap(),
            tolerance: 1e-12,
            max_sweeps: NonZeroUsize::new(100).unwrap(),
        },
    ]
}
#[test]
fn distinct_profiles_keep_disagreement_and_replay_exactly() {
    let engine = Engine::with_builtins().unwrap();
    for method in methods() {
        let first = engine
            .derive_empirical(
                &[input("profile-b/m", -1.0), input("profile-a/m", 1.0)],
                method,
                "g",
                Timestamp::new("now"),
            )
            .unwrap();
        let replay = engine
            .derive_empirical(
                &[input("profile-a/m", 1.0), input("profile-b/m", -1.0)],
                method,
                "g",
                Timestamp::new("now"),
            )
            .unwrap();
        assert_eq!(first.len(), 2);
        assert_ne!(
            first[0].structure.as_ref().unwrap().value(),
            first[1].structure.as_ref().unwrap().value()
        );
        for (a, b) in first.iter().zip(&replay) {
            let a_structure = a.structure.as_ref().unwrap();
            assert_eq!(a_structure.provenance().inputs, vec![a.measurement.clone()]);
            assert_eq!(
                serde_json::to_vec(&(
                    a_structure.id(),
                    a_structure.value(),
                    a_structure.provenance()
                ))
                .unwrap(),
                serde_json::to_vec(&(
                    b.structure.as_ref().unwrap().id(),
                    b.structure.as_ref().unwrap().value(),
                    b.structure.as_ref().unwrap().provenance()
                ))
                .unwrap()
            );
            assert!(a_structure.value().value.get("label").is_none());
        }
    }
}
#[test]
fn invalid_selection_and_parameters_fail_without_fabricating_structures() {
    let engine = Engine::with_builtins().unwrap();
    let method = methods()[0];
    assert!(engine
        .derive_empirical(&[], method, "g", Timestamp::new("now"))
        .is_err());
    assert!(engine
        .derive_empirical(
            &[input("a", 1.0), input("a", -1.0)],
            method,
            "g",
            Timestamp::new("now")
        )
        .is_err());
    for method in [
        EmpiricalMethod::Communities {
            threshold: f64::NAN,
            minimum_samples: NonZeroUsize::new(2).unwrap(),
        },
        EmpiricalMethod::Spectral {
            minimum_samples: NonZeroUsize::new(2).unwrap(),
            tolerance: 0.0,
            max_sweeps: NonZeroUsize::new(1).unwrap(),
        },
    ] {
        assert!(engine
            .derive_empirical(&[input("a", 1.0)], method, "g", Timestamp::new("now"))
            .is_err());
    }
    let collector = unclip_epistemic::DependencyCollector::default();
    let source = input("a", 1.0);
    let mut measurement = collector.read(&source).clone();
    measurement.reading = Reading::InsufficientEvidence { have: 1, need: 2 };
    let sparse = Tracked::from_recorded(DerivedId::new("sparse"), measurement.clone());
    for method in methods() {
        assert!(engine
            .derive_empirical(
                std::slice::from_ref(&sparse),
                method,
                "g",
                Timestamp::new("now")
            )
            .unwrap()[0]
            .structure
            .is_none());
    }
    measurement.reading = Reading::Value {
        value: MeasurementValue::Scalar(0.0),
    };
    assert!(engine
        .derive_empirical(
            &[Tracked::from_recorded(
                DerivedId::new("scalar"),
                measurement
            )],
            method,
            "g",
            Timestamp::new("now")
        )
        .is_err());
}

#[test]
fn matrix_sample_floor_is_not_a_measured_empty_structure() {
    let engine = Engine::with_builtins().unwrap();
    for method in [
        EmpiricalMethod::Communities {
            threshold: 0.5,
            minimum_samples: NonZeroUsize::new(5).unwrap(),
        },
        EmpiricalMethod::Spectral {
            minimum_samples: NonZeroUsize::new(5).unwrap(),
            tolerance: 1e-12,
            max_sweeps: NonZeroUsize::new(100).unwrap(),
        },
    ] {
        let result = engine
            .derive_empirical(&[input("a", 1.0)], method, "g", Timestamp::new("now"))
            .unwrap();
        assert_eq!(result[0].measurement, DerivedId::new("a"));
        assert!(result[0].structure.is_none());
    }
    let measured = engine
        .derive_empirical(
            &[input("a", -1.0)],
            methods()[0],
            "g",
            Timestamp::new("now"),
        )
        .unwrap();
    assert!(measured[0].structure.is_some());
}

#[test]
fn interpreted_measurements_cannot_be_reused_as_empirical_evidence() {
    let source = input("calculated", 1.0);
    let dependencies = DependencyCollector::default();
    let value = dependencies.read(&source).clone();
    let params = serde_json::json!({"model": "fixture/labeler"});
    let interpreted = InterpretationToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("interpreted/measurement"),
            producer: PluginId::new("interpret.fixture"),
            algorithm: "interpret.fixture".into(),
            version: semver::Version::new(1, 0, 0),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("now"),
            domain_version: None,
            frame_version: None,
            model: Some(ModelRef::versioned("fixture/labeler", "1")),
        },
        dependencies,
    )
    .emit(value);

    let error = Engine::with_builtins()
        .unwrap()
        .derive_empirical(
            &[Tracked::from(&interpreted)],
            methods()[0],
            "run",
            Timestamp::new("now"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("must be calculated evidence"));
    assert_eq!(interpreted.provenance().operation, Operation::Interpreted);
}
