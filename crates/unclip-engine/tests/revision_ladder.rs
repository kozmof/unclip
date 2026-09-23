use std::collections::BTreeMap;

use serde_json::json;
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainId, DomainSnapshot, PropertyValue, Unit, UnitId,
    UnitKind,
};
use unclip_engine::{
    ComparisonPair, ConstraintAssessment, ConstraintStatus, CounterfactualEvidence, DeltaProfile,
    Engine, ExperimentConstraint, NullEvidence, ProfileDelta, RelationBindings, RevisionStep,
    RevisionTestOutcome,
};
use unclip_epistemic::{
    hash_params, DependencyCollector, DerivedId, DomainVersion, EmitMetadata, ExperimentToken,
    FrameVersion, Operation, PluginId, Timestamp, Tracked,
};
use unclip_measure::{Delta, MeasurementValue, Reading};

fn domain() -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new("d"),
        version: DomainVersion::new("1"),
        units: [
            (
                UnitId::new("existing"),
                Unit {
                    id: UnitId::new("existing"),
                    kind: UnitKind::AtomicMeaning,
                    label: Some("known".into()),
                    properties: BTreeMap::from([("weight".into(), PropertyValue::Number(0.5))]),
                },
            ),
            (
                UnitId::new("target"),
                Unit {
                    id: UnitId::new("target"),
                    kind: UnitKind::AtomicMeaning,
                    label: Some("destination".into()),
                    properties: BTreeMap::new(),
                },
            ),
        ]
        .into_iter()
        .collect(),
        relations: BTreeMap::new(),
    }
}

