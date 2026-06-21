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

/// Open an existing database, erroring if the file is not there.
///
/// Only `init` should create a database; every other command requires one to
/// already exist. Without this guard a typo in `--db` would silently open a
/// fresh, empty database (SQLite `mode=rwc` creates the file), making commands
/// look like they ran against an empty archive. Migrations are still applied so
/// an existing database is transparently upgraded.
pub async fn open_existing(path: &Path) -> anyhow::Result<DatabaseConnection> {
    if !path.exists() {
        anyhow::bail!(
            "database not found: {} (run `unclip init` to create it)",
            path.display()
        );
    }
    open(path).await
}

/// A bundle of repositories sharing one connection.
pub struct Repos {
    pub branches: SeaOrmBranchRepository,
    pub frames: SeaOrmFrameRepository,
    pub history: SeaOrmHistoryRepository,
    pub patterns: SeaOrmPatternRepository,
}

/// Open the database and construct the repositories over a shared connection.
///
/// `create` distinguishes `init` (which may create the database) from every
/// other command (which requires it to already exist).
pub async fn open_repos(path: &Path, create: bool) -> anyhow::Result<Repos> {
    let conn = if create {
        open(path).await?
    } else {
        open_existing(path).await?
    };
    Ok(Repos {
        branches: SeaOrmBranchRepository::new(conn.clone()),
        frames: SeaOrmFrameRepository::new(conn.clone()),
        history: SeaOrmHistoryRepository::new(conn.clone()),
        patterns: SeaOrmPatternRepository::new(conn),
    })
}
