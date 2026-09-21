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

fn community_proposal() -> CandidateProposal {
    let mut c = proposal();
    c.kind = CandidateKind::CompositeMeaning;
    c.value=json!({
        "pattern":{"matching":"empirical_community","members":["existing","target"]},
        "evidence":{"structure":"community-result","community_index":0,"result":{"metric":"spearman","threshold":0.8,"minimum_samples":2,"communities":[["existing","target"]],"assessed_pairs":1,"qualifying_pairs":1,"unassessed":[]}},
        "selection":{"metric":"spearman","minimum_samples":2,"minimum_members":2}
    }).as_object().unwrap().clone();
    c
}
#[test]
fn generated_community_applies_as_anonymous_composite_with_explicit_members() {
    use unclip_engine::{CandidateInputs, MeasurementRun};
    use unclip_epistemic::PluginId;
    use unclip_plugin::{EngineProfile, PluginSelection};
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any("generate.community")],
            ..Default::default()
        })
        .unwrap();
    let template = community_proposal();
    let structures = [Tracked::from_recorded(
        DerivedId::new("community-result"),
        unclip_measure::EmpiricalStructure {
            kind: "communities".into(),
            value: template.value["evidence"]["result"].clone(),
        },
    )];
    let candidates = engine
        .generate_candidates(
            &plan,
            CandidateInputs {
                domain_version_id: &template.domain_version_id,
                measurements: &[],
                observations: &[],
                structures: &structures,
            },
            MeasurementRun {
                id: "generation",
                timestamp: Timestamp::new("now"),
                params: &BTreeMap::from([(
                    PluginId::new("generate.community"),
                    json!({"metric":"spearman","minimum_samples":2,"minimum_members":2}),
                )]),
            },
        )
        .unwrap();
    assert_eq!(candidates.len(), 1);
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), relation_domain());
    let candidate = Tracked::from_derived(&candidates[0], candidates[0].value().clone());
    let result = engine
        .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
        .unwrap();
    assert_eq!(
        DependencyCollector::default().read(&baseline),
        &relation_domain()
    );
    assert_eq!(result.value().domain.units.len(), 3);
    let unit = &result.value().domain.units[&result.value().added_units[0]];
    assert_eq!(unit.kind, UnitKind::CompositeMeaning);
    assert_eq!(unit.label, None);
    assert_eq!(
        unit.properties["members"],
        PropertyValue::Structured(json!(["existing", "target"]))
    );
    assert_eq!(
        unit.properties["candidate_evidence"],
        PropertyValue::Structured(serde_json::Value::Object(
            candidates[0].value().value.clone()
        ))
    );
    assert!(result.value().added_relations.is_empty());
    assert!(result.value().property_changes.is_empty());
    assert_eq!(
        result,
        engine
            .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
            .unwrap()
    );
}
#[test]
fn community_application_requires_baseline_members_and_consistent_evidence() {
    assert!(apply(domain(), community_proposal()).is_err());
    for (field, key, value) in [
        ("pattern", "members", json!(["existing", "existing"])),
        ("evidence", "community_index", json!(1)),
        ("evidence", "structure", json!("")),
        ("selection", "metric", json!("kendall")),
        ("selection", "minimum_samples", json!(3)),
        ("selection", "minimum_members", json!(3)),
    ] {
        let mut c = community_proposal();
        c.value[field][key] = value;
        assert!(apply(relation_domain(), c).is_err());
    }
    let mut c = community_proposal();
    c.value["evidence"]["result"]["qualifying_pairs"] = json!(0);
    assert!(apply(relation_domain(), c).is_err());
}

