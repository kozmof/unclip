//! unclip-migration — SeaORM migrations defining the SQLite schema.

#![forbid(unsafe_code)]

pub use sea_orm_migration::prelude::*;

mod m20260620_000001_create_core_tables;
mod m20260620_000002_create_pattern_entries;
mod m20260702_000003_add_frame_slot_position;
mod m20260703_000004_harden_domain_constraints;
mod m20260703_000005_harden_pattern_paths;
mod m20260703_000006_harden_branch_records;
mod m20260705_000007_multi_value_avoid_o2o;
mod m20260918_000008_create_provenance_and_runs;
mod m20260918_000009_create_domain;
mod m20260918_000010_create_measurement_frames;
mod m20260918_000011_create_observations;
mod m20260918_000012_create_measurements;
mod m20260919_000013_create_experiments;

struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260620_000001_create_core_tables::Migration),
            Box::new(m20260620_000002_create_pattern_entries::Migration),
            Box::new(m20260702_000003_add_frame_slot_position::Migration),
            Box::new(m20260703_000004_harden_domain_constraints::Migration),
            Box::new(m20260703_000005_harden_pattern_paths::Migration),
            Box::new(m20260703_000006_harden_branch_records::Migration),
            Box::new(m20260705_000007_multi_value_avoid_o2o::Migration),
            Box::new(m20260918_000008_create_provenance_and_runs::Migration),
            Box::new(m20260918_000009_create_domain::Migration),
            Box::new(m20260918_000010_create_measurement_frames::Migration),
            Box::new(m20260918_000011_create_observations::Migration),
            Box::new(m20260918_000012_create_measurements::Migration),
            Box::new(m20260919_000013_create_experiments::Migration),
        ]
    }
}

/// Apply pending migrations atomically on SQLite.
///
/// SeaORM intentionally does not create an outer transaction for SQLite
/// migrations. Running the migrator on a transaction here keeps every schema
/// change and the corresponding migration-ledger row in one atomic unit.
pub async fn up(
    db: &sea_orm::DatabaseConnection,
    steps: Option<u32>,
) -> Result<(), sea_orm::DbErr> {
    use sea_orm::TransactionTrait;

    let txn = db.begin().await?;
    Migrator::up(&txn, steps).await?;
    txn.commit().await
}

