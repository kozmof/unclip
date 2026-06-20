//! Database location and connection bootstrap for the CLI.

use std::path::Path;

use unclip_store::SeaOrmBranchRepository;

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

/// Open the database (creating and migrating it if needed) and return a
/// branch repository over it.
pub async fn open_repo(path: &Path) -> anyhow::Result<SeaOrmBranchRepository> {
    let url = db_url(path);
    let db = unclip_store::connect_and_migrate(&url).await?;
    Ok(SeaOrmBranchRepository::new(db))
}