fn latent_proposal() -> CandidateProposal {
    let mut c = proposal();
    c.kind = CandidateKind::LatentAxis;
    let q = std::f64::consts::FRAC_1_SQRT_2;
    c.value=json!({"pattern":{"matching":"empirical_spectral_axis","units":["existing","target"],"eigenvalue":-1.0,"loadings":[q,-q]},"evidence":{"structure":"spectrum","eigenpair_index":1,"result":{"metric":"relative_rank_variance","units":["existing","target"],"eigenpairs":[{"eigenvalue":1.0,"loadings":[q,q]},{"eigenvalue":-1.0,"loadings":[q,-q]}],"minimum_cell_samples":4,"tolerance":1e-12,"sweeps":1}},"selection":{"metric":"relative_rank_variance","minimum_samples":2,"minimum_absolute_eigenvalue":0.5}}).as_object().unwrap().clone();
    c
}
#[test]
fn generated_latent_axes_retain_signed_spectral_evidence_without_mutation() {
    use unclip_engine::{CandidateInputs, MeasurementRun};
    use unclip_epistemic::PluginId;
    use unclip_plugin::{EngineProfile, PluginSelection};
    let template = latent_proposal();
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any("generate.latent-axis")],
            ..Default::default()
        })
        .unwrap();
    let structures = [Tracked::from_recorded(
        DerivedId::new("spectrum"),
        unclip_measure::EmpiricalStructure {
            kind: "spectral".into(),
            value: template.value["evidence"]["result"].clone(),
        },
    )];
    let candidates = engine
        .generate_candidates(
            &plan,
            CandidateInputs {
                domain_version_id: &template.domain_version_id,
                measurements: &[],
                observations: &[],
                structures: &structures,
            },
            MeasurementRun {
                id: "generation",
                timestamp: Timestamp::new("now"),
                params: &BTreeMap::from([(
                    PluginId::new("generate.latent-axis"),
                    template.value["selection"].clone(),
                )]),
            },
        )
        .unwrap();
    assert_eq!(candidates.len(), 2);
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), relation_domain());
    for generated in candidates {
        let candidate = Tracked::from_derived(&generated, generated.value().clone());
        let result = engine
            .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
            .unwrap();
        let unit = &result.value().domain.units[&result.value().added_units[0]];
        assert_eq!(unit.kind, UnitKind::LatentAxis);
        assert_eq!(unit.label, None);
        assert_eq!(
            unit.properties["eigenvalue"],
            PropertyValue::Number(
                generated.value().value["pattern"]["eigenvalue"]
                    .as_f64()
                    .unwrap()
            )
        );
        assert_eq!(
            unit.properties["loadings"],
            PropertyValue::Structured(generated.value().value["pattern"]["loadings"].clone())
        );
        assert_eq!(
            unit.properties["candidate_evidence"],
            PropertyValue::Structured(serde_json::Value::Object(generated.value().value.clone()))
        );
        assert_eq!(
            DependencyCollector::default().read(&baseline),
            &relation_domain()
        );
        assert_eq!(
            result,
            engine
                .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
                .unwrap()
        );
    }
}
#[test]
fn latent_application_requires_existing_units_and_consistent_spectral_evidence() {
    assert!(apply(domain(), latent_proposal()).is_err());
    for (field, key, value) in [
        ("pattern", "eigenvalue", json!(1.0)),
        ("pattern", "loadings", json!([1.0, 0.0])),
        ("pattern", "units", json!(["target", "existing"])),
        ("evidence", "eigenpair_index", json!(2)),
        ("evidence", "structure", json!("")),
        ("selection", "metric", json!("spearman")),
        ("selection", "minimum_samples", json!(5)),
        ("selection", "minimum_absolute_eigenvalue", json!(2.0)),
        ("selection", "minimum_absolute_eigenvalue", json!(0)),
    ] {
        let mut c = latent_proposal();
        c.value[field][key] = value;
        assert!(apply(relation_domain(), c).is_err());
    }
    let mut malformed = latent_proposal();
    malformed.value["evidence"]["result"]["eigenpairs"][1]["loadings"] = json!([0.0, 0.0]);
    assert!(apply(relation_domain(), malformed).is_err());
}

