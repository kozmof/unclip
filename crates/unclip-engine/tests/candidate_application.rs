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

fn weighted_domain() -> DomainSnapshot {
    let mut d = domain();
    d.units
        .get_mut(&UnitId::new("existing"))
        .unwrap()
        .properties
        .insert("weight".into(), PropertyValue::Integer(2));
    let id = unclip_domain::RelationId::new("r");
    d.relations.insert(
        id.clone(),
        unclip_domain::Relation {
            id,
            source: UnitId::new("existing"),
            target: UnitId::new("existing"),
            kind: "self".into(),
            properties: BTreeMap::from([("weight".into(), PropertyValue::Number(0.5))]),
        },
    );
    d
}
fn weight_proposal(kind: &str, id: &str, value: serde_json::Value) -> CandidateProposal {
    let mut c = proposal();
    c.kind = CandidateKind::WeightRevision;
    c.value["pattern"] = json!({"matching":"numeric_property_revision","target":{"kind":kind,"id":id},"property":"weight","proposed_value":value});
    c
}
#[test]
fn weight_revisions_preserve_types_record_changes_and_leave_baseline_intact() {
    for (kind, id, value, expected) in [
        ("unit", "existing", json!(-1), PropertyValue::Integer(-1)),
        ("relation", "r", json!(0.0), PropertyValue::Number(0.0)),
    ] {
        let baseline = Tracked::from_recorded(DerivedId::new("baseline"), weighted_domain());
        let proposal = weight_proposal(kind, id, value);
        let candidate = Tracked::from_recorded(DerivedId::new("proposal"), proposal.clone());
        let engine = Engine::with_builtins().unwrap();
        let result = engine
            .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
            .unwrap();
        assert_eq!(
            DependencyCollector::default().read(&baseline),
            &weighted_domain()
        );
        assert!(result.value().added_units.is_empty());
        assert_eq!(result.value().property_changes.len(), 1);
        let change = &result.value().property_changes[0];
        assert_eq!(change.after, expected);
        let stored = if kind == "unit" {
            &result.value().domain.units[&UnitId::new(id)].properties["weight"]
        } else {
            &result.value().domain.relations[&unclip_domain::RelationId::new(id)].properties
                ["weight"]
        };
        assert_eq!(stored, &expected);
        assert_eq!(
            change.before,
            if kind == "unit" {
                PropertyValue::Integer(2)
            } else {
                PropertyValue::Number(0.5)
            }
        );
        assert_eq!(
            result.provenance().inputs,
            vec![DerivedId::new("baseline"), DerivedId::new("proposal")]
        );
        assert_eq!(result, apply(weighted_domain(), proposal).unwrap());
    }
}
#[test]
fn weight_application_rejects_missing_or_invalid_numeric_evidence() {
    for value in [
        json!(true),
        json!("1"),
        json!(null),
        json!(9_007_199_254_740_993u64),
    ] {
        assert!(apply(
            weighted_domain(),
            weight_proposal("unit", "existing", value)
        )
        .is_err());
    }
    assert!(apply(
        weighted_domain(),
        weight_proposal("unit", "missing", json!(1))
    )
    .is_err());
    assert!(apply(domain(), weight_proposal("unit", "existing", json!(1))).is_err());
    let mut invalid = weighted_domain();
    invalid
        .units
        .get_mut(&UnitId::new("existing"))
        .unwrap()
        .properties
        .insert("weight".into(), PropertyValue::Number(f64::NAN));
    assert!(apply(invalid, weight_proposal("unit", "existing", json!(1))).is_err());
}

