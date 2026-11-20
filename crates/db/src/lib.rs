//! Storage layer.
//!
//! Depends on `core` and is depended on by no binding: a `use klapz_db::…` inside an API
//! binding would mean the layering has broken down.

pub mod access_repository;
mod chapter_store;
mod closest_store;
pub mod collection_repository;
mod common_fields;
mod completion;
pub mod entity;
mod extra_store;
pub mod import;
mod item_order;
mod marker_store;
pub mod metadata_repository;
pub mod migration;
pub mod play_queue_repository;
pub mod playback_repository;
pub mod playlist_repository;
pub mod repository;
pub mod scan_repository;
mod tracks;
pub mod watch_repository;
pub mod write;

pub use sea_orm::DatabaseConnection;
use sea_orm::{ConnectOptions, Database, DbErr};
use sea_orm_migration::MigratorTrait;

pub use access_repository::SqlAccessRepository;
pub use collection_repository::SqlCollectionRepository;
pub use metadata_repository::SqlMetadataRepository;
pub use play_queue_repository::SqlPlayQueueRepository;
pub use playback_repository::SqlPlaybackRepository;
pub use playlist_repository::SqlPlaylistRepository;
pub use repository::SqlLibraryRepository;
pub use scan_repository::SqlScanRepository;
pub use watch_repository::SqlWatchRepository;

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