fn coupling_proposal() -> CandidateProposal {
    let mut c = proposal();
    c.kind = CandidateKind::DynamicCoupling;
    c.value=json!({"pattern":{"matching":"thresholded_pairwise_association","metric":"spearman","units":["existing","target"]},"evidence":{"measurement":"matrix","sensor":"sensor.spearman","sensor_version":"0.1.0","context":{"values":{}},"cell":{"status":"value","value":0.9,"sample_count":4}},"selection":{"threshold":0.8,"minimum_samples":2},"causal_claim":false}).as_object().unwrap().clone();
    c
}
#[test]
fn generated_pairwise_couplings_apply_with_metric_specific_evidence() {
    use unclip_engine::{CandidateInputs, MeasurementRun};
    use unclip_epistemic::PluginId;
    use unclip_measure::{Measurement, MeasurementContext, MeasurementValue, Reading};
    use unclip_plugin::{EngineProfile, PluginSelection};
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any("generate.pairwise-coupling")],
            ..Default::default()
        })
        .unwrap();
    for (metric, value, threshold) in [
        ("spearman", 0.9, 0.8),
        ("kendall", -0.5, -0.6),
        ("relative_rank_variance", 0.1, 0.2),
        ("mutual_information", 0.9, 0.8),
    ] {
        let cell = json!({"status":"value","value":value,"sample_count":4});
        let matrix=serde_json::from_value(json!({"metric":metric,"units":["existing","target"],"cells":[[cell.clone(),cell.clone()],[cell.clone(),cell]]})).unwrap();
        let inputs = [Tracked::from_recorded(
            DerivedId::new("matrix"),
            Measurement {
                sensor: PluginId::new(format!("sensor.{metric}")),
                sensor_version: "0.1.0".parse().unwrap(),
                reading: Reading::Value {
                    value: MeasurementValue::PairwiseMatrix(matrix),
                },
                confidence: None,
                sample_count: Some(4),
                context: MeasurementContext::default(),
            },
        )];
        let template = coupling_proposal();
        let candidates = engine
            .generate_candidates(
                &plan,
                CandidateInputs {
                    domain_version_id: &template.domain_version_id,
                    measurements: &inputs,
                    observations: &[],
                    structures: &[],
                },
                MeasurementRun {
                    id: metric,
                    timestamp: Timestamp::new("now"),
                    params: &BTreeMap::from([(
                        PluginId::new("generate.pairwise-coupling"),
                        json!({"metric":metric,"threshold":threshold,"minimum_samples":2}),
                    )]),
                },
            )
            .unwrap();
        assert_eq!(candidates.len(), 1);
        let baseline = Tracked::from_recorded(DerivedId::new("baseline"), relation_domain());
        let candidate = Tracked::from_derived(&candidates[0], candidates[0].value().clone());
        let result = engine
            .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
            .unwrap();
        let unit = &result.value().domain.units[&result.value().added_units[0]];
        assert_eq!(unit.kind, UnitKind::DynamicCoupling);
        assert_eq!(unit.label, None);
        assert_eq!(
            unit.properties["causal_claim"],
            PropertyValue::Boolean(false)
        );
        assert_eq!(
            unit.properties["candidate_evidence"],
            PropertyValue::Structured(serde_json::Value::Object(
                candidates[0].value().value.clone()
            ))
        );
        assert_eq!(
            DependencyCollector::default().read(&baseline),
            &relation_domain()
        );
        assert_eq!(
            result,
            engine
                .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
                .unwrap()
        );
    }
}
#[test]
fn coupling_application_rejects_missing_units_sparse_cells_and_false_selection_claims() {
    assert!(apply(domain(), coupling_proposal()).is_err());
    for (field, key, value) in [
        ("pattern", "units", json!(["existing", "existing"])),
        ("pattern", "matching", json!("lagged_association")),
        ("selection", "threshold", json!(0.95)),
        ("selection", "minimum_samples", json!(5)),
        ("evidence", "measurement", json!("")),
        (
            "evidence",
            "cell",
            json!({"status":"undefined","sample_count":4}),
        ),
        (
            "evidence",
            "cell",
            json!({"status":"value","value":2.0,"sample_count":4}),
        ),
    ] {
        let mut c = coupling_proposal();
        c.value[field][key] = value;
        assert!(apply(relation_domain(), c).is_err());
    }
    let mut c = coupling_proposal();
    c.value["causal_claim"] = json!(true);
    assert!(apply(relation_domain(), c).is_err());
}

