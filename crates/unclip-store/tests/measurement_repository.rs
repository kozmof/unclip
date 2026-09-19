use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::json;
use unclip_domain::{FrameId, UnitId};
use unclip_epistemic::{DerivedId, FrameVersion, ParameterHash, PluginId};
use unclip_measure::{
    EmpiricalStructure, Measurement, MeasurementContext, MeasurementKind, MeasurementProfile,
    MeasurementValue, Reading,
};
use unclip_store::{
    connect_and_migrate, EmpiricalStructureRecord, MeasurementProfileHeader, MeasurementRecord,
    MeasurementRepository, SeaOrmMeasurementRepository, SensorRunRecord, StoreError,
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

#[tokio::test]
async fn empirical_structure_round_trips_with_profile_link() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    seed_parents(&db).await;
    let repo = SeaOrmMeasurementRepository::new(db);
    repo.insert_profile(header("structure-profile"), Vec::new())
        .await
        .unwrap();
    let structure = EmpiricalStructureRecord {
        id: "structure".into(),
        profile_id: Some("structure-profile".into()),
        provenance: DerivedId::new("value-prov"),
        created_at: "now".into(),
        structure: EmpiricalStructure {
            kind: "cluster".into(),
            value: json!({"members": ["a", "b"]}),
        },
    };

    repo.insert_empirical_structure(structure.clone())
        .await
        .unwrap();

    assert_eq!(
        repo.get_empirical_structure("structure")
            .await
            .unwrap()
            .unwrap(),
        structure
    );
}

#[tokio::test]
async fn pairwise_matrix_preserves_labels_sparse_cells_and_exact_values() {
    use unclip_measure::{
        pairwise_matrix, PairwiseMetric, RankPosition, RankSample, RankTrajectory,
    };
    use unclip_observe::ObservationId;

    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    seed_parents(&db).await;
    let repo = SeaOrmMeasurementRepository::new(db);
    repo.insert_sensor_run(sensor_run("value-run", "sensor.value"))
        .await
        .unwrap();
    let trajectories = ["a", "b", "c"].map(|unit| RankTrajectory {
        unit: UnitId::new(unit),
        samples: (0..3)
            .map(|i| RankSample {
                observation: ObservationId::new(i.to_string()),
                position: if unit == "c" {
                    RankPosition::Unknown
                } else {
                    RankPosition::Ranked { rank: i + 1 }
                },
            })
            .collect(),
    });
    let matrix = pairwise_matrix(&trajectories, PairwiseMetric::MutualInformation).unwrap();
    let mut record = value_record("matrix", 0.0);
    record.kind = MeasurementKind::Matrix;
    record.measurement.reading = Reading::Value {
        value: MeasurementValue::PairwiseMatrix(matrix),
    };
    let expected = MeasurementProfile {
        measurements: vec![record.measurement.clone()],
    };
    repo.insert_profile(header("matrix-profile"), vec![record])
        .await
        .unwrap();
    assert_eq!(
        repo.get_profile("matrix-profile").await.unwrap().unwrap(),
        expected
    );
}

fn calculated_structure(
    id: &str,
    inputs: &[&str],
    value: EmpiricalStructure,
) -> unclip_epistemic::Calculated<EmpiricalStructure> {
    use unclip_epistemic::{
        hash_params, CalculationToken, DependencyCollector, DomainVersion, EmitMetadata, Timestamp,
        Tracked,
    };
    let dependencies = DependencyCollector::default();
    for input in inputs {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(*input), ()));
    }
    let params = json!({"threshold": 0.5, "minimum_samples": 2, "tolerance": 1e-12, "max_sweeps": 100, "window": 2, "minimum_shift": 2.0, "regime_starts": [0, 2]});
    CalculationToken::from_harness(
        EmitMetadata {
            id: DerivedId::new(id),
            producer: PluginId::new("structure.fixture"),
            algorithm: value.kind.clone(),
            version: semver::Version::new(1, 0, 0),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("2026-09-19T00:00:00Z"),
            domain_version: Some(DomainVersion::new("domain-version")),
            frame_version: Some(FrameVersion::new("frame-version")),
            model: None,
        },
        dependencies,
    )
    .emit(value)
}

