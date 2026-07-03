use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Harden invariants that were previously enforced only by repository code.
///
/// The unique indexes match the map/set semantics of frame constraints. The
/// triggers protect pattern rows written by other SQLite clients; the no-op
/// update also validates existing rows while the migration transaction is
/// active, so an invalid database cannot be marked as successfully migrated.
const UP_SQL: &str = r#"
CREATE UNIQUE INDEX idx_frame_slot_o2o_identity
  ON frame_slot_o2o_values(slot_id, mode, name);

CREATE UNIQUE INDEX idx_frame_slot_o2m_identity
  ON frame_slot_o2m_values(slot_id, mode, name, value);

CREATE TRIGGER pattern_entries_validate_insert
BEFORE INSERT ON pattern_entries
WHEN NOT (
  length(trim(COALESCE(NEW.pattern, ''))) > 0
  AND NEW.enabled IN (0, 1)
  AND (
    (
      NEW.target_kind IN ('o2o', 'o2m')
      AND length(COALESCE(NEW.target_name, '')) > 0
      AND length(COALESCE(NEW.target_value, '')) > 0
    )
    OR
    (
      NEW.target_kind IN ('branch', 'collapse')
      AND NEW.target_name IS NULL
      AND length(COALESCE(NEW.target_value, '')) > 1
      AND substr(NEW.target_value, 1, 1) = '/'
    )
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid pattern entry');
END;

CREATE TRIGGER pattern_entries_validate_update
BEFORE UPDATE ON pattern_entries
WHEN NOT (
  length(trim(COALESCE(NEW.pattern, ''))) > 0
  AND NEW.enabled IN (0, 1)
  AND (
    (
      NEW.target_kind IN ('o2o', 'o2m')
      AND length(COALESCE(NEW.target_name, '')) > 0
      AND length(COALESCE(NEW.target_value, '')) > 0
    )
    OR
    (
      NEW.target_kind IN ('branch', 'collapse')
      AND NEW.target_name IS NULL
      AND length(COALESCE(NEW.target_value, '')) > 1
      AND substr(NEW.target_value, 1, 1) = '/'
    )
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid pattern entry');
END;

UPDATE pattern_entries SET enabled = enabled;
"#;

const DOWN_SQL: &str = r#"
DROP TRIGGER IF EXISTS pattern_entries_validate_update;
DROP TRIGGER IF EXISTS pattern_entries_validate_insert;
DROP INDEX IF EXISTS idx_frame_slot_o2m_identity;
DROP INDEX IF EXISTS idx_frame_slot_o2o_identity;
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
