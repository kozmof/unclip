//! Conservative batching limits for SQLite statements.

/// Keep `IN (...)` lists below SQLite's historical 999-variable limit.
pub(crate) const ID_CHUNK: usize = 500;

/// Maximum rows in a bulk insert.
///
/// The widest active models currently bind fewer than ten values per row, so
/// 100 rows remain below the historical 999-variable limit with room to spare.
pub(crate) const INSERT_ROW_CHUNK: usize = 100;

/// Narrow a domain branch id (`i64`) to the `i32` stored in SQLite.
///
/// The domain model deliberately uses `i64` ids; every conversion to the
/// storage width funnels through here so the failure message stays uniform.
pub(crate) fn sqlite_branch_id(id: i64) -> anyhow::Result<i32> {
    use anyhow::Context;
    i32::try_from(id).context("branch id exceeds SQLite INTEGER range")
}
