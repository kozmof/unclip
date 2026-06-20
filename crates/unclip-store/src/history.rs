//! Usage history and selection-packet persistence.

use std::collections::HashSet;

use sea_orm::{
    ActiveValue::{NotSet, Set},
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use unclip_entity::{selection_packets, usage_history};

/// Aggregate usage info for a single branch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageSummary {
    pub count: u64,
    pub last_used: Option<String>,
}

/// Record to persist when saving a packet.
pub struct PacketRecord<'a> {
    pub id: &'a str,
    pub frame_name: Option<&'a str>,
    pub seed: Option<u64>,
    pub query_json: Option<&'a str>,
    pub packet_json: &'a str,
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
            used_at: Set(chrono::Utc::now().to_rfc3339()),
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

    /// Usage count and last-used timestamp for a branch.
    pub async fn usage_for(&self, branch_id: i64) -> anyhow::Result<UsageSummary> {
        let count = usage_history::Entity::find()
            .filter(usage_history::Column::BranchId.eq(branch_id as i32))
            .count(&self.db)
            .await?;
        let last = usage_history::Entity::find()
            .filter(usage_history::Column::BranchId.eq(branch_id as i32))
            .order_by_desc(usage_history::Column::UsedAt)
            .one(&self.db)
            .await?
            .map(|r| r.used_at);
        Ok(UsageSummary {
            count,
            last_used: last,
        })
    }

    /// Persist a selection packet.
    pub async fn save_packet(&self, record: PacketRecord<'_>) -> anyhow::Result<()> {
        let am = selection_packets::ActiveModel {
            id: Set(record.id.to_string()),
            frame_name: Set(record.frame_name.map(str::to_string)),
            seed: Set(record.seed.map(|s| s as i64)),
            created_at: Set(chrono::Utc::now().to_rfc3339()),
            query_json: Set(record.query_json.map(str::to_string)),
            packet_json: Set(record.packet_json.to_string()),
        };
        selection_packets::Entity::insert(am).exec(&self.db).await?;
        Ok(())
    }
}
