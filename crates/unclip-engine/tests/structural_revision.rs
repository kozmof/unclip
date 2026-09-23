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
    Experimental, FrameVersion, PluginId, Timestamp, Tracked,
};
use unclip_measure::{Delta, MeasurementValue, Reading};

fn domain() -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new("d"),
        version: DomainVersion::new("1"),
        units: BTreeMap::from([(
            UnitId::new("existing"),
            Unit {
                id: UnitId::new("existing"),
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

fn prior(outcome: RevisionTestOutcome, step: RevisionStep) -> Experimental<RevisionAttempt> {
    let dependencies = DependencyCollector::default();
    for id in [
        "coupling-candidate",
        "coupling-counterfactual",
        "coupling-experiment",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    let params = json!({"fixture": "prior"});
    ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("coupling-ladder/revision/dynamic-coupling"),
            producer: PluginId::new("experiment.revision-ladder"),
            algorithm: "minimal_revision_dynamic_coupling".into(),
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
        prior: Some(DerivedId::new("relation-ladder/revision/delta-e")),
        candidate: DerivedId::new("coupling-candidate"),
        counterfactual: DerivedId::new("coupling-counterfactual"),
        experiment: DerivedId::new("coupling-experiment"),
        baseline: DerivedId::new("baseline"),
        frame: DerivedId::new("frame"),
        split: DerivedId::new("split"),
        outcome,
        reason: "dynamic coupling evidence was reviewed".into(),
    })
}

fn motif_null() -> NullEvidence {
    NullEvidence {
        id: DerivedId::new("motif-null"),
        model: PluginId::new("null.existing-motif"),
        reading: Reading::Value {
            value: MeasurementValue::Structured(json!({
                "model": "existing_motif_exact_pattern",
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
        before: DerivedId::new("motif-before"),
        after: DerivedId::new("motif-after"),
    };
    DeltaProfile {
        pairs: vec![pair.clone()],
        deltas: vec![ProfileDelta {
            pair,
            id: DerivedId::new("motif-delta"),
            delta: Delta {
                comparator: PluginId::new("compare.scalar-difference"),
                value: MeasurementValue::Scalar(0.25),
            },
        }],
        unmatched_before: vec![],
        unmatched_after: vec![],
    }
}

fn fixture(
    include_null: bool,
    constraint_status: Option<ConstraintStatus>,
) -> (
    Engine,
    Tracked<CandidateProposal>,
    unclip_epistemic::Calculated<unclip_engine::CounterfactualSnapshot>,
    Experimental<CounterfactualEvidence>,
) {
    let engine = Engine::with_builtins().unwrap();
    let candidate = Tracked::from_recorded(DerivedId::new("motif-candidate"), proposal());
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), domain());
    let counterfactual = engine
        .apply_candidate(&baseline, &candidate, "motif-trial", Timestamp::new("now"))
        .unwrap();
    let dependencies = DependencyCollector::default();
    dependencies.read(&candidate);
    dependencies.read(&Tracked::from(&counterfactual));
    for id in [
        "baseline",
        "frame",
        "split",
        "motif-before",
        "motif-after",
        "motif-comparison",
        "motif-delta",
    ] {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(id), ()));
    }
    if include_null {
        dependencies.read(&Tracked::from_recorded(DerivedId::new("motif-null"), ()));
    }
    let params = json!({"fixture": "motif"});
    let experiment = ExperimentToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("motif-experiment"),
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
        before: vec![DerivedId::new("motif-before")],
        after: vec![DerivedId::new("motif-after")],
        comparison: DerivedId::new("motif-comparison"),
        delta_profile: comparison(),
        null_results: include_null.then(motif_null).into_iter().collect(),
        constraint_assessment: constraint_status
            .as_ref()
            .map(|_| DerivedId::new("motif-constraints")),
        constraints: constraint_status.into_iter().map(constraint).collect(),
        transfer_measurements: vec![],
        pareto_assessment: None,
        pareto: None,
    });
    (engine, candidate, counterfactual, experiment)
}

#[test]
fn graph_motif_records_the_ordered_structural_attempt() {
    let prior = prior(
        RevisionTestOutcome::Insufficient,
        RevisionStep::DynamicCoupling,
    );
    let (engine, candidate, counterfactual, experiment) =
        fixture(true, Some(ConstraintStatus::Satisfied));
    let record = || {
        engine
            .record_structural_test(
                &prior,
                &candidate,
                &counterfactual,
                &experiment,
                RevisionTestOutcome::Sufficient,
                "the recurring motif explains held-out evidence after coupling failed",
                "structural-ladder",
                Timestamp::new("now"),
            )
            .unwrap()
    };
    let attempt = record();
    assert_eq!(attempt, record());
    assert_eq!(attempt.value().step, RevisionStep::Structural);
    assert_eq!(attempt.value().prior, Some(prior.id().clone()));
    assert_eq!(
        attempt.provenance().inputs,
        vec![
            DerivedId::new("coupling-ladder/revision/dynamic-coupling"),
            DerivedId::new("motif-candidate"),
            DerivedId::new("motif-experiment"),
            DerivedId::new("motif-trial/counterfactual"),
        ]
    );
}

#[test]
fn structural_step_rejects_sufficient_or_out_of_order_prior_attempts() {
    let (engine, candidate, counterfactual, experiment) = fixture(true, None);
    for prior in [
        prior(
            RevisionTestOutcome::Sufficient,
            RevisionStep::DynamicCoupling,
        ),
        prior(RevisionTestOutcome::Insufficient, RevisionStep::DeltaE),
    ] {
        assert!(engine
            .record_structural_test(
                &prior,
                &candidate,
                &counterfactual,
                &experiment,
                RevisionTestOutcome::Insufficient,
                "reviewed",
                "structural-ladder",
                Timestamp::new("now"),
            )
            .is_err());
    }
}

#[test]
fn structural_step_requires_motif_null_constraints_and_supported_kind() {
    let prior = prior(
        RevisionTestOutcome::Insufficient,
        RevisionStep::DynamicCoupling,
    );
    let (engine, candidate, counterfactual, experiment) = fixture(false, None);
    assert!(engine
        .record_structural_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "structural-ladder",
            Timestamp::new("now"),
        )
        .is_err());

    let (engine, candidate, counterfactual, experiment) =
        fixture(true, Some(ConstraintStatus::Violated));
    assert!(engine
        .record_structural_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Sufficient,
            "reviewed",
            "structural-ladder",
            Timestamp::new("now"),
        )
        .is_err());
    assert!(engine
        .record_structural_test(
            &prior,
            &candidate,
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "structural-ladder",
            Timestamp::new("now"),
        )
        .is_ok());

    let mut unsupported = proposal();
    unsupported.kind = CandidateKind::SemanticRole;
    assert!(engine
        .record_structural_test(
            &prior,
            &Tracked::from_recorded(DerivedId::new("motif-candidate"), unsupported),
            &counterfactual,
            &experiment,
            RevisionTestOutcome::Insufficient,
            "reviewed",
            "structural-ladder",
            Timestamp::new("now"),
        )
        .is_err());
}
