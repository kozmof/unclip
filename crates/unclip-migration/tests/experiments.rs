use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};

async fn count(db: &DatabaseConnection, sql: &str) -> i64 {
    db.query_one(Statement::from_string(DbBackend::Sqlite, sql))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap()
}

async fn seed(db: &DatabaseConnection) {
    db.execute_unprepared(r#"
        INSERT INTO domains(id, created_at) VALUES ('d','now'), ('other','now');
        INSERT INTO domain_versions(id,domain_id,version,predecessor_id,created_at) VALUES
            ('d1','d','1',NULL,'now'), ('d2','d','2','d1','now'),
            ('d3','d','3','d2','now'), ('other1','other','1',NULL,'now');
        INSERT INTO measurement_frames(id,domain_id,created_at) VALUES ('f','d','now');
        INSERT INTO frame_versions(id,frame_id,version,domain_version_id,created_at) VALUES
            ('f1','f','1','d1','now'), ('f2','f','2','d2','now');
        INSERT INTO engine_runs(id,resolved_plan_json,status,started_at) VALUES ('run','{}','running','now');
        INSERT INTO provenance(derived_id,operation,producer,algorithm,version,params_json,params_hash,timestamp) VALUES
            ('evidence','inferred','fixture','fixture','1','{}','hash','now'),
            ('candidate','calculated','fixture','fixture','1','{}','hash','now'),
            ('candidate2','calculated','fixture','fixture','1','{}','hash','now'),
            ('experiment','experimental','fixture','fixture','1','{}','hash','now'),
            ('delta','calculated','fixture','fixture','1','{}','hash','now'),
            ('revision','experimental','fixture','fixture','1','{}','hash','now'),
            ('spare','calculated','fixture','fixture','1','{}','hash','now');
        INSERT INTO provenance_inputs(derived_id,input_derived_id,position) VALUES
            ('candidate','evidence',0), ('experiment','candidate',0), ('experiment','evidence',1),
            ('delta','experiment',0), ('revision','experiment',0);
        INSERT INTO observations(id,provenance_id,source) VALUES ('training','evidence','fixture'),('held-out','evidence','fixture');
        INSERT INTO measurement_profiles(id,engine_run_id,frame_version_id,provenance_id,created_at) VALUES
            ('before','run','f1','evidence','now'), ('after','run','f1','evidence','now');
    "#).await.unwrap();
}
async fn candidate(db: &DatabaseConnection) {
    db.execute_unprepared("INSERT INTO candidates(id,domain_version_id,kind,value_json,provenance_id,created_at) VALUES ('c','d1','weight_revision','{\"weights\":{}}','candidate','now')").await.unwrap();
}
async fn experiment(db: &DatabaseConnection) {
    db.execute_unprepared("INSERT INTO experiments(id,engine_run_id,candidate_id,domain_version_id,frame_version_id,plan_json,status,provenance_id,started_at) VALUES ('exp','run','c','d1','f1','{}','planned','experiment','now')").await.unwrap();
}
async fn database() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    db.execute_unprepared("PRAGMA foreign_keys=ON")
        .await
        .unwrap();
    unclip_migration::up(&db, None).await.unwrap();
    seed(&db).await;
    candidate(&db).await;
    experiment(&db).await;
    db
}

#[tokio::test]
async fn upgrade_and_rollback_preserve_existing_measurement_data() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    db.execute_unprepared("PRAGMA foreign_keys=ON")
        .await
        .unwrap();
    unclip_migration::up(&db, Some(12)).await.unwrap();
    seed(&db).await;
    unclip_migration::up(&db, None).await.unwrap();
    candidate(&db).await;
    experiment(&db).await;
    assert_eq!(
        count(&db, "SELECT count(*) AS count FROM measurement_profiles").await,
        2
    );
    assert_eq!(count(&db,"SELECT count(*) AS count FROM sqlite_master WHERE type='table' AND name IN ('candidates','experiments','experiment_observations','experiment_deltas','domain_revisions')").await,5);
    assert_eq!(count(&db,"SELECT count(*) AS count FROM sqlite_master WHERE type='index' AND name LIKE 'idx_experiment%'").await,7);
    unclip_migration::down(&db, Some(2)).await.unwrap();
    assert_eq!(
        count(&db, "SELECT count(*) AS count FROM measurement_profiles").await,
        2
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) AS count FROM sqlite_master WHERE type='table' AND name='candidates'"
        )
        .await,
        0
    );
    unclip_migration::up(&db, None).await.unwrap();
    candidate(&db).await;
    experiment(&db).await;
}

