//! Database location and connection bootstrap for the CLI.

use std::path::Path;

use anyhow::Context;
use sea_orm::{ConnectOptions, DatabaseConnection};
use unclip_store::{
    SeaOrmBranchRepository, SeaOrmFrameRepository, SeaOrmHistoryRepository, SeaOrmPatternRepository,
};

/// Build SQLite connection options for the given file path.
///
/// The path is resolved to an absolute path and passed to SQLx as a native
/// filesystem `Path`. This avoids URL-significant characters and preserves
/// non-UTF-8 input until SQLx can reject it instead of opening a lossy
/// replacement path.
fn db_options(path: &Path, create: bool) -> anyhow::Result<ConnectOptions> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|err| anyhow::anyhow!("failed to resolve database path: {err}"))?
    };

    // Select the SQLite open mode before replacing only the filename through
    // SQLx's native Path API. Existing-database callers use `mode=rw`, so the
    // open itself—not a racy preflight existence check—guarantees that a
    // missing file cannot be silently recreated.
    let url = if create {
        "sqlite://unclip-placeholder?mode=rwc"
    } else {
        "sqlite://unclip-placeholder?mode=rw"
    };
    let mut options = ConnectOptions::new(url);
    options.map_sqlx_sqlite_opts(move |sqlite| sqlite.filename(&abs));
    Ok(options)
}

/// Open the database, creating and migrating it if needed.
pub async fn open(path: &Path) -> anyhow::Result<DatabaseConnection> {
    unclip_store::connect_and_migrate_with_options(db_options(path, true)?)
        .await
        .map_err(Into::into)
}

/// Open an existing database, erroring if the file is not there.
///
/// Only `init` should create a database; every other command opens SQLite in
/// read-write-only mode. This makes the existence requirement atomic with the
/// open: a typo or a file removed concurrently cannot create a fresh, empty
/// archive. Migrations are still applied so an existing database is
/// transparently upgraded.
pub async fn open_existing(path: &Path) -> anyhow::Result<DatabaseConnection> {
    unclip_store::connect_and_migrate_with_options(db_options(path, false)?)
        .await
        .with_context(|| {
            format!(
                "database not found or could not be opened: {} (run `unclip init` to create it)",
                path.display()
            )
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn open_rejects_non_utf8_path_without_lossy_replacement() {
        use std::os::unix::ffi::OsStringExt;

        let bytes: Vec<u8> = format!("/tmp/unclip-non-utf8-{}-", std::process::id())
            .into_bytes()
            .into_iter()
            .chain([0xff])
            .chain(b".db".iter().copied())
            .collect();
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes));
        let error = open(&path).await.unwrap_err().to_string();
        assert!(error.contains("valid UTF-8"), "got: {error}");
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_handles_url_significant_path_characters() {
        let path =
            std::path::PathBuf::from(format!("/tmp/unclip-od d?x#y%z-{}.db", std::process::id()));
        let db = open(&path).await.unwrap();
        assert!(path.exists());
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn open_existing_never_creates_a_missing_database() {
        let path = std::env::temp_dir().join(format!(
            "unclip-missing-existing-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_nanos()
        ));
        assert!(!path.exists());

        let error = open_existing(&path).await.unwrap_err().to_string();

        assert!(error.contains("database not found"), "got: {error}");
        assert!(!path.exists());
    }
}
