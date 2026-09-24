use std::collections::BTreeMap;

use serde_json::json;
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainId, DomainSnapshot, Unit, UnitId, UnitKind,
};
use unclip_engine::{
    ComparisonPair, ConstraintAssessment, ConstraintStatus, CounterfactualEvidence, DeltaProfile,
    Engine, ExperimentConstraint, NullEvidence, ProfileDelta, RevisionAttempt, RevisionStep,
    RevisionTestOutcome,
};
use unclip_epistemic::{
    hash_params, DependencyCollector, DerivedId, DomainVersion, EmitMetadata, ExperimentToken,
    Experimental, FrameVersion, Operation, PluginId, Timestamp, Tracked,
};
use unclip_measure::{Delta, MeasurementValue, Reading};

fn domain() -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new("d"),
        version: DomainVersion::new("1"),
        units: BTreeMap::from([(
            UnitId::new("known"),
            Unit {
                id: UnitId::new("known"),
                kind: UnitKind::AtomicMeaning,
                label: Some("known".into()),
                properties: BTreeMap::new(),
            },
        )]),
        relations: BTreeMap::new(),
    }
}

fn proposal() -> CandidateProposal {
    CandidateProposal {
        domain_version_id: serde_json::to_string(&("d", "1")).unwrap(),
        kind: CandidateKind::AtomicMeaning,
        value: json!({
            "pattern": {
                "matching": "exact_observed_label",
                "observed_label": "novel"
            },
            "observation_count": 2,
            "observations": ["o1", "o2"],
            "examples": [
                {
                    "observation": "o1",
                    "unit": "u1",
                    "measurements": ["residual-1"]
                },
                {
                    "observation": "o2",
                    "unit": "u2",
                    "measurements": ["residual-2"]
                }
            ]
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn prior(outcome: RevisionTestOutcome, step: RevisionStep) -> Experimental<RevisionAttempt> {
    let dependencies = DependencyCollector::default();
    for id in [
        "motif-candidate",
        "motif-counterfactual",
        "motif-experiment",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    let params = json!({"fixture": "structural-prior"});
    ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("structural-ladder/revision/structural"),
            producer: PluginId::new("experiment.revision-ladder"),
            algorithm: "minimal_revision_structural".into(),
            version: semver::Version::new(0, 1, 0),
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
        step,
        prior: Some(DerivedId::new("coupling-ladder/revision/dynamic-coupling")),
        candidate: DerivedId::new("motif-candidate"),
        counterfactual: DerivedId::new("motif-counterfactual"),
        experiment: DerivedId::new("motif-experiment"),
        baseline: DerivedId::new("baseline"),
        frame: DerivedId::new("frame"),
        split: DerivedId::new("split"),
        outcome,
        reason: "structural evidence was reviewed".into(),
    })
}

fn existing_unit_null() -> NullEvidence {
    NullEvidence {
        id: DerivedId::new("unit-null"),
        model: PluginId::new("null.existing-unit"),
        reading: Reading::Value {
            value: MeasurementValue::Structured(json!({
                "model": "existing_domain_exact_match",
                "matching": "exact_observed_label",
                "match_count": 0,
                "has_existing_alternative": false
            })),
        },
    }
}

fn constraint(status: ConstraintStatus) -> ConstraintAssessment {
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

fn comparison() -> DeltaProfile {
    let pair = ComparisonPair {
        before: DerivedId::new("atomic-before"),
        after: DerivedId::new("atomic-after"),
    };
    DeltaProfile {
        pairs: vec![pair.clone()],
        deltas: vec![ProfileDelta {
            pair,
            id: DerivedId::new("atomic-delta"),
            delta: Delta {
                comparator: PluginId::new("compare.scalar-difference"),
                value: MeasurementValue::Scalar(0.5),
            },
        }],
        unmatched_before: vec![],
        unmatched_after: vec![],
    }
}

fn fixture(
    prior: &Experimental<RevisionAttempt>,
    include_null: bool,
    constraint_status: Option<ConstraintStatus>,
) -> (
    Engine,
    Tracked<CandidateProposal>,
    unclip_epistemic::Calculated<unclip_engine::CounterfactualSnapshot>,
    Experimental<CounterfactualEvidence>,
) {
    let engine = Engine::with_builtins().unwrap();
    let candidate = Tracked::from_recorded(DerivedId::new("atomic-candidate"), proposal());
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), domain());
    let counterfactual = engine
        .apply_delta_v_candidate(
            prior,
            &baseline,
            &candidate,
            "atomic-trial",
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
        "atomic-before",
        "atomic-after",
        "atomic-comparison",
        "atomic-delta",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    if include_null {
        dependencies.read(&Tracked::from_recorded(DerivedId::new("unit-null"), ()));
    }

    let params = json!({"fixture": "atomic"});
    let experiment = ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("atomic-experiment"),
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
        before: vec![DerivedId::new("atomic-before")],
        after: vec![DerivedId::new("atomic-after")],
        comparison: DerivedId::new("atomic-comparison"),
        delta_profile: comparison(),
        null_results: include_null.then(existing_unit_null).into_iter().collect(),
        constraint_assessment: constraint_status
            .as_ref()
            .map(|_| DerivedId::new("atomic-constraints")),
        constraints: constraint_status.into_iter().map(constraint).collect(),
        transfer_measurements: vec![],
        pareto_assessment: None,
        pareto: None,
    });
    (engine, candidate, counterfactual, experiment)
}

#[test]
fn delta_v_records_the_ordered_atomic_membership_attempt() {
    let prior = prior(RevisionTestOutcome::Insufficient, RevisionStep::Structural);
    let (engine, candidate, counterfactual, experiment) =
        fixture(&prior, true, Some(ConstraintStatus::Satisfied));

    let record = || {
        engine
            .record_delta_v_test(
                &prior,
                &candidate,
                &counterfactual,
                &experiment,
                RevisionTestOutcome::Sufficient,
                "persistent held-out residual evidence requires one new anonymous unit",
                "atomic-ladder",
                Timestamp::new("now"),
            )
            .unwrap()
    };
    let attempt = record();
    assert_eq!(attempt, record());
    assert_eq!(
        counterfactual.provenance().params["revision_step"],
        "delta_v"
    );
    assert_eq!(attempt.provenance().operation, Operation::Experimental);
    assert_eq!(attempt.value().step, RevisionStep::DeltaV);
    assert_eq!(attempt.value().prior, Some(prior.id().clone()));
    assert_eq!(
        attempt.provenance().inputs,
        vec![
            DerivedId::new("atomic-candidate"),
            DerivedId::new("atomic-experiment"),
            DerivedId::new("atomic-trial/counterfactual"),
            DerivedId::new("structural-ladder/revision/structural"),
        ]
    );
}

#[test]
fn sufficient_or_out_of_order_structural_attempt_stops_delta_v_before_application() {
    let engine = Engine::with_builtins().unwrap();
    let candidate = Tracked::from_recorded(DerivedId::new("atomic-candidate"), proposal());
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), domain());
    let sufficient = prior(RevisionTestOutcome::Sufficient, RevisionStep::Structural);
    let error = engine
        .apply_delta_v_candidate(
            &sufficient,
            &baseline,
            &candidate,
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("insufficient structural attempt"));
    let out_of_order = prior(
        RevisionTestOutcome::Insufficient,
        RevisionStep::DynamicCoupling,
    );
    assert!(engine
        .apply_delta_v_candidate(
            &out_of_order,
            &baseline,
            &candidate,
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .is_err());
    let mut non_atomic = proposal();
    non_atomic.kind = CandidateKind::CompositeMeaning;
    assert!(engine
        .apply_delta_v_candidate(
            &prior(RevisionTestOutcome::Insufficient, RevisionStep::Structural),
            &baseline,
            &Tracked::from_recorded(DerivedId::new("atomic-candidate"), non_atomic),
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .is_err());

    let prior = prior(RevisionTestOutcome::Insufficient, RevisionStep::Structural);
    let (engine, candidate, _, experiment) = fixture(&prior, true, None);
    let unauthorized = engine
        .apply_candidate(
            &Tracked::from_recorded(DerivedId::new("baseline"), domain()),
            &candidate,
            "atomic-trial",
            Timestamp::new("now"),
        )
        .unwrap();
    assert!(engine
        .record_delta_v_test(
            &prior,
            &candidate,
            &unauthorized,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .is_err());
}

#[test]
fn delta_v_requires_complete_residual_null_and_constraint_evidence() {
    let prior = prior(RevisionTestOutcome::Insufficient, RevisionStep::Structural);
    let (engine, candidate, counterfactual, experiment) = fixture(&prior, false, None);
    assert!(engine
        .record_delta_v_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .is_err());

    let (engine, candidate, counterfactual, experiment) =
        fixture(&prior, true, Some(ConstraintStatus::Violated));
    assert!(engine
        .record_delta_v_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Sufficient,
            "reviewed",
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .is_err());
    assert!(engine
        .record_delta_v_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .is_ok());

    let mut incomplete = proposal();
    incomplete.value["examples"][0]["measurements"] = json!([]);
    assert!(engine
        .record_delta_v_test(
            &prior,
            &Tracked::from_recorded(DerivedId::new("atomic-candidate"), incomplete),
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .is_err());

    let mut unsupported = proposal();
    unsupported.kind = CandidateKind::CompositeMeaning;
    assert!(engine
        .record_delta_v_test(
            &prior,
            &Tracked::from_recorded(DerivedId::new("atomic-candidate"), unsupported),
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "atomic-ladder",
            Timestamp::new("now"),
        )
        .is_err());
}
