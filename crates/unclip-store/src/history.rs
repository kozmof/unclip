//! Usage history and selection-packet persistence.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use sea_orm::{
    ActiveValue::{NotSet, Set},
    DatabaseConnection, DatabaseTransaction, DbBackend, EntityTrait, FromQueryResult, QueryOrder,
    QuerySelect, Statement, TransactionTrait,
};
use unclip_entity::{selection_packets, usage_history};

use crate::{
    sqlite_limits::{sqlite_branch_id, INSERT_ROW_CHUNK},
    StoreResult,
};

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

/// A packet and its selected branch ids, persisted atomically. Single-packet
/// callers pass a one-element batch; there is deliberately no separate
/// single-packet record type or code path.
pub struct PacketUsageRecord {
    pub id: String,
    pub frame_name: Option<String>,
    pub seed: Option<u64>,
    pub query_json: Option<String>,
    pub packet_json: String,
    pub branch_ids: Vec<i64>,
}

/// Persistence boundary used by sampling and usage-reporting commands.
#[async_trait]
pub trait HistoryRepository: Sync {
    async fn recent_branch_ids(&self, limit: u64) -> StoreResult<HashSet<i64>>;
    async fn usage_summaries(&self, branch_ids: &[i64]) -> StoreResult<HashMap<i64, UsageSummary>>;
    async fn usage_for(&self, branch_id: i64) -> StoreResult<UsageSummary>;
    /// Persist packets and their usage rows atomically: a failure on any packet
    /// rolls back the whole batch.
    async fn save_packets_with_usages(
        &self,
        records: &[PacketUsageRecord],
        command: &str,
    ) -> StoreResult<()>;
}

/// Store all 64 bits of an RNG seed in SQLite's signed 64-bit INTEGER.
///
/// This is a bit-preserving representation, not a numeric narrowing: seeds
/// above `i64::MAX` appear negative in the auxiliary database column, while
/// the packet JSON retains their ordinary unsigned representation.
fn encode_seed(seed: u64) -> i64 {
    i64::from_be_bytes(seed.to_be_bytes())
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
    ) -> StoreResult<()> {
        let branch_id = sqlite_branch_id(branch_id)?;
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
}

#[async_trait]
impl HistoryRepository for SeaOrmHistoryRepository {
    /// The set of branch ids appearing in the most recent `limit` usage rows.
    async fn recent_branch_ids(&self, limit: u64) -> StoreResult<HashSet<i64>> {
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
    async fn usage_summaries(&self, branch_ids: &[i64]) -> StoreResult<HashMap<i64, UsageSummary>> {
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
    async fn usage_for(&self, branch_id: i64) -> StoreResult<UsageSummary> {
        Ok(self
            .usage_summaries(&[branch_id])
            .await?
            .remove(&branch_id)
            .unwrap_or_default())
    }

    /// Persist a batch of packets and all their usage rows atomically.
    ///
    /// Every packet row and one `usage_history` row per selected branch are
    /// written in a single transaction sharing one `used_at`/`created_at`
    /// timestamp, so a failure in any packet rolls back every packet and usage
    /// row in the batch — matching the all-or-nothing behavior of branch and
    /// frame imports. Single-packet callers pass a one-element slice.
    async fn save_packets_with_usages(
        &self,
        records: &[PacketUsageRecord],
        command: &str,
    ) -> StoreResult<()> {
        let ts = now();
        let txn = self.db.begin().await?;

        for record in records {
            let packet = selection_packets::ActiveModel {
                id: Set(record.id.clone()),
                frame_name: Set(record.frame_name.clone()),
                seed: Set(record.seed.map(encode_seed)),
                created_at: Set(ts.clone()),
                query_json: Set(record.query_json.clone()),
                packet_json: Set(record.packet_json.clone()),
            };
            selection_packets::Entity::insert(packet).exec(&txn).await?;
            insert_usages(&txn, &ts, command, &record.id, &record.branch_ids).await?;
        }

        txn.commit().await?;
        Ok(())
    }
}

/// Insert one usage row per branch id, chunked below SQLite's bound-variable
/// limit, all sharing `ts` and pointing at `packet_id`.
async fn insert_usages(
    txn: &DatabaseTransaction,
    ts: &str,
    command: &str,
    packet_id: &str,
    branch_ids: &[i64],
) -> StoreResult<()> {
    if branch_ids.is_empty() {
        return Ok(());
    }
    let usages: Vec<usage_history::ActiveModel> = branch_ids
        .iter()
        .map(|&id| {
            Ok(usage_history::ActiveModel {
                id: NotSet,
                branch_id: Set(sqlite_branch_id(id)?),
                used_at: Set(ts.to_string()),
                command: Set(Some(command.to_string())),
                context: Set(None),
                packet_id: Set(Some(packet_id.to_string())),
            })
        })
        .collect::<anyhow::Result<_>>()?;
    for chunk in usages.chunks(INSERT_ROW_CHUNK) {
        usage_history::Entity::insert_many(chunk.iter().cloned())
            .exec(txn)
            .await?;
    }
    Ok(())
}
