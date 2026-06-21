//! Database connection helpers.

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use unclip_migration::{Migrator, MigratorTrait};

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
pub async fn connect(url: &str) -> anyhow::Result<DatabaseConnection> {
    let mut opt = ConnectOptions::new(url.to_owned());
    opt.max_connections(1).min_connections(1);
    let db = Database::connect(opt).await?;
    db.execute_unprepared("PRAGMA foreign_keys = ON;").await?;
    db.execute_unprepared("PRAGMA busy_timeout = 5000;").await?;
    Ok(db)
}

/// Open a connection and run all pending migrations.
pub async fn connect_and_migrate(url: &str) -> anyhow::Result<DatabaseConnection> {
    let db = connect(url).await?;
    Migrator::up(&db, None).await?;
    Ok(db)
}