#[tokio::test]
async fn evidence_splits_and_typed_deltas_preserve_independent_results() {
    let db = database().await;
    db.execute_unprepared(r#"
        INSERT INTO experiment_observations VALUES ('exp','training','training',0),('exp','held-out','held_out',0);
        INSERT INTO experiment_deltas(id,experiment_id,before_profile_id,after_profile_id,comparator_id,comparator_version,kind,value_json,provenance_id,created_at)
        VALUES ('delta','exp','before','after','compare.scalar','1','scalar','0','delta','now');
    "#).await.unwrap();
    for sql in [
        "INSERT INTO experiment_observations VALUES ('exp','training','held_out',1)",
        "INSERT INTO experiment_observations VALUES ('exp','absent','held_out',1)",
        "INSERT INTO experiment_observations VALUES ('exp','held-out','test',1)",
        "UPDATE candidates SET value_json='{}' WHERE id='c'",
        "UPDATE experiment_deltas SET value_json='1' WHERE id='delta'",
        "DELETE FROM provenance WHERE derived_id='candidate'",
        "DELETE FROM candidates WHERE id='c'",
        "DELETE FROM measurement_profiles WHERE id='before'",
    ] {
        assert!(db.execute_unprepared(sql).await.is_err(), "accepted {sql}");
    }
    assert_eq!(count(&db,"SELECT count(*) AS count FROM experiment_deltas WHERE kind='scalar' AND value_json='0'").await,1);
    assert_eq!(
        count(
            &db,
            "SELECT count(*) AS count FROM provenance_inputs WHERE derived_id='experiment'"
        )
        .await,
        2
    );
}

#[tokio::test]
async fn experiments_require_consistent_versions_and_valid_transitions() {
    let db = database().await;
    for sql in [
        "UPDATE experiments SET frame_version_id='f2' WHERE id='exp'",
        "UPDATE experiments SET status='completed',result_json='{}',completed_at='later' WHERE id='exp'",
        "UPDATE experiments SET status='running',result_json='{}' WHERE id='exp'",
        "UPDATE experiments SET status='failed' WHERE id='exp'",
        "INSERT INTO candidates VALUES ('bad','d1','label','{}','spare','now')",
        "INSERT INTO candidates VALUES ('bad','d1','relation','[]','spare','now')",
        "INSERT INTO candidates VALUES ('bad','d1','relation','{','spare','now')",
        "INSERT INTO candidates VALUES ('bad','d1','relation','{}','missing','now')",
        "INSERT INTO experiments VALUES ('bad','run','c','d2','f2','{}','planned',NULL,'spare','now',NULL)",
        "INSERT INTO experiments VALUES ('bad','run','c','d1','f2','{}','planned',NULL,'spare','now',NULL)",
    ] { assert!(db.execute_unprepared(sql).await.is_err(),"accepted {sql}"); }
    db.execute_unprepared("UPDATE experiments SET status='running' WHERE id='exp'; UPDATE experiments SET status='completed',result_json='{\"deltas\":[]}',completed_at='later' WHERE id='exp'").await.unwrap();
    assert!(db.execute_unprepared("UPDATE experiments SET status='running',result_json=NULL,completed_at=NULL WHERE id='exp'").await.is_err());
}

#[tokio::test]
async fn revisions_require_completed_evidence_and_successive_domain_versions() {
    let db = database().await;
    let revision = "INSERT INTO domain_revisions VALUES ('rev','c','exp','d1','d2','weight adjustment','{}','revision','now')";
    assert!(db.execute_unprepared(revision).await.is_err());
    db.execute_unprepared("UPDATE experiments SET status='running' WHERE id='exp'; UPDATE experiments SET status='completed',result_json='{}',completed_at='later' WHERE id='exp'").await.unwrap();
    for target in ["d1", "d3", "other1"] {
        let sql = format!("INSERT INTO domain_revisions VALUES ('rev','c','exp','d1','{target}','reason','{{}}','revision','now')");
        assert!(db.execute_unprepared(&sql).await.is_err());
    }
    db.execute_unprepared(
        "INSERT INTO candidates VALUES ('c2','d1','relation','{}','candidate2','now')",
    )
    .await
    .unwrap();
    assert!(db.execute_unprepared("INSERT INTO domain_revisions VALUES ('rev','c2','exp','d1','d2','reason','{}','revision','now')").await.is_err());
    db.execute_unprepared(revision).await.unwrap();
    assert!(db
        .execute_unprepared("UPDATE domain_revisions SET reason='changed' WHERE id='rev'")
        .await
        .is_err());
    assert!(db
        .execute_unprepared("DELETE FROM experiments WHERE id='exp'")
        .await
        .is_err());
    assert_eq!(
        count(&db, "SELECT count(*) AS count FROM domain_revisions").await,
        1
    );
    // A recorded revision does not create or modify domain versions itself.
    assert_eq!(
        count(&db, "SELECT count(*) AS count FROM domain_versions").await,
        4
    );
}

#[tokio::test]
async fn observation_selection_is_frozen_when_an_experiment_starts() {
    let db = database().await;
    db.execute_unprepared("INSERT INTO experiment_observations VALUES ('exp','training','training',0); UPDATE experiments SET status='running' WHERE id='exp'").await.unwrap();
    for sql in [
        "INSERT INTO experiment_observations VALUES ('exp','held-out','held_out',0)",
        "UPDATE experiment_observations SET split='held_out' WHERE experiment_id='exp'",
        "DELETE FROM experiment_observations WHERE experiment_id='exp'",
    ] {
        assert!(db.execute_unprepared(sql).await.is_err(), "accepted {sql}");
    }
    db.execute_unprepared(
        "UPDATE experiments SET status='failed',completed_at='later' WHERE id='exp'",
    )
    .await
    .unwrap();
    assert_eq!(count(&db,"SELECT count(*) AS count FROM experiments WHERE status='failed' AND result_json IS NULL").await,1);
}
