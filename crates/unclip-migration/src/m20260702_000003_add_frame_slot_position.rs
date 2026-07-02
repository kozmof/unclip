use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"
ALTER TABLE frame_slots
  ADD COLUMN position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0);

UPDATE frame_slots AS current
SET position = (
  SELECT COUNT(*)
  FROM frame_slots AS earlier
  WHERE earlier.frame_id = current.frame_id
    AND earlier.id < current.id
);

CREATE UNIQUE INDEX idx_frame_slots_frame_position
  ON frame_slots(frame_id, position);
"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"
DROP INDEX IF EXISTS idx_frame_slots_frame_position;
ALTER TABLE frame_slots DROP COLUMN position;
"#,
        )
        .await?;
        Ok(())
    }
}
