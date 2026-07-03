use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

#[tokio::test]
async fn frame_value_constraints_reject_ambiguous_rows() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    unclip_migration::up(&db, None).await.unwrap();
    let exec = |sql: &'static str| db.execute(Statement::from_string(DbBackend::Sqlite, sql));

    exec("INSERT INTO frames (id, name) VALUES (1, 'f')")
        .await
        .unwrap();
    exec("INSERT INTO frame_slots (id, frame_id, name) VALUES (1, 1, 's')")
        .await
        .unwrap();

    exec(
        "INSERT INTO frame_slot_o2o_values (slot_id, mode, name, value) \
         VALUES (1, 'require', 'axis', 'place')",
    )
    .await
    .unwrap();
    assert!(
        exec(
            "INSERT INTO frame_slot_o2o_values (slot_id, mode, name, value) \
             VALUES (1, 'require', 'axis', 'character')"
        )
        .await
        .is_err(),
        "a slot cannot have two o2o values for the same mode and name"
    );

    exec(
        "INSERT INTO frame_slot_o2m_values (slot_id, mode, name, value) \
         VALUES (1, 'require', 'topic', 'transit')",
    )
    .await
    .unwrap();
    assert!(
        exec(
            "INSERT INTO frame_slot_o2m_values (slot_id, mode, name, value) \
             VALUES (1, 'require', 'topic', 'transit')"
        )
        .await
        .is_err(),
        "duplicate o2m constraint values must be rejected"
    );
}

#[tokio::test]
async fn pattern_constraints_reject_malformed_direct_writes() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    unclip_migration::up(&db, None).await.unwrap();
    let exec = |sql: &'static str| db.execute(Statement::from_string(DbBackend::Sqlite, sql));

    exec(
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('station', 'o2m', 'topic', 'transit', 1)",
    )
    .await
    .unwrap();

    for invalid in [
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES (' ', 'o2m', 'topic', 'transit', 1)",
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('station', 'unknown', NULL, '/station', 1)",
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('station', 'o2m', NULL, 'transit', 1)",
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('station', 'branch', NULL, 'relative', 1)",
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('station', 'branch', NULL, '/bad//path', 1)",
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('station', 'branch', NULL, '/trailing/', 1)",
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('station', 'branch', NULL, '/white space', 1)",
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('station', 'branch', NULL, '/station', 2)",
    ] {
        assert!(
            exec(invalid).await.is_err(),
            "malformed pattern row must be rejected: {invalid}"
        );
    }

    assert!(
        exec("UPDATE pattern_entries SET enabled = 3 WHERE pattern = 'station'")
            .await
            .is_err(),
        "updates must enforce the same constraints as inserts"
    );
}

#[tokio::test]
async fn hardening_rejects_invalid_existing_rows_atomically() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    unclip_migration::up(&db, Some(3)).await.unwrap();
    db.execute_unprepared(
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('bad', 'unknown', NULL, NULL, 1)",
    )
    .await
    .unwrap();

    assert!(unclip_migration::up(&db, None).await.is_err());

    let objects = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master \
             WHERE name IN ('idx_frame_slot_o2o_identity', \
                            'pattern_entries_validate_insert')",
        ))
        .await
        .unwrap();
    assert!(
        objects.is_empty(),
        "failed hardening must roll back indexes and triggers"
    );
}

#[tokio::test]
async fn path_hardening_validates_existing_version_four_databases() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    unclip_migration::up(&db, Some(4)).await.unwrap();
    db.execute_unprepared(
        "INSERT INTO pattern_entries \
         (pattern, target_kind, target_name, target_value, enabled) \
         VALUES ('bad path', 'branch', NULL, '/bad//path', 1)",
    )
    .await
    .unwrap();

    assert!(unclip_migration::up(&db, None).await.is_err());

    let triggers = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master \
             WHERE name = 'pattern_entries_validate_path_insert'",
        ))
        .await
        .unwrap();
    assert!(
        triggers.is_empty(),
        "a failed path-hardening migration must roll back its triggers"
    );

    db.execute_unprepared("DELETE FROM pattern_entries WHERE pattern = 'bad path'")
        .await
        .unwrap();
    unclip_migration::up(&db, None).await.unwrap();
    assert!(db
        .execute_unprepared(
            "INSERT INTO pattern_entries \
             (pattern, target_kind, target_name, target_value, enabled) \
             VALUES ('still bad', 'branch', NULL, '/bad//path', 1)",
        )
        .await
        .is_err());
}

#[tokio::test]
async fn branch_constraints_reject_malformed_direct_writes() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    unclip_migration::up(&db, None).await.unwrap();
    let exec = |sql: &'static str| db.execute(Statement::from_string(DbBackend::Sqlite, sql));

    exec(
        "INSERT INTO branches (id, path, weight, created_at, updated_at) \
         VALUES (1, '/valid', 1.0, 'now', 'now')",
    )
    .await
    .unwrap();

    for invalid in [
        "INSERT INTO branches (path, weight, created_at, updated_at) \
         VALUES ('relative', 1.0, 'now', 'now')",
        "INSERT INTO branches (path, weight, created_at, updated_at) \
         VALUES ('/bad//path', 1.0, 'now', 'now')",
        "INSERT INTO branches (path, weight, created_at, updated_at) \
         VALUES ('/bad path', 1.0, 'now', 'now')",
        "INSERT INTO branches (path, weight, created_at, updated_at) \
         VALUES ('/negative', -1.0, 'now', 'now')",
        "INSERT INTO branch_o2o_values (branch_id, name, value) \
         VALUES (1, '', 'place')",
        "INSERT INTO branch_o2m_values (branch_id, name, value) \
         VALUES (1, 'topic', '')",
        "INSERT INTO branch_references (branch_id, type, value) \
         VALUES (1, '', 'ref')",
    ] {
        assert!(
            exec(invalid).await.is_err(),
            "malformed branch data must be rejected: {invalid}"
        );
    }

    assert!(exec(
        "INSERT INTO branch_o2o_values (branch_id, name, value) \
         VALUES (1, 'axis' || char(10), 'place')"
    )
    .await
    .is_err());
    assert!(exec("UPDATE branches SET weight = -1 WHERE id = 1")
        .await
        .is_err());
}

#[tokio::test]
async fn branch_hardening_validates_existing_version_five_databases() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    unclip_migration::up(&db, Some(5)).await.unwrap();
    db.execute_unprepared(
        "INSERT INTO branches (path, weight, created_at, updated_at) \
         VALUES ('relative', 1.0, 'now', 'now')",
    )
    .await
    .unwrap();

    assert!(unclip_migration::up(&db, None).await.is_err());

    let triggers = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master \
             WHERE name = 'branches_validate_insert'",
        ))
        .await
        .unwrap();
    assert!(
        triggers.is_empty(),
        "a failed branch-hardening migration must roll back its triggers"
    );

    db.execute_unprepared("DELETE FROM branches WHERE path = 'relative'")
        .await
        .unwrap();
    unclip_migration::up(&db, None).await.unwrap();
    assert!(db
        .execute_unprepared(
            "INSERT INTO branches (path, weight, created_at, updated_at) \
             VALUES ('still-relative', 1.0, 'now', 'now')",
        )
        .await
        .is_err());
}
