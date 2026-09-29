//! Database connection helpers.

use crate::StoreResult;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};

/// Open a connection to the given SQLite URL and enable foreign keys.
///
/// The pool is pinned to a single connection. SQLite `PRAGMA`s are
/// connection-scoped, so a pool with several connections would leave
/// `foreign_keys`/`busy_timeout` set only on whichever connection happened to
/// run them. Since this is a single-process CLI, one connection is plenty and
/// guarantees both pragmas apply to every query. It also keeps in-memory
/// databases (whose schema lives only in their connection) working.
///
/// `busy_timeout` lets a second writer (e.g. a concurrent CLI invocation in a
/// separate process) wait briefly for a lock instead of failing immediately
/// with "database is locked".
///
/// `journal_mode = WAL` lets readers proceed while a writer holds the database.
/// Under the default rollback journal a second `unclip` process blocks for the
/// whole `busy_timeout` even when it only reads. WAL is persistent (it is
/// recorded in the file header, not per connection), so setting it here also
/// upgrades databases created before it was requested; an in-memory database
/// ignores the request, which is why its failure is tolerated below.
pub async fn connect(url: &str) -> StoreResult<DatabaseConnection> {
    connect_with_options(ConnectOptions::new(url.to_owned())).await
}

/// Open a connection from pre-built options.
///
/// This is used by path-based callers that need to pass a native filesystem
/// `Path` to SQLx instead of round-tripping it through a UTF-8 connection URL.
pub async fn connect_with_options(mut opt: ConnectOptions) -> StoreResult<DatabaseConnection> {
    opt.max_connections(1).min_connections(1);
    let db = Database::connect(opt).await?;
    db.execute_unprepared("PRAGMA foreign_keys = ON;").await?;
    db.execute_unprepared("PRAGMA busy_timeout = 5000;").await?;
    // An in-memory database has no file to journal and refuses WAL, and a
    // read-only medium cannot rewrite the header. Neither is a reason to fail
    // the connection: both still work correctly under the default journal.
    let _ = db.execute_unprepared("PRAGMA journal_mode = WAL;").await;
    Ok(db)
}

/// Open a connection and run all pending migrations.
pub async fn connect_and_migrate(url: &str) -> StoreResult<DatabaseConnection> {
    connect_and_migrate_with_options(ConnectOptions::new(url.to_owned())).await
}

/// Open configured connection options and run all pending migrations.
pub async fn connect_and_migrate_with_options(
    opt: ConnectOptions,
) -> StoreResult<DatabaseConnection> {
    let db = connect_with_options(opt).await?;
    unclip_migration::up(&db, None).await?;
    Ok(db)
}
