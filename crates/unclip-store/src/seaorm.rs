//! Database connection helpers.

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use unclip_migration::{Migrator, MigratorTrait};

/// Open a connection to the given SQLite URL and enable foreign keys.
///
/// In-memory databases are pinned to a single pooled connection so the schema
/// and data survive for the lifetime of the pool.
pub async fn connect(url: &str) -> anyhow::Result<DatabaseConnection> {
    let mut opt = ConnectOptions::new(url.to_owned());
    if url.contains(":memory:") {
        opt.max_connections(1).min_connections(1);
    }
    let db = Database::connect(opt).await?;
    db.execute_unprepared("PRAGMA foreign_keys = ON;").await?;
    Ok(db)
}

/// Open a connection and run all pending migrations.
pub async fn connect_and_migrate(url: &str) -> anyhow::Result<DatabaseConnection> {
    let db = connect(url).await?;
    Migrator::up(&db, None).await?;
    Ok(db)
}
