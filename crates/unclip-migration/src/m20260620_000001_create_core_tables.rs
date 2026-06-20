use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Core MVP schema (DRAFT §16).
///
/// Written as raw SQL so the composite primary keys, `ON DELETE CASCADE`
/// behavior, and index shapes match the specification exactly. SQLite is the
/// only target, so portability to other backends is not a concern here.
const UP_SQL: &str = r#"
CREATE TABLE branches (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  path TEXT NOT NULL UNIQUE,
  parent_path TEXT,
  title TEXT,
  description TEXT,
  weight REAL NOT NULL DEFAULT 1.0,
  metadata_json TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE branch_o2o_values (
  branch_id INTEGER NOT NULL,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (branch_id, name),
  FOREIGN KEY (branch_id) REFERENCES branches(id) ON DELETE CASCADE
);

CREATE TABLE branch_o2m_values (
  branch_id INTEGER NOT NULL,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (branch_id, name, value),
  FOREIGN KEY (branch_id) REFERENCES branches(id) ON DELETE CASCADE
);

CREATE TABLE branch_references (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  branch_id INTEGER NOT NULL,
  type TEXT NOT NULL,
  value TEXT NOT NULL,
  note TEXT,
  FOREIGN KEY (branch_id) REFERENCES branches(id) ON DELETE CASCADE
);

CREATE TABLE frames (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE,
  description TEXT
);

CREATE TABLE frame_slots (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  frame_id INTEGER NOT NULL,
  name TEXT NOT NULL,
  under_path TEXT,
  count INTEGER NOT NULL DEFAULT 1,
  avoid_recent INTEGER NOT NULL DEFAULT 0,
  weighted INTEGER NOT NULL DEFAULT 0,
  metadata_suggest_json TEXT,
  FOREIGN KEY (frame_id) REFERENCES frames(id) ON DELETE CASCADE
);

CREATE TABLE frame_slot_o2o_values (
  slot_id INTEGER NOT NULL,
  mode TEXT NOT NULL,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  FOREIGN KEY (slot_id) REFERENCES frame_slots(id) ON DELETE CASCADE
);

CREATE TABLE frame_slot_o2m_values (
  slot_id INTEGER NOT NULL,
  mode TEXT NOT NULL,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  FOREIGN KEY (slot_id) REFERENCES frame_slots(id) ON DELETE CASCADE
);

CREATE TABLE usage_history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  branch_id INTEGER NOT NULL,
  used_at TEXT NOT NULL,
  command TEXT,
  context TEXT,
  packet_id TEXT,
  FOREIGN KEY (branch_id) REFERENCES branches(id) ON DELETE CASCADE
);

CREATE TABLE selection_packets (
  id TEXT PRIMARY KEY,
  frame_name TEXT,
  seed INTEGER,
  created_at TEXT NOT NULL,
  query_json TEXT,
  packet_json TEXT NOT NULL
);

CREATE INDEX idx_branches_path ON branches(path);
CREATE INDEX idx_branches_parent_path ON branches(parent_path);
CREATE INDEX idx_branch_o2o_name_value ON branch_o2o_values(name, value, branch_id);
CREATE INDEX idx_branch_o2m_name_value ON branch_o2m_values(name, value, branch_id);
CREATE INDEX idx_usage_history_branch_id ON usage_history(branch_id);
CREATE INDEX idx_usage_history_used_at ON usage_history(used_at);
"#;

/// Tables dropped in reverse dependency order.
const DOWN_TABLES: &[&str] = &[
    "selection_packets",
    "usage_history",
    "frame_slot_o2m_values",
    "frame_slot_o2o_values",
    "frame_slots",
    "frames",
    "branch_references",
    "branch_o2m_values",
    "branch_o2o_values",
    "branches",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for table in DOWN_TABLES {
            db.execute_unprepared(&format!("DROP TABLE IF EXISTS {table};"))
                .await?;
        }
        Ok(())
    }
}
