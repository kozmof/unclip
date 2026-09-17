use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE measurement_frames (
  id TEXT PRIMARY KEY NOT NULL,
  domain_id TEXT NOT NULL,
  label TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY (domain_id) REFERENCES domains(id) ON DELETE CASCADE
);

CREATE TABLE frame_versions (
  id TEXT PRIMARY KEY NOT NULL,
  frame_id TEXT NOT NULL,
  version TEXT NOT NULL,
  domain_version_id TEXT NOT NULL,
  predecessor_id TEXT,
  created_at TEXT NOT NULL,
  UNIQUE (frame_id, version),
  UNIQUE (id, domain_version_id),
  FOREIGN KEY (frame_id) REFERENCES measurement_frames(id) ON DELETE CASCADE,
  FOREIGN KEY (domain_version_id) REFERENCES domain_versions(id) ON DELETE RESTRICT,
  FOREIGN KEY (predecessor_id) REFERENCES frame_versions(id) ON DELETE RESTRICT,
  CHECK (predecessor_id IS NULL OR predecessor_id <> id)
);

CREATE TABLE frame_axes (
  frame_version_id TEXT NOT NULL,
  domain_version_id TEXT NOT NULL,
  position INTEGER NOT NULL CHECK (position >= 0),
  unit_id TEXT NOT NULL,
  label TEXT,
  PRIMARY KEY (frame_version_id, position),
  UNIQUE (frame_version_id, unit_id),
  FOREIGN KEY (frame_version_id, domain_version_id)
    REFERENCES frame_versions(id, domain_version_id) ON DELETE CASCADE,
  FOREIGN KEY (domain_version_id, unit_id)
    REFERENCES units(domain_version_id, id) ON DELETE RESTRICT
);

CREATE INDEX idx_measurement_frames_domain ON measurement_frames(domain_id);
CREATE INDEX idx_frame_versions_frame ON frame_versions(frame_id, version);
CREATE INDEX idx_frame_versions_domain_version ON frame_versions(domain_version_id);
CREATE INDEX idx_frame_axes_unit ON frame_axes(domain_version_id, unit_id);

CREATE TRIGGER frame_versions_immutable
BEFORE UPDATE ON frame_versions
BEGIN SELECT RAISE(ABORT, 'frame versions are immutable'); END;

CREATE TRIGGER frame_axes_immutable
BEFORE UPDATE ON frame_axes
BEGIN SELECT RAISE(ABORT, 'versioned frame axes are immutable'); END;
"#;

const DOWN_SQL: &str = r#"
DROP TRIGGER IF EXISTS frame_axes_immutable;
DROP TRIGGER IF EXISTS frame_versions_immutable;
DROP TABLE IF EXISTS frame_axes;
DROP TABLE IF EXISTS frame_versions;
DROP TABLE IF EXISTS measurement_frames;
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
