use std::collections::BTreeMap;

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::json;
use unclip_domain::{DomainId, DomainSnapshot, PropertyValue, Unit, UnitId, UnitKind};
use unclip_epistemic::{
    hash_params, ops, DependencyCollector, Derived, DerivedId, EmitMetadata, EmitToken,
    OperationKind, PluginId, Timestamp, Tracked,
};
use unclip_measure::{Delta, MeasurementValue};
use unclip_observe::ObservationId;
use unclip_store::{
    connect_and_migrate, CandidateKind, CandidateProposal, CandidateRepository,
    CompletedExperimentBundle, DomainReader, DomainRevision, DomainRevisionRepository,
    ExperimentDelta, ExperimentMeasurementProfile, ExperimentOutcome, ExperimentRepository,
    MeasurementProfileHeader, ProvenanceRepository, SeaOrmDomainRepository,
    SeaOrmExperimentRepository, SeaOrmProvenanceRepository, StoredProvenance,
};

fn derived<T, O: OperationKind>(
    id: &str,
    producer: &str,
    inputs: &[&str],
    value: T,
) -> Derived<T, O> {
    let dependencies = DependencyCollector::default();
    for input in inputs {
        dependencies.read(&Tracked::from_recorded(DerivedId::new(*input), ()));
    }
    let params = json!({"fixture":true});
    EmitToken::from_harness(
        EmitMetadata {
            id: DerivedId::new(id),
            producer: PluginId::new(producer),
            algorithm: producer.into(),
            version: "0.1.0".parse().unwrap(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("done"),
            domain_version: None,
            frame_version: None,
            model: None,
        },
        dependencies,
    )
    .emit(value)
}
fn proposal() -> CandidateProposal {
    CandidateProposal {
        domain_version_id: "d1".into(),
        kind: CandidateKind::WeightRevision,
        value: json!({"weights":{"a":0.5}}).as_object().unwrap().clone(),
    }
}
fn candidate(id: &str) -> unclip_epistemic::Calculated<CandidateProposal> {
    derived(id, "generate.fixture", &["seed"], proposal())
}
fn outcome() -> ExperimentOutcome {
    ExperimentOutcome {
        candidate_id: DerivedId::new("candidate"),
        domain_version_id: "d1".into(),
        frame_version_id: "f1".into(),
        plan: json!({"comparators":["compare.scalar","compare.vector"]})
            .as_object()
            .unwrap()
            .clone(),
        result: json!({"constraints":{"pass":true},"null_models":[]})
            .as_object()
            .unwrap()
            .clone(),
        training: vec![ObservationId::new("training")],
        held_out: vec![ObservationId::new("held-2"), ObservationId::new("held-1")],
        started_at: "start".into(),
    }
}
const INPUTS: &[&str] = &[
    "candidate",
    "training-p",
    "held-1-p",
    "held-2-p",
    "scalar",
    "vector",
];
fn experiment(value: ExperimentOutcome) -> unclip_epistemic::Experimental<ExperimentOutcome> {
    derived("experiment", "experiment.fixture", INPUTS, value)
}
fn deltas() -> Vec<ExperimentDelta> {
    [
        ("scalar", MeasurementValue::Scalar(0.0)),
        ("vector", MeasurementValue::Vector(vec![1.0, -1.0])),
    ]
    .into_iter()
    .map(|(id, value)| {
        let producer = format!("compare.{id}");
        ExperimentDelta {
            before_profile_id: "before".into(),
            after_profile_id: "after".into(),
            calculated: derived(
                id,
                &producer,
                &["before-p", "after-p"],
                Delta {
                    comparator: PluginId::new(&producer),
                    value,
                },
            ),
        }
    })
    .collect()
}
fn revision() -> DomainRevision {
    DomainRevision {
        candidate_id: DerivedId::new("candidate"),
        experiment_id: DerivedId::new("experiment"),
        from_version_id: "d1".into(),
        to_version_id: "d2".into(),
        reason: "explicit weight revision".into(),
        evidence: json!({"constraints":"passed"}).as_object().unwrap().clone(),
    }
}
async fn setup() -> (
    DatabaseConnection,
    SeaOrmExperimentRepository,
    SeaOrmProvenanceRepository,
) {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    db.execute_unprepared(r#"
        INSERT INTO domains(id,created_at) VALUES ('d','start');
        INSERT INTO domain_versions(id,domain_id,version,predecessor_id,created_at) VALUES ('d1','d','1',NULL,'start'),('d2','d','2','d1','done');
        INSERT INTO measurement_frames(id,domain_id,created_at) VALUES ('f','d','start');
        INSERT INTO frame_versions(id,frame_id,version,domain_version_id,created_at) VALUES ('f1','f','1','d1','start');
        INSERT INTO engine_runs(id,resolved_plan_json,status,started_at) VALUES ('run','{}','running','start');
    "#).await.unwrap();
    let provenance = SeaOrmProvenanceRepository::new(db.clone());
    for id in [
        "seed",
        "training-p",
        "held-1-p",
        "held-2-p",
        "before-p",
        "after-p",
    ] {
        let value = derived::<_, ops::Inference>(id, "infer.fixture", &[], ());
        provenance
            .insert_provenance(unclip_store::StoredProvenance {
                id: value.id().clone(),
                run_id: None,
                provenance: value.provenance().clone(),
            })
            .await
            .unwrap();
    }
    db.execute_unprepared(r#"
        INSERT INTO observations(id,provenance_id,source) VALUES ('training','training-p','fixture'),('held-1','held-1-p','fixture'),('held-2','held-2-p','fixture');
        INSERT INTO measurement_profiles(id,engine_run_id,frame_version_id,provenance_id,created_at) VALUES
            ('before','run','f1','before-p','start'),('after','run','f1','after-p','done');
    "#).await.unwrap();
    let repo = SeaOrmExperimentRepository::new(db.clone());
    (db, repo, provenance)
}
async fn count(db: &DatabaseConnection, table: &str) -> i64 {
    db.query_one(Statement::from_string(
        DbBackend::Sqlite,
        format!("SELECT count(*) AS count FROM {table}"),
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "count")
    .unwrap()
}
async fn assert_no_experiment(db: &DatabaseConnection, provenance: &SeaOrmProvenanceRepository) {
    for table in [
        "experiments",
        "experiment_observations",
        "experiment_deltas",
    ] {
        assert_eq!(count(db, table).await, 0, "{table}");
    }
    for id in ["experiment", "scalar", "vector", "bundle-summary"] {
        assert!(
            provenance
                .get_provenance(&DerivedId::new(id))
                .await
                .unwrap()
                .is_none(),
            "{id}"
        );
    }
}

#[tokio::test]
async fn completed_evidence_round_trips_without_scalarizing_or_changing_domains() {
    let (db, repo, provenance) = setup().await;
    let c = candidate("candidate");
    repo.insert_candidate(Some("run".into()), c.clone())
        .await
        .unwrap();
    assert_eq!(
        repo.get_candidate(c.id()).await.unwrap().unwrap().proposal,
        *c.value()
    );
    assert_eq!(
        provenance
            .get_provenance(c.id())
            .await
            .unwrap()
            .unwrap()
            .provenance,
        *c.provenance()
    );
    let result = experiment(outcome());
    let comparisons = deltas();
    repo.insert_completed_experiment("run", result.clone(), comparisons.clone())
        .await
        .unwrap();
    let stored = repo
        .get_completed_experiment(result.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.outcome, *result.value());
    assert_eq!(stored.completed_at, "done");
    assert_eq!(stored.run_id, "run");
    assert_eq!(stored.deltas.len(), 2);
    for (stored, calculated) in stored.deltas.iter().zip(&comparisons) {
        assert_eq!(stored.delta, *calculated.calculated.value());
        assert_eq!(
            stored.comparator_version,
            calculated.calculated.provenance().version
        );
        assert_eq!(stored.before_profile_id, "before");
        assert_eq!(stored.after_profile_id, "after");
        assert_eq!(
            provenance
                .get_provenance(&stored.id)
                .await
                .unwrap()
                .unwrap()
                .provenance,
            *calculated.calculated.provenance()
        );
    }
    assert_eq!(
        provenance
            .get_provenance(result.id())
            .await
            .unwrap()
            .unwrap()
            .provenance,
        *result.provenance()
    );
    let rev =
        derived::<_, ops::Experiment>("revision", "revision.fixture", &["experiment"], revision());
    repo.insert_domain_revision(Some("run".into()), rev.clone())
        .await
        .unwrap();
    assert_eq!(
        repo.get_domain_revision(rev.id())
            .await
            .unwrap()
            .unwrap()
            .revision,
        *rev.value()
    );
    let ancestors = provenance.ancestors(rev.id()).await.unwrap();
    for expected in [
        "experiment",
        "candidate",
        "seed",
        "scalar",
        "vector",
        "held-1-p",
        "held-2-p",
        "before-p",
        "after-p",
    ] {
        assert!(ancestors.contains(&DerivedId::new(expected)));
    }
    assert_eq!(count(&db, "domain_versions").await, 2);
    assert!(repo
        .get_candidate(&DerivedId::new("absent"))
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .get_completed_experiment(&DerivedId::new("absent"))
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .get_domain_revision(&DerivedId::new("absent"))
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .insert_completed_experiment("run", result, comparisons)
        .await
        .is_err());
    assert_eq!(count(&db, "experiments").await, 1);
}

#[tokio::test]
async fn candidates_require_existing_evidence_and_roll_back_provenance_on_invalid_domain() {
    let (db, repo, provenance) = setup().await;
    for inputs in [vec![], vec!["absent"]] {
        assert!(repo
            .insert_candidate(
                None,
                derived("candidate", "generate.fixture", &inputs, proposal())
            )
            .await
            .is_err());
        assert!(provenance
            .get_provenance(&DerivedId::new("candidate"))
            .await
            .unwrap()
            .is_none());
    }
    let mut value = proposal();
    value.domain_version_id = "absent".into();
    assert!(repo
        .insert_candidate(
            None,
            derived("candidate", "generate.fixture", &["seed"], value)
        )
        .await
        .is_err());
    assert_eq!(count(&db, "candidates").await, 0);
    assert!(provenance
        .get_provenance(&DerivedId::new("candidate"))
        .await
        .unwrap()
        .is_none());
    repo.insert_candidate(None, candidate("candidate"))
        .await
        .unwrap();
    assert!(repo
        .insert_candidate(None, candidate("candidate"))
        .await
        .is_err());
    assert_eq!(count(&db, "candidates").await, 1);
}

#[tokio::test]
async fn experiments_reject_overlap_untracked_inputs_and_invalid_versions_atomically() {
    let (db, repo, provenance) = setup().await;
    repo.insert_candidate(None, candidate("candidate"))
        .await
        .unwrap();
    for change in 0..6 {
        let mut value = outcome();
        match change {
            0 => value.held_out.clear(),
            1 => value.held_out.push(ObservationId::new("training")),
            2 => value.held_out.push(ObservationId::new("absent")),
            3 => value.candidate_id = DerivedId::new("absent"),
            4 => value.frame_version_id = "absent".into(),
            _ => value.domain_version_id = "d2".into(),
        }
        assert!(repo
            .insert_completed_experiment("run", experiment(value), deltas())
            .await
            .is_err());
        assert_no_experiment(&db, &provenance).await;
    }
    for omitted in ["candidate", "held-1-p", "scalar"] {
        let inputs = INPUTS
            .iter()
            .copied()
            .filter(|id| *id != omitted)
            .collect::<Vec<_>>();
        let result = derived("experiment", "experiment.fixture", &inputs, outcome());
        assert!(repo
            .insert_completed_experiment("run", result, deltas())
            .await
            .is_err());
        assert_no_experiment(&db, &provenance).await;
    }
}

#[tokio::test]
async fn invalid_or_duplicate_deltas_roll_back_the_entire_bundle() {
    let (db, repo, provenance) = setup().await;
    repo.insert_candidate(None, candidate("candidate"))
        .await
        .unwrap();
    for change in 0..5 {
        let mut values = deltas();
        match change {
            0 => values[1].after_profile_id = "absent".into(),
            1 => {
                let duplicate = values[0].clone();
                values.push(duplicate);
            }
            2 => {
                values[1].calculated = derived(
                    "vector",
                    "compare.vector",
                    &["before-p"],
                    Delta {
                        comparator: PluginId::new("compare.vector"),
                        value: MeasurementValue::Vector(vec![1.0]),
                    },
                )
            }
            3 => {
                values[1].calculated = derived(
                    "vector",
                    "wrong-producer",
                    &["before-p", "after-p"],
                    Delta {
                        comparator: PluginId::new("compare.vector"),
                        value: MeasurementValue::Vector(vec![1.0]),
                    },
                )
            }
            _ => {
                values[1].calculated = derived(
                    "vector",
                    "compare.vector",
                    &["before-p", "after-p"],
                    Delta {
                        comparator: PluginId::new("compare.vector"),
                        value: MeasurementValue::Vector(vec![f64::NAN]),
                    },
                )
            }
        }
        assert!(repo
            .insert_completed_experiment("run", experiment(outcome()), values)
            .await
            .is_err());
        assert_no_experiment(&db, &provenance).await;
    }
}

#[tokio::test]
async fn revision_failures_do_not_leave_provenance_or_create_versions() {
    let (db, repo, provenance) = setup().await;
    repo.insert_candidate(None, candidate("candidate"))
        .await
        .unwrap();
    repo.insert_completed_experiment("run", experiment(outcome()), deltas())
        .await
        .unwrap();
    for change in 0..4 {
        let mut value = revision();
        match change {
            0 => value.to_version_id = "absent".into(),
            1 => value.candidate_id = DerivedId::new("absent"),
            2 => value.experiment_id = DerivedId::new("absent"),
            _ => value.reason = " ".into(),
        }
        assert!(repo
            .insert_domain_revision(
                None,
                derived("revision", "revision.fixture", &["experiment"], value)
            )
            .await
            .is_err());
        assert_eq!(count(&db, "domain_revisions").await, 0);
        assert!(provenance
            .get_provenance(&DerivedId::new("revision"))
            .await
            .unwrap()
            .is_none());
    }
    assert!(repo
        .insert_domain_revision(
            None,
            derived("revision", "revision.fixture", &["seed"], revision())
        )
        .await
        .is_err());
    assert_eq!(count(&db, "domain_versions").await, 2);
}

#[tokio::test]
async fn candidate_listing_is_domain_scoped_ordered_and_bounded() {
    let (_, repo, _) = setup().await;
    for id in ["c", "a", "b"] {
        repo.insert_candidate(None, candidate(id)).await.unwrap();
    }
    let first = repo.list_candidates("d1", None, 2).await.unwrap();
    assert_eq!(
        first.iter().map(|c| c.id.0.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(first[0].proposal, proposal());
    let next = repo
        .list_candidates("d1", Some(&first[1].id), 2)
        .await
        .unwrap();
    assert_eq!(
        next.iter().map(|c| c.id.0.as_str()).collect::<Vec<_>>(),
        vec!["c"]
    );
    assert!(repo
        .list_candidates("d2", None, 2)
        .await
        .unwrap()
        .is_empty());
    assert!(repo.list_candidates("d1", None, 0).await.is_err());
    assert!(repo.list_candidates("d1", None, 1001).await.is_err());
}

fn bundle_profile(id: &str, provenance: &str) -> ExperimentMeasurementProfile {
    ExperimentMeasurementProfile {
        header: MeasurementProfileHeader {
            id: id.into(),
            engine_run_id: "run".into(),
            observation_id: None,
            frame: unclip_domain::FrameId::new("f"),
            frame_version: unclip_epistemic::FrameVersion::new("1"),
            provenance: DerivedId::new(provenance),
            created_at: "start".into(),
        },
        sensor_runs: vec![],
        measurements: vec![],
    }
}

fn completed_bundle(value: ExperimentOutcome) -> CompletedExperimentBundle {
    let deltas = deltas()
        .into_iter()
        .map(|mut delta| {
            delta.before_profile_id = "bundle-before".into();
            delta.after_profile_id = "bundle-after".into();
            delta
        })
        .collect();
    CompletedExperimentBundle {
        prerequisite_provenance: vec![],
        profiles: vec![
            bundle_profile("bundle-before", "before-p"),
            bundle_profile("bundle-after", "after-p"),
        ],
        post_delta_provenance: {
            let value: unclip_epistemic::Calculated<()> = derived(
                "bundle-summary",
                "compare.profile",
                &["scalar", "vector"],
                (),
            );
            vec![StoredProvenance {
                id: value.id().clone(),
                run_id: Some("run".into()),
                provenance: value.provenance().clone(),
            }]
        },
        experiment: experiment(value),
        deltas,
    }
}

#[tokio::test]
async fn completed_bundle_inserts_profiles_deltas_and_experiment_atomically() {
    let (db, repo, provenance) = setup().await;
    repo.insert_candidate(Some("run".into()), candidate("candidate"))
        .await
        .unwrap();
    let bundle = completed_bundle(outcome());
    repo.insert_completed_experiment_bundle("run", bundle)
        .await
        .unwrap();
    let stored = repo
        .get_completed_experiment(&DerivedId::new("experiment"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.deltas.len(), 2);
    assert!(stored
        .deltas
        .iter()
        .all(|delta| delta.before_profile_id == "bundle-before"
            && delta.after_profile_id == "bundle-after"));
    assert_eq!(count(&db, "measurement_profiles").await, 4);
    assert!(provenance
        .get_provenance(&DerivedId::new("bundle-summary"))
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn completed_bundle_rolls_back_staged_profiles_and_delta_provenance() {
    let (db, repo, provenance) = setup().await;
    repo.insert_candidate(Some("run".into()), candidate("candidate"))
        .await
        .unwrap();
    let mut value = outcome();
    value.frame_version_id = "absent".into();
    assert!(repo
        .insert_completed_experiment_bundle("run", completed_bundle(value))
        .await
        .is_err());
    assert_eq!(count(&db, "measurement_profiles").await, 2);
    assert_no_experiment(&db, &provenance).await;
}

fn successor(version: &str) -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new("d"),
        version: unclip_epistemic::DomainVersion::new(version),
        units: BTreeMap::from([(
            UnitId::new("accepted"),
            Unit {
                id: UnitId::new("accepted"),
                kind: UnitKind::AtomicMeaning,
                label: None,
                properties: BTreeMap::from([(
                    "candidate_evidence".into(),
                    PropertyValue::Structured(json!({"candidate": "candidate"})),
                )]),
            },
        )]),
        relations: BTreeMap::new(),
    }
}

fn applied_revision(id: &str, version: &str) -> unclip_epistemic::Experimental<DomainRevision> {
    let to_version_id = serde_json::to_string(&("d", version)).unwrap();
    derived(
        id,
        "revision.apply",
        &["experiment"],
        DomainRevision {
            candidate_id: DerivedId::new("candidate"),
            experiment_id: DerivedId::new("experiment"),
            from_version_id: "d1".into(),
            to_version_id,
            reason: "held-out evidence accepted the candidate".into(),
            evidence: json!({"verdict": "sufficient"})
                .as_object()
                .unwrap()
                .clone(),
        },
    )
}

#[tokio::test]
async fn accepted_revision_creates_one_immutable_successor_and_rejects_stale_baselines() {
    let (db, repo, provenance) = setup().await;
    db.execute_unprepared("DELETE FROM domain_versions WHERE id = 'd2'")
        .await
        .unwrap();
    repo.insert_candidate(None, candidate("candidate"))
        .await
        .unwrap();
    repo.insert_completed_experiment("run", experiment(outcome()), deltas())
        .await
        .unwrap();

    let snapshot = successor("2");
    let revision = applied_revision("applied-revision", "2");
    repo.apply_domain_revision(Some("run".into()), snapshot.clone(), revision.clone())
        .await
        .unwrap();

    let domains = SeaOrmDomainRepository::new(db.clone());
    let baseline = domains
        .get_domain_version(
            &DomainId::new("d"),
            &unclip_epistemic::DomainVersion::new("1"),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(baseline.units.is_empty());
    assert_eq!(
        domains
            .get_domain_version(
                &DomainId::new("d"),
                &unclip_epistemic::DomainVersion::new("2"),
            )
            .await
            .unwrap(),
        Some(snapshot)
    );
    let predecessor: String = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT predecessor_id FROM domain_versions WHERE version = '2'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "predecessor_id")
        .unwrap();
    assert_eq!(predecessor, "d1");
    assert_eq!(
        repo.get_domain_revision(revision.id())
            .await
            .unwrap()
            .unwrap()
            .revision,
        *revision.value()
    );

    let stale = applied_revision("stale-revision", "3");
    assert!(matches!(
        repo.apply_domain_revision(None, successor("3"), stale.clone())
            .await
            .unwrap_err(),
        unclip_store::StoreError::Conflict { .. }
    ));
    assert!(domains
        .get_domain_version(
            &DomainId::new("d"),
            &unclip_epistemic::DomainVersion::new("3"),
        )
        .await
        .unwrap()
        .is_none());
    assert!(provenance
        .get_provenance(stale.id())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn failed_revision_application_rolls_back_the_successor_and_provenance() {
    let (db, repo, provenance) = setup().await;
    db.execute_unprepared("DELETE FROM domain_versions WHERE id = 'd2'")
        .await
        .unwrap();
    repo.insert_candidate(None, candidate("candidate"))
        .await
        .unwrap();
    repo.insert_completed_experiment("run", experiment(outcome()), deltas())
        .await
        .unwrap();

    let mut invalid = applied_revision("invalid-revision", "2");
    invalid = derived(
        "invalid-revision",
        "revision.apply",
        &["experiment"],
        DomainRevision {
            candidate_id: DerivedId::new("other"),
            ..invalid.value().clone()
        },
    );
    assert!(repo
        .apply_domain_revision(None, successor("2"), invalid.clone())
        .await
        .is_err());

    let domains = SeaOrmDomainRepository::new(db.clone());
    assert!(domains
        .get_domain_version(
            &DomainId::new("d"),
            &unclip_epistemic::DomainVersion::new("2"),
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(count(&db, "domain_versions").await, 1);
    assert_eq!(count(&db, "domain_revisions").await, 0);
    assert!(provenance
        .get_provenance(invalid.id())
        .await
        .unwrap()
        .is_none());
}
