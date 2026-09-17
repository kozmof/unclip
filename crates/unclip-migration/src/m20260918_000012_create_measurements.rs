use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE sensor_runs (
  id TEXT PRIMARY KEY NOT NULL,
  engine_run_id TEXT NOT NULL,
  sensor_id TEXT NOT NULL,
  sensor_version TEXT NOT NULL,
  params_json TEXT NOT NULL CHECK (json_valid(params_json)),
  params_hash TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('planned', 'running', 'completed', 'failed')),
  started_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE (engine_run_id, sensor_id),
  FOREIGN KEY (engine_run_id) REFERENCES engine_runs(id) ON DELETE CASCADE,
  CHECK ((status = 'completed' AND completed_at IS NOT NULL) OR status != 'completed')
);

CREATE TABLE measurement_profiles (
  id TEXT PRIMARY KEY NOT NULL,
  engine_run_id TEXT NOT NULL,
  observation_id TEXT,
  frame_version_id TEXT NOT NULL,
  provenance_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY (engine_run_id) REFERENCES engine_runs(id) ON DELETE CASCADE,
  FOREIGN KEY (observation_id) REFERENCES observations(id) ON DELETE CASCADE,
  FOREIGN KEY (frame_version_id) REFERENCES frame_versions(id) ON DELETE RESTRICT,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE
);

CREATE TABLE measurements (
  id TEXT PRIMARY KEY NOT NULL,
  profile_id TEXT NOT NULL,
  sensor_run_id TEXT NOT NULL,
  provenance_id TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK (kind IN (
    'scalar', 'vector', 'matrix', 'distribution', 'events', 'graph',
    'ranking', 'partition', 'structured'
  )),
  status TEXT NOT NULL CHECK (status IN (
    'value', 'not_applicable', 'not_measured', 'insufficient_evidence'
  )),
  value_json TEXT CHECK (value_json IS NULL OR json_valid(value_json)),
  confidence REAL CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
  sample_count INTEGER CHECK (sample_count IS NULL OR sample_count >= 0),
  context_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(context_json)),
  FOREIGN KEY (profile_id) REFERENCES measurement_profiles(id) ON DELETE CASCADE,
  FOREIGN KEY (sensor_run_id) REFERENCES sensor_runs(id) ON DELETE CASCADE,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE,
  CHECK (
    (status = 'value' AND value_json IS NOT NULL) OR
    (status != 'value' AND value_json IS NULL)
  )
);

CREATE TABLE empirical_structures (
  id TEXT PRIMARY KEY NOT NULL,
  profile_id TEXT,
  kind TEXT NOT NULL CHECK (length(kind) > 0),
  value_json TEXT NOT NULL CHECK (json_valid(value_json)),
  provenance_id TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  FOREIGN KEY (profile_id) REFERENCES measurement_profiles(id) ON DELETE SET NULL,
  FOREIGN KEY (provenance_id) REFERENCES provenance(derived_id) ON DELETE CASCADE
);

CREATE INDEX idx_sensor_runs_engine ON sensor_runs(engine_run_id);
CREATE INDEX idx_measurement_profiles_run ON measurement_profiles(engine_run_id);
CREATE INDEX idx_measurement_profiles_observation ON measurement_profiles(observation_id);
CREATE INDEX idx_measurements_profile ON measurements(profile_id);
CREATE INDEX idx_measurements_sensor_run ON measurements(sensor_run_id);
CREATE INDEX idx_measurements_status_kind ON measurements(status, kind);
CREATE INDEX idx_empirical_structures_profile ON empirical_structures(profile_id);
CREATE INDEX idx_empirical_structures_kind ON empirical_structures(kind);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS empirical_structures;
DROP TABLE IF EXISTS measurements;
DROP TABLE IF EXISTS measurement_profiles;
DROP TABLE IF EXISTS sensor_runs;
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
