use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::json;
use unclip_epistemic::{hash_params, DerivedId, Operation, PluginId, Provenance, Timestamp};
use unclip_store::{
    connect_and_migrate, EngineRunRecord, EngineRunRepository, EngineRunStatus,
    MeasurementRepository, ProvenanceRepository, SeaOrmEngineRunRepository,
    SeaOrmMeasurementRepository, SeaOrmProvenanceRepository, SensorRunRecord, StoreError,
    StoredProvenance,
};

fn run() -> EngineRunRecord {
    EngineRunRecord {
        id: "run".into(),
        resolved_plan: json!({
            "sensors": [
                {"id": "sensor.z", "version": "1.0.0", "params_hash": "z"},
                {"id": "sensor.a", "version": "2.0.0", "params_hash": "a"}
            ]
        }),
        status: EngineRunStatus::Planned,
        started_at: "start".into(),
        completed_at: None,
        metadata: json!({"request": "fixture"}),
    }
}

fn sensor(id: &str, plugin: &str) -> SensorRunRecord {
    SensorRunRecord {
        id: id.into(),
        engine_run_id: "run".into(),
        sensor: PluginId::new(plugin),
        sensor_version: semver::Version::new(1, 0, 0),
        params: json!({}),
        params_hash: unclip_epistemic::ParameterHash::new("hash"),
        status: "completed".into(),
        started_at: "start".into(),
        completed_at: Some("end".into()),
    }
}

fn provenance(id: &str) -> StoredProvenance {
    let params = json!({});
    StoredProvenance {
        id: DerivedId::new(id),
        run_id: Some("run".into()),
        provenance: Provenance {
            operation: Operation::Calculated,
            producer: PluginId::new("test"),
            algorithm: "fixture".into(),
            version: semver::Version::new(1, 0, 0),
            params_hash: hash_params(&params),
            params,
            inputs: Vec::new(),
            source: None,
            timestamp: Timestamp::new("now"),
            domain_version: None,
            frame_version: None,
            model: None,
        },
    }
}

async fn seed_profile_parents(db: &DatabaseConnection) {
    db.execute_unprepared(
        r#"
        INSERT INTO domains(id, label, created_at) VALUES ('domain', NULL, 'now');
        INSERT INTO domain_versions(id, domain_id, version, predecessor_id, created_at)
          VALUES ('domain-version', 'domain', '1', NULL, 'now');
        INSERT INTO measurement_frames(id, domain_id, label, created_at)
          VALUES ('frame', 'domain', NULL, 'now');
        INSERT INTO frame_versions(id, frame_id, version, domain_version_id, predecessor_id, created_at)
          VALUES ('frame-version', 'frame', '1', 'domain-version', NULL, 'now');
        INSERT INTO measurement_profiles(
          id, engine_run_id, observation_id, frame_version_id, provenance_id, created_at
        ) VALUES ('profile-b', 'run', NULL, 'frame-version', 'prov-b', 'now');
        "#,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn lifecycle_and_replay_bundle_round_trip() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let runs = SeaOrmEngineRunRepository::new(db.clone());
    let expected = run();
    runs.insert_run(expected.clone()).await.unwrap();
    assert_eq!(runs.get_run("run").await.unwrap().unwrap(), expected);

    runs.transition_run("run", EngineRunStatus::Running, None)
        .await
        .unwrap();

    let provenance_repo = SeaOrmProvenanceRepository::new(db.clone());
    provenance_repo
        .insert_provenance(provenance("prov-b"))
        .await
        .unwrap();
    provenance_repo
        .insert_provenance(provenance("prov-a"))
        .await
        .unwrap();

    let measurements = SeaOrmMeasurementRepository::new(db.clone());
    measurements
        .insert_sensor_run(sensor("run-z", "sensor.z"))
        .await
        .unwrap();
    measurements
        .insert_sensor_run(sensor("run-a", "sensor.a"))
        .await
        .unwrap();
    seed_profile_parents(&db).await;

    runs.transition_run("run", EngineRunStatus::Completed, Some("end".into()))
        .await
        .unwrap();
    let replay = runs.replay_run("run").await.unwrap().unwrap();
    assert_eq!(replay.run.status, EngineRunStatus::Completed);
    assert_eq!(replay.run.completed_at.as_deref(), Some("end"));
    assert_eq!(
        replay
            .sensor_runs
            .iter()
            .map(|run| run.sensor.0.as_str())
            .collect::<Vec<_>>(),
        vec!["sensor.a", "sensor.z"]
    );
    assert_eq!(replay.provenance_ids, vec!["prov-a", "prov-b"]);
    assert_eq!(replay.profile_ids, vec!["profile-b"]);
}

#[tokio::test]
async fn invalid_or_stale_transitions_are_rejected() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let runs = SeaOrmEngineRunRepository::new(db);
    runs.insert_run(run()).await.unwrap();

    let error = runs
        .transition_run("run", EngineRunStatus::Completed, Some("end".into()))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Conflict { .. }));

    runs.transition_run("run", EngineRunStatus::Failed, Some("end".into()))
        .await
        .unwrap();
    let error = runs
        .transition_run("run", EngineRunStatus::Running, None)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Conflict { .. }));
}
