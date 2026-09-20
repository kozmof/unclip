use serde_json::json;
use std::collections::BTreeMap;
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_engine::{Engine, MeasurementRun};
use unclip_epistemic::{Calculated, DerivedId, PluginId, SourceRef, Timestamp, Tracked};
use unclip_measure::{MeasurementValue, Reading};
use unclip_observe::{Observation, ObservationId, ObservedUnit, ObservedUnitId};
use unclip_plugin::{EngineProfile, PluginSelection};

fn candidate(kind: CandidateKind, left: &str, right: &str) -> Tracked<CandidateProposal> {
    Tracked::from_recorded(DerivedId::new("candidate"), CandidateProposal { domain_version_id:"d1".into(),kind,value:json!({"pattern":{"matching":"exact_directed_observed_relation","source_label":left,"target_label":right,"relation_kind":"near"}}).as_object().unwrap().clone() })
}
fn observation(id: &str, labels: &[&str]) -> Tracked<Observation> {
    Tracked::from_recorded(
        DerivedId::new(id),
        Observation {
            id: ObservationId::new(id),
            source: SourceRef::new("fixture"),
            observed_at: None,
            context: BTreeMap::new(),
            relations: vec![],
            units: labels
                .iter()
                .enumerate()
                .map(|(index, label)| ObservedUnit {
                    id: ObservedUnitId::new(index.to_string()),
                    label: (*label).into(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                })
                .collect(),
        },
    )
}
fn evaluate(
    candidate: &Tracked<CandidateProposal>,
    observations: &[Tracked<Observation>],
    params: serde_json::Value,
) -> unclip_plugin::Result<Vec<Calculated<Reading>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            null_models: vec![PluginSelection::any("null.random-cooccurrence")],
            ..Default::default()
        })
        .unwrap();
    engine.evaluate_null_models(
        &plan,
        candidate,
        observations,
        MeasurementRun {
            id: "null-run",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("null.random-cooccurrence"), params)]),
        },
    )
}
fn params() -> serde_json::Value {
    json!({"minimum_observations":2})
}
#[test]
fn distinct_observation_presence_preserves_counts_and_replays_with_provenance() {
    let c = candidate(CandidateKind::Relation, "a", "b");
    let mut observations = vec![
        observation("z", &["a", "a", "b"]),
        observation("a", &["a", "b"]),
        observation("b", &[]),
        observation("c", &["other"]),
    ];
    let results = evaluate(&c, &observations, params()).unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = results[0].value()
    else {
        panic!("expected null result")
    };
    assert_eq!(value["sample_count"], 4);
    assert_eq!(value["source_count"], 2);
    assert_eq!(value["target_count"], 2);
    assert_eq!(value["observed_overlap"], 2);
    assert_eq!(value["expected_overlap"], 1.0);
    assert!((value["upper_tail_probability"].as_f64().unwrap() - 1.0 / 6.0).abs() < 1e-14);
    assert_eq!(value["selection_adjusted"], false);
    assert_eq!(
        results[0].provenance().inputs,
        vec![
            DerivedId::new("a"),
            DerivedId::new("b"),
            DerivedId::new("c"),
            DerivedId::new("candidate"),
            DerivedId::new("z")
        ]
    );
    assert_eq!(results[0].provenance().params, params());
    observations.reverse();
    assert_eq!(evaluate(&c, &observations, params()).unwrap(), results);
}
#[test]
fn sparse_unsupported_and_degenerate_evidence_remain_distinct() {
    let c = candidate(CandidateKind::Relation, "a", "b");
    let sparse = evaluate(&c, &[observation("a", &["a"])], params()).unwrap();
    assert_eq!(
        *sparse[0].value(),
        Reading::InsufficientEvidence { have: 1, need: 2 }
    );
    assert_eq!(sparse[0].provenance().inputs.len(), 2);
    for c in [
        candidate(CandidateKind::Relation, "a", "a"),
        candidate(CandidateKind::LatentAxis, "a", "b"),
    ] {
        let result = evaluate(&c, &[], params()).unwrap();
        assert!(matches!(result[0].value(), Reading::NotApplicable { .. }));
        assert_eq!(
            result[0].provenance().inputs,
            vec![DerivedId::new("candidate")]
        );
    }
    let result = evaluate(
        &c,
        &[observation("a", &[]), observation("b", &[])],
        params(),
    )
    .unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = result[0].value()
    else {
        panic!()
    };
    assert_eq!(value["upper_tail_probability"], 1.0);
    assert_eq!(value["expected_overlap"], 0.0);
}
#[test]
fn repeated_identities_and_invalid_configuration_are_errors() {
    let c = candidate(CandidateKind::Relation, "a", "b");
    assert!(evaluate(
        &c,
        &[observation("a", &[]), observation("a", &[])],
        params()
    )
    .is_err());
    for params in [
        json!({}),
        json!({"minimum_observations":1}),
        json!({"minimum_observations":2,"alpha":0.05}),
    ] {
        assert!(evaluate(&c, &[], params).is_err());
    }
    assert!(evaluate(&candidate(CandidateKind::Relation, "", "b"), &[], params()).is_err());
}