fn proposal(kind: CandidateKind) -> CandidateProposal {
    CandidateProposal {
        domain_version_id: serde_json::to_string(&("d", "1")).unwrap(),
        kind,
        value: json!({
            "pattern": {
                "matching": "numeric_property_revision",
                "target": {"kind": "unit", "id": "existing"},
                "property": "weight",
                "proposed_value": 0.75
            }
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn comparison(before: &str, after: &str, delta: &str) -> DeltaProfile {
    let pair = ComparisonPair {
        before: DerivedId::new(before),
        after: DerivedId::new(after),
    };
    DeltaProfile {
        pairs: vec![pair.clone()],
        deltas: vec![ProfileDelta {
            pair,
            id: DerivedId::new(delta),
            delta: Delta {
                comparator: PluginId::new("compare.scalar-difference"),
                value: MeasurementValue::Scalar(0.25),
            },
        }],
        unmatched_before: vec![],
        unmatched_after: vec![],
    }
}

fn weight_null() -> NullEvidence {
    NullEvidence {
        id: DerivedId::new("weight-null"),
        model: PluginId::new("null.weight-change"),
        reading: Reading::Value {
            value: MeasurementValue::Structured(json!({
                "model": "retain_existing_numeric_property",
                "within_tolerance": false
            })),
        },
    }
}

fn constraint(status: ConstraintStatus) -> ConstraintAssessment {
    ConstraintAssessment {
        constraint: ExperimentConstraint::ComplexityBudget {
            maximum_added_units: 0,
            maximum_added_relations: 0,
            maximum_property_changes: 1,
        },
        status,
        observed: BTreeMap::from([
            ("added_units".into(), 0),
            ("added_relations".into(), 0),
            ("property_changes".into(), 1),
        ]),
        reading: None,
        transfer: None,
    }
}

fn fixture(
    kind: CandidateKind,
    include_null: bool,
    constraint_status: Option<ConstraintStatus>,
) -> (
    Engine,
    Tracked<CandidateProposal>,
    unclip_epistemic::Calculated<unclip_engine::CounterfactualSnapshot>,
    unclip_epistemic::Experimental<CounterfactualEvidence>,
) {
    let engine = Engine::with_builtins().unwrap();
    let candidate = Tracked::from_recorded(DerivedId::new("candidate"), proposal(kind));
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), domain());
    let counterfactual = engine
        .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
        .unwrap();
    let dependencies = DependencyCollector::default();
    dependencies.read(&candidate);
    dependencies.read(&Tracked::from(&counterfactual));
    for id in [
        "baseline",
        "frame",
        "split",
        "before",
        "after",
        "comparison",
        "delta",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    if include_null {
        dependencies.read(&Tracked::from_recorded(DerivedId::new("weight-null"), ()));
    }
    let params = json!({"fixture": true});
    let experiment = ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("experiment"),
            producer: PluginId::new("experiment.counterfactual"),
            algorithm: "held_out_counterfactual_comparison".into(),
            version: semver::Version::new(0, 5, 0),
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
        before: vec![DerivedId::new("before")],
        after: vec![DerivedId::new("after")],
        comparison: DerivedId::new("comparison"),
        delta_profile: comparison("before", "after", "delta"),
        null_results: include_null.then(weight_null).into_iter().collect(),
        constraint_assessment: constraint_status
            .as_ref()
            .map(|_| DerivedId::new("constraints")),
        constraints: constraint_status.into_iter().map(constraint).collect(),
        transfer_measurements: vec![],
        pareto_assessment: None,
        pareto: None,
    });
    (engine, candidate, counterfactual, experiment)
}

#[test]
fn delta_w_records_an_explicit_replayable_experimental_verdict() {
    for outcome in [
        RevisionTestOutcome::Sufficient,
        RevisionTestOutcome::Insufficient,
    ] {
        let (engine, candidate, counterfactual, experiment) = fixture(
            CandidateKind::WeightRevision,
            true,
            Some(ConstraintStatus::Satisfied),
        );
        let record = || {
            engine
                .record_delta_w_test(
                    &candidate,
                    &counterfactual,
                    &experiment,
                    outcome,
                    "held-out evidence and explicit constraints were reviewed",
                    "ladder",
                    Timestamp::new("now"),
                )
                .unwrap()
        };
        let attempt = record();
        assert_eq!(attempt, record());
        assert_eq!(attempt.provenance().operation, Operation::Experimental);
        assert_eq!(attempt.value().step, RevisionStep::DeltaW);
        assert_eq!(attempt.value().outcome, outcome);
        assert_eq!(attempt.value().candidate, DerivedId::new("candidate"));
        assert_eq!(
            attempt.provenance().inputs,
            vec![
                DerivedId::new("candidate"),
                DerivedId::new("experiment"),
                DerivedId::new("trial/counterfactual")
            ]
        );
        assert_eq!(
            attempt.provenance().domain_version,
            Some(DomainVersion::new("1"))
        );
        assert_eq!(
            attempt.provenance().frame_version,
            Some(FrameVersion::new("1"))
        );
    }
}

#[test]
fn delta_w_requires_its_typed_counterfactual_and_null_evidence() {
    let (engine, candidate, counterfactual, experiment) =
        fixture(CandidateKind::WeightRevision, false, None);
    let test = |outcome| {
        engine.record_delta_w_test(
            &candidate,
            &counterfactual,
            &experiment,
            outcome,
            "reviewed",
            "ladder",
            Timestamp::new("now"),
        )
    };
    assert!(test(RevisionTestOutcome::Sufficient).is_err());
    assert!(test(RevisionTestOutcome::Insufficient).is_err());

    let (engine, candidate, counterfactual, experiment) = fixture(
        CandidateKind::WeightRevision,
        true,
        Some(ConstraintStatus::Violated),
    );
    assert!(engine
        .record_delta_w_test(
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Sufficient,
            "reviewed",
            "ladder",
            Timestamp::new("now"),
        )
        .is_err());
    assert!(engine
        .record_delta_w_test(
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "ladder",
            Timestamp::new("now"),
        )
        .is_ok());
}

#[test]
fn delta_w_rejects_larger_revision_kinds_and_unreasoned_verdicts() {
    let (engine, candidate, counterfactual, experiment) =
        fixture(CandidateKind::WeightRevision, true, None);
    assert!(engine
        .record_delta_w_test(
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Sufficient,
            " ",
            "ladder",
            Timestamp::new("now"),
        )
        .is_err());

    let mut relation = proposal(CandidateKind::Relation);
    relation.value["pattern"] = json!({
        "matching": "exact_directed_observed_relation",
        "source_label": "a",
        "target_label": "b",
        "relation_kind": "near"
    });
    let relation = Tracked::from_recorded(DerivedId::new("candidate"), relation);
    assert!(engine
        .record_delta_w_test(
            &relation,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "ladder",
            Timestamp::new("now"),
        )
        .is_err());
}

fn relation_proposal() -> CandidateProposal {
    CandidateProposal {
        domain_version_id: serde_json::to_string(&("d", "1")).unwrap(),
        kind: CandidateKind::Relation,
        value: json!({
            "pattern": {
                "matching": "exact_directed_observed_relation",
                "source_label": "known",
                "target_label": "destination",
                "relation_kind": "near"
            }
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn existing_relation_null() -> NullEvidence {
    NullEvidence {
        id: DerivedId::new("relation-null"),
        model: PluginId::new("null.existing-relation"),
        reading: Reading::Value {
            value: MeasurementValue::Structured(json!({
                "model": "existing_domain_exact_match",
                "match_count": 0,
                "has_existing_alternative": false
            })),
        },
    }
}

fn relation_constraint(status: ConstraintStatus) -> ConstraintAssessment {
    ConstraintAssessment {
        constraint: ExperimentConstraint::ComplexityBudget {
            maximum_added_units: 0,
            maximum_added_relations: 1,
            maximum_property_changes: 0,
        },
        status,
        observed: BTreeMap::from([
            ("added_units".into(), 0),
            ("added_relations".into(), 1),
            ("property_changes".into(), 0),
        ]),
        reading: None,
        transfer: None,
    }
}

fn relation_fixture(
    include_null: bool,
    constraint_status: Option<ConstraintStatus>,
    frame_version: &str,
) -> (
    Engine,
    Tracked<CandidateProposal>,
    unclip_epistemic::Calculated<unclip_engine::CounterfactualSnapshot>,
    unclip_epistemic::Experimental<CounterfactualEvidence>,
) {
    let engine = Engine::with_builtins().unwrap();
    let candidate =
        Tracked::from_recorded(DerivedId::new("relation-candidate"), relation_proposal());
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), domain());
    let counterfactual = engine
        .apply_candidate_with_relation_bindings(
            &baseline,
            &candidate,
            Some(&RelationBindings {
                source: UnitId::new("existing"),
                target: UnitId::new("target"),
            }),
            "relation-trial",
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
        "relation-before",
        "relation-after",
        "relation-comparison",
        "relation-delta",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    if include_null {
        dependencies.read(&Tracked::from_recorded(DerivedId::new("relation-null"), ()));
    }
    let params = json!({"fixture": "relation"});
    let experiment = ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("relation-experiment"),
            producer: PluginId::new("experiment.counterfactual"),
            algorithm: "held_out_counterfactual_comparison".into(),
            version: semver::Version::new(0, 5, 0),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("now"),
            domain_version: Some(DomainVersion::new("1")),
            frame_version: Some(FrameVersion::new(frame_version)),
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
        before: vec![DerivedId::new("relation-before")],
        after: vec![DerivedId::new("relation-after")],
        comparison: DerivedId::new("relation-comparison"),
        delta_profile: comparison("relation-before", "relation-after", "relation-delta"),
        null_results: include_null
            .then(existing_relation_null)
            .into_iter()
            .collect(),
        constraint_assessment: constraint_status
            .as_ref()
            .map(|_| DerivedId::new("relation-constraints")),
        constraints: constraint_status
            .into_iter()
            .map(relation_constraint)
            .collect(),
        transfer_measurements: vec![],
        pareto_assessment: None,
        pareto: None,
    });
    (engine, candidate, counterfactual, experiment)
}

fn delta_w_prior(
    outcome: RevisionTestOutcome,
) -> unclip_epistemic::Experimental<unclip_engine::RevisionAttempt> {
    let (engine, candidate, counterfactual, experiment) =
        fixture(CandidateKind::WeightRevision, true, None);
    engine
        .record_delta_w_test(
            &candidate,
            &counterfactual,
            &experiment,
            outcome,
            "weight evidence was reviewed first",
            "weight-ladder",
            Timestamp::new("now"),
        )
        .unwrap()
}

#[test]
fn delta_e_requires_and_records_an_insufficient_delta_w_attempt() {
    let prior = delta_w_prior(RevisionTestOutcome::Insufficient);
    let (engine, candidate, counterfactual, experiment) =
        relation_fixture(true, Some(ConstraintStatus::Satisfied), "1");
    let record = || {
        engine
            .record_delta_e_test(
                &prior,
                &candidate,
                &counterfactual,
                &experiment,
                RevisionTestOutcome::Sufficient,
                "the relation explains held-out evidence after weight revision failed",
                "relation-ladder",
                Timestamp::new("now"),
            )
            .unwrap()
    };
    let attempt = record();
    assert_eq!(attempt, record());
    assert_eq!(attempt.value().step, RevisionStep::DeltaE);
    assert_eq!(attempt.value().prior, Some(prior.id().clone()));
    assert_eq!(attempt.value().baseline, DerivedId::new("baseline"));
    assert_eq!(attempt.value().frame, DerivedId::new("frame"));
    assert_eq!(attempt.value().split, DerivedId::new("split"));
    assert_eq!(
        attempt.provenance().inputs,
        vec![
            DerivedId::new("relation-candidate"),
            DerivedId::new("relation-experiment"),
            DerivedId::new("relation-trial/counterfactual"),
            DerivedId::new("weight-ladder/revision/delta-w"),
        ]
    );
}

#[test]
fn sufficient_or_different_context_delta_w_attempts_stop_delta_e() {
    let (engine, candidate, counterfactual, experiment) = relation_fixture(true, None, "1");
    let record = |prior| {
        engine.record_delta_e_test(
            prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "relation-ladder",
            Timestamp::new("now"),
        )
    };
    assert!(record(&delta_w_prior(RevisionTestOutcome::Sufficient)).is_err());

    let prior = delta_w_prior(RevisionTestOutcome::Insufficient);
    let (engine, candidate, counterfactual, experiment) = relation_fixture(true, None, "2");
    assert!(engine
        .record_delta_e_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "relation-ladder",
            Timestamp::new("now"),
        )
        .is_err());
}

#[test]
fn delta_e_requires_relation_null_evidence_and_satisfied_constraints() {
    let prior = delta_w_prior(RevisionTestOutcome::Insufficient);
    let (engine, candidate, counterfactual, experiment) = relation_fixture(false, None, "1");
    assert!(engine
        .record_delta_e_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "relation-ladder",
            Timestamp::new("now"),
        )
        .is_err());

    let (engine, candidate, counterfactual, experiment) =
        relation_fixture(true, Some(ConstraintStatus::Violated), "1");
    assert!(engine
        .record_delta_e_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Sufficient,
            "reviewed",
            "relation-ladder",
            Timestamp::new("now"),
        )
        .is_err());
    assert!(engine
        .record_delta_e_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "relation-ladder",
            Timestamp::new("now"),
        )
        .is_ok());
}

