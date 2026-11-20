//! One-off import from a real Plex database.
//!
//! The Plex database is a reference, not our schema, so this reads it with plain SQL and maps
//! into our entities. It is the only code allowed to know Plex's column names and encodings,
//! which is what keeps those out of `core` and the bindings.
//!
//! The source is opened read-only: it is a snapshot and must stay untouched. No account is
//! ever read, so the real names and e-mail addresses it holds are never copied.

mod analysis;
pub mod chapters;
pub mod collections;
pub mod extras;
pub mod library;
pub mod markers;
pub mod metadata;
pub mod tags;

use std::path::Path;

use sea_orm::{Database, DatabaseConnection, DbErr, QueryResult, TryGetable};

pub use chapters::ChapterSummary;
pub use collections::CollectionSummary;
pub use extras::ExtraSummary;
pub use library::LibrarySummary;
pub use markers::MarkerSummary;
pub use metadata::MetadataSummary;
pub use tags::TagSummary;

/// How many source rows to read per statement.
///
/// Paged rather than loaded whole: the reference library holds 947,346 tracks, and an importer
/// whose memory scales with the library fails on the libraries that need it most.
pub(crate) const SOURCE_PAGE: u64 = 5_000;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("cannot read the Plex database at {path}: {source}")]
    Source {
        path: String,
        #[source]
        source: DbErr,
    },
    #[error("a required column is missing from the Plex database: {0}")]
    MissingColumn(String),
    #[error("writing imported rows failed: {0}")]
    Destination(#[from] DbErr),
}

pub(crate) fn source_error(path: &str, source: DbErr) -> ImportError {
    ImportError::Source {
        path: path.to_owned(),
        source,
    }
}

/// Everything one import run copied.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImportSummary {
    pub sections: usize,
    pub roots: usize,
    pub items: usize,
    pub title_words: usize,
    pub media: usize,
    pub files: usize,
    pub streams: usize,
    pub tags: TagSummary,
    pub markers: MarkerSummary,
    pub chapters: ChapterSummary,
    pub collections: CollectionSummary,
    pub extras: ExtraSummary,
    /// Rows skipped because the parent they reference does not exist.
    pub orphans: usize,
}

/// Copies a library's structure and contents into our schema.
///
/// Existing rows are replaced, so a re-import is idempotent and picks up changes made on the
/// Plex side.
///
/// # Errors
///
/// Fails if the source cannot be opened or read, or if writing fails.
pub async fn import_library(
    destination: &DatabaseConnection,
    plex_db: &Path,
) -> Result<ImportSummary, ImportError> {
    let path = plex_db.display().to_string();
    let source = open_read_only(plex_db).await?;

    let structure = library::import(destination, &source, &path).await?;
    let contents = metadata::import(destination, &source, &path).await?;

    Ok(ImportSummary {
        sections: structure.sections,
        roots: structure.roots,
        items: contents.items,
        title_words: contents.title_words,
        media: contents.media,
        files: contents.files,
        streams: contents.streams,
        tags: contents.tags,
        markers: contents.markers,
        chapters: contents.chapters,
        collections: contents.collections,
        extras: contents.extras,
        orphans: contents.orphans,
    })
}

async fn open_read_only(path: &Path) -> Result<DatabaseConnection, ImportError> {
    // Forward slashes and `mode=ro` so the snapshot is never modified, and so the URL is
    // valid on Windows where `Path` uses backslashes.
    let url = format!(
        "sqlite://{}?mode=ro",
        path.display()
            .to_string()
            .replace(std::path::MAIN_SEPARATOR, "/")
    );
    Database::connect(url)
        .await
        .map_err(|source| source_error(&path.display().to_string(), source))
}

/// Reading columns out of Plex's loosely-typed rows.
///
/// Plex leaves columns null that mean "false", writes empty strings where a value is absent,
/// and stores dates as epochs. Centralising those conventions keeps the import passes
/// readable, and keeps the quirks in one place to point at.
pub(crate) trait PlexRow {
    /// A column that must be present, because the row is useless without it.
    ///
    /// # Errors
    ///
    /// Returns [`ImportError::MissingColumn`] when the column is absent or the wrong type.
    fn plex_get<T: TryGetable>(&self, column: &str) -> Result<T, ImportError>;

    /// A column that may be null.
    fn plex_opt<T: TryGetable>(&self, column: &str) -> Option<T>;

    /// Text that may be null *or* empty, both meaning absent.
    ///
    /// Plex writes `''` for an unset `name_sort` and for an embedded track's `url`; relaying
    /// that would publish an empty string where the API documents null.
    fn plex_text(&self, column: &str) -> Option<String>;

    /// A flag Plex only writes when true, leaving it null otherwise.
    fn plex_flag(&self, column: &str) -> bool;
}

impl PlexRow for QueryResult {
    fn plex_get<T: TryGetable>(&self, column: &str) -> Result<T, ImportError> {
        self.try_get("", column)
            .map_err(|_| ImportError::MissingColumn(column.to_owned()))
    }

    fn plex_opt<T: TryGetable>(&self, column: &str) -> Option<T> {
        self.try_get("", column).ok()
    }

    fn plex_text(&self, column: &str) -> Option<String> {
        self.plex_opt::<String>(column)
            .filter(|text| !text.trim().is_empty())
    }

    fn plex_flag(&self, column: &str) -> bool {
        self.plex_opt::<i32>(column).unwrap_or(0) != 0
    }
}
