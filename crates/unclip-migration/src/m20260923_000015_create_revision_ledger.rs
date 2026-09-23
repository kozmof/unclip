use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE candidate_interpretations (
  id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
  candidate_id TEXT NOT NULL,
  value_json TEXT NOT NULL CHECK (json_valid(value_json) AND json_type(value_json) = 'object'),
  provenance_id TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  UNIQUE (id, candidate_id),
  FOREIGN KEY (candidate_id) REFERENCES candidates(id) ON DELETE RESTRICT,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX idx_domain_revisions_id_candidate
  ON domain_revisions(id, candidate_id);

CREATE TABLE domain_revision_interpretations (
  revision_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  interpretation_id TEXT NOT NULL,
  position INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (revision_id, interpretation_id),
  UNIQUE (revision_id, position),
  FOREIGN KEY (revision_id, candidate_id)
    REFERENCES domain_revisions(id, candidate_id) ON DELETE RESTRICT,
  FOREIGN KEY (interpretation_id, candidate_id)
    REFERENCES candidate_interpretations(id, candidate_id) ON DELETE RESTRICT
);

CREATE INDEX idx_candidate_interpretations_candidate
  ON candidate_interpretations(candidate_id, created_at, id);
CREATE INDEX idx_revision_interpretations_interpretation
  ON domain_revision_interpretations(interpretation_id);

CREATE TRIGGER candidate_interpretations_immutable
BEFORE UPDATE ON candidate_interpretations
BEGIN SELECT RAISE(ABORT, 'candidate interpretations are immutable'); END;
CREATE TRIGGER candidate_interpretations_no_delete
BEFORE DELETE ON candidate_interpretations
BEGIN SELECT RAISE(ABORT, 'candidate interpretations are immutable'); END;
CREATE TRIGGER domain_revision_interpretations_immutable
BEFORE UPDATE ON domain_revision_interpretations
BEGIN SELECT RAISE(ABORT, 'revision interpretation links are immutable'); END;
CREATE TRIGGER domain_revision_interpretations_no_delete
BEFORE DELETE ON domain_revision_interpretations
BEGIN SELECT RAISE(ABORT, 'revision interpretation links are immutable'); END;
"#;

const DOWN_SQL: &str = r#"
DROP TRIGGER IF EXISTS domain_revision_interpretations_no_delete;
DROP TRIGGER IF EXISTS domain_revision_interpretations_immutable;
DROP TRIGGER IF EXISTS candidate_interpretations_no_delete;
DROP TRIGGER IF EXISTS candidate_interpretations_immutable;
DROP TABLE IF EXISTS domain_revision_interpretations;
DROP INDEX IF EXISTS idx_domain_revisions_id_candidate;
DROP TABLE IF EXISTS candidate_interpretations;
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
