//! Conservative batching limits for SQLite statements.

use sea_orm::{ActiveModelTrait, DatabaseTransaction, DbErr, EntityTrait};

/// Keep `IN (...)` lists below SQLite's historical 999-variable limit.
pub(crate) const ID_CHUNK: usize = 500;

/// Maximum rows in a bulk insert.
///
/// The widest active models currently bind fewer than ten values per row, so
/// 100 rows remain below the historical 999-variable limit with room to spare.
pub(crate) const INSERT_ROW_CHUNK: usize = 100;

/// Insert every row in bounded batches, consuming them.
///
/// Chunking keeps each statement below SQLite's bound-variable limit. The rows
/// are *drained* rather than borrowed and cloned: `insert_many` takes an
/// `IntoIterator` of owned models, so passing `chunks(..).iter().cloned()`
/// would copy every `ActiveModel` on its way into SQL for no reason. Empty
/// batches are skipped because SeaORM rejects a zero-row insert.
pub(crate) async fn insert_chunked<E, A>(
    txn: &DatabaseTransaction,
    mut rows: Vec<A>,
) -> Result<(), DbErr>
where
    E: EntityTrait,
    A: ActiveModelTrait<Entity = E>,
{
    while !rows.is_empty() {
        let take = rows.len().min(INSERT_ROW_CHUNK);
        E::insert_many(rows.drain(..take)).exec(txn).await?;
    }
    Ok(())
}

// Row ids need no width conversion: SQLite's `INTEGER PRIMARY KEY` is a 64-bit
// rowid, and the entities declare `i64` to match, so a domain `Branch::id` maps
// straight through. Narrowing to `i32` here used to add a fallible conversion
// on every read and write path for no storage benefit.
