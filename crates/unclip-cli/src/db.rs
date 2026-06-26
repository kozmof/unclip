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
///
/// The path is percent-encoded before being placed in the URL. Without this, a
/// directory containing URL-significant characters (`?` would start the query
/// string, `#` a fragment, `%` an escape) would be silently truncated or
/// mis-decoded, opening the wrong file.
pub fn db_url(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    format!("sqlite://{}?mode=rwc", encode_path(&abs.to_string_lossy()))
}

/// Percent-encode a filesystem path for use in a `sqlite://` URL.
///
/// Every byte outside an unreserved set is `%XX`-escaped. Path separators (`/`)
/// are preserved so the URL still names the same file; sqlx percent-decodes the
/// path back to the original bytes when opening the connection.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        match byte {
            b'/' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn db_url_percent_encodes_significant_characters() {
        let url = db_url(&PathBuf::from("/tmp/od d?x#y%z/db.sqlite"));
        assert_eq!(url, "sqlite:///tmp/od%20d%3Fx%23y%25z/db.sqlite?mode=rwc");
        // Separators survive and the mode query is still distinguishable.
        assert!(url.starts_with("sqlite:///tmp/"));
        assert!(url.ends_with("?mode=rwc"));
    }

    #[test]
    fn db_url_leaves_plain_paths_readable() {
        assert_eq!(
            db_url(&PathBuf::from("/home/user/.unclip/db.sqlite")),
            "sqlite:///home/user/.unclip/db.sqlite?mode=rwc"
        );
    }
}
