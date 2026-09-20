use serde_json::json;
use std::collections::BTreeMap;
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainId, DomainSnapshot, Relation, RelationId, Unit, UnitId,
    UnitKind,
};
use unclip_engine::{Engine, MeasurementRun, NullInputs};
use unclip_epistemic::{Calculated, DerivedId, DomainVersion, Timestamp, Tracked};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};
fn domain() -> DomainSnapshot {
    let units = [
        ("a", "left", UnitKind::AtomicMeaning),
        ("b", "right", UnitKind::AtomicMeaning),
        ("c", "left", UnitKind::AtomicMeaning),
        ("d", "left", UnitKind::CompositeMeaning),
    ]
    .into_iter()
    .map(|(id, label, kind)| {
        (
            UnitId::new(id),
            Unit {
                id: UnitId::new(id),
                label: Some(label.into()),
                kind,
                properties: BTreeMap::new(),
            },
        )
    })
    .collect();
    let relations = [
        ("r1", "a", "b", "near"),
        ("r2", "c", "b", "near"),
        ("reverse", "b", "a", "near"),
        ("different", "a", "b", "other"),
    ]
    .into_iter()
    .map(|(id, source, target, kind)| {
        (
            RelationId::new(id),
            Relation {
                id: RelationId::new(id),
                source: UnitId::new(source),
                target: UnitId::new(target),
                kind: kind.into(),
                properties: BTreeMap::new(),
            },
        )
    })
    .collect();
    DomainSnapshot {
        id: DomainId::new("d"),
        version: DomainVersion::new("1"),
        units,
        relations,
    }
}
fn candidate(relation: bool) -> CandidateProposal {
    CandidateProposal {domain_version_id:serde_json::to_string(&("d","1")).unwrap(),kind:if relation {CandidateKind::Relation} else {CandidateKind::AtomicMeaning},value:if relation {json!({"pattern":{"matching":"exact_directed_observed_relation","source_label":"left","target_label":"right","relation_kind":"near"}})} else {json!({"pattern":{"matching":"exact_observed_label","observed_label":"left"}})}.as_object().unwrap().clone()}
}
fn evaluate(
    id: &str,
    c: CandidateProposal,
    d: Option<DomainSnapshot>,
) -> unclip_plugin::Result<Vec<Calculated<Reading>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            null_models: vec![PluginSelection::any(id)],
            ..Default::default()
        })
        .unwrap();
    let domain = d.map(|d| Tracked::from_recorded(DerivedId::new("domain"), d));
    engine.evaluate_null_models_with_inputs(
        &plan,
        &Tracked::from_recorded(DerivedId::new("candidate"), c),
        NullInputs {
            domain: domain.as_ref(),
            ..Default::default()
        },
        MeasurementRun {
            id: "null",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::new(),
        },
    )
}
fn value(results: &[Calculated<Reading>]) -> &serde_json::Value {
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = results[0].value()
    else {
        panic!("expected structured match result")
    };
    value
}
#[test]
fn preserves_all_existing_identities_and_exact_direction_kind_and_unit_kind() {
    for (plugin, relation, key, ids) in [
        ("null.existing-unit", false, "unit", ["a", "c"]),
        ("null.existing-relation", true, "relation", ["r1", "r2"]),
    ] {
        let results = evaluate(plugin, candidate(relation), Some(domain())).unwrap();
        assert_eq!(value(&results)["match_count"], 2);
        assert_eq!(value(&results)["matches"][0][key], ids[0]);
        assert_eq!(value(&results)["matches"][1][key], ids[1]);
        assert_eq!(
            results[0].provenance().inputs,
            vec![DerivedId::new("candidate"), DerivedId::new("domain")]
        );
        assert_eq!(
            evaluate(plugin, candidate(relation), Some(domain())).unwrap(),
            results
        );
    }
    let mut c = candidate(false);
    c.value["pattern"]["observed_label"] = json!("Left");
    let results = evaluate("null.existing-unit", c, Some(domain())).unwrap();
    assert_eq!(value(&results)["match_count"], 0);
    assert_eq!(value(&results)["has_existing_alternative"], false);
}
#[test]
fn missing_baselines_and_unsupported_candidates_are_distinct_from_zero_matches() {
    let results = evaluate("null.existing-unit", candidate(false), None).unwrap();
    assert_eq!(
        *results[0].value(),
        Reading::InsufficientEvidence { have: 0, need: 1 }
    );
    assert_eq!(
        results[0].provenance().inputs,
        vec![DerivedId::new("candidate")]
    );
    let results = evaluate("null.existing-unit", candidate(true), Some(domain())).unwrap();
    assert!(matches!(results[0].value(), Reading::NotApplicable { .. }));
    assert_eq!(
        results[0].provenance().inputs,
        vec![DerivedId::new("candidate")]
    );
}
#[test]
fn rejects_wrong_versions_dangling_endpoints_and_inconsistent_keys() {
    let mut wrong = domain();
    wrong.version = DomainVersion::new("2");
    let mut dangling = domain();
    dangling.units.remove(&UnitId::new("b"));
    let mut inconsistent = domain();
    inconsistent.units.get_mut(&UnitId::new("a")).unwrap().id = UnitId::new("wrong");
    for domain in [wrong, dangling, inconsistent] {
        assert!(evaluate("null.existing-relation", candidate(true), Some(domain)).is_err());
    }
    let mut c = candidate(false);
    c.value["pattern"]["observed_label"] = json!("");
    assert!(evaluate("null.existing-unit", c, None).is_err());
}
