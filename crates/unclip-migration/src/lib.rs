//! unclip-migration — SeaORM migrations defining the SQLite schema.

#![forbid(unsafe_code)]

pub use sea_orm_migration::prelude::*;

mod m20260620_000001_create_core_tables;
mod m20260620_000002_create_pattern_entries;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260620_000001_create_core_tables::Migration),
            Box::new(m20260620_000002_create_pattern_entries::Migration),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    #[tokio::test]
    async fn migration_creates_all_core_tables() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&db, None).await.unwrap();

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
        Migrator::down(&db, None).await.unwrap();
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
    async fn frame_slot_constraints_reject_invalid_rows() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&db, None).await.unwrap();

        let exec = |sql: &'static str| {
            db.execute(Statement::from_string(DbBackend::Sqlite, sql))
        };

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
            exec("INSERT INTO frame_slot_o2o_values (slot_id, mode, name, value) \
                  VALUES (1, 'bogus', 'k', 'v')")
                .await
                .is_err(),
            "unknown o2o mode must be rejected"
        );
        assert!(
            exec("INSERT INTO frame_slot_o2m_values (slot_id, mode, name, value) \
                  VALUES (1, 'default', 'k', 'v')")
                .await
                .is_err(),
            "o2m mode 'default' is not valid and must be rejected"
        );
    }
}
