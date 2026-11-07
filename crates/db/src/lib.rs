//! Storage layer.
//!
//! Depends on `core` and is depended on by no binding: a `use open_plex_db::…` inside an API
//! binding would mean the layering has broken down.

pub mod entity;
pub mod import;
pub mod metadata_repository;
pub mod migration;
pub mod repository;
pub mod write;

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use sea_orm_migration::MigratorTrait;

pub use metadata_repository::SqlMetadataRepository;
pub use repository::SqlLibraryRepository;

/// Opens a connection pool and brings the schema up to date.
///
/// Running migrations on connect means a fresh checkout needs no separate setup step, and
/// every test run exercises the migrations too.
///
/// # Errors
///
/// Fails if the database cannot be reached or a migration fails.
pub async fn connect(url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(url.to_owned());
    options.sqlx_logging_level(tracing::log::LevelFilter::Debug);

    let connection = Database::connect(options).await?;
    migration::Migrator::up(&connection, None).await?;
    Ok(connection)
}
