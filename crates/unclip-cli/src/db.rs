//! Database location and connection bootstrap for the CLI.

use std::path::Path;

use anyhow::Context;
use sea_orm::{ConnectOptions, DatabaseConnection};
use unclip_store::{
    SeaOrmBranchRepository, SeaOrmDomainRepository, SeaOrmEngineRunRepository,
    SeaOrmFrameRepository, SeaOrmHistoryRepository, SeaOrmMeasurementRepository,
    SeaOrmObservationRepository, SeaOrmPatternRepository, SeaOrmProvenanceRepository,
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
/// archive. Migrations are still applied so an existing database is upgraded,
/// and the upgrade is reported on stderr: it is one-way, so a build older than
/// this one can no longer open the file.
pub async fn open_existing(path: &Path) -> anyhow::Result<DatabaseConnection> {
    let (db, applied) = unclip_store::connect_and_migrate_counted(db_options(path, false)?)
        .await
        .with_context(|| {
            // The existence check is only for wording the failure, and runs
            // after the open has already failed — it is not the preflight
            // check `db_options` deliberately avoids, and cannot reintroduce
            // the race that one would. It is here because the message used to
            // advise `unclip init` for every failure, including a database
            // that plainly exists and could not be opened for some other
            // reason: a schema newer than this build, a permissions problem, a
            // corrupt file. Telling someone to create an archive they already
            // have sends them looking in the wrong place.
            if path.exists() {
                format!("database could not be opened: {}", path.display())
            } else {
                format!(
                    "database not found: {} (run `unclip init` to create it)",
                    path.display()
                )
            }
        })?;
    if applied > 0 {
        crate::output::errln!(
            "upgraded database schema of {} ({applied} migration{})",
            path.display(),
            if applied == 1 { "" } else { "s" }
        );
    }
    Ok(db)
}

/// A bundle of repositories sharing one connection.
pub struct Repos {
    pub experiments: unclip_store::SeaOrmExperimentRepository,
    pub branches: SeaOrmBranchRepository,
    pub domains: SeaOrmDomainRepository,
    pub engine_runs: SeaOrmEngineRunRepository,
    pub frames: SeaOrmFrameRepository,
    pub history: SeaOrmHistoryRepository,
    pub measurements: SeaOrmMeasurementRepository,
    pub patterns: SeaOrmPatternRepository,
    pub observations: SeaOrmObservationRepository,
    pub provenance: SeaOrmProvenanceRepository,
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
        experiments: unclip_store::SeaOrmExperimentRepository::new(conn.clone()),
        branches: SeaOrmBranchRepository::new(conn.clone()),
        domains: SeaOrmDomainRepository::new(conn.clone()),
        engine_runs: SeaOrmEngineRunRepository::new(conn.clone()),
        frames: SeaOrmFrameRepository::new(conn.clone()),
        history: SeaOrmHistoryRepository::new(conn.clone()),
        measurements: SeaOrmMeasurementRepository::new(conn.clone()),
        patterns: SeaOrmPatternRepository::new(conn.clone()),
        observations: SeaOrmObservationRepository::new(conn.clone()),
        provenance: SeaOrmProvenanceRepository::new(conn),
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
        assert!(error.contains("unclip init"), "got: {error}");
        assert!(!path.exists());
    }

    /// A file that exists but cannot be opened is not a missing database, and
    /// advising `unclip init` for it sends the reader to the wrong problem.
    #[cfg(unix)]
    #[tokio::test]
    async fn open_existing_does_not_advise_init_for_a_database_that_exists() {
        let path = std::env::temp_dir().join(format!(
            "unclip-unopenable-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_nanos()
        ));
        // A directory at the path exists and is not a SQLite database, so the
        // open fails for a reason that has nothing to do with absence.
        std::fs::create_dir(&path).unwrap();

        let error = open_existing(&path).await.unwrap_err().to_string();
        std::fs::remove_dir(&path).unwrap();

        assert!(error.contains("could not be opened"), "got: {error}");
        assert!(
            !error.contains("unclip init"),
            "a database that exists must not be reported as one to create: {error}"
        );
    }
}
