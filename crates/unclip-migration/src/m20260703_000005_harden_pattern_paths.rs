use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Make direct SQLite writes obey the same path invariants as the Rust domain.
///
/// Migration 004 validates the pattern target shape. These additional triggers
/// reject empty path segments, trailing slashes, and every Unicode code point
/// Rust classifies as whitespace or control. The no-op update validates rows
/// written before this migration and keeps the schema upgrade atomic.
const UP_SQL: &str = r#"
CREATE TRIGGER pattern_entries_validate_path_insert
BEFORE INSERT ON pattern_entries
WHEN NEW.target_kind IN ('branch', 'collapse') AND NOT (
  length(COALESCE(NEW.target_value, '')) > 1
  AND substr(NEW.target_value, 1, 1) = '/'
  AND substr(NEW.target_value, -1, 1) != '/'
  AND instr(NEW.target_value, '//') = 0
  AND instr(NEW.target_value, char(0)) = 0
  AND NOT EXISTS (
    WITH RECURSIVE path_chars(position, codepoint) AS (
      SELECT 1, unicode(substr(NEW.target_value, 1, 1))
      UNION ALL
      SELECT position + 1, unicode(substr(NEW.target_value, position + 1, 1))
      FROM path_chars WHERE position < length(NEW.target_value)
    )
    SELECT 1 FROM path_chars
    WHERE codepoint BETWEEN 0 AND 31
       OR codepoint BETWEEN 127 AND 159
       OR codepoint IN (32, 160, 5760, 8192, 8193, 8194, 8195,
                        8196, 8197, 8198, 8199, 8200, 8201, 8202,
                        8232, 8233, 8239, 8287, 12288)
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid pattern target path');
END;

CREATE TRIGGER pattern_entries_validate_path_update
BEFORE UPDATE ON pattern_entries
WHEN NEW.target_kind IN ('branch', 'collapse') AND NOT (
  length(COALESCE(NEW.target_value, '')) > 1
  AND substr(NEW.target_value, 1, 1) = '/'
  AND substr(NEW.target_value, -1, 1) != '/'
  AND instr(NEW.target_value, '//') = 0
  AND instr(NEW.target_value, char(0)) = 0
  AND NOT EXISTS (
    WITH RECURSIVE path_chars(position, codepoint) AS (
      SELECT 1, unicode(substr(NEW.target_value, 1, 1))
      UNION ALL
      SELECT position + 1, unicode(substr(NEW.target_value, position + 1, 1))
      FROM path_chars WHERE position < length(NEW.target_value)
    )
    SELECT 1 FROM path_chars
    WHERE codepoint BETWEEN 0 AND 31
       OR codepoint BETWEEN 127 AND 159
       OR codepoint IN (32, 160, 5760, 8192, 8193, 8194, 8195,
                        8196, 8197, 8198, 8199, 8200, 8201, 8202,
                        8232, 8233, 8239, 8287, 12288)
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid pattern target path');
END;

UPDATE pattern_entries
SET target_value = target_value
WHERE target_kind IN ('branch', 'collapse');
"#;

const DOWN_SQL: &str = r#"
DROP TRIGGER IF EXISTS pattern_entries_validate_path_update;
DROP TRIGGER IF EXISTS pattern_entries_validate_path_insert;
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
