//! Throwaway helper: materialize the schema into a file-based SQLite database
//! so sea-orm-cli can generate entities from it. Not part of the build.

use sea_orm::Database;
use unclip_migration::{Migrator, MigratorTrait};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sqlite:///tmp/unclip_schema.db?mode=rwc".to_string());
    let db = Database::connect(&url).await?;
    Migrator::up(&db, None).await?;
    println!("schema written to {url}");
    Ok(())
}