/// Roll back migrations atomically on SQLite.
pub async fn down(
    db: &sea_orm::DatabaseConnection,
    steps: Option<u32>,
) -> Result<(), sea_orm::DbErr> {
    use sea_orm::TransactionTrait;

    let txn = db.begin().await?;
    Migrator::down(&txn, steps).await?;
    txn.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    #[tokio::test]
    async fn migration_creates_all_core_tables() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, None).await.unwrap();

        let rows = db
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
            ))
            .await
            .unwrap();

        let tables: Vec<String> = rows
            .iter()
            .map(|r| r.try_get::<String>("", "name").unwrap())
            .collect();

        for expected in [
            "branch_o2m_values",
            "branch_o2o_values",
            "branch_references",
            "branches",
            "frame_slot_o2m_values",
            "frame_slot_o2o_values",
            "frame_slots",
            "frames",
            "selection_packets",
            "usage_history",
        ] {
            assert!(
                tables.iter().any(|t| t == expected),
                "missing table `{expected}`; got {tables:?}"
            );
        }

        // Down migration cleanly removes the core tables.
        down(&db, None).await.unwrap();
        let rows = db
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT name FROM sqlite_master WHERE type='table' AND name='branches'",
            ))
            .await
            .unwrap();
        assert!(rows.is_empty(), "branches table should be dropped");
    }

    #[tokio::test]
    async fn frame_slot_position_migration_backfills_in_insert_order() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, Some(2)).await.unwrap();
        db.execute_unprepared(
            "INSERT INTO frames (id, name) VALUES (1, 'f'); \
             INSERT INTO frame_slots (id, frame_id, name) VALUES \
             (10, 1, 'first'), (20, 1, 'second')",
        )
        .await
        .unwrap();

        up(&db, None).await.unwrap();
        let rows = db
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT name, position FROM frame_slots ORDER BY position",
            ))
            .await
            .unwrap();
        assert_eq!(rows[0].try_get::<String>("", "name").unwrap(), "first");
        assert_eq!(rows[0].try_get::<i32>("", "position").unwrap(), 0);
        assert_eq!(rows[1].try_get::<String>("", "name").unwrap(), "second");
        assert_eq!(rows[1].try_get::<i32>("", "position").unwrap(), 1);
    }

    #[tokio::test]
    async fn failed_migration_rolls_back_and_can_be_retried() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, Some(2)).await.unwrap();

        // Force the final statement in migration 3 to fail after its ALTER and
        // UPDATE statements have run. SQLite index names are database-global.
        db.execute_unprepared(
            "CREATE TABLE migration_collision (value INTEGER); \
             CREATE INDEX idx_frame_slots_frame_position \
             ON migration_collision(value);",
        )
        .await
        .unwrap();

        assert!(up(&db, None).await.is_err());

        let columns = db
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA table_info(frame_slots)",
            ))
            .await
            .unwrap();
        assert!(
            columns
                .iter()
                .all(|row| row.try_get::<String>("", "name").unwrap() != "position"),
            "the earlier ALTER TABLE must roll back with the failed migration"
        );

        db.execute_unprepared("DROP INDEX idx_frame_slots_frame_position")
            .await
            .unwrap();
        up(&db, None).await.unwrap();

        let columns = db
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA table_info(frame_slots)",
            ))
            .await
            .unwrap();
        assert!(columns
            .iter()
            .any(|row| row.try_get::<String>("", "name").unwrap() == "position"));
    }

    #[tokio::test]
    async fn frame_slot_constraints_reject_invalid_rows() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, None).await.unwrap();

        let exec = |sql: &'static str| db.execute(Statement::from_string(DbBackend::Sqlite, sql));

        // A frame and a valid slot to hang value rows off of.
        exec("INSERT INTO frames (id, name) VALUES (1, 'f')")
            .await
            .unwrap();
        exec("INSERT INTO frame_slots (id, frame_id, name, count) VALUES (1, 1, 's', 1)")
            .await
            .unwrap();

        // count must be positive.
        assert!(
            exec("INSERT INTO frame_slots (frame_id, name, count) VALUES (1, 's0', 0)")
                .await
                .is_err(),
            "non-positive count must be rejected"
        );
        // booleans are constrained to 0/1.
        assert!(
            exec("INSERT INTO frame_slots (frame_id, name, avoid_recent) VALUES (1, 's2', 2)")
                .await
                .is_err(),
            "out-of-range avoid_recent must be rejected"
        );
        // slot names are unique within a frame.
        assert!(
            exec("INSERT INTO frame_slots (frame_id, name) VALUES (1, 's')")
                .await
                .is_err(),
            "duplicate slot name within a frame must be rejected"
        );
        // value modes are constrained to the known discriminators.
        assert!(
            exec(
                "INSERT INTO frame_slot_o2o_values (slot_id, mode, name, value) \
                  VALUES (1, 'bogus', 'k', 'v')"
            )
            .await
            .is_err(),
            "unknown o2o mode must be rejected"
        );
        assert!(
            exec(
                "INSERT INTO frame_slot_o2m_values (slot_id, mode, name, value) \
                  VALUES (1, 'default', 'k', 'v')"
            )
            .await
            .is_err(),
            "o2m mode 'default' is not valid and must be rejected"
        );
    }
    #[tokio::test]
    async fn provenance_and_run_constraints_preserve_a_traversable_dag() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, None).await.unwrap();
        db.execute_unprepared("PRAGMA foreign_keys = ON")
            .await
            .unwrap();

        db.execute_unprepared(
            "INSERT INTO engine_runs
               (id, resolved_plan_json, status, started_at, completed_at)
             VALUES
               ('run-1', '{}', 'completed', '2026-09-17T00:00:00Z',
                '2026-09-17T00:01:00Z');
             INSERT INTO provenance
               (derived_id, run_id, operation, producer, algorithm, version,
                params_json, params_hash, timestamp)
             VALUES
               ('input-1', 'run-1', 'inferred', 'infer.fixture', 'fixture',
                '0.1.0', '{}', 'hash-a', '2026-09-17T00:00:00Z'),
               ('result-1', 'run-1', 'calculated', 'sensor.fixture', 'fixture',
                '0.1.0', '{}', 'hash-b', '2026-09-17T00:00:01Z');
             INSERT INTO provenance_inputs
               (derived_id, input_derived_id, position)
             VALUES ('result-1', 'input-1', 0);",
        )
        .await
        .unwrap();

        let edge = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT input_derived_id FROM provenance_inputs
                 WHERE derived_id = 'result-1'",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            edge.try_get::<String>("", "input_derived_id").unwrap(),
            "input-1"
        );

        for invalid in [
            "INSERT INTO provenance
               (derived_id, operation, producer, algorithm, version,
                params_json, params_hash, timestamp)
             VALUES ('bad-op', 'guessed', 'x', 'x', '0.1.0', '{}', 'h', 't')",
            "INSERT INTO provenance_inputs
               (derived_id, input_derived_id, position)
             VALUES ('result-1', 'missing', 1)",
            "INSERT INTO provenance_inputs
               (derived_id, input_derived_id, position)
             VALUES ('result-1', 'result-1', 1)",
            "INSERT INTO engine_runs
               (id, resolved_plan_json, status, started_at)
             VALUES ('bad-run', '{}', 'completed', 't')",
        ] {
            assert!(
                db.execute_unprepared(invalid).await.is_err(),
                "invalid row unexpectedly accepted: {invalid}"
            );
        }

        db.execute_unprepared("DELETE FROM engine_runs WHERE id = 'run-1'")
            .await
            .unwrap();
        let remaining = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM provenance",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "count")
            .unwrap();
        assert_eq!(remaining, 0);
    }
    #[tokio::test]
    async fn domain_versions_own_immutable_typed_graphs() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, None).await.unwrap();
        db.execute_unprepared("PRAGMA foreign_keys = ON")
            .await
            .unwrap();

        db.execute_unprepared(
            r#"INSERT INTO domains (id, label, created_at)
               VALUES ('coffee', 'Coffee', '2026-09-17T00:00:00Z');
             INSERT INTO domain_versions (id, domain_id, version, created_at)
               VALUES ('coffee@1', 'coffee', '1', '2026-09-17T00:00:00Z');
             INSERT INTO units (domain_version_id, id, kind, label) VALUES
               ('coffee@1', 'sensory', 'atomic_meaning', 'Sensory'),
               ('coffee@1', 'social', 'semantic_role', 'Social');
             INSERT INTO relations
               (domain_version_id, id, source_unit_id, target_unit_id, kind)
               VALUES ('coffee@1', 'r1', 'sensory', 'social', 'supports');
             INSERT INTO unit_properties
               (domain_version_id, unit_id, name, value_kind, number_value)
               VALUES ('coffee@1', 'sensory', 'weight', 'number', 0.8);
             INSERT INTO relation_properties
               (domain_version_id, relation_id, name, value_kind, structured_json)
               VALUES ('coffee@1', 'r1', 'evidence', 'structured', '{"count":2}');"#,
        )
        .await
        .unwrap();

        for invalid in [
            "UPDATE units SET label = 'Changed'
               WHERE domain_version_id = 'coffee@1' AND id = 'sensory'",
            "INSERT INTO relations
               (domain_version_id, id, source_unit_id, target_unit_id, kind)
               VALUES ('coffee@1', 'bad', 'sensory', 'missing', 'supports')",
            "INSERT INTO unit_properties
               (domain_version_id, unit_id, name, value_kind, integer_value)
               VALUES ('coffee@1', 'sensory', 'bad', 'text', 1)",
        ] {
            assert!(
                db.execute_unprepared(invalid).await.is_err(),
                "invalid versioned graph mutation unexpectedly succeeded: {invalid}"
            );
        }

        db.execute_unprepared(
            "INSERT INTO domain_versions
               (id, domain_id, version, predecessor_id, created_at)
             VALUES ('coffee@2', 'coffee', '2', 'coffee@1',
                     '2026-09-17T00:01:00Z');
             INSERT INTO units (domain_version_id, id, kind, label)
               VALUES ('coffee@2', 'sensory', 'atomic_meaning', 'Revised sensory');
             INSERT INTO unit_properties
               (domain_version_id, unit_id, name, value_kind, number_value)
               VALUES ('coffee@2', 'sensory', 'weight', 'number', 0.9);",
        )
        .await
        .unwrap();

        let weights = db
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT domain_version_id, number_value FROM unit_properties
                 WHERE unit_id = 'sensory' ORDER BY domain_version_id",
            ))
            .await
            .unwrap();
        assert_eq!(weights.len(), 2);
        assert_eq!(weights[0].try_get::<f64>("", "number_value").unwrap(), 0.8);
        assert_eq!(weights[1].try_get::<f64>("", "number_value").unwrap(), 0.9);
    }
    #[tokio::test]
    async fn frame_versions_are_immutable_ordered_and_domain_pinned() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, None).await.unwrap();
        db.execute_unprepared("PRAGMA foreign_keys = ON")
            .await
            .unwrap();

        db.execute_unprepared(
            "INSERT INTO domains (id, created_at) VALUES ('coffee', 't');
             INSERT INTO domain_versions (id, domain_id, version, created_at)
               VALUES ('coffee@1', 'coffee', '1', 't');
             INSERT INTO units (domain_version_id, id, kind) VALUES
               ('coffee@1', 'sensory', 'atomic_meaning'),
               ('coffee@1', 'social', 'semantic_role');
             INSERT INTO measurement_frames (id, domain_id, created_at)
               VALUES ('coffee.general', 'coffee', 't');
             INSERT INTO frame_versions
               (id, frame_id, version, domain_version_id, created_at)
               VALUES ('coffee.general@1', 'coffee.general', '1', 'coffee@1', 't');
             INSERT INTO frame_axes
               (frame_version_id, domain_version_id, position, unit_id) VALUES
               ('coffee.general@1', 'coffee@1', 0, 'sensory'),
               ('coffee.general@1', 'coffee@1', 1, 'social');",
        )
        .await
        .unwrap();

        for invalid in [
            "UPDATE frame_axes SET position = 2
               WHERE frame_version_id = 'coffee.general@1' AND position = 1",
            "INSERT INTO frame_axes
               (frame_version_id, domain_version_id, position, unit_id)
             VALUES ('coffee.general@1', 'coffee@1', 0, 'social')",
            "INSERT INTO frame_axes
               (frame_version_id, domain_version_id, position, unit_id)
             VALUES ('coffee.general@1', 'coffee@1', 2, 'sensory')",
        ] {
            assert!(
                db.execute_unprepared(invalid).await.is_err(),
                "invalid frame mutation unexpectedly succeeded: {invalid}"
            );
        }

        db.execute_unprepared(
            "INSERT INTO frame_versions
               (id, frame_id, version, domain_version_id, predecessor_id, created_at)
             VALUES ('coffee.general@2', 'coffee.general', '2', 'coffee@1',
                     'coffee.general@1', 'later');
             INSERT INTO frame_axes
               (frame_version_id, domain_version_id, position, unit_id)
             VALUES ('coffee.general@2', 'coffee@1', 0, 'social');",
        )
        .await
        .unwrap();

        let versions = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM frame_versions
                 WHERE domain_version_id = 'coffee@1'",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "count")
            .unwrap();
        assert_eq!(versions, 2);
    }
    #[tokio::test]
    async fn observations_preserve_ambiguous_alignment_ties_and_unknown_tail() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, None).await.unwrap();
        db.execute_unprepared("PRAGMA foreign_keys = ON")
            .await
            .unwrap();

        db.execute_unprepared(
            "INSERT INTO provenance
               (derived_id, operation, producer, algorithm, version,
                params_json, params_hash, timestamp)
             VALUES ('p1', 'inferred', 'infer.fixture', 'fixture', '0.1.0',
                     '{}', 'hash', 't');
             INSERT INTO domains (id, created_at) VALUES ('coffee', 't');
             INSERT INTO domain_versions (id, domain_id, version, created_at)
               VALUES ('coffee@1', 'coffee', '1', 't');
             INSERT INTO units (domain_version_id, id, kind) VALUES
               ('coffee@1', 'sensory', 'atomic_meaning'),
               ('coffee@1', 'social', 'semantic_role'),
               ('coffee@1', 'production', 'semantic_role');
             INSERT INTO observations
               (id, provenance_id, source) VALUES ('x1', 'p1', 'fixture');
             INSERT INTO observed_units
               (observation_id, id, label, provenance_id) VALUES
               ('x1', 'a', 'appearance', 'p1'),
               ('x1', 'b', 'sharing', 'p1'),
               ('x1', 'c', 'making', 'p1');
             INSERT INTO observed_relations
               (observation_id, id, source_unit_id, target_unit_id, kind,
                provenance_id)
               VALUES ('x1', 'or1', 'a', 'b', 'supports', 'p1');
             INSERT INTO alignments
               (id, observation_id, provenance_id) VALUES ('a1', 'x1', 'p1');
             INSERT INTO alignment_candidates
               (alignment_id, observation_id, observed_unit_id,
                domain_version_id, domain_unit_id, position, confidence,
                provenance_id) VALUES
               ('a1', 'x1', 'a', 'coffee@1', 'sensory', 0, 0.7, 'p1'),
               ('a1', 'x1', 'a', 'coffee@1', 'social', 1, 0.3, 'p1');
             INSERT INTO rankings
               (id, observation_id, provenance_id) VALUES ('rank1', 'x1', 'p1');
             INSERT INTO ranking_entries
               (ranking_id, observation_id, observed_unit_id, state, tier,
                position, provenance_id) VALUES
               ('rank1', 'x1', 'a', 'ranked', 0, 0, 'p1'),
               ('rank1', 'x1', 'b', 'ranked', 0, 1, 'p1'),
               ('rank1', 'x1', 'c', 'unknown', -1, 0, 'p1');",
        )
        .await
        .unwrap();

        let candidate_count = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM alignment_candidates
                 WHERE alignment_id = 'a1' AND observed_unit_id = 'a'",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "count")
            .unwrap();
        assert_eq!(candidate_count, 2);

        let tied_count = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM ranking_entries
                 WHERE ranking_id = 'rank1' AND state = 'ranked' AND tier = 0",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "count")
            .unwrap();
        assert_eq!(tied_count, 2);

        for invalid in [
            "INSERT INTO observed_relations
               (observation_id, id, source_unit_id, target_unit_id, kind,
                provenance_id)
             VALUES ('x1', 'bad', 'a', 'missing', 'supports', 'p1')",
            "INSERT INTO alignment_candidates
               (alignment_id, observation_id, observed_unit_id,
                domain_version_id, domain_unit_id, position, confidence,
                provenance_id)
             VALUES ('a1', 'x1', 'b', 'coffee@1', 'social', 2, 1.1, 'p1')",
            "INSERT INTO ranking_entries
               (ranking_id, observation_id, observed_unit_id, state, tier,
                position, provenance_id)
             VALUES ('rank1', 'x1', 'c', 'unknown', 1, 1, 'p1')",
        ] {
            assert!(
                db.execute_unprepared(invalid).await.is_err(),
                "invalid observation row unexpectedly succeeded: {invalid}"
            );
        }
    }
    #[tokio::test]
    async fn measurements_keep_typed_values_and_sparse_statuses_distinct() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        up(&db, None).await.unwrap();
        db.execute_unprepared("PRAGMA foreign_keys = ON")
            .await
            .unwrap();

        db.execute_unprepared(
            r#"INSERT INTO engine_runs
               (id, resolved_plan_json, status, started_at, completed_at)
             VALUES ('run-m', '{}', 'completed', 't', 't2');
             INSERT INTO provenance
               (derived_id, run_id, operation, producer, algorithm, version,
                params_json, params_hash, timestamp) VALUES
               ('p-profile', 'run-m', 'calculated', 'engine', 'profile',
                '0.1.0', '{}', 'h1', 't'),
               ('p-zero', 'run-m', 'calculated', 'sensor.coverage', 'coverage',
                '0.1.0', '{}', 'h2', 't'),
               ('p-na', 'run-m', 'calculated', 'sensor.kendall', 'kendall',
                '0.1.0', '{}', 'h3', 't'),
               ('p-g', 'run-m', 'calculated', 'engine', 'structure',
                '0.1.0', '{}', 'h4', 't');
             INSERT INTO domains (id, created_at) VALUES ('coffee', 't');
             INSERT INTO domain_versions (id, domain_id, version, created_at)
               VALUES ('coffee@1', 'coffee', '1', 't');
             INSERT INTO units (domain_version_id, id, kind)
               VALUES ('coffee@1', 'sensory', 'atomic_meaning');
             INSERT INTO measurement_frames (id, domain_id, created_at)
               VALUES ('coffee.general', 'coffee', 't');
             INSERT INTO frame_versions
               (id, frame_id, version, domain_version_id, created_at)
               VALUES ('coffee.general@1', 'coffee.general', '1', 'coffee@1', 't');
             INSERT INTO frame_axes
               (frame_version_id, domain_version_id, position, unit_id)
               VALUES ('coffee.general@1', 'coffee@1', 0, 'sensory');
             INSERT INTO sensor_runs
               (id, engine_run_id, sensor_id, sensor_version, params_json,
                params_hash, status, started_at, completed_at)
               VALUES ('sr1', 'run-m', 'sensor.coverage', '0.1.0', '{}', 'h2',
                       'completed', 't', 't2');
             INSERT INTO measurement_profiles
               (id, engine_run_id, frame_version_id, provenance_id, created_at)
               VALUES ('mp1', 'run-m', 'coffee.general@1', 'p-profile', 't');
             INSERT INTO measurements
               (id, profile_id, sensor_run_id, provenance_id, kind, status,
                value_json, sample_count) VALUES
               ('m-zero', 'mp1', 'sr1', 'p-zero', 'scalar', 'value', '0.0', 1),
               ('m-na', 'mp1', 'sr1', 'p-na', 'scalar', 'not_applicable',
                NULL, 0);
             INSERT INTO empirical_structures
               (id, profile_id, kind, value_json, provenance_id, created_at)
               VALUES ('g1', 'mp1', 'fixture', '{"nodes":[]}', 'p-g', 't');"#,
        )
        .await
        .unwrap();

        let zero = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT status, value_json FROM measurements WHERE id = 'm-zero'",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(zero.try_get::<String>("", "status").unwrap(), "value");
        assert_eq!(zero.try_get::<String>("", "value_json").unwrap(), "0.0");

        let sparse = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM measurements
                 WHERE status != 'value' AND value_json IS NULL",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "count")
            .unwrap();
        assert_eq!(sparse, 1);

        for invalid in [
            "INSERT INTO measurements
               (id, profile_id, sensor_run_id, provenance_id, kind, status)
             VALUES ('bad-value', 'mp1', 'sr1', 'p-g', 'scalar', 'value')",
            "INSERT INTO measurements
               (id, profile_id, sensor_run_id, provenance_id, kind, status,
                value_json)
             VALUES ('bad-sparse', 'mp1', 'sr1', 'p-g', 'scalar',
                     'not_measured', '0')",
            "INSERT INTO measurements
               (id, profile_id, sensor_run_id, provenance_id, kind, status)
             VALUES ('bad-kind', 'mp1', 'sr1', 'p-g', 'score',
                     'not_measured')",
        ] {
            assert!(
                db.execute_unprepared(invalid).await.is_err(),
                "invalid measurement unexpectedly succeeded: {invalid}"
            );
        }
    }
}
