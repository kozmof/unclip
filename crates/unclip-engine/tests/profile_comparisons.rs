use serde_json::json;
use std::collections::BTreeMap;
use unclip_engine::{ComparisonPair, Engine, MeasurementRun, ProfileComparisonResult};
use unclip_epistemic::{DerivedId, PluginId, Timestamp, Tracked};
use unclip_measure::{Measurement, MeasurementContext, MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};
fn input(id: &str, value: f64) -> Tracked<Measurement> {
    Tracked::from_recorded(
        DerivedId::new(id),
        Measurement {
            sensor: PluginId::new("sensor.fixture"),
            sensor_version: "1.0.0".parse().unwrap(),
            reading: Reading::Value {
                value: MeasurementValue::Scalar(value),
            },
            confidence: None,
            sample_count: Some(4),
            context: MeasurementContext::default(),
        },
    )
}
fn pair(before: &str, after: &str) -> ComparisonPair {
    ComparisonPair {
        before: DerivedId::new(before),
        after: DerivedId::new(after),
    }
}
fn compare(
    before: &[Tracked<Measurement>],
    after: &[Tracked<Measurement>],
    pairs: &[ComparisonPair],
    reverse: bool,
) -> unclip_plugin::Result<ProfileComparisonResult> {
    let engine = Engine::with_builtins().unwrap();
    let mut comparators = vec![
        PluginSelection::any("compare.scalar-difference"),
        PluginSelection::any("compare.graph-identity"),
    ];
    if reverse {
        comparators.reverse();
    }
    let plan = engine
        .plan(&EngineProfile {
            comparators,
            ..Default::default()
        })
        .unwrap();
    engine.compare_profiles(
        &plan,
        before,
        after,
        pairs,
        MeasurementRun {
            id: "comparison",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::new(),
        },
    )
}
#[test]
fn profile_pairing_is_explicit_replayable_and_preserves_independent_deltas() {
    let a = [input("b1", 1.0), input("b2", 5.0), input("left-only", 9.0)];
    let b = [input("a1", 3.0), input("a2", 4.0), input("right-only", 8.0)];
    let result = compare(&a, &b, &[pair("b2", "a2"), pair("b1", "a1")], false).unwrap();
    let profile = result.profile.value();
    assert_eq!(profile.deltas.len(), 4);
    assert_eq!(result.deltas.len(), 4);
    assert_eq!(profile.pairs, vec![pair("b1", "a1"), pair("b2", "a2")]);
    assert_eq!(profile.unmatched_before, vec![DerivedId::new("left-only")]);
    assert_eq!(profile.unmatched_after, vec![DerivedId::new("right-only")]);
    assert_eq!(result.profile.provenance().inputs.len(), 10);
    for (index, delta) in result.deltas.iter().enumerate() {
        assert_eq!(
            delta.provenance().inputs,
            if index < 2 {
                vec![DerivedId::new("a1"), DerivedId::new("b1")]
            } else {
                vec![DerivedId::new("a2"), DerivedId::new("b2")]
            }
        );
        assert_eq!(profile.deltas[index].delta, *delta.value());
        assert_eq!(profile.deltas[index].id, *delta.id());
    }
    let replay = compare(
        &[input("left-only", 9.0), input("b2", 5.0), input("b1", 1.0)],
        &[input("right-only", 8.0), input("a2", 4.0), input("a1", 3.0)],
        &[pair("b1", "a1"), pair("b2", "a2")],
        true,
    )
    .unwrap();
    assert_eq!(result.profile, replay.profile);
    assert_eq!(result.deltas, replay.deltas);
    assert_eq!(
        result.profile.provenance().params["comparators"][0]["id"],
        "compare.graph-identity"
    );
    assert_eq!(
        result.profile.provenance().params["comparators"][0]["version"],
        "0.1.0"
    );
    assert_eq!(
        result.profile.provenance().params["pairs"],
        json!([{"before":"b1","after":"a1"},{"before":"b2","after":"a2"}])
    );
}
#[test]
fn unmatched_evidence_is_not_paired_implicitly() {
    let result = compare(&[input("b", 1.0)], &[input("a", 2.0)], &[], false).unwrap();
    assert!(result.deltas.is_empty());
    assert!(result.profile.value().pairs.is_empty());
    assert_eq!(
        result.profile.value().unmatched_before,
        vec![DerivedId::new("b")]
    );
    assert_eq!(
        result.profile.provenance().inputs,
        vec![DerivedId::new("a"), DerivedId::new("b")]
    );
    let shared = compare(
        &[input("same", 1.0)],
        &[input("same", 1.0)],
        &[pair("same", "same")],
        false,
    )
    .unwrap();
    assert_eq!(
        shared.deltas[0].provenance().inputs,
        vec![DerivedId::new("same")]
    );
}
#[test]
fn invalid_or_ambiguous_pairings_fail_without_partial_profiles() {
    for pairs in [
        vec![pair("missing", "a1")],
        vec![pair("b1", "a1"), pair("b1", "a2")],
        vec![pair("b1", "a1"), pair("b2", "a1")],
        vec![pair("b1", "a1"), pair("b1", "a1")],
    ] {
        assert!(compare(
            &[input("b1", 1.0), input("b2", 2.0)],
            &[input("a1", 1.0), input("a2", 2.0)],
            &pairs,
            false
        )
        .is_err());
    }
    assert!(compare(&[input("b", 1.0), input("b", 1.0)], &[], &[], false).is_err());
    assert!(compare(&[input("same", 1.0)], &[input("same", 2.0)], &[], false).is_err());
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&EngineProfile::default()).unwrap();
    assert!(engine
        .compare_profiles(
            &plan,
            &[],
            &[],
            &[],
            MeasurementRun {
                id: "empty",
                timestamp: Timestamp::new("now"),
                params: &BTreeMap::new()
            }
        )
        .is_err());
}

#[test]
fn derived_output_ids_cannot_alias_input_evidence() {
    assert!(compare(&[input("comparison/profile", 1.0)], &[], &[], false).is_err());
    assert!(compare(
        &[input("comparison/pairs/0/compare.scalar-difference", 1.0)],
        &[input("a", 2.0)],
        &[pair("comparison/pairs/0/compare.scalar-difference", "a")],
        false
    )
    .is_err());
}
