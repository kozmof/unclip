use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// `branches.path` is declared `TEXT NOT NULL UNIQUE`, and SQLite implements
// that constraint with an automatic index (`sqlite_autoindex_branches_1`).
// `idx_branches_path` covered the identical single column, so every branch
// insert, update, and delete maintained two identical B-trees while no query
// could use the second. Dropping it changes no query plan: the unique index
// that remains serves the same lookups and prefix scans.
const UP_SQL: &str = r#"
DROP INDEX IF EXISTS idx_branches_path;
"#;

// Recreating the redundant index is what "undo" means here, even though it buys
// nothing: a `down` that left the schema without it would not restore the state
// this migration was applied to.
const DOWN_SQL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_branches_path ON branches(path);
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    async fn index_names(db: &sea_orm::DatabaseConnection) -> Vec<String> {
        db.query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'branches'",
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String>("", "name").unwrap())
        .collect()
    }

    /// The explicit duplicate is gone and the constraint's own index remains.
    ///
    /// Asserting the automatic index survives is the point: dropping the
    /// explicit one is only safe because the `UNIQUE` constraint still indexes
    /// the same column, and a future migration that relaxed that constraint
    /// would silently leave `path` unindexed.
    #[tokio::test]
    async fn branch_path_keeps_exactly_one_index() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        crate::up(&db, None).await.unwrap();

        let names = index_names(&db).await;
        assert!(
            !names.iter().any(|name| name == "idx_branches_path"),
            "redundant index still present: {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|name| name.starts_with("sqlite_autoindex")),
            "the UNIQUE constraint's own index is missing: {names:?}"
        );
    }

    /// `path` lookups still resolve through an index rather than a table scan.
    #[tokio::test]
    async fn branch_path_lookup_still_uses_an_index() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        crate::up(&db, None).await.unwrap();

        let plan = db
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "EXPLAIN QUERY PLAN SELECT id FROM branches WHERE path = '/a'",
            ))
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get::<String>("", "detail").unwrap())
            .collect::<Vec<_>>()
            .join("; ");

        assert!(
            plan.contains("USING INDEX") || plan.contains("USING COVERING INDEX"),
            "path lookup no longer uses an index: {plan}"
        );
    }
}
