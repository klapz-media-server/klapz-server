//! Scanning a library: finding its files, grouping them into items, and keeping storage in step.
//!
//! Plex's own split, measured in `docs/scanner/`: walking, deciding what changed, grouping and
//! storing happen in the server, here; reading a file's content happens in a separate process,
//! behind [`MediaAnalyzer`]. Files are reached only through [`LibraryFiles`], so a library kept
//! somewhere other than a local disk changes nothing in this module.
//!
//! [`LibraryFiles`]: crate::files::LibraryFiles

mod control;
pub mod layout;
mod walk;

use klapz_scanner::report::Analysis;

use crate::files::{FileLocation, RelativePath};
use crate::ids::{MediaId, PartId, RatingKey, SectionId};

pub use control::{BackgroundScans, ScanControl};
pub use walk::{scan_section, ScanOutcome, ScanRequest, Scanner};

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("no library section with id {0}")]
    SectionNotFound(SectionId),
    #[error("the library is a kind this server does not scan")]
    UnsupportedKind,
    #[error("{0} is not below any of the library's roots")]
    OutsideRoots(String),
    #[error("a scan of this library is already running")]
    AlreadyRunning,
    #[error("the scan was cancelled")]
    Cancelled,
    /// The analyzer cannot run at all. The scan stops before storing the file it was on, so
    /// that the next scan, once the setup is fixed, finds it new and analyzes it.
    #[error("the media analyzer cannot run: {0}")]
    AnalyzerUnavailable(String),
    #[error("scan storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Debug, thiserror::Error)]
pub enum AnalyzeError {
    /// The file is corrupt, truncated or not media: recorded against it, not retried.
    #[error("the file could not be read as media: {0}")]
    Unreadable(String),
    /// The analyzer itself failed — it could not start, or timed out.
    #[error("media analysis failed")]
    Failed(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The analyzer cannot run for any file: it is missing, or its tools are. Retrying the
    /// next file would fail the same way.
    #[error("the media analyzer cannot run: {0}")]
    Unavailable(String),
}

/// Reads one file's content: the separate scanner process, run by the server.
#[async_trait::async_trait]
pub trait MediaAnalyzer: Send + Sync + std::fmt::Debug {
    async fn analyze(&self, file: &FileLocation) -> Result<Analysis, AnalyzeError>;
}

/// A part storage already holds, as the skip rule compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPart {
    pub part: PartId,
    pub media: MediaId,
    pub item: RatingKey,
    /// As recorded, what [`LibraryFiles::stored_path`] gave.
    ///
    /// [`LibraryFiles::stored_path`]: crate::files::LibraryFiles::stored_path
    pub path: String,
    pub size: u64,
    /// Unix seconds; the reference keeps the file's in the part's `updated_at`.
    pub modified_at: i64,
}

/// One file of a version, as found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct NewPart {
    pub path: String,
    pub size: u64,
    pub modified_at: i64,
    /// Absent when the file could not be analyzed: the item is still listed, as the reference
    /// lists files its analysis failed on.
    pub analysis: Option<Analysis>,
    pub sidecars: Vec<NewSidecar>,
}

/// A subtitle file beside a media file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSidecar {
    /// As recorded, what [`LibraryFiles::stored_path`] gave.
    ///
    /// [`LibraryFiles::stored_path`]: crate::files::LibraryFiles::stored_path
    pub path: String,
    /// The extension, lower-cased: `srt`, `ass`, `vtt`…
    pub codec: String,
    /// As the file name gives it: `fr`, `fre`, `pt-BR`.
    pub language: Option<String>,
    pub forced: bool,
    pub hearing_impaired: bool,
}

/// A version of an item: one file, or several stacked (`cd1`, `cd2`).
#[derive(Debug, Clone, PartialEq)]
pub struct NewMedia {
    pub parts: Vec<NewPart>,
}

/// An item found in the library, with every version of it found together.
#[derive(Debug, Clone, PartialEq)]
pub enum NewItem {
    Movie {
        title: String,
        year: Option<i32>,
        versions: Vec<NewMedia>,
    },
    /// Its show and season are found by title and number, and made when absent.
    Episode {
        show: ShowKey,
        season: i32,
        episode: i32,
        /// The release date's midnight UTC, for an episode named by its date.
        released_on: Option<i64>,
        versions: Vec<NewMedia>,
    },
}

/// What tells two shows of a library apart before any metadata agent: the cleaned folder name
/// and its year.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShowKey {
    pub title: String,
    pub year: Option<i32>,
}

/// Storage port for what a scan writes.
#[async_trait::async_trait]
pub trait ScanRepository: Send + Sync + std::fmt::Debug {
    /// The section's parts whose recorded path starts with `path_prefix`.
    async fn known_parts(
        &self,
        section: SectionId,
        path_prefix: &str,
    ) -> Result<Vec<KnownPart>, ScanError>;

    /// Writes a new item with its versions, its show and season when they are new, and returns
    /// its key. `now` is the scan's time, which becomes the item's `addedAt`.
    async fn add_item(
        &self,
        section: SectionId,
        item: NewItem,
        now: i64,
    ) -> Result<RatingKey, ScanError>;

    /// Rewrites a part whose file changed — size, time, analysis and tracks — keeping its id
    /// and its item, as the reference reuses a path it already knows.
    async fn update_part(&self, part: PartId, file: NewPart, now: i64) -> Result<(), ScanError>;

    /// Deletes parts whose file is gone, then each version left without a part, each item left
    /// without a version, and each season and show left without children.
    async fn remove_parts(&self, parts: &[PartId]) -> Result<(), ScanError>;

    /// Records the end of a scan that changed the section, or not.
    async fn finish(&self, section: SectionId, at: i64, changed: bool) -> Result<(), ScanError>;
}

/// A library file found by the walk, named under its root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundFile {
    pub path: RelativePath,
    pub size: u64,
    pub modified_at: i64,
}
