use std::collections::BTreeMap;

use serde_json::json;
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainId, DomainSnapshot, PropertyValue, Unit, UnitId,
    UnitKind,
};
use unclip_engine::{
    ComparisonPair, ConstraintAssessment, ConstraintStatus, CounterfactualEvidence, DeltaProfile,
    Engine, ExperimentConstraint, NullEvidence, ProfileDelta, RevisionStep, RevisionTestOutcome,
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
        units: BTreeMap::from([(
            UnitId::new("existing"),
            Unit {
                id: UnitId::new("existing"),
                kind: UnitKind::AtomicMeaning,
                label: Some("known".into()),
                properties: BTreeMap::from([("weight".into(), PropertyValue::Number(0.5))]),
            },
        )]),
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

fn comparison() -> DeltaProfile {
    let pair = ComparisonPair {
        before: DerivedId::new("before"),
        after: DerivedId::new("after"),
    };
    DeltaProfile {
        pairs: vec![pair.clone()],
        deltas: vec![ProfileDelta {
            pair,
            id: DerivedId::new("delta"),
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
        delta_profile: comparison(),
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