fn ranking(
    id: &str,
    tiers: &[&[&str]],
    unknown: &[&str],
) -> Tracked<unclip_observe::PartialRanking> {
    Tracked::from_recorded(
        DerivedId::new(format!("ranking/{id}")),
        unclip_observe::PartialRanking {
            observation: ObservationId::new(id),
            tiers: tiers
                .iter()
                .map(|tier| unclip_observe::RankTier {
                    units: tier.iter().map(|unit| ObservedUnitId::new(*unit)).collect(),
                })
                .collect(),
            unknown: unknown
                .iter()
                .map(|unit| ObservedUnitId::new(*unit))
                .collect(),
        },
    )
}
fn evaluate_ranking(
    observations: &[Tracked<Observation>],
    rankings: &[Tracked<unclip_observe::PartialRanking>],
) -> unclip_plugin::Result<Vec<Calculated<Reading>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            null_models: vec![PluginSelection::any("null.ranking-constraints")],
            ..Default::default()
        })
        .unwrap();
    engine.evaluate_null_models_with_rankings(
        &plan,
        &candidate(CandidateKind::Relation, "a", "b"),
        observations,
        rankings,
        MeasurementRun {
            id: "ranking-null",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("null.ranking-constraints"), params())]),
        },
    )
}
#[test]
fn ranking_constraints_preserve_ties_and_unknowns_and_replay() {
    let mut observations = (0..5)
        .map(|i| observation(&i.to_string(), &["a", "b"]))
        .collect::<Vec<_>>();
    let mut rankings = vec![
        ranking("0", &[&["0"], &["1"]], &[]),
        ranking("1", &[&["0"], &["1"]], &[]),
        ranking("2", &[&["0", "1"]], &[]),
        ranking("3", &[&["0"]], &["1"]),
    ];
    let results = evaluate_ranking(&observations, &rankings).unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = results[0].value()
    else {
        panic!()
    };
    assert_eq!(value["sample_count"], 2);
    assert_eq!(value["source_before_target"], 2);
    assert_eq!(value["source_after_target"], 0);
    assert_eq!(value["ties"], 1);
    assert_eq!(value["skipped"], 2);
    assert_eq!(value["expected_source_before_target"], 1.0);
    assert_eq!(value["upper_tail_probability"], 0.25);
    assert_eq!(results[0].provenance().inputs.len(), 10);
    observations.reverse();
    rankings.reverse();
    assert_eq!(evaluate_ranking(&observations, &rankings).unwrap(), results);
    let reversed = vec![
        ranking("0", &[&["1"], &["0"]], &[]),
        ranking("1", &[&["1"], &["0"]], &[]),
    ];
    let result = evaluate_ranking(&observations, &reversed).unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = result[0].value()
    else {
        panic!()
    };
    assert_eq!(value["upper_tail_probability"], 1.0);
    assert_eq!(value["source_after_target"], 2);
}
#[test]
fn ranking_sparse_and_invalid_evidence_are_not_completed_or_duplicated() {
    let observations = vec![observation("0", &["a", "b"]), observation("1", &["a", "b"])];
    let result = evaluate_ranking(&observations, &[ranking("0", &[&["0"], &["1"]], &[])]).unwrap();
    assert_eq!(
        *result[0].value(),
        Reading::InsufficientEvidence { have: 1, need: 2 }
    );
    for rankings in [
        vec![ranking("missing", &[], &[])],
        vec![ranking("0", &[&[]], &[])],
        vec![ranking("0", &[&["0"]], &["0"])],
        vec![ranking("0", &[&["absent"]], &[])],
        vec![ranking("0", &[], &[]), ranking("0", &[], &[])],
    ] {
        assert!(evaluate_ranking(&observations, &rankings).is_err());
    }
    assert!(evaluate_ranking(
        &[observation("0", &["a", "a", "b"])],
        &[ranking("0", &[&["0"], &["2"]], &["1"])]
    )
    .is_err());
}
