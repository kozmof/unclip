use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainId, DomainSnapshot, PropertyValue, Unit, UnitId,
    UnitKind,
};
use unclip_engine::{
    ComparisonPair, CounterfactualEvidence, DeltaProfile, Engine, MeasurementRun, NullEvidence,
    NullInputs, ProfileDelta, RevisionAttempt, RevisionStep, RevisionTestOutcome,
};
use unclip_epistemic::{
    hash_params, Calculated, DependencyCollector, DerivedId, DomainVersion, EmitMetadata,
    ExperimentToken, Experimental, FrameVersion, PluginId, Timestamp, Tracked,
};
use unclip_measure::{Delta, MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};

fn atomic(id: &str) -> Unit {
    Unit {
        id: UnitId::new(id),
        kind: UnitKind::AtomicMeaning,
        label: Some(id.into()),
        properties: BTreeMap::new(),
    }
}

fn domain(existing_transformation: bool) -> DomainSnapshot {
    let mut units = ["a", "b", "c"]
        .into_iter()
        .map(|id| (UnitId::new(id), atomic(id)))
        .collect::<BTreeMap<_, _>>();
    if existing_transformation {
        units.insert(
            UnitId::new("existing-transformation"),
            Unit {
                id: UnitId::new("existing-transformation"),
                kind: UnitKind::Transformation,
                label: Some("prior interpretation is ignored".into()),
                properties: BTreeMap::from([(
                    "transformation_pattern".into(),
                    PropertyValue::Structured(proposal().value["pattern"].clone()),
                )]),
            },
        );
    }
    DomainSnapshot {
        id: DomainId::new("d"),
        version: DomainVersion::new("1"),
        units,
        relations: BTreeMap::new(),
    }
}

fn proposal() -> CandidateProposal {
    CandidateProposal {
        domain_version_id: serde_json::to_string(&("d", "1")).unwrap(),
        kind: CandidateKind::Transformation,
        value: json!({
            "pattern":{
                "matching":"exact_unit_state_transition",
                "before":["a","b"],
                "after":["b","c"]
            },
            "transitions":[
                {"before":"state-1","after":"state-2"},
                {"before":"state-3","after":"state-4"}
            ],
            "measurements":["change-1","change-2"]
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn apply(
    candidate: CandidateProposal,
    baseline: DomainSnapshot,
) -> unclip_plugin::Result<Calculated<unclip_engine::CounterfactualSnapshot>> {
    Engine::with_builtins().unwrap().apply_candidate(
        &Tracked::from_recorded(DerivedId::new("baseline"), baseline),
        &Tracked::from_recorded(DerivedId::new("transformation-candidate"), candidate),
        "transformation-trial",
        Timestamp::new("now"),
    )
}

fn evaluate_null(
    candidate: CandidateProposal,
    baseline: Option<DomainSnapshot>,
    params: Value,
) -> unclip_plugin::Result<Calculated<Reading>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&EngineProfile {
        null_models: vec![PluginSelection::any("null.existing-transformation")],
        ..Default::default()
    })?;
    let baseline = baseline.map(|value| Tracked::from_recorded(DerivedId::new("baseline"), value));
    let mut results = engine.evaluate_null_models_with_inputs(
        &plan,
        &Tracked::from_recorded(DerivedId::new("transformation-candidate"), candidate),
        NullInputs {
            domain: baseline.as_ref(),
            ..Default::default()
        },
        MeasurementRun {
            id: "transformation-null",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("null.existing-transformation"), params)]),
        },
    )?;
    Ok(results.remove(0))
}

fn null_value(result: &Calculated<Reading>) -> &Value {
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = result.value()
    else {
        panic!("expected measured transformation null")
    };
    value
}

