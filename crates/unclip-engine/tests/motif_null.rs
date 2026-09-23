use std::collections::BTreeMap;

use serde_json::{json, Value};
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainId, DomainSnapshot, PropertyValue, Unit, UnitId,
    UnitKind,
};
use unclip_engine::{Engine, MeasurementRun, NullInputs};
use unclip_epistemic::{Calculated, DerivedId, DomainVersion, PluginId, Timestamp, Tracked};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};

fn proposal() -> CandidateProposal {
    CandidateProposal {
        domain_version_id: serde_json::to_string(&("d", "1")).unwrap(),
        kind: CandidateKind::GraphMotif,
        value: json!({
            "pattern": {
                "matching": "exact_directed_two_edge_path",
                "nodes": [
                    {"position": 0, "observed_label": "source"},
                    {"position": 1, "observed_label": "middle"},
                    {"position": 2, "observed_label": "target"}
                ],
                "edges": [
                    {"source": 0, "target": 1, "kind": "supports"},
                    {"source": 1, "target": 2, "kind": "enables"}
                ]
            },
            "observation_count": 2,
            "observations": ["o1", "o2"],
            "examples": [
                {
                    "observation": "o1",
                    "units": ["o1-a", "o1-b", "o1-c"],
                    "edges": [
                        {"relation": "o1-r1", "uncertainty": 0.1, "measurements": ["m1"]},
                        {"relation": "o1-r2", "uncertainty": 0.2, "measurements": ["m2"]}
                    ]
                },
                {
                    "observation": "o2",
                    "units": ["o2-a", "o2-b", "o2-c"],
                    "edges": [
                        {"relation": "o2-r1", "uncertainty": null, "measurements": ["m3"]},
                        {"relation": "o2-r2", "uncertainty": 0.0, "measurements": ["m4"]}
                    ]
                }
            ]
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn domain(existing: bool) -> DomainSnapshot {
    let mut units = BTreeMap::new();
    if existing {
        units.insert(
            UnitId::new("motif"),
            Unit {
                id: UnitId::new("motif"),
                kind: UnitKind::GraphMotif,
                label: Some("prior interpretation".into()),
                properties: BTreeMap::from([(
                    "graph_pattern".into(),
                    PropertyValue::Structured(proposal().value["pattern"].clone()),
                )]),
            },
        );
    }
    units.insert(
        UnitId::new("other"),
        Unit {
            id: UnitId::new("other"),
            kind: UnitKind::GraphMotif,
            label: None,
            properties: BTreeMap::from([(
                "graph_pattern".into(),
                PropertyValue::Structured(json!({"matching": "different"})),
            )]),
        },
    );
    DomainSnapshot {
        id: DomainId::new("d"),
        version: DomainVersion::new("1"),
        units,
        relations: BTreeMap::new(),
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
            null_models: vec![PluginSelection::any("null.existing-motif")],
            ..Default::default()
        })
        .unwrap();
    let baseline = baseline.map(|value| Tracked::from_recorded(DerivedId::new("domain"), value));
    let mut outputs = engine.evaluate_null_models_with_inputs(
        &plan,
        &Tracked::from_recorded(DerivedId::new("candidate"), candidate),
        NullInputs {
            domain: baseline.as_ref(),
            ..Default::default()
        },
        MeasurementRun {
            id: "null",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("null.existing-motif"), params)]),
        },
    )?;
    Ok(outputs.remove(0))
}

fn value(result: &Calculated<Reading>) -> &Value {
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = result.value()
    else {
        panic!("expected measured motif null")
    };
    value
}

#[test]
fn exact_existing_patterns_are_retained_without_using_labels() {
    for (existing, count) in [(false, 0), (true, 1)] {
        let result = evaluate(proposal(), Some(domain(existing)), json!({})).unwrap();
        assert_eq!(value(&result)["model"], "existing_motif_exact_pattern");
        assert_eq!(value(&result)["match_count"], count);
        assert_eq!(value(&result)["has_existing_alternative"], existing);
        if existing {
            assert_eq!(value(&result)["matches"], json!(["motif"]));
        }
        assert_eq!(
            result.provenance().inputs,
            vec![DerivedId::new("candidate"), DerivedId::new("domain")]
        );
    }
}

#[test]
fn missing_baselines_and_unsupported_candidates_remain_explicit() {
    let missing = evaluate(proposal(), None, json!({})).unwrap();
    assert_eq!(
        *missing.value(),
        Reading::InsufficientEvidence { have: 0, need: 1 }
    );
    assert_eq!(
        missing.provenance().inputs,
        vec![DerivedId::new("candidate")]
    );

    let mut unsupported = proposal();
    unsupported.kind = CandidateKind::SemanticRole;
    let result = evaluate(unsupported, Some(domain(false)), json!({})).unwrap();
    assert!(matches!(result.value(), Reading::NotApplicable { .. }));
    assert_eq!(
        result.provenance().inputs,
        vec![DerivedId::new("candidate")]
    );
}

#[test]
fn malformed_motifs_versions_and_parameters_are_rejected() {
    let mut malformed = proposal();
    malformed.value["examples"] = json!([]);
    assert!(evaluate(malformed, Some(domain(false)), json!({})).is_err());

    let mut wrong_version = proposal();
    wrong_version.domain_version_id = "other".into();
    assert!(evaluate(wrong_version, Some(domain(false)), json!({})).is_err());
    assert!(evaluate(
        proposal(),
        Some(domain(false)),
        json!({"approximate": true})
    )
    .is_err());
}
