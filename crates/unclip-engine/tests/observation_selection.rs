use serde_json::json;
use std::collections::BTreeMap;
use unclip_engine::{Engine, MeasurementRun, ObservationSplit};
use unclip_epistemic::{DerivedId, SourceRef, Timestamp, Tracked};
use unclip_observe::{Observation, ObservationId};
use unclip_plugin::EngineProfile;
use unclip_store::{connect_and_migrate, EngineRunRepository, SeaOrmEngineRunRepository};

fn observation(id: &str, provenance: &str) -> Tracked<Observation> {
    Tracked::from_recorded(
        DerivedId::new(provenance),
        Observation {
            id: ObservationId::new(id),
            source: SourceRef::new("fixture"),
            observed_at: None,
            units: vec![],
            relations: vec![],
            context: BTreeMap::from([("genre".into(), json!("fixture"))]),
        },
    )
}
fn ids(values: &[&str]) -> Vec<ObservationId> {
    values.iter().map(|id| ObservationId::new(*id)).collect()
}

#[tokio::test]
async fn explicit_splits_replay_from_persisted_run_with_shared_batch_provenance() {
    let engine = Engine::with_builtins().unwrap();
    let inputs = [
        observation("a", "batch"),
        observation("b", "batch"),
        observation("c", "other"),
        observation("unused", "unused"),
    ];
    let selected = engine
        .select_observations(
            &inputs,
            &ids(&["b"]),
            &ids(&["c", "a"]),
            "run",
            Timestamp::new("now"),
        )
        .unwrap();
    assert_eq!(
        selected.provenance().inputs,
        vec![
            DerivedId::new("batch"),
            DerivedId::new("other"),
            DerivedId::new("unused")
        ]
    );
    assert_eq!(
        selected
            .value()
            .held_out
            .iter()
            .map(|entry| entry.value.id.clone())
            .collect::<Vec<_>>(),
        ids(&["c", "a"])
    );
    let reversed = inputs.into_iter().rev().collect::<Vec<_>>();
    assert_eq!(
        selected,
        engine
            .select_observations(
                &reversed,
                &ids(&["b"]),
                &ids(&["c", "a"]),
                "run",
                Timestamp::new("now")
            )
            .unwrap()
    );
    let plan = engine.plan(&EngineProfile::default()).unwrap();
    let params = BTreeMap::new();
    let record = engine
        .observation_split_run_record(
            &plan,
            &selected,
            MeasurementRun {
                id: "run",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
            json!({"purpose":"experiment"}),
        )
        .unwrap();
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let repository = SeaOrmEngineRunRepository::new(db);
    repository.insert_run(record.clone()).await.unwrap();
    let restored = repository.get_run("run").await.unwrap().unwrap();
    assert_eq!(record, restored);
    let split: ObservationSplit =
        serde_json::from_value(restored.metadata["observation_split"]["value"].clone()).unwrap();
    assert_eq!(&split, selected.value());
    assert_eq!(
        restored.metadata["observation_split"]["provenance"],
        serde_json::to_value(selected.provenance()).unwrap()
    );
    assert_eq!(restored.metadata["request"]["purpose"], "experiment");
    assert!(engine
        .observation_split_run_record(
            &plan,
            &selected,
            MeasurementRun {
                id: "different",
                timestamp: Timestamp::new("now"),
                params: &params
            },
            json!({})
        )
        .is_err());
}

#[test]
fn rejects_ambiguous_missing_overlapping_or_empty_selections() {
    let engine = Engine::with_builtins().unwrap();
    let inputs = [observation("a", "batch"), observation("b", "batch")];
    for (training, held_out) in [
        (ids(&["a"]), ids(&["a"])),
        (ids(&["a", "a"]), ids(&["b"])),
        (ids(&[]), ids(&["b", "b"])),
        (ids(&[]), ids(&["absent"])),
        (ids(&["absent"]), ids(&["b"])),
        (ids(&["a"]), ids(&[])),
    ] {
        assert!(engine
            .select_observations(&inputs, &training, &held_out, "run", Timestamp::new("now"))
            .is_err());
    }
    for inputs in [
        vec![observation("a", "first"), observation("a", "second")],
        vec![observation("a", "")],
        vec![observation("", "batch")],
        vec![observation("a", "run/observation-split")],
    ] {
        assert!(engine
            .select_observations(&inputs, &[], &ids(&["a"]), "run", Timestamp::new("now"))
            .is_err());
    }
    assert!(engine
        .select_observations(&inputs, &[], &ids(&["a"]), " ", Timestamp::new("now"))
        .is_err());
    assert!(engine
        .select_observations(&inputs, &[], &ids(&["a"]), "run", Timestamp::new("now"))
        .is_ok());
}
