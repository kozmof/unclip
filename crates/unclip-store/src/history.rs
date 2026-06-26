//! Usage history and selection-packet persistence.

use std::collections::{HashMap, HashSet};

use anyhow::Context;
use sea_orm::{
    ActiveValue::{NotSet, Set},
    DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryOrder, QuerySelect,
    Statement, TransactionTrait,
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
        let branch_id =
            i32::try_from(branch_id).context("branch id exceeds SQLite INTEGER range")?;
        let am = usage_history::ActiveModel {
            id: NotSet,
            branch_id: Set(branch_id),
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
            // `id` breaks ties so rows sharing a millisecond timestamp have a
            // stable, deterministic order under `LIMIT`.
            .order_by_desc(usage_history::Column::Id)
            .limit(limit)
            .all(&self.db)
            .await?;
        Ok(rows.into_iter().map(|r| r.branch_id as i64).collect())
    }

    /// Usage count and last-used timestamp for many branches.
    ///
    /// Branches with no usage are simply absent from the returned map; callers
    /// treat a missing entry as `UsageSummary::default()` (zero uses).
    ///
    /// The ids are queried in chunks of bound parameters. `stats`/`stale` can
    /// pass every matched branch, so a single `IN (...)` would otherwise grow
    /// past SQLite's bound-variable limit as an archive grows.
    pub async fn usage_summaries(
        &self,
        branch_ids: &[i64],
    ) -> anyhow::Result<HashMap<i64, UsageSummary>> {
        // Stay well under SQLite's default `SQLITE_MAX_VARIABLE_NUMBER` (999 on
        // older builds) with margin to spare.
        const CHUNK: usize = 500;

        let mut summaries = HashMap::new();
        for chunk in branch_ids.chunks(CHUNK) {
            // Bind the ids as parameters rather than interpolating them: one `?`
            // placeholder per id keeps the values out of the SQL text entirely.
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT branch_id, COUNT(*) AS count, MAX(used_at) AS last_used \
                 FROM usage_history WHERE branch_id IN ({placeholders}) GROUP BY branch_id"
            );
            let values = chunk.iter().map(|&id| id.into());
            let rows = UsageRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                values,
            ))
            .all(&self.db)
            .await?;
            summaries.extend(rows.into_iter().map(|r| {
                (
                    r.branch_id,
                    UsageSummary {
                        count: r.count.max(0) as u64,
                        last_used: r.last_used,
                    },
                )
            }));
        }
        Ok(summaries)
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
        let seed = record
            .seed
            .map(|s| i64::try_from(s).context("packet seed exceeds SQLite INTEGER range"))
            .transpose()?;
        let am = selection_packets::ActiveModel {
            id: Set(record.id.to_string()),
            frame_name: Set(record.frame_name.map(str::to_string)),
            seed: Set(seed),
            created_at: Set(now()),
            query_json: Set(record.query_json.map(str::to_string)),
            packet_json: Set(record.packet_json.to_string()),
        };
        selection_packets::Entity::insert(am).exec(&self.db).await?;
        Ok(())
    }

    /// Persist a packet and the usage rows for its selected branches atomically.
    ///
    /// The packet row and one `usage_history` row per id in `branch_ids` are
    /// written in a single transaction, so a failure part-way through cannot
    /// leave a packet without its usages (or vice versa). All rows share one
    /// `used_at`/`created_at` timestamp.
    pub async fn save_packet_with_usages(
        &self,
        record: PacketRecord<'_>,
        command: &str,
        branch_ids: &[i64],
    ) -> anyhow::Result<()> {
        let ts = now();
        let txn = self.db.begin().await?;
        let seed = record
            .seed
            .map(|s| i64::try_from(s).context("packet seed exceeds SQLite INTEGER range"))
            .transpose()?;

        let packet = selection_packets::ActiveModel {
            id: Set(record.id.to_string()),
            frame_name: Set(record.frame_name.map(str::to_string)),
            seed: Set(seed),
            created_at: Set(ts.clone()),
            query_json: Set(record.query_json.map(str::to_string)),
            packet_json: Set(record.packet_json.to_string()),
        };
        selection_packets::Entity::insert(packet).exec(&txn).await?;

        if !branch_ids.is_empty() {
            let usages: Vec<usage_history::ActiveModel> = branch_ids
                .iter()
                .map(|&id| {
                    Ok(usage_history::ActiveModel {
                        id: NotSet,
                        branch_id: Set(
                            i32::try_from(id).context("branch id exceeds SQLite INTEGER range")?
                        ),
                        used_at: Set(ts.clone()),
                        command: Set(Some(command.to_string())),
                        context: Set(None),
                        packet_id: Set(Some(record.id.to_string())),
                    })
                })
                .collect::<anyhow::Result<_>>()?;
            usage_history::Entity::insert_many(usages)
                .exec(&txn)
                .await?;
        }

        txn.commit().await?;
        Ok(())
    }
}