#[test]
fn transformation_application_is_anonymous_directional_and_replayable() {
    let result = apply(proposal(), domain(false)).unwrap();
    assert_eq!(result, apply(proposal(), domain(false)).unwrap());
    assert_eq!(
        result.value().added_units,
        vec![UnitId::new("candidate:transformation-candidate")]
    );
    assert!(result.value().added_relations.is_empty());
    assert!(result.value().property_changes.is_empty());

    let transformation =
        &result.value().domain.units[&UnitId::new("candidate:transformation-candidate")];
    assert_eq!(transformation.kind, UnitKind::Transformation);
    assert_eq!(transformation.label, None);
    assert_eq!(
        transformation.properties["transformation_pattern"],
        PropertyValue::Structured(proposal().value["pattern"].clone())
    );
    assert_eq!(
        transformation.properties["before_units"],
        PropertyValue::Structured(json!(["a", "b"]))
    );
    assert_eq!(
        transformation.properties["after_units"],
        PropertyValue::Structured(json!(["b", "c"]))
    );
    assert_eq!(
        transformation.properties["causal_claim"],
        PropertyValue::Boolean(false)
    );
    assert_eq!(
        transformation.properties["candidate_evidence"],
        PropertyValue::Structured(Value::Object(proposal().value))
    );
    assert_eq!(
        result.provenance().inputs,
        vec![
            DerivedId::new("baseline"),
            DerivedId::new("transformation-candidate")
        ]
    );
}

#[test]
fn exact_transformation_null_preserves_direction_and_ignores_labels() {
    for (existing, count) in [(false, 0), (true, 1)] {
        let result = evaluate_null(proposal(), Some(domain(existing)), json!({})).unwrap();
        assert_eq!(
            null_value(&result)["model"],
            "existing_transformation_exact_pattern"
        );
        assert_eq!(null_value(&result)["match_count"], count);
        assert_eq!(null_value(&result)["has_existing_alternative"], existing);
    }

    let mut reversed = proposal();
    reversed.value["pattern"]["before"] = json!(["b", "c"]);
    reversed.value["pattern"]["after"] = json!(["a", "b"]);
    let result = evaluate_null(reversed, Some(domain(true)), json!({})).unwrap();
    assert_eq!(null_value(&result)["match_count"], 0);

    assert_eq!(
        *evaluate_null(proposal(), None, json!({})).unwrap().value(),
        Reading::InsufficientEvidence { have: 0, need: 1 }
    );
    let mut unsupported = proposal();
    unsupported.kind = CandidateKind::SemanticRole;
    assert!(matches!(
        evaluate_null(unsupported, Some(domain(false)), json!({}))
            .unwrap()
            .value(),
        Reading::NotApplicable { .. }
    ));
}

#[test]
fn transformation_rejects_false_or_malformed_evidence() {
    let mut unchanged = proposal();
    unchanged.value["pattern"]["after"] = json!(["a", "b"]);
    assert!(apply(unchanged, domain(false)).is_err());

    let mut missing_unit = proposal();
    missing_unit.value["pattern"]["after"] = json!(["b", "missing"]);
    assert!(apply(missing_unit, domain(false)).is_err());

    let mut sparse = proposal();
    sparse.value["transitions"] = json!([{"before":"state-1","after":"state-2"}]);
    assert!(apply(sparse, domain(false)).is_err());

    let mut unsorted = proposal();
    unsorted.value["transitions"] = json!([
        {"before":"state-3","after":"state-4"},
        {"before":"state-1","after":"state-2"}
    ]);
    assert!(apply(unsorted, domain(false)).is_err());

    let mut invented = proposal();
    invented
        .value
        .insert("semantic_label".into(), json!("invented"));
    assert!(apply(invented, domain(false)).is_err());
    assert!(evaluate_null(proposal(), Some(domain(false)), json!({"approximate":true})).is_err());
}

fn prior() -> Experimental<RevisionAttempt> {
    let dependencies = DependencyCollector::default();
    for id in [
        "coupling-candidate",
        "coupling-counterfactual",
        "coupling-experiment",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    let params = json!({"fixture":"transformation-prior"});
    ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("coupling-ladder/revision/dynamic-coupling"),
            producer: PluginId::new("experiment.revision-ladder"),
            algorithm: "minimal_revision_dynamic_coupling".into(),
            version: "0.1.0".parse().unwrap(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("now"),
            domain_version: Some(DomainVersion::new("1")),
            frame_version: Some(FrameVersion::new("1")),
            model: None,
        },
        dependencies,
    )
    .emit(RevisionAttempt {
        step: RevisionStep::DynamicCoupling,
        prior: Some(DerivedId::new("relation-ladder/revision/delta-e")),
        candidate: DerivedId::new("coupling-candidate"),
        counterfactual: DerivedId::new("coupling-counterfactual"),
        experiment: DerivedId::new("coupling-experiment"),
        baseline: DerivedId::new("baseline"),
        frame: DerivedId::new("frame"),
        split: DerivedId::new("split"),
        outcome: RevisionTestOutcome::Insufficient,
        reason: "dynamic coupling was insufficient".into(),
    })
}