fn coupling_proposal() -> CandidateProposal {
    CandidateProposal {
        domain_version_id: serde_json::to_string(&("d", "1")).unwrap(),
        kind: CandidateKind::DynamicCoupling,
        value: json!({
            "pattern": {
                "matching": "thresholded_pairwise_association",
                "metric": "spearman",
                "units": ["existing", "target"]
            },
            "evidence": {
                "measurement": "coupling-matrix",
                "sensor": "sensor.spearman",
                "sensor_version": "0.1.0",
                "context": {"values": {}},
                "cell": {"status": "value", "value": 0.8, "sample_count": 4}
            },
            "selection": {"threshold": 0.7, "minimum_samples": 2},
            "causal_claim": false
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn coupling_zero_null() -> NullEvidence {
    NullEvidence {
        id: DerivedId::new("coupling-null"),
        model: PluginId::new("null.coupling-zero"),
        reading: Reading::Value {
            value: MeasurementValue::Structured(json!({
                "model": "zero_association_baseline",
                "observed_value": 0.8,
                "baseline_value": 0.0,
                "within_tolerance": false,
                "causal_claim": false
            })),
        },
    }
}

fn coupling_constraint(status: ConstraintStatus) -> ConstraintAssessment {
    ConstraintAssessment {
        constraint: ExperimentConstraint::ComplexityBudget {
            maximum_added_units: 1,
            maximum_added_relations: 0,
            maximum_property_changes: 0,
        },
        status,
        observed: BTreeMap::from([
            ("added_units".into(), 1),
            ("added_relations".into(), 0),
            ("property_changes".into(), 0),
        ]),
        reading: None,
        transfer: None,
    }
}

fn coupling_fixture(
    include_null: bool,
    constraint_status: Option<ConstraintStatus>,
) -> (
    Engine,
    Tracked<CandidateProposal>,
    unclip_epistemic::Calculated<unclip_engine::CounterfactualSnapshot>,
    unclip_epistemic::Experimental<CounterfactualEvidence>,
) {
    let engine = Engine::with_builtins().unwrap();
    let candidate =
        Tracked::from_recorded(DerivedId::new("coupling-candidate"), coupling_proposal());
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), domain());
    let counterfactual = engine
        .apply_candidate(
            &baseline,
            &candidate,
            "coupling-trial",
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
        "coupling-before",
        "coupling-after",
        "coupling-comparison",
        "coupling-delta",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    if include_null {
        dependencies.read(&Tracked::from_recorded(DerivedId::new("coupling-null"), ()));
    }
    let params = json!({"fixture": "coupling"});
    let experiment = ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("coupling-experiment"),
            producer: PluginId::new("experiment.counterfactual"),
            algorithm: "held_out_counterfactual_comparison".into(),
            version: semver::Version::new(0, 5, 0),
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
        before: vec![DerivedId::new("coupling-before")],
        after: vec![DerivedId::new("coupling-after")],
        comparison: DerivedId::new("coupling-comparison"),
        delta_profile: comparison("coupling-before", "coupling-after", "coupling-delta"),
        null_results: include_null.then(coupling_zero_null).into_iter().collect(),
        constraint_assessment: constraint_status
            .as_ref()
            .map(|_| DerivedId::new("coupling-constraints")),
        constraints: constraint_status
            .into_iter()
            .map(coupling_constraint)
            .collect(),
        transfer_measurements: vec![],
        pareto_assessment: None,
        pareto: None,
    });
    (engine, candidate, counterfactual, experiment)
}

fn delta_e_prior(
    outcome: RevisionTestOutcome,
) -> unclip_epistemic::Experimental<unclip_engine::RevisionAttempt> {
    let weight = delta_w_prior(RevisionTestOutcome::Insufficient);
    let (engine, candidate, counterfactual, experiment) = relation_fixture(true, None, "1");
    engine
        .record_delta_e_test(
            &weight,
            &candidate,
            &counterfactual,
            &experiment,
            outcome,
            "relation evidence was reviewed after weight revision failed",
            "relation-ladder",
            Timestamp::new("now"),
        )
        .unwrap()
}

#[test]
fn dynamic_coupling_requires_and_records_an_insufficient_delta_e_attempt() {
    let prior = delta_e_prior(RevisionTestOutcome::Insufficient);
    let (engine, candidate, counterfactual, experiment) =
        coupling_fixture(true, Some(ConstraintStatus::Satisfied));
    let record = || {
        engine
            .record_dynamic_coupling_test(
                &prior,
                &candidate,
                &counterfactual,
                &experiment,
                RevisionTestOutcome::Sufficient,
                "non-causal coupling explains held-out evidence after relation revision failed",
                "coupling-ladder",
                Timestamp::new("now"),
            )
            .unwrap()
    };
    let attempt = record();
    assert_eq!(attempt, record());
    assert_eq!(attempt.value().step, RevisionStep::DynamicCoupling);
    assert_eq!(attempt.value().prior, Some(prior.id().clone()));
    assert_eq!(attempt.value().baseline, DerivedId::new("baseline"));
    assert_eq!(
        attempt.provenance().inputs,
        vec![
            DerivedId::new("coupling-candidate"),
            DerivedId::new("coupling-experiment"),
            DerivedId::new("coupling-trial/counterfactual"),
            DerivedId::new("relation-ladder/revision/delta-e"),
        ]
    );
}

#[test]
fn sufficient_or_out_of_order_prior_attempts_stop_dynamic_coupling() {
    let (engine, candidate, counterfactual, experiment) = coupling_fixture(true, None);
    let record = |prior| {
        engine.record_dynamic_coupling_test(
            prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "coupling-ladder",
            Timestamp::new("now"),
        )
    };
    let sufficient_delta_e = delta_e_prior(RevisionTestOutcome::Sufficient);
    assert!(record(&sufficient_delta_e).is_err());
    let out_of_order_delta_w = delta_w_prior(RevisionTestOutcome::Insufficient);
    assert!(record(&out_of_order_delta_w).is_err());
}

#[test]
fn dynamic_coupling_requires_its_null_and_satisfied_constraints() {
    let prior = delta_e_prior(RevisionTestOutcome::Insufficient);
    let (engine, candidate, counterfactual, experiment) = coupling_fixture(false, None);
    assert!(engine
        .record_dynamic_coupling_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "coupling-ladder",
            Timestamp::new("now"),
        )
        .is_err());

    let (engine, candidate, counterfactual, experiment) =
        coupling_fixture(true, Some(ConstraintStatus::Violated));
    assert!(engine
        .record_dynamic_coupling_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Sufficient,
            "reviewed",
            "coupling-ladder",
            Timestamp::new("now"),
        )
        .is_err());
    assert!(engine
        .record_dynamic_coupling_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "coupling-ladder",
            Timestamp::new("now"),
        )
        .is_ok());
}
