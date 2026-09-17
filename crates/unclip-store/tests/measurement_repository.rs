use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::json;
use unclip_domain::{FrameId, UnitId};
use unclip_epistemic::{DerivedId, FrameVersion, ParameterHash, PluginId};
use unclip_measure::{
    Measurement, MeasurementContext, MeasurementKind, MeasurementProfile, MeasurementValue, Reading,
};
use unclip_store::{
    connect_and_migrate, MeasurementProfileHeader, MeasurementRecord, MeasurementRepository,
    SeaOrmMeasurementRepository, SensorRunRecord, StoreError,
};

async fn seed_parents(db: &DatabaseConnection) {
    db.execute_unprepared(
        r#"
        INSERT INTO domains(id, label, created_at) VALUES ('domain', NULL, 'now');
        INSERT INTO domain_versions(id, domain_id, version, predecessor_id, created_at)
          VALUES ('domain-version', 'domain', '1', NULL, 'now');
        INSERT INTO measurement_frames(id, domain_id, label, created_at) VALUES ('frame', 'domain', NULL, 'now');
        INSERT INTO frame_versions(id, frame_id, version, domain_version_id, predecessor_id, created_at)
          VALUES ('frame-version', 'frame', '1', 'domain-version', NULL, 'now');
        INSERT INTO engine_runs(id, resolved_plan_json, status, started_at, completed_at, metadata_json)
          VALUES ('run', '{}', 'completed', 'now', 'now', '{}');
        INSERT INTO provenance(
          derived_id, run_id, operation, producer, algorithm, version, params_json,
          params_hash, source, timestamp, domain_version, frame_version, model
        ) VALUES
          ('profile-prov', 'run', 'calculated', 'test', 'profile', '1.0.0', '{}',
           'hash', NULL, 'now', NULL, NULL, NULL),
          ('value-prov', 'run', 'calculated', 'sensor.value', 'value', '1.0.0', '{}',
           'hash', NULL, 'now', NULL, NULL, NULL),
          ('sparse-prov', 'run', 'calculated', 'sensor.sparse', 'sparse', '1.0.0', '{}',
           'hash', NULL, 'now', NULL, NULL, NULL);
        "#,
    )
    .await
    .unwrap();
}

fn sensor_run(id: &str, sensor: &str) -> SensorRunRecord {
    SensorRunRecord {
        id: id.into(),
        engine_run_id: "run".into(),
        sensor: PluginId::new(sensor),
        sensor_version: semver::Version::new(1, 0, 0),
        params: json!({}),
        params_hash: ParameterHash::new("hash"),
        status: "completed".into(),
        started_at: "now".into(),
        completed_at: Some("now".into()),
    }
}

fn header(id: &str) -> MeasurementProfileHeader {
    MeasurementProfileHeader {
        id: id.into(),
        engine_run_id: "run".into(),
        observation_id: None,
        frame: FrameId::new("frame"),
        frame_version: FrameVersion::new("1"),
        provenance: DerivedId::new("profile-prov"),
        created_at: "now".into(),
    }
}

fn value_record(id: &str, value: f64) -> MeasurementRecord {
    MeasurementRecord {
        id: id.into(),
        sensor_run_id: "value-run".into(),
        provenance: DerivedId::new("value-prov"),
        kind: MeasurementKind::Scalar,
        measurement: Measurement {
            sensor: PluginId::new("sensor.value"),
            sensor_version: semver::Version::new(1, 0, 0),
            reading: Reading::Value {
                value: MeasurementValue::Scalar(value),
            },
            confidence: Some(0.9),
            sample_count: Some(3),
            context: MeasurementContext {
                values: [("axis".into(), json!(UnitId::new("x")))]
                    .into_iter()
                    .collect(),
            },
        },
    }
}

fn sparse_record() -> MeasurementRecord {
    MeasurementRecord {
        id: "b-sparse".into(),
        sensor_run_id: "sparse-run".into(),
        provenance: DerivedId::new("sparse-prov"),
        kind: MeasurementKind::Ranking,
        measurement: Measurement {
            sensor: PluginId::new("sensor.sparse"),
            sensor_version: semver::Version::new(1, 0, 0),
            reading: Reading::InsufficientEvidence { have: 2, need: 5 },
            confidence: None,
            sample_count: Some(2),
            context: MeasurementContext::default(),
        },
    }
}

#[tokio::test]
async fn typed_values_and_sparse_readings_round_trip() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    seed_parents(&db).await;
    let repo = SeaOrmMeasurementRepository::new(db);
    repo.insert_sensor_run(sensor_run("value-run", "sensor.value"))
        .await
        .unwrap();
    repo.insert_sensor_run(sensor_run("sparse-run", "sensor.sparse"))
        .await
        .unwrap();

    let value = value_record("a-value", 0.0);
    let sparse = sparse_record();
    let expected = MeasurementProfile {
        measurements: vec![value.measurement.clone(), sparse.measurement.clone()],
    };
    repo.insert_profile(header("profile"), vec![value, sparse])
        .await
        .unwrap();

    assert_eq!(
        repo.get_profile("profile").await.unwrap().unwrap(),
        expected
    );
}

#[tokio::test]
async fn invalid_nested_measurement_rolls_back_the_profile() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    seed_parents(&db).await;
    let repo = SeaOrmMeasurementRepository::new(db);
    repo.insert_sensor_run(sensor_run("value-run", "sensor.value"))
        .await
        .unwrap();

    let error = repo
        .insert_profile(
            header("invalid-profile"),
            vec![value_record("invalid", f64::NAN)],
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidRequest { .. }));
    assert!(repo.get_profile("invalid-profile").await.unwrap().is_none());
}

#[tokio::test]
async fn missing_nested_provenance_rolls_back_the_profile() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    seed_parents(&db).await;
    let repo = SeaOrmMeasurementRepository::new(db);
    repo.insert_sensor_run(sensor_run("value-run", "sensor.value"))
        .await
        .unwrap();
    let mut record = value_record("missing-provenance", 1.0);
    record.provenance = DerivedId::new("missing");

    repo.insert_profile(header("rolled-back-profile"), vec![record])
        .await
        .unwrap_err();

    assert!(repo
        .get_profile("rolled-back-profile")
        .await
        .unwrap()
        .is_none());
}
