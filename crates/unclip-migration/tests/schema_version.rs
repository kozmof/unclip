//! Opening an archive written by a newer build must fail, not migrate it.

use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

/// Every command migrates on open, so an older build opening a newer archive
/// would otherwise upgrade a schema it does not understand and then read the
/// rows through whatever it made of them.
///
/// The migrator refuses instead, because a ledger row with no migration file
/// is an error to it. That behaviour is load-bearing and belongs to SeaORM
/// rather than to this crate, which is exactly why it is pinned here: nothing
/// in this repository would otherwise notice if it changed.
#[tokio::test]
async fn a_database_newer_than_this_build_is_refused_rather_than_migrated() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    unclip_migration::up(&db, None).await.unwrap();
    // Re-opening an up-to-date archive is the ordinary case and must succeed.
    unclip_migration::up(&db, None).await.unwrap();

    // Simulate a newer build having applied a migration this one does not have.
    db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "INSERT INTO seaql_migrations (version, applied_at) \
         VALUES ('m20270101_000017_from_the_future', 0)",
    ))
    .await
    .unwrap();

    let error = unclip_migration::up(&db, None)
        .await
        .expect_err("a newer database must be refused");
    let message = error.to_string();
    assert!(
        message.contains("m20270101_000017_from_the_future"),
        "the failure must name the unknown migration, got: {message}"
    );
    assert!(
        message.contains("missing"),
        "the failure must say the migration file is missing, got: {message}"
    );
}
