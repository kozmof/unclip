use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Allow several avoided o2o values per `(slot, name)`.
///
/// `avoid_o2o` became a multi-value exclusion (matching `avoid_o2m` and the
/// query shape), so the one-value-per-name unique index must only apply to the
/// genuinely one-to-one modes (`require`, `default`). Avoid rows instead get
/// set semantics: unique per `(slot, mode, name, value)`.
const UP_SQL: &str = r#"
DROP INDEX idx_frame_slot_o2o_identity;

CREATE UNIQUE INDEX idx_frame_slot_o2o_identity
  ON frame_slot_o2o_values(slot_id, mode, name)
  WHERE mode IN ('require', 'default');

CREATE UNIQUE INDEX idx_frame_slot_o2o_avoid_identity
  ON frame_slot_o2o_values(slot_id, mode, name, value)
  WHERE mode = 'avoid';
"#;

const DOWN_SQL: &str = r#"
DROP INDEX IF EXISTS idx_frame_slot_o2o_avoid_identity;
DROP INDEX IF EXISTS idx_frame_slot_o2o_identity;

CREATE UNIQUE INDEX idx_frame_slot_o2o_identity
  ON frame_slot_o2o_values(slot_id, mode, name);
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
