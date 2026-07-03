use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Enforce repository-level branch invariants for direct SQLite writers too.
///
/// SQLite cannot add `CHECK` constraints to existing tables, so paired insert
/// and update triggers protect branch paths/weights and the non-empty,
/// control-free indexed/reference strings. The final no-op updates validate
/// pre-existing rows before the migration is recorded.
const UP_SQL: &str = r#"
CREATE TRIGGER branches_validate_insert
BEFORE INSERT ON branches
WHEN NOT (
  length(COALESCE(NEW.path, '')) > 1
  AND substr(NEW.path, 1, 1) = '/'
  AND substr(NEW.path, -1, 1) != '/'
  AND instr(NEW.path, '//') = 0
  AND typeof(NEW.weight) IN ('integer', 'real')
  AND NEW.weight >= 0
  AND NEW.weight <= 1.7976931348623157e308
  AND NOT EXISTS (
    WITH RECURSIVE chars(position, codepoint) AS (
      SELECT 1, unicode(substr(NEW.path, 1, 1))
      UNION ALL
      SELECT position + 1, unicode(substr(NEW.path, position + 1, 1))
      FROM chars WHERE position < length(NEW.path)
    )
    SELECT 1 FROM chars
    WHERE codepoint BETWEEN 0 AND 31
       OR codepoint BETWEEN 127 AND 159
       OR codepoint IN (32, 160, 5760, 8192, 8193, 8194, 8195,
                        8196, 8197, 8198, 8199, 8200, 8201, 8202,
                        8232, 8233, 8239, 8287, 12288)
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid branch record');
END;

CREATE TRIGGER branches_validate_update
BEFORE UPDATE ON branches
WHEN NOT (
  length(COALESCE(NEW.path, '')) > 1
  AND substr(NEW.path, 1, 1) = '/'
  AND substr(NEW.path, -1, 1) != '/'
  AND instr(NEW.path, '//') = 0
  AND typeof(NEW.weight) IN ('integer', 'real')
  AND NEW.weight >= 0
  AND NEW.weight <= 1.7976931348623157e308
  AND NOT EXISTS (
    WITH RECURSIVE chars(position, codepoint) AS (
      SELECT 1, unicode(substr(NEW.path, 1, 1))
      UNION ALL
      SELECT position + 1, unicode(substr(NEW.path, position + 1, 1))
      FROM chars WHERE position < length(NEW.path)
    )
    SELECT 1 FROM chars
    WHERE codepoint BETWEEN 0 AND 31
       OR codepoint BETWEEN 127 AND 159
       OR codepoint IN (32, 160, 5760, 8192, 8193, 8194, 8195,
                        8196, 8197, 8198, 8199, 8200, 8201, 8202,
                        8232, 8233, 8239, 8287, 12288)
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid branch record');
END;

CREATE TRIGGER branch_o2o_values_validate_insert
BEFORE INSERT ON branch_o2o_values
WHEN NOT (
  length(COALESCE(NEW.name, '')) > 0
  AND length(COALESCE(NEW.value, '')) > 0
  AND NOT EXISTS (
    WITH RECURSIVE chars(text, position, codepoint) AS (
      SELECT NEW.name || NEW.value, 1, unicode(substr(NEW.name || NEW.value, 1, 1))
      UNION ALL
      SELECT text, position + 1, unicode(substr(text, position + 1, 1))
      FROM chars WHERE position < length(text)
    )
    SELECT 1 FROM chars
    WHERE codepoint BETWEEN 0 AND 31 OR codepoint BETWEEN 127 AND 159
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid branch o2o value');
END;

CREATE TRIGGER branch_o2o_values_validate_update
BEFORE UPDATE ON branch_o2o_values
WHEN NOT (
  length(COALESCE(NEW.name, '')) > 0
  AND length(COALESCE(NEW.value, '')) > 0
  AND NOT EXISTS (
    WITH RECURSIVE chars(text, position, codepoint) AS (
      SELECT NEW.name || NEW.value, 1, unicode(substr(NEW.name || NEW.value, 1, 1))
      UNION ALL
      SELECT text, position + 1, unicode(substr(text, position + 1, 1))
      FROM chars WHERE position < length(text)
    )
    SELECT 1 FROM chars
    WHERE codepoint BETWEEN 0 AND 31 OR codepoint BETWEEN 127 AND 159
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid branch o2o value');
END;

CREATE TRIGGER branch_o2m_values_validate_insert
BEFORE INSERT ON branch_o2m_values
WHEN NOT (
  length(COALESCE(NEW.name, '')) > 0
  AND length(COALESCE(NEW.value, '')) > 0
  AND NOT EXISTS (
    WITH RECURSIVE chars(text, position, codepoint) AS (
      SELECT NEW.name || NEW.value, 1, unicode(substr(NEW.name || NEW.value, 1, 1))
      UNION ALL
      SELECT text, position + 1, unicode(substr(text, position + 1, 1))
      FROM chars WHERE position < length(text)
    )
    SELECT 1 FROM chars
    WHERE codepoint BETWEEN 0 AND 31 OR codepoint BETWEEN 127 AND 159
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid branch o2m value');
END;

CREATE TRIGGER branch_o2m_values_validate_update
BEFORE UPDATE ON branch_o2m_values
WHEN NOT (
  length(COALESCE(NEW.name, '')) > 0
  AND length(COALESCE(NEW.value, '')) > 0
  AND NOT EXISTS (
    WITH RECURSIVE chars(text, position, codepoint) AS (
      SELECT NEW.name || NEW.value, 1, unicode(substr(NEW.name || NEW.value, 1, 1))
      UNION ALL
      SELECT text, position + 1, unicode(substr(text, position + 1, 1))
      FROM chars WHERE position < length(text)
    )
    SELECT 1 FROM chars
    WHERE codepoint BETWEEN 0 AND 31 OR codepoint BETWEEN 127 AND 159
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid branch o2m value');
END;

CREATE TRIGGER branch_references_validate_insert
BEFORE INSERT ON branch_references
WHEN NOT (
  length(COALESCE(NEW.type, '')) > 0
  AND length(COALESCE(NEW.value, '')) > 0
  AND NOT EXISTS (
    WITH RECURSIVE chars(text, position, codepoint) AS (
      SELECT NEW.type || NEW.value, 1, unicode(substr(NEW.type || NEW.value, 1, 1))
      UNION ALL
      SELECT text, position + 1, unicode(substr(text, position + 1, 1))
      FROM chars WHERE position < length(text)
    )
    SELECT 1 FROM chars
    WHERE codepoint BETWEEN 0 AND 31 OR codepoint BETWEEN 127 AND 159
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid branch reference');
END;

CREATE TRIGGER branch_references_validate_update
BEFORE UPDATE ON branch_references
WHEN NOT (
  length(COALESCE(NEW.type, '')) > 0
  AND length(COALESCE(NEW.value, '')) > 0
  AND NOT EXISTS (
    WITH RECURSIVE chars(text, position, codepoint) AS (
      SELECT NEW.type || NEW.value, 1, unicode(substr(NEW.type || NEW.value, 1, 1))
      UNION ALL
      SELECT text, position + 1, unicode(substr(text, position + 1, 1))
      FROM chars WHERE position < length(text)
    )
    SELECT 1 FROM chars
    WHERE codepoint BETWEEN 0 AND 31 OR codepoint BETWEEN 127 AND 159
  )
)
BEGIN
  SELECT RAISE(ABORT, 'invalid branch reference');
END;

UPDATE branches SET path = path;
UPDATE branch_o2o_values SET name = name;
UPDATE branch_o2m_values SET name = name;
UPDATE branch_references SET type = type;
"#;

const DOWN_SQL: &str = r#"
DROP TRIGGER IF EXISTS branch_references_validate_update;
DROP TRIGGER IF EXISTS branch_references_validate_insert;
DROP TRIGGER IF EXISTS branch_o2m_values_validate_update;
DROP TRIGGER IF EXISTS branch_o2m_values_validate_insert;
DROP TRIGGER IF EXISTS branch_o2o_values_validate_update;
DROP TRIGGER IF EXISTS branch_o2o_values_validate_insert;
DROP TRIGGER IF EXISTS branches_validate_update;
DROP TRIGGER IF EXISTS branches_validate_insert;
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
