use std::collections::BTreeMap;

use serde_json::{json, Value};
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainId, DomainSnapshot, Unit, UnitId, UnitKind,
};
use unclip_engine::{Engine, MeasurementRun, NullInputs};
use unclip_epistemic::{Calculated, DerivedId, DomainVersion, PluginId, Timestamp, Tracked};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};

fn domain() -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new("d"),
        version: DomainVersion::new("1"),
        units: ["a", "b"]
            .into_iter()
            .map(|id| {
                (
                    UnitId::new(id),
                    Unit {
                        id: UnitId::new(id),
                        kind: UnitKind::AtomicMeaning,
                        label: Some(id.into()),
                        properties: BTreeMap::new(),
                    },
                )
            })
            .collect(),
        relations: BTreeMap::new(),
    }
}

fn pairwise(metric: &str, value: f64, threshold: f64) -> CandidateProposal {
    CandidateProposal {
        domain_version_id: serde_json::to_string(&("d", "1")).unwrap(),
        kind: CandidateKind::DynamicCoupling,
        value: json!({
            "pattern": {
                "matching": "thresholded_pairwise_association",
                "metric": metric,
                "units": ["a", "b"]
            },
            "evidence": {
                "measurement": "matrix",
                "sensor": format!("sensor.{metric}"),
                "sensor_version": "0.1.0",
                "context": {"values": {}},
                "cell": {"status": "value", "value": value, "sample_count": 4}
            },
            "selection": {"threshold": threshold, "minimum_samples": 2},
            "causal_claim": false
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn temporal(value: f64) -> CandidateProposal {
    let sequence = json!([
        {"observation": "o1", "position": 0},
        {"observation": "o2", "position": 1},
        {"observation": "o3", "position": 2}
    ]);
    CandidateProposal {
        domain_version_id: serde_json::to_string(&("d", "1")).unwrap(),
        kind: CandidateKind::DynamicCoupling,
        value: json!({
            "pattern": {
                "matching": "lagged_directional_association",
                "source": "a",
                "target": "b",
                "lag_steps": 1,
                "sequence": sequence
            },
            "evidence": {
                "measurement": "lagged",
                "sensor": "sensor.lagged-dependency",
                "sensor_version": "0.1.0",
                "coefficient": value,
                "sample_count": 2,
                "context": {"values": {
                    "source": "a",
                    "target": "b",
                    "lag_steps": 1,
                    "sequence": sequence
                }}
            },
            "selection": {"threshold": 0.5, "minimum_samples": 2},
            "causal_claim": false
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn evaluate(
    candidate: CandidateProposal,
    baseline: Option<DomainSnapshot>,
    params: Value,
) -> unclip_plugin::Result<Calculated<Reading>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            null_models: vec![PluginSelection::any("null.coupling-zero")],
            ..Default::default()
        })
        .unwrap();
    let baseline = baseline.map(|value| Tracked::from_recorded(DerivedId::new("domain"), value));
    let mut results = engine.evaluate_null_models_with_inputs(
        &plan,
        &Tracked::from_recorded(DerivedId::new("candidate"), candidate),
        NullInputs {
            domain: baseline.as_ref(),
            ..Default::default()
        },
        MeasurementRun {
            id: "null",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("null.coupling-zero"), params)]),
        },
    )?;
    Ok(results.remove(0))
}

fn value(result: &Calculated<Reading>) -> &Value {
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = result.value()
    else {
        panic!("expected measured structured null")
    };
    value
}

#[test]
fn supported_pairwise_and_temporal_couplings_keep_signed_zero_distance() {
    for (candidate, observed, tolerance, within) in [
        (pairwise("spearman", -0.8, -0.9), -0.8_f64, 0.7, false),
        (pairwise("mutual_information", 0.2, 0.1), 0.2, 0.2, true),
        (temporal(0.75), 0.75, 0.0, false),
    ] {
        let result = evaluate(
            candidate.clone(),
            Some(domain()),
            json!({"absolute_tolerance": tolerance}),
        )
        .unwrap();
        assert_eq!(value(&result)["model"], "zero_association_baseline");
        assert_eq!(value(&result)["observed_value"], observed);
        assert_eq!(value(&result)["absolute_distance"], observed.abs());
        assert_eq!(value(&result)["within_tolerance"], within);
        assert_eq!(value(&result)["causal_claim"], false);
        assert_eq!(
            result.provenance().inputs,
            vec![DerivedId::new("candidate"), DerivedId::new("domain")]
        );
        assert_eq!(
            evaluate(
                candidate,
                Some(domain()),
                json!({"absolute_tolerance": tolerance})
            )
            .unwrap(),
            result
        );
    }
}

#[test]
fn unsupported_or_missing_baselines_remain_explicit() {
    let unavailable = evaluate(
        pairwise("spearman", 0.8, 0.7),
        None,
        json!({"absolute_tolerance": 0.1}),
    )
    .unwrap();
    assert_eq!(
        *unavailable.value(),
        Reading::InsufficientEvidence { have: 0, need: 1 }
    );
    assert_eq!(
        unavailable.provenance().inputs,
        vec![DerivedId::new("candidate")]
    );

    let variance = evaluate(
        pairwise("relative_rank_variance", 0.1, 0.2),
        Some(domain()),
        json!({"absolute_tolerance": 0.1}),
    )
    .unwrap();
    assert!(matches!(variance.value(), Reading::NotApplicable { .. }));

    let mut relation = pairwise("spearman", 0.8, 0.7);
    relation.kind = CandidateKind::Relation;
    let unsupported =
        evaluate(relation, Some(domain()), json!({"absolute_tolerance": 0.1})).unwrap();
    assert!(matches!(unsupported.value(), Reading::NotApplicable { .. }));
    assert_eq!(
        unsupported.provenance().inputs,
        vec![DerivedId::new("candidate")]
    );
}

#[test]
fn invalid_configuration_and_coupling_evidence_are_rejected() {
    for params in [
        json!({}),
        json!({"absolute_tolerance": -0.1}),
        json!({"absolute_tolerance": 0.1, "alpha": 0.05}),
    ] {
        assert!(evaluate(pairwise("spearman", 0.8, 0.7), Some(domain()), params).is_err());
    }
    let mut causal = temporal(0.75);
    causal.value["causal_claim"] = json!(true);
    assert!(evaluate(causal, Some(domain()), json!({"absolute_tolerance": 0.1})).is_err());
}
