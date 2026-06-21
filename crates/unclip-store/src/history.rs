//! Usage history and selection-packet persistence.

use std::collections::{HashMap, HashSet};

use sea_orm::{
    ActiveValue::{NotSet, Set},
    DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryOrder, QuerySelect, Statement,
};
use unclip_entity::{selection_packets, usage_history};

/// Aggregate usage info for a single branch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageSummary {
    pub count: u64,
    pub last_used: Option<String>,
}

/// Row shape for the batched usage aggregate query.
#[derive(Debug, FromQueryResult)]
struct UsageRow {
    branch_id: i64,
    count: i64,
    last_used: Option<String>,
}

/// Record to persist when saving a packet.
pub struct PacketRecord<'a> {
    pub id: &'a str,
    pub frame_name: Option<&'a str>,
    pub seed: Option<u64>,
    pub query_json: Option<&'a str>,
    pub packet_json: &'a str,
}

/// Current UTC time in a fixed-width, `Z`-suffixed RFC3339 form.
///
/// Fixed millisecond precision and a literal `Z` make these timestamps
/// lexically sortable, which `recent_branch_ids` and the `MAX(used_at)`
/// aggregate rely on for correct ordering. Shared so every timestamp written
/// to the database — and every packet `created_at` produced by the CLI — uses
/// one canonical format.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Usage history and packet store.
pub struct SeaOrmHistoryRepository {
    db: DatabaseConnection,
}

impl SeaOrmHistoryRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Record that a branch was used.
    pub async fn record_usage(
        &self,
        branch_id: i64,
        command: &str,
        context: Option<&str>,
        packet_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let am = usage_history::ActiveModel {
            id: NotSet,
            branch_id: Set(branch_id as i32),
            used_at: Set(now()),
            command: Set(Some(command.to_string())),
            context: Set(context.map(str::to_string)),
            packet_id: Set(packet_id.map(str::to_string)),
        };
        usage_history::Entity::insert(am).exec(&self.db).await?;
        Ok(())
    }

    /// The set of branch ids appearing in the most recent `limit` usage rows.
    pub async fn recent_branch_ids(&self, limit: u64) -> anyhow::Result<HashSet<i64>> {
        let rows = usage_history::Entity::find()
            .order_by_desc(usage_history::Column::UsedAt)
            .limit(limit)
            .all(&self.db)
            .await?;
        Ok(rows.into_iter().map(|r| r.branch_id as i64).collect())
    }

    /// Usage count and last-used timestamp for many branches in one query.
    ///
    /// Branches with no usage are simply absent from the returned map; callers
    /// treat a missing entry as `UsageSummary::default()` (zero uses).
    pub async fn usage_summaries(
        &self,
        branch_ids: &[i64],
    ) -> anyhow::Result<HashMap<i64, UsageSummary>> {
        if branch_ids.is_empty() {
            return Ok(HashMap::new());
        }
        // `branch_ids` are integers from the store, never user text.
        let id_list = branch_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT branch_id, COUNT(*) AS count, MAX(used_at) AS last_used \
             FROM usage_history WHERE branch_id IN ({id_list}) GROUP BY branch_id"
        );
        let rows = UsageRow::find_by_statement(Statement::from_string(DbBackend::Sqlite, sql))
            .all(&self.db)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.branch_id,
                    UsageSummary {
                        count: r.count.max(0) as u64,
                        last_used: r.last_used,
                    },
                )
            })
            .collect())
    }

    /// Usage count and last-used timestamp for a branch.
    ///
    /// Delegates to the batched aggregate so a single branch and a set of
    /// branches share one code path (and one query shape).
    pub async fn usage_for(&self, branch_id: i64) -> anyhow::Result<UsageSummary> {
        Ok(self
            .usage_summaries(&[branch_id])
            .await?
            .remove(&branch_id)
            .unwrap_or_default())
    }

    /// Persist a selection packet.
    pub async fn save_packet(&self, record: PacketRecord<'_>) -> anyhow::Result<()> {
        let am = selection_packets::ActiveModel {
            id: Set(record.id.to_string()),
            frame_name: Set(record.frame_name.map(str::to_string)),
            seed: Set(record.seed.map(|s| s as i64)),
            created_at: Set(now()),
            query_json: Set(record.query_json.map(str::to_string)),
            packet_json: Set(record.packet_json.to_string()),
        };
        selection_packets::Entity::insert(am).exec(&self.db).await?;
        Ok(())
    }
}