fn relation_domain() -> DomainSnapshot {
    let mut d = domain();
    let id = UnitId::new("target");
    d.units.insert(
        id.clone(),
        Unit {
            id,
            kind: UnitKind::AtomicMeaning,
            label: Some("destination".into()),
            properties: BTreeMap::new(),
        },
    );
    d
}
fn relation_proposal() -> CandidateProposal {
    let mut c = proposal();
    c.kind = CandidateKind::Relation;
    c.value["pattern"] = json!({"matching":"exact_directed_observed_relation","source_label":"known","target_label":"destination","relation_kind":"near"});
    c
}
fn apply_relation(
    d: DomainSnapshot,
    c: CandidateProposal,
    source: &str,
    target: &str,
) -> unclip_plugin::Result<unclip_epistemic::Calculated<unclip_engine::CounterfactualSnapshot>> {
    Engine::with_builtins()
        .unwrap()
        .apply_candidate_with_relation_bindings(
            &Tracked::from_recorded(DerivedId::new("baseline"), d),
            &Tracked::from_recorded(DerivedId::new("proposal"), c),
            Some(&unclip_engine::RelationBindings {
                source: UnitId::new(source),
                target: UnitId::new(target),
            }),
            "trial",
            Timestamp::new("now"),
        )
}
#[test]
fn relation_application_records_explicit_endpoints_and_preserves_baseline() {
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), relation_domain());
    let candidate = Tracked::from_recorded(DerivedId::new("proposal"), relation_proposal());
    let binding = unclip_engine::RelationBindings {
        source: UnitId::new("existing"),
        target: UnitId::new("target"),
    };
    let result = Engine::with_builtins()
        .unwrap()
        .apply_candidate_with_relation_bindings(
            &baseline,
            &candidate,
            Some(&binding),
            "trial",
            Timestamp::new("now"),
        )
        .unwrap();
    assert_eq!(
        DependencyCollector::default().read(&baseline),
        &relation_domain()
    );
    assert!(result.value().added_units.is_empty() && result.value().property_changes.is_empty());
    let id = unclip_domain::RelationId::new("candidate:proposal");
    assert_eq!(result.value().added_relations, vec![id.clone()]);
    let relation = &result.value().domain.relations[&id];
    assert_eq!(relation.source, binding.source);
    assert_eq!(relation.target, binding.target);
    assert_eq!(relation.kind, "near");
    assert_eq!(
        relation.properties["candidate_evidence"],
        PropertyValue::Structured(serde_json::Value::Object(relation_proposal().value))
    );
    assert_eq!(
        result.provenance().params["relation_bindings"],
        json!({"source":"existing","target":"target"})
    );
    assert_eq!(
        result,
        apply_relation(relation_domain(), relation_proposal(), "existing", "target").unwrap()
    );
}
#[test]
fn relation_application_requires_valid_bindings_and_rejects_existing_edges() {
    assert!(apply(relation_domain(), relation_proposal()).is_err());
    for (source, target) in [("missing", "target"), ("target", "existing")] {
        assert!(apply_relation(relation_domain(), relation_proposal(), source, target).is_err());
    }
    assert!(apply_relation(relation_domain(), proposal(), "existing", "target").is_err());
    let mut duplicate = relation_domain();
    let id = unclip_domain::RelationId::new("existing-edge");
    duplicate.relations.insert(
        id.clone(),
        unclip_domain::Relation {
            id,
            source: UnitId::new("existing"),
            target: UnitId::new("target"),
            kind: "near".into(),
            properties: BTreeMap::new(),
        },
    );
    assert!(apply_relation(duplicate.clone(), relation_proposal(), "existing", "target").is_err());
    duplicate
        .relations
        .get_mut(&unclip_domain::RelationId::new("existing-edge"))
        .unwrap()
        .kind = "other".into();
    assert!(apply_relation(duplicate, relation_proposal(), "existing", "target").is_ok());
    let mut ambiguous = relation_domain();
    let id = UnitId::new("other-source");
    ambiguous.units.insert(
        id.clone(),
        Unit {
            id,
            kind: UnitKind::AtomicMeaning,
            label: Some("known".into()),
            properties: BTreeMap::new(),
        },
    );
    let result = apply_relation(ambiguous, relation_proposal(), "other-source", "target").unwrap();
    assert_eq!(
        result.value().domain.relations[&unclip_domain::RelationId::new("candidate:proposal")]
            .source,
        UnitId::new("other-source")
    );
}