fn temporal_proposal() -> CandidateProposal {
    let mut c = proposal();
    c.kind = CandidateKind::DynamicCoupling;
    let sequence = json!([{"observation":"z","position":0},{"observation":"a","position":10},{"observation":"m","position":30},{"observation":"b","position":40}]);
    c.value=json!({"pattern":{"matching":"lagged_directional_association","source":"existing","target":"target","lag_steps":1,"sequence":sequence},"evidence":{"measurement":"lagged","sensor":"sensor.lagged-dependency","sensor_version":"0.1.0","coefficient":-0.5,"sample_count":3,"context":{"values":{"source":"existing","target":"target","lag_steps":1,"sequence":sequence}}},"selection":{"threshold":-0.6,"minimum_samples":2},"causal_claim":false}).as_object().unwrap().clone();
    c
}
#[test]
fn generated_temporal_coupling_preserves_direction_lag_order_and_signed_evidence() {
    use unclip_engine::{CandidateInputs, MeasurementRun};
    use unclip_epistemic::PluginId;
    use unclip_measure::{Measurement, MeasurementValue, Reading};
    use unclip_plugin::{EngineProfile, PluginSelection};
    let template = temporal_proposal();
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any("generate.temporal-coupling")],
            ..Default::default()
        })
        .unwrap();
    let inputs = [Tracked::from_recorded(
        DerivedId::new("lagged"),
        Measurement {
            sensor: PluginId::new("sensor.lagged-dependency"),
            sensor_version: "0.1.0".parse().unwrap(),
            reading: Reading::Value {
                value: MeasurementValue::Scalar(-0.5),
            },
            confidence: None,
            sample_count: Some(3),
            context: serde_json::from_value(template.value["evidence"]["context"].clone()).unwrap(),
        },
    )];
    let candidates = engine
        .generate_candidates(
            &plan,
            CandidateInputs {
                domain_version_id: &template.domain_version_id,
                measurements: &inputs,
                observations: &[],
                structures: &[],
            },
            MeasurementRun {
                id: "temporal",
                timestamp: Timestamp::new("now"),
                params: &BTreeMap::from([(
                    PluginId::new("generate.temporal-coupling"),
                    template.value["selection"].clone(),
                )]),
            },
        )
        .unwrap();
    assert_eq!(candidates.len(), 1);
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), relation_domain());
    let candidate = Tracked::from_derived(&candidates[0], candidates[0].value().clone());
    let result = engine
        .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
        .unwrap();
    let unit = &result.value().domain.units[&result.value().added_units[0]];
    assert_eq!(unit.kind, UnitKind::DynamicCoupling);
    assert_eq!(unit.label, None);
    assert_eq!(
        unit.properties["units"],
        PropertyValue::Structured(json!(["existing", "target"]))
    );
    assert_eq!(
        unit.properties["candidate_pattern"],
        PropertyValue::Structured(template.value["pattern"].clone())
    );
    assert_eq!(
        unit.properties["causal_claim"],
        PropertyValue::Boolean(false)
    );
    assert_eq!(
        unit.properties["candidate_evidence"],
        PropertyValue::Structured(serde_json::Value::Object(
            candidates[0].value().value.clone()
        ))
    );
    assert_eq!(
        DependencyCollector::default().read(&baseline),
        &relation_domain()
    );
    assert_eq!(
        result,
        engine
            .apply_candidate(&baseline, &candidate, "trial", Timestamp::new("now"))
            .unwrap()
    );
}
#[test]
fn temporal_application_rejects_inconsistent_order_lag_counts_and_direction() {
    assert!(apply(domain(), temporal_proposal()).is_err());
    for (field, key, value) in [
        ("pattern", "source", json!("target")),
        ("pattern", "lag_steps", json!(0)),
        ("pattern", "lag_steps", json!(2)),
        ("evidence", "sensor", json!("sensor.spearman")),
        ("evidence", "sample_count", json!(4)),
        ("evidence", "coefficient", json!(-0.9)),
        ("selection", "minimum_samples", json!(4)),
        ("selection", "threshold", json!(2.0)),
    ] {
        let mut c = temporal_proposal();
        c.value[field][key] = value;
        assert!(apply(relation_domain(), c).is_err());
    }
    let mut c = temporal_proposal();
    c.value["pattern"]["sequence"][1]["position"] = json!(0);
    assert!(apply(relation_domain(), c).is_err());
    let mut c = temporal_proposal();
    c.value["evidence"]["context"]["values"]["sequence"][1]["observation"] = json!("different");
    assert!(apply(relation_domain(), c).is_err());
}
