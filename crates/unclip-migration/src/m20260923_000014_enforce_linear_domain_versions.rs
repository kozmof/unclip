use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE UNIQUE INDEX idx_domain_versions_predecessor
ON domain_versions(predecessor_id)
WHERE predecessor_id IS NOT NULL;
"#;

const DOWN_SQL: &str = r#"
DROP INDEX IF EXISTS idx_domain_versions_predecessor;
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
    use sea_orm::{ConnectionTrait, Database};

    #[tokio::test]
    async fn one_domain_version_cannot_have_two_successors() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        crate::up(&db, None).await.unwrap();
        db.execute_unprepared(
            "INSERT INTO domains(id,created_at) VALUES ('d','now'); \
             INSERT INTO domain_versions(id,domain_id,version,predecessor_id,created_at) \
             VALUES ('d1','d','1',NULL,'now'),('d2','d','2','d1','now')",
        )
        .await
        .unwrap();

        assert!(db
            .execute_unprepared(
                "INSERT INTO domain_versions(id,domain_id,version,predecessor_id,created_at) \
                 VALUES ('d3','d','3','d1','now')",
            )
            .await
            .is_err());
    }
}
