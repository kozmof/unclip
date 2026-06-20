//! Database location and connection bootstrap for the CLI.

use std::path::Path;

use sea_orm::DatabaseConnection;
use unclip_store::{
    SeaOrmBranchRepository, SeaOrmFrameRepository, SeaOrmHistoryRepository, SeaOrmPatternRepository,
};

/// Build a SQLite connection URL for the given file path.
///
/// The path is resolved to an absolute path and opened in read-write-create
/// mode so the database file is created on first use.
pub fn db_url(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    format!("sqlite://{}?mode=rwc", abs.display())
}

/// Open the database, creating and migrating it if needed.
pub async fn open(path: &Path) -> anyhow::Result<DatabaseConnection> {
    let url = db_url(path);
    unclip_store::connect_and_migrate(&url).await
}

/// A bundle of repositories sharing one connection.
pub struct Repos {
    pub branches: SeaOrmBranchRepository,
    pub frames: SeaOrmFrameRepository,
    pub history: SeaOrmHistoryRepository,
    pub patterns: SeaOrmPatternRepository,
}

/// Open the database and construct the repositories over a shared connection.
pub async fn open_repos(path: &Path) -> anyhow::Result<Repos> {
    let conn = open(path).await?;
    Ok(Repos {
        branches: SeaOrmBranchRepository::new(conn.clone()),
        frames: SeaOrmFrameRepository::new(conn.clone()),
        history: SeaOrmHistoryRepository::new(conn.clone()),
        patterns: SeaOrmPatternRepository::new(conn),
    })
}
