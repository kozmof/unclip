use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE candidates (
  id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
  domain_version_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN (
    'atomic_meaning', 'composite_meaning', 'relation', 'graph_motif',
    'semantic_role', 'transformation', 'dynamic_coupling', 'latent_axis',
    'cross_domain_structure', 'weight_revision'
  )),
  value_json TEXT NOT NULL CHECK (json_valid(value_json) AND json_type(value_json) = 'object'),
  provenance_id TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  UNIQUE (id, domain_version_id),
  FOREIGN KEY (domain_version_id) REFERENCES domain_versions(id) ON DELETE RESTRICT,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE RESTRICT
);

CREATE TABLE experiments (
  id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
  engine_run_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  domain_version_id TEXT NOT NULL,
  frame_version_id TEXT NOT NULL,
  plan_json TEXT NOT NULL CHECK (json_valid(plan_json) AND json_type(plan_json) = 'object'),
  status TEXT NOT NULL CHECK (status IN ('planned', 'running', 'completed', 'failed')),
  result_json TEXT CHECK (result_json IS NULL OR (json_valid(result_json) AND json_type(result_json) = 'object')),
  provenance_id TEXT NOT NULL UNIQUE,
  started_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE (id, candidate_id, domain_version_id),
  FOREIGN KEY (engine_run_id) REFERENCES engine_runs(id) ON DELETE RESTRICT,
  FOREIGN KEY (candidate_id, domain_version_id) REFERENCES candidates(id, domain_version_id) ON DELETE RESTRICT,
  FOREIGN KEY (frame_version_id, domain_version_id) REFERENCES frame_versions(id, domain_version_id) ON DELETE RESTRICT,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE RESTRICT,
  CHECK ((status = 'completed' AND result_json IS NOT NULL AND completed_at IS NOT NULL)
      OR (status != 'completed' AND result_json IS NULL)),
  CHECK ((status IN ('completed', 'failed') AND completed_at IS NOT NULL)
      OR (status IN ('planned', 'running') AND completed_at IS NULL))
);

CREATE TABLE experiment_observations (
  experiment_id TEXT NOT NULL,
  observation_id TEXT NOT NULL,
  split TEXT NOT NULL CHECK (split IN ('training', 'held_out')),
  position INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (experiment_id, observation_id),
  UNIQUE (experiment_id, split, position),
  FOREIGN KEY (experiment_id) REFERENCES experiments(id) ON DELETE CASCADE,
  FOREIGN KEY (observation_id) REFERENCES observations(id) ON DELETE RESTRICT
);

CREATE TABLE experiment_deltas (
  id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
  experiment_id TEXT NOT NULL,
  before_profile_id TEXT NOT NULL,
  after_profile_id TEXT NOT NULL,
  comparator_id TEXT NOT NULL CHECK (length(comparator_id) > 0),
  comparator_version TEXT NOT NULL CHECK (length(comparator_version) > 0),
  kind TEXT NOT NULL CHECK (kind IN (
    'scalar', 'vector', 'matrix', 'distribution', 'events', 'graph',
    'ranking', 'partition', 'structured'
  )),
  value_json TEXT NOT NULL CHECK (json_valid(value_json)),
  provenance_id TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  FOREIGN KEY (experiment_id) REFERENCES experiments(id) ON DELETE RESTRICT,
  FOREIGN KEY (before_profile_id) REFERENCES measurement_profiles(id) ON DELETE RESTRICT,
  FOREIGN KEY (after_profile_id) REFERENCES measurement_profiles(id) ON DELETE RESTRICT,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE RESTRICT
);

CREATE TABLE domain_revisions (
  id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
  candidate_id TEXT NOT NULL,
  experiment_id TEXT NOT NULL,
  from_version_id TEXT NOT NULL,
  to_version_id TEXT NOT NULL UNIQUE,
  reason TEXT NOT NULL CHECK (length(trim(reason)) > 0),
  evidence_json TEXT NOT NULL CHECK (json_valid(evidence_json) AND json_type(evidence_json) = 'object'),
  provenance_id TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  FOREIGN KEY (experiment_id, candidate_id, from_version_id)
    REFERENCES experiments(id, candidate_id, domain_version_id) ON DELETE RESTRICT,
  FOREIGN KEY (from_version_id) REFERENCES domain_versions(id) ON DELETE RESTRICT,
  FOREIGN KEY (to_version_id) REFERENCES domain_versions(id) ON DELETE RESTRICT,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE RESTRICT,
  CHECK (from_version_id <> to_version_id)
);

