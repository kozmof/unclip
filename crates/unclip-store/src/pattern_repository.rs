//! Repository for the pattern dictionary (DRAFT §18).

use sea_orm::{
    ActiveValue::{NotSet, Set},
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};
use unclip_core::{PatternEntry, PatternTarget};
use unclip_entity::pattern_entries;

/// A stored pattern entry with its id and enabled flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPattern {
    pub id: i64,
    pub entry: PatternEntry,
    pub enabled: bool,
}

/// Persistence for user-defined pattern entries.
pub struct SeaOrmPatternRepository {
    db: DatabaseConnection,
}

impl SeaOrmPatternRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Add a pattern entry; returns its new id.
    pub async fn add(&self, entry: &PatternEntry) -> anyhow::Result<i64> {
        let (kind, name, value) = target_columns(&entry.target);
        let am = pattern_entries::ActiveModel {
            id: NotSet,
            pattern: Set(entry.pattern.clone()),
            target_kind: Set(kind.to_string()),
            target_name: Set(name),
            target_value: Set(value),
            branch_id: Set(None),
            enabled: Set(1),
        };
        let res = pattern_entries::Entity::insert(am).exec(&self.db).await?;
        Ok(res.last_insert_id as i64)
    }

    /// List all stored patterns, ordered by id.
    pub async fn list(&self) -> anyhow::Result<Vec<StoredPattern>> {
        let rows = pattern_entries::Entity::find()
            .order_by_asc(pattern_entries::Column::Id)
            .all(&self.db)
            .await?;
        rows.into_iter().map(row_to_stored).collect()
    }

    /// All enabled pattern entries as matcher input.
    pub async fn all_enabled(&self) -> anyhow::Result<Vec<PatternEntry>> {
        let rows = pattern_entries::Entity::find()
            .filter(pattern_entries::Column::Enabled.eq(1))
            .all(&self.db)
            .await?;
        rows.into_iter()
            .map(|r| row_to_stored(r).map(|s| s.entry))
            .collect()
    }
}

fn target_columns(target: &PatternTarget) -> (&'static str, Option<String>, Option<String>) {
    match target {
        PatternTarget::O2m { name, value } => ("o2m", Some(name.clone()), Some(value.clone())),
        PatternTarget::O2o { name, value } => ("o2o", Some(name.clone()), Some(value.clone())),
        PatternTarget::Branch { path } => ("branch", None, Some(path.clone())),
        PatternTarget::CollapsePattern { path } => ("collapse", None, Some(path.clone())),
    }
}

fn row_to_stored(row: pattern_entries::Model) -> anyhow::Result<StoredPattern> {
    let target = match row.target_kind.as_str() {
        "o2m" => PatternTarget::O2m {
            name: require(row.target_name, "target_name")?,
            value: require(row.target_value, "target_value")?,
        },
        "o2o" => PatternTarget::O2o {
            name: require(row.target_name, "target_name")?,
            value: require(row.target_value, "target_value")?,
        },
        "branch" => PatternTarget::Branch {
            path: require(row.target_value, "target_value")?,
        },
        "collapse" => PatternTarget::CollapsePattern {
            path: require(row.target_value, "target_value")?,
        },
        other => anyhow::bail!("unknown pattern target_kind `{other}`"),
    };
    Ok(StoredPattern {
        id: row.id as i64,
        entry: PatternEntry {
            pattern: row.pattern,
            target,
        },
        enabled: row.enabled != 0,
    })
}

fn require(value: Option<String>, field: &str) -> anyhow::Result<String> {
    value.ok_or_else(|| anyhow::anyhow!("pattern entry missing `{field}`"))
}
