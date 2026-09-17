use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE domains (
  id TEXT PRIMARY KEY NOT NULL,
  label TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE domain_versions (
  id TEXT PRIMARY KEY NOT NULL,
  domain_id TEXT NOT NULL,
  version TEXT NOT NULL,
  predecessor_id TEXT,
  created_at TEXT NOT NULL,
  UNIQUE (domain_id, version),
  FOREIGN KEY (domain_id) REFERENCES domains(id) ON DELETE CASCADE,
  FOREIGN KEY (predecessor_id) REFERENCES domain_versions(id) ON DELETE RESTRICT,
  CHECK (predecessor_id IS NULL OR predecessor_id <> id)
);

CREATE TABLE units (
  domain_version_id TEXT NOT NULL,
  id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN (
    'atomic_meaning', 'composite_meaning', 'semantic_role', 'graph_motif',
    'transformation', 'dynamic_coupling', 'latent_axis', 'cross_domain_structure'
  )),
  label TEXT,
  PRIMARY KEY (domain_version_id, id),
  FOREIGN KEY (domain_version_id) REFERENCES domain_versions(id) ON DELETE CASCADE
);

CREATE TABLE relations (
  domain_version_id TEXT NOT NULL,
  id TEXT NOT NULL,
  source_unit_id TEXT NOT NULL,
  target_unit_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (length(kind) > 0),
  PRIMARY KEY (domain_version_id, id),
  FOREIGN KEY (domain_version_id) REFERENCES domain_versions(id) ON DELETE CASCADE,
  FOREIGN KEY (domain_version_id, source_unit_id)
    REFERENCES units(domain_version_id, id) ON DELETE CASCADE,
  FOREIGN KEY (domain_version_id, target_unit_id)
    REFERENCES units(domain_version_id, id) ON DELETE CASCADE
);

CREATE TABLE unit_properties (
  domain_version_id TEXT NOT NULL,
  unit_id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(name) > 0),
  value_kind TEXT NOT NULL CHECK (value_kind IN (
    'boolean', 'integer', 'number', 'text', 'structured'
  )),
  boolean_value INTEGER CHECK (boolean_value IN (0, 1)),
  integer_value INTEGER,
  number_value REAL,
  text_value TEXT,
  structured_json TEXT CHECK (structured_json IS NULL OR json_valid(structured_json)),
  PRIMARY KEY (domain_version_id, unit_id, name),
  FOREIGN KEY (domain_version_id, unit_id)
    REFERENCES units(domain_version_id, id) ON DELETE CASCADE,
  CHECK (
    (value_kind = 'boolean' AND boolean_value IS NOT NULL
      AND integer_value IS NULL AND number_value IS NULL
      AND text_value IS NULL AND structured_json IS NULL) OR
    (value_kind = 'integer' AND boolean_value IS NULL
      AND integer_value IS NOT NULL AND number_value IS NULL
      AND text_value IS NULL AND structured_json IS NULL) OR
    (value_kind = 'number' AND boolean_value IS NULL
      AND integer_value IS NULL AND number_value IS NOT NULL
      AND text_value IS NULL AND structured_json IS NULL) OR
    (value_kind = 'text' AND boolean_value IS NULL
      AND integer_value IS NULL AND number_value IS NULL
      AND text_value IS NOT NULL AND structured_json IS NULL) OR
    (value_kind = 'structured' AND boolean_value IS NULL
      AND integer_value IS NULL AND number_value IS NULL
      AND text_value IS NULL AND structured_json IS NOT NULL)
  )
);

CREATE TABLE relation_properties (
  domain_version_id TEXT NOT NULL,
  relation_id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(name) > 0),
  value_kind TEXT NOT NULL CHECK (value_kind IN (
    'boolean', 'integer', 'number', 'text', 'structured'
  )),
  boolean_value INTEGER CHECK (boolean_value IN (0, 1)),
  integer_value INTEGER,
  number_value REAL,
  text_value TEXT,
  structured_json TEXT CHECK (structured_json IS NULL OR json_valid(structured_json)),
  PRIMARY KEY (domain_version_id, relation_id, name),
  FOREIGN KEY (domain_version_id, relation_id)
    REFERENCES relations(domain_version_id, id) ON DELETE CASCADE,
  CHECK (
    (value_kind = 'boolean' AND boolean_value IS NOT NULL
      AND integer_value IS NULL AND number_value IS NULL
      AND text_value IS NULL AND structured_json IS NULL) OR
    (value_kind = 'integer' AND boolean_value IS NULL
      AND integer_value IS NOT NULL AND number_value IS NULL
      AND text_value IS NULL AND structured_json IS NULL) OR
    (value_kind = 'number' AND boolean_value IS NULL
      AND integer_value IS NULL AND number_value IS NOT NULL
      AND text_value IS NULL AND structured_json IS NULL) OR
    (value_kind = 'text' AND boolean_value IS NULL
      AND integer_value IS NULL AND number_value IS NULL
      AND text_value IS NOT NULL AND structured_json IS NULL) OR
    (value_kind = 'structured' AND boolean_value IS NULL
      AND integer_value IS NULL AND number_value IS NULL
      AND text_value IS NULL AND structured_json IS NOT NULL)
  )
);

CREATE INDEX idx_domain_versions_domain ON domain_versions(domain_id, version);
CREATE INDEX idx_relations_source ON relations(domain_version_id, source_unit_id);
CREATE INDEX idx_relations_target ON relations(domain_version_id, target_unit_id);

CREATE TRIGGER domain_versions_immutable
BEFORE UPDATE ON domain_versions
BEGIN SELECT RAISE(ABORT, 'domain versions are immutable'); END;

CREATE TRIGGER units_immutable
BEFORE UPDATE ON units
BEGIN SELECT RAISE(ABORT, 'versioned units are immutable'); END;

CREATE TRIGGER relations_immutable
BEFORE UPDATE ON relations
BEGIN SELECT RAISE(ABORT, 'versioned relations are immutable'); END;

CREATE TRIGGER unit_properties_immutable
BEFORE UPDATE ON unit_properties
BEGIN SELECT RAISE(ABORT, 'versioned unit properties are immutable'); END;

CREATE TRIGGER relation_properties_immutable
BEFORE UPDATE ON relation_properties
BEGIN SELECT RAISE(ABORT, 'versioned relation properties are immutable'); END;
"#;

const DOWN_SQL: &str = r#"
DROP TRIGGER IF EXISTS relation_properties_immutable;
DROP TRIGGER IF EXISTS unit_properties_immutable;
DROP TRIGGER IF EXISTS relations_immutable;
DROP TRIGGER IF EXISTS units_immutable;
DROP TRIGGER IF EXISTS domain_versions_immutable;
DROP TABLE IF EXISTS relation_properties;
DROP TABLE IF EXISTS unit_properties;
DROP TABLE IF EXISTS relations;
DROP TABLE IF EXISTS units;
DROP TABLE IF EXISTS domain_versions;
DROP TABLE IF EXISTS domains;
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
