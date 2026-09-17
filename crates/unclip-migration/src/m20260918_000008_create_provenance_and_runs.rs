use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE engine_runs (
  id TEXT PRIMARY KEY NOT NULL,
  resolved_plan_json TEXT NOT NULL CHECK (json_valid(resolved_plan_json)),
  status TEXT NOT NULL CHECK (status IN ('planned', 'running', 'completed', 'failed')),
  started_at TEXT NOT NULL,
  completed_at TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  CHECK ((status = 'completed' AND completed_at IS NOT NULL) OR status != 'completed')
);

CREATE TABLE provenance (
  derived_id TEXT PRIMARY KEY NOT NULL,
  run_id TEXT,
  operation TEXT NOT NULL
    CHECK (operation IN ('inferred', 'calculated', 'experimental', 'interpreted')),
  producer TEXT NOT NULL,
  algorithm TEXT NOT NULL,
  version TEXT NOT NULL,
  params_json TEXT NOT NULL CHECK (json_valid(params_json)),
  params_hash TEXT NOT NULL,
  source TEXT,
  timestamp TEXT NOT NULL,
  domain_version TEXT,
  frame_version TEXT,
  model TEXT,
  FOREIGN KEY (run_id) REFERENCES engine_runs(id) ON DELETE CASCADE
);

CREATE TABLE provenance_inputs (
  derived_id TEXT NOT NULL,
  input_derived_id TEXT NOT NULL,
  position INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (derived_id, input_derived_id),
  UNIQUE (derived_id, position),
  FOREIGN KEY (derived_id) REFERENCES provenance(derived_id) ON DELETE CASCADE,
  FOREIGN KEY (input_derived_id) REFERENCES provenance(derived_id) ON DELETE CASCADE,
  CHECK (derived_id <> input_derived_id)
);

CREATE INDEX idx_provenance_run ON provenance(run_id);
CREATE INDEX idx_provenance_operation_producer ON provenance(operation, producer);
CREATE INDEX idx_provenance_inputs_input ON provenance_inputs(input_derived_id);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS provenance_inputs;
DROP TABLE IF EXISTS provenance;
DROP TABLE IF EXISTS engine_runs;
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
