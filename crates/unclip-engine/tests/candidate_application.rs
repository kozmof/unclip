use serde_json::json;
use std::collections::BTreeMap;
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainId, DomainSnapshot, PropertyValue, Unit, UnitId,
    UnitKind,
};
use unclip_engine::Engine;
use unclip_epistemic::{DependencyCollector, DerivedId, DomainVersion, Timestamp, Tracked};
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
    CandidateProposal {domain_version_id:serde_json::to_string(&("d","1")).unwrap(),kind:CandidateKind::AtomicMeaning,value:json!({"pattern":{"matching":"exact_observed_label","observed_label":"new evidence"},"observation_count":2,"examples":[{"observation":"o1"},{"observation":"o2"}]}).as_object().unwrap().clone()}
}
fn apply(
    domain: DomainSnapshot,
    candidate: CandidateProposal,
) -> unclip_plugin::Result<unclip_epistemic::Calculated<unclip_engine::CounterfactualSnapshot>> {
    Engine::with_builtins().unwrap().apply_candidate(
        &Tracked::from_recorded(DerivedId::new("baseline"), domain),
        &Tracked::from_recorded(DerivedId::new("proposal"), candidate),
        "trial",
        Timestamp::new("now"),
    )
}
#[test]
fn atomic_application_is_anonymous_tracked_replayable_and_leaves_baseline_intact() {
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), domain());
    let candidate = Tracked::from_recorded(DerivedId::new("proposal"), proposal());
    let engine = Engine::with_builtins().unwrap();
    let result = engine
        .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
        .unwrap();
    assert_eq!(DependencyCollector::default().read(&baseline), &domain());
    assert_eq!(DependencyCollector::default().read(&candidate), &proposal());
    assert_eq!(
        result.value().domain.version,
        DomainVersion::new("counterfactual:trial")
    );
    assert_eq!(result.value().domain.units.len(), 2);
    assert_eq!(
        result.value().domain.units[&UnitId::new("existing")],
        domain().units[&UnitId::new("existing")]
    );
    let added = &result.value().domain.units[&UnitId::new("candidate:proposal")];
    assert_eq!(added.label, None);
    assert_eq!(
        added.properties["candidate_pattern"],
        PropertyValue::Structured(proposal().value["pattern"].clone())
    );
    assert_eq!(
        added.properties["candidate_evidence"],
        PropertyValue::Structured(serde_json::Value::Object(proposal().value))
    );
    assert_eq!(
        result.provenance().inputs,
        vec![DerivedId::new("baseline"), DerivedId::new("proposal")]
    );
    assert_eq!(
        result,
        engine
            .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
            .unwrap()
    );
}
#[test]
fn wrong_baselines_unsupported_kinds_invalid_patterns_and_collisions_are_errors() {
    let mut wrong = proposal();
    wrong.domain_version_id = "different".into();
    assert!(apply(domain(), wrong).is_err());
    let mut unsupported = proposal();
    unsupported.kind = CandidateKind::Relation;
    assert!(apply(domain(), unsupported).is_err());
    for pattern in [
        json!({"matching":"other","observed_label":"x"}),
        json!({"matching":"exact_observed_label","observed_label":" "}),
        json!({"matching":"exact_observed_label","observed_label":"x","semantic_label":"invented"}),
    ] {
        let mut candidate = proposal();
        candidate.value["pattern"] = pattern;
        assert!(apply(domain(), candidate).is_err());
    }
    let mut collision = domain();
    let id = UnitId::new("candidate:proposal");
    collision.units.insert(
        id.clone(),
        Unit {
            id,
            kind: UnitKind::AtomicMeaning,
            label: None,
            properties: BTreeMap::new(),
        },
    );
    assert!(apply(collision, proposal()).is_err());
    let baseline = Tracked::from_recorded(DerivedId::new("trial/counterfactual"), domain());
    let candidate = Tracked::from_recorded(DerivedId::new("proposal"), proposal());
    assert!(Engine::with_builtins()
        .unwrap()
        .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
        .is_err());
    assert!(Engine::with_builtins()
        .unwrap()
        .apply_candidate(&baseline, &candidate, "", Timestamp::new("now"))
        .is_err());
}
