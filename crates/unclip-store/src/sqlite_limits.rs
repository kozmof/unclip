//! Conservative batching limits for SQLite statements.

/// Keep `IN (...)` lists below SQLite's historical 999-variable limit.
pub(crate) const ID_CHUNK: usize = 500;

/// Maximum rows in a bulk insert.
///
/// The widest active models currently bind fewer than ten values per row, so
/// 100 rows remain below the historical 999-variable limit with room to spare.
pub(crate) const INSERT_ROW_CHUNK: usize = 100;
