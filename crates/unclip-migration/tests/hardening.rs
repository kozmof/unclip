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
