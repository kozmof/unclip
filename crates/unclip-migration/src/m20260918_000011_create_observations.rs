use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE observations (
  id TEXT PRIMARY KEY NOT NULL,
  provenance_id TEXT NOT NULL,
  source TEXT NOT NULL,
  observed_at TEXT,
  context_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(context_json)),
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE
);

CREATE TABLE observed_units (
  observation_id TEXT NOT NULL,
  id TEXT NOT NULL,
  label TEXT NOT NULL,
  salience REAL,
  uncertainty REAL CHECK (uncertainty IS NULL OR (uncertainty >= 0 AND uncertainty <= 1)),
  context_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(context_json)),
  provenance_id TEXT NOT NULL,
  PRIMARY KEY (observation_id, id),
  FOREIGN KEY (observation_id) REFERENCES observations(id) ON DELETE CASCADE,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE
);

CREATE TABLE observed_relations (
  observation_id TEXT NOT NULL,
  id TEXT NOT NULL,
  source_unit_id TEXT NOT NULL,
  target_unit_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (length(kind) > 0),
  uncertainty REAL CHECK (uncertainty IS NULL OR (uncertainty >= 0 AND uncertainty <= 1)),
  provenance_id TEXT NOT NULL,
  PRIMARY KEY (observation_id, id),
  FOREIGN KEY (observation_id) REFERENCES observations(id) ON DELETE CASCADE,
  FOREIGN KEY (observation_id, source_unit_id)
    REFERENCES observed_units(observation_id, id) ON DELETE CASCADE,
  FOREIGN KEY (observation_id, target_unit_id)
    REFERENCES observed_units(observation_id, id) ON DELETE CASCADE,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE
);

CREATE TABLE alignments (
  id TEXT PRIMARY KEY NOT NULL,
  observation_id TEXT NOT NULL,
  provenance_id TEXT NOT NULL,
  UNIQUE (id, observation_id),
  FOREIGN KEY (observation_id) REFERENCES observations(id) ON DELETE CASCADE,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE
);

CREATE TABLE alignment_candidates (
  alignment_id TEXT NOT NULL,
  observation_id TEXT NOT NULL,
  observed_unit_id TEXT NOT NULL,
  domain_version_id TEXT NOT NULL,
  domain_unit_id TEXT NOT NULL,
  position INTEGER NOT NULL CHECK (position >= 0),
  confidence REAL NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
  evidence_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(evidence_json)),
  provenance_id TEXT NOT NULL,
  PRIMARY KEY (alignment_id, observed_unit_id, domain_version_id, domain_unit_id),
  UNIQUE (alignment_id, position),
  FOREIGN KEY (alignment_id, observation_id)
    REFERENCES alignments(id, observation_id) ON DELETE CASCADE,
  FOREIGN KEY (observation_id, observed_unit_id)
    REFERENCES observed_units(observation_id, id) ON DELETE CASCADE,
  FOREIGN KEY (domain_version_id, domain_unit_id)
    REFERENCES units(domain_version_id, id) ON DELETE RESTRICT,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE
);

CREATE TABLE rankings (
  id TEXT PRIMARY KEY NOT NULL,
  observation_id TEXT NOT NULL,
  provenance_id TEXT NOT NULL,
  UNIQUE (id, observation_id),
  FOREIGN KEY (observation_id) REFERENCES observations(id) ON DELETE CASCADE,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE
);

CREATE TABLE ranking_entries (
  ranking_id TEXT NOT NULL,
  observation_id TEXT NOT NULL,
  observed_unit_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('ranked', 'unknown')),
  tier INTEGER NOT NULL,
  position INTEGER NOT NULL CHECK (position >= 0),
  provenance_id TEXT NOT NULL,
  PRIMARY KEY (ranking_id, observed_unit_id),
  UNIQUE (ranking_id, state, tier, position),
  FOREIGN KEY (ranking_id, observation_id)
    REFERENCES rankings(id, observation_id) ON DELETE CASCADE,
  FOREIGN KEY (observation_id, observed_unit_id)
    REFERENCES observed_units(observation_id, id) ON DELETE CASCADE,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE,
  CHECK ((state = 'ranked' AND tier >= 0) OR (state = 'unknown' AND tier = -1))
);

CREATE INDEX idx_observed_units_provenance ON observed_units(provenance_id);
CREATE INDEX idx_observed_relations_provenance ON observed_relations(provenance_id);
CREATE INDEX idx_alignments_observation ON alignments(observation_id);
CREATE INDEX idx_alignment_candidates_observed
  ON alignment_candidates(observation_id, observed_unit_id);
CREATE INDEX idx_alignment_candidates_domain
  ON alignment_candidates(domain_version_id, domain_unit_id);
CREATE INDEX idx_rankings_observation ON rankings(observation_id);
CREATE INDEX idx_ranking_entries_order
  ON ranking_entries(ranking_id, state, tier, position);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS ranking_entries;
DROP TABLE IF EXISTS rankings;
DROP TABLE IF EXISTS alignment_candidates;
DROP TABLE IF EXISTS alignments;
DROP TABLE IF EXISTS observed_relations;
DROP TABLE IF EXISTS observed_units;
DROP TABLE IF EXISTS observations;
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