CREATE INDEX idx_candidates_domain ON candidates(domain_version_id, kind);
CREATE INDEX idx_experiments_run ON experiments(engine_run_id);
CREATE INDEX idx_experiments_candidate ON experiments(candidate_id, status);
CREATE INDEX idx_experiments_frame ON experiments(frame_version_id, domain_version_id);
CREATE INDEX idx_experiment_observations_observation ON experiment_observations(observation_id);
CREATE INDEX idx_experiment_deltas_experiment ON experiment_deltas(experiment_id, comparator_id);
CREATE INDEX idx_experiment_deltas_before ON experiment_deltas(before_profile_id);
CREATE INDEX idx_experiment_deltas_after ON experiment_deltas(after_profile_id);
CREATE INDEX idx_domain_revisions_experiment ON domain_revisions(experiment_id, candidate_id, from_version_id);
CREATE INDEX idx_domain_revisions_from ON domain_revisions(from_version_id);

CREATE TRIGGER experiment_observations_insert BEFORE INSERT ON experiment_observations
WHEN (SELECT status FROM experiments WHERE id = NEW.experiment_id) != 'planned'
BEGIN SELECT RAISE(ABORT, 'observation selection requires a planned experiment'); END;
CREATE TRIGGER experiment_observations_update BEFORE UPDATE ON experiment_observations
BEGIN SELECT RAISE(ABORT, 'experiment observation selections are immutable'); END;
CREATE TRIGGER experiment_observations_delete BEFORE DELETE ON experiment_observations
WHEN (SELECT status FROM experiments WHERE id = OLD.experiment_id) != 'planned'
BEGIN SELECT RAISE(ABORT, 'started experiment observations are immutable'); END;

CREATE TRIGGER experiments_identity_immutable BEFORE UPDATE ON experiments
WHEN OLD.id IS NOT NEW.id OR OLD.engine_run_id IS NOT NEW.engine_run_id
  OR OLD.candidate_id IS NOT NEW.candidate_id OR OLD.domain_version_id IS NOT NEW.domain_version_id
  OR OLD.frame_version_id IS NOT NEW.frame_version_id OR OLD.plan_json IS NOT NEW.plan_json
  OR OLD.provenance_id IS NOT NEW.provenance_id OR OLD.started_at IS NOT NEW.started_at
BEGIN SELECT RAISE(ABORT, 'experiment inputs are immutable'); END;
CREATE TRIGGER experiments_transition BEFORE UPDATE ON experiments
WHEN NOT ((OLD.status = 'planned' AND NEW.status IN ('running', 'failed'))
       OR (OLD.status = 'running' AND NEW.status IN ('completed', 'failed')))
BEGIN SELECT RAISE(ABORT, 'invalid experiment status transition'); END;

CREATE TRIGGER candidates_immutable BEFORE UPDATE ON candidates
BEGIN SELECT RAISE(ABORT, 'candidates are immutable'); END;
CREATE TRIGGER experiment_deltas_immutable BEFORE UPDATE ON experiment_deltas
BEGIN SELECT RAISE(ABORT, 'experiment deltas are immutable'); END;
CREATE TRIGGER domain_revisions_immutable BEFORE UPDATE ON domain_revisions
BEGIN SELECT RAISE(ABORT, 'domain revisions are immutable'); END;
CREATE TRIGGER domain_revisions_validate BEFORE INSERT ON domain_revisions
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM domain_versions old JOIN domain_versions new
      ON old.domain_id = new.domain_id AND new.predecessor_id = old.id
    WHERE old.id = NEW.from_version_id AND new.id = NEW.to_version_id
  ) THEN RAISE(ABORT, 'revision must link successive versions of one domain') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM experiments WHERE id = NEW.experiment_id AND status = 'completed'
  ) THEN RAISE(ABORT, 'revision requires a completed experiment') END;
END;
"#;

const DOWN_SQL: &str = r#"
DROP TRIGGER IF EXISTS experiment_observations_insert;
DROP TRIGGER IF EXISTS experiment_observations_update;
DROP TRIGGER IF EXISTS experiment_observations_delete;
DROP TRIGGER IF EXISTS experiments_transition;
DROP TRIGGER IF EXISTS experiments_identity_immutable;
DROP TRIGGER IF EXISTS domain_revisions_validate;
DROP TRIGGER IF EXISTS domain_revisions_immutable;
DROP TRIGGER IF EXISTS experiment_deltas_immutable;
DROP TRIGGER IF EXISTS candidates_immutable;
DROP TABLE IF EXISTS domain_revisions;
DROP TABLE IF EXISTS experiment_deltas;
DROP TABLE IF EXISTS experiment_observations;
DROP TABLE IF EXISTS experiments;
DROP TABLE IF EXISTS candidates;
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