fn comparison() -> DeltaProfile {
    let pair = ComparisonPair {
        before: DerivedId::new("transformation-before"),
        after: DerivedId::new("transformation-after"),
    };
    DeltaProfile {
        pairs: vec![pair.clone()],
        deltas: vec![ProfileDelta {
            pair,
            id: DerivedId::new("transformation-delta"),
            delta: Delta {
                comparator: PluginId::new("compare.scalar-difference"),
                value: MeasurementValue::Scalar(0.25),
            },
        }],
        unmatched_before: vec![],
        unmatched_after: vec![],
    }
}

fn structural_fixture(
    include_transformation_null: bool,
) -> (
    Engine,
    Tracked<CandidateProposal>,
    Calculated<unclip_engine::CounterfactualSnapshot>,
    Experimental<CounterfactualEvidence>,
) {
    let engine = Engine::with_builtins().unwrap();
    let candidate = Tracked::from_recorded(DerivedId::new("transformation-candidate"), proposal());
    let counterfactual = engine
        .apply_candidate(
            &Tracked::from_recorded(DerivedId::new("baseline"), domain(false)),
            &candidate,
            "transformation-trial",
            Timestamp::new("now"),
        )
        .unwrap();
    let dependencies = DependencyCollector::default();
    dependencies.read(&candidate);
    dependencies.read(&Tracked::from(&counterfactual));
    for id in [
        "baseline",
        "frame",
        "split",
        "transformation-before",
        "transformation-after",
        "transformation-comparison",
        "transformation-delta",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    if include_transformation_null {
        dependencies.read(&Tracked::from_recorded(
            DerivedId::new("transformation-null"),
            (),
        ));
    }
    let params = json!({"fixture":"transformation"});
    let experiment = ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("transformation-experiment"),
            producer: PluginId::new("experiment.counterfactual"),
            algorithm: "held_out_counterfactual_comparison".into(),
            version: "0.5.0".parse().unwrap(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("now"),
            domain_version: Some(DomainVersion::new("1")),
            frame_version: Some(FrameVersion::new("1")),
            model: None,
        },
        dependencies,
    )
    .emit(CounterfactualEvidence {
        baseline: DerivedId::new("baseline"),
        frame: DerivedId::new("frame"),
        split: DerivedId::new("split"),
        counterfactual: counterfactual.id().clone(),
        candidate: candidate.id().clone(),
        before: vec![DerivedId::new("transformation-before")],
        after: vec![DerivedId::new("transformation-after")],
        comparison: DerivedId::new("transformation-comparison"),
        delta_profile: comparison(),
        null_results: include_transformation_null
            .then(|| NullEvidence {
                id: DerivedId::new("transformation-null"),
                model: PluginId::new("null.existing-transformation"),
                reading: Reading::Value {
                    value: MeasurementValue::Structured(json!({
                        "model":"existing_transformation_exact_pattern",
                        "match_count":0,
                        "has_existing_alternative":false
                    })),
                },
            })
            .into_iter()
            .collect(),
        constraint_assessment: None,
        constraints: vec![],
        transfer_measurements: vec![],
        pareto_assessment: None,
        pareto: None,
    });
    (engine, candidate, counterfactual, experiment)
}

#[test]
fn transformation_records_only_with_exact_structural_null_evidence() {
    let prior = prior();
    let (engine, candidate, counterfactual, experiment) = structural_fixture(true);
    let attempt = engine
        .record_structural_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Sufficient,
            "the repeated state transition explains held-out evidence after coupling failed",
            "transformation-ladder",
            Timestamp::new("now"),
        )
        .unwrap();
    assert_eq!(attempt.value().step, RevisionStep::Structural);
    assert_eq!(attempt.value().prior, Some(prior.id().clone()));
    assert_eq!(
        attempt
            .provenance()
            .inputs
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            prior.id().clone(),
            candidate.id().clone(),
            counterfactual.id().clone(),
            experiment.id().clone(),
        ])
    );

    let (_, candidate, counterfactual, experiment) = structural_fixture(false);
    assert!(engine
        .record_structural_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "transformation-ladder",
            Timestamp::new("now"),
        )
        .is_err());
}