#[tokio::test]
async fn calculated_empirical_payloads_round_trip_with_complete_provenance() {
    use std::num::NonZeroUsize;
    use unclip_epistemic::Operation;
    use unclip_measure::{
        detect_change_points, detect_communities, pairwise_matrix, spectral_decomposition,
        ChangePointDetection, CommunityDetection, ObservationSequence, OrderedObservation,
        PairwiseMetric, RankPosition, RankSample, RankTrajectory, SpectralDecomposition,
    };
    use unclip_observe::ObservationId;
    use unclip_store::{ProvenanceRepository, SeaOrmProvenanceRepository};

    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    seed_parents(&db).await;
    let provenance = SeaOrmProvenanceRepository::new(db.clone());
    let repo = SeaOrmMeasurementRepository::new(db);
    repo.insert_profile(header("empirical-profile"), Vec::new())
        .await
        .unwrap();
    let trajectories = ["a", "b"].map(|unit| RankTrajectory {
        unit: UnitId::new(unit),
        samples: [1, 1, 3, 3]
            .iter()
            .enumerate()
            .map(|(index, &rank)| RankSample {
                observation: ObservationId::new(index.to_string()),
                position: RankPosition::Ranked { rank },
            })
            .collect(),
    });
    let matrix = pairwise_matrix(&trajectories, PairwiseMetric::Spearman).unwrap();
    let two = NonZeroUsize::new(2).unwrap();
    let communities = detect_communities(&matrix, 0.5, two).unwrap().unwrap();
    let spectral = spectral_decomposition(&matrix, two, 1e-12, NonZeroUsize::new(100).unwrap())
        .unwrap()
        .unwrap();
    let sequence = ObservationSequence::new(
        (0..4)
            .map(|index| OrderedObservation {
                observation: ObservationId::new(index.to_string()),
                position: index,
            })
            .collect(),
    )
    .unwrap();
    let changes = detect_change_points(&sequence, &trajectories[0], two, 2.0)
        .unwrap()
        .unwrap();
    let regimes = unclip_measure::RegimePartition::new(sequence.clone(), vec![0, 2]).unwrap();
    let payloads = [
        EmpiricalStructure::try_from(communities.clone()).unwrap(),
        EmpiricalStructure::try_from(spectral.clone()).unwrap(),
        EmpiricalStructure::try_from(changes.clone()).unwrap(),
        EmpiricalStructure::try_from(regimes.clone()).unwrap(),
    ];
    for payload in payloads {
        let derived = calculated_structure(
            &payload.kind,
            &["value-prov", "profile-prov"],
            payload.clone(),
        );
        repo.insert_calculated_structure(
            Some("run".into()),
            Some("empirical-profile".into()),
            derived.clone(),
        )
        .await
        .unwrap();
        let record = repo
            .get_empirical_structure(&payload.kind)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.structure, payload);
        assert_eq!(record.provenance, *derived.id());
        assert_eq!(record.created_at, derived.provenance().timestamp.0);
        assert_eq!(record.profile_id.as_deref(), Some("empirical-profile"));
        let stored = provenance
            .get_provenance(derived.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.provenance, *derived.provenance());
        assert_eq!(stored.provenance.operation, Operation::Calculated);
        assert_eq!(stored.run_id.as_deref(), Some("run"));
        assert_eq!(
            provenance.ancestors(derived.id()).await.unwrap(),
            vec![DerivedId::new("profile-prov"), DerivedId::new("value-prov")]
        );
        repo.insert_calculated_structure(
            Some("run".into()),
            Some("empirical-profile".into()),
            derived,
        )
        .await
        .unwrap_err();
        assert_eq!(
            repo.get_empirical_structure(&payload.kind)
                .await
                .unwrap()
                .unwrap(),
            record
        );
        match payload.kind.as_str() {
            "communities" => assert_eq!(
                CommunityDetection::try_from(&record.structure).unwrap(),
                communities
            ),
            "spectral" => assert_eq!(
                SpectralDecomposition::try_from(&record.structure).unwrap(),
                spectral
            ),
            "change_points" => assert_eq!(
                ChangePointDetection::try_from(&record.structure).unwrap(),
                changes
            ),
            "regimes" => assert_eq!(
                unclip_measure::RegimePartition::try_from(&record.structure).unwrap(),
                regimes
            ),
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn calculated_structure_failures_roll_back_provenance_edges_and_payload() {
    use unclip_store::{ProvenanceRepository, SeaOrmProvenanceRepository};
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    seed_parents(&db).await;
    let provenance = SeaOrmProvenanceRepository::new(db.clone());
    let repo = SeaOrmMeasurementRepository::new(db);
    let payload = EmpiricalStructure {
        kind: "anonymous_test_structure".into(),
        value: json!({"members":["a"]}),
    };
    for (id, inputs, profile, kind) in [
        (
            "missing-input",
            vec!["profile-prov", "z-missing"],
            None,
            "test",
        ),
        (
            "missing-profile",
            vec!["value-prov"],
            Some("absent-profile"),
            "test",
        ),
        ("empty-kind", vec!["value-prov"], None, ""),
        ("no-evidence", vec![], None, "test"),
    ] {
        let mut value = payload.clone();
        value.kind = kind.into();
        let derived = calculated_structure(id, &inputs, value);
        repo.insert_calculated_structure(Some("run".into()), profile.map(str::to_owned), derived)
            .await
            .unwrap_err();
        assert!(repo.get_empirical_structure(id).await.unwrap().is_none());
        assert!(provenance
            .get_provenance(&DerivedId::new(id))
            .await
            .unwrap()
            .is_none());
    }
    let legacy = EmpiricalStructureRecord {
        id: "collision".into(),
        profile_id: None,
        provenance: DerivedId::new("value-prov"),
        created_at: "before".into(),
        structure: payload.clone(),
    };
    repo.insert_empirical_structure(legacy.clone())
        .await
        .unwrap();
    let derived = calculated_structure("collision", &["value-prov"], payload);
    assert!(matches!(
        repo.insert_calculated_structure(Some("run".into()), None, derived)
            .await
            .unwrap_err(),
        StoreError::AlreadyExists { .. }
    ));
    assert!(provenance
        .get_provenance(&DerivedId::new("collision"))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.get_empirical_structure("collision")
            .await
            .unwrap()
            .unwrap(),
        legacy
    );
}
