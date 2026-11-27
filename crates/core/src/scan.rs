//! Scanning a library: finding its files, grouping them into items, and keeping storage in step.
//!
//! Plex's own split, measured in `docs/scanner/`: walking, deciding what changed, grouping and
//! storing happen in the server, here; reading a file's content happens in a separate process,
//! behind [`MediaAnalyzer`]. Files are reached only through [`LibraryFiles`], so a library kept
//! somewhere other than a local disk changes nothing in this module.
//!
//! A scan lists what it finds from names, sizes and times alone, and queues each new or changed
//! file; [`BackgroundAnalysis`] works through them afterwards, task by task ([`MediaTask`]):
//! every file's headers before anything else, so that a library is browsable as soon as it is
//! walked and playable soon after, then what reads a whole file — its packets, its subtitle
//! tracks, its seeking thumbnails — each where its setting is on.
//!
//! [`LibraryFiles`]: crate::files::LibraryFiles

mod analysis;
mod control;
pub mod layout;
mod walk;

use klapz_scanner::report::{Analysis, DeepAnalysis};

use crate::files::{FileLocation, FilesError, RelativePath};
use crate::ids::{MediaId, PartId, RatingKey, SectionId};
use crate::language::LanguageTag;
use crate::settings::{self, SettingDefinition, Switch};

pub use analysis::{AnalysisTools, BackgroundAnalysis};
pub use control::{BackgroundScans, LibraryAnalysis, ScanControl};
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
    /// A directory of the library could not be listed: a disk or a share failing mid-scan.
    #[error("cannot list {path}: {source}")]
    Files { path: String, source: FilesError },
    /// The analyzer cannot run at all. What is queued stays queued, to be read once the setup
    /// is fixed.
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
    /// The file cannot be opened: gone since its scan, or on a disk that is not mounted. It is
    /// read once a scan finds it again.
    #[error("the file cannot be opened: {0}")]
    Unreachable(String),
    /// The analyzer itself failed — it could not start, or timed out.
    #[error("media analysis failed")]
    Failed(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The analyzer cannot run for any file: it is missing, or its tools are. Retrying the
    /// next file would fail the same way.
    #[error("the media analyzer cannot run: {0}")]
    Unavailable(String),
}

/// Reads one file's content: ffprobe, run by the server as a child process, in the two steps
/// the reference also takes apart (`docs/scanner/analysis.md`). Dropping a read stops it, its
/// process included.
#[async_trait::async_trait]
pub trait MediaAnalyzer: Send + Sync + std::fmt::Debug {
    /// Tracks, codecs, chapters, and the durations and rates the file declares: its headers
    /// and the probe window, what the reference reads when a scan finds a file.
    async fn read_headers(&self, file: &FileLocation) -> Result<Analysis, AnalyzeError>;

    /// The rates and span measured over every packet, which reads the whole file: the rest of
    /// the reference's deep analysis. `duration_ms` is what the headers declared.
    async fn measure(
        &self,
        file: &FileLocation,
        duration_ms: Option<u64>,
    ) -> Result<DeepAnalysis, AnalyzeError>;
}

/// Makes files out of a file's content and keeps them by its content hash, so that every copy
/// of a file shares them and none is made twice. Run as children of the server, as reads are;
/// dropping a run stops it, its process included.
#[async_trait::async_trait]
pub trait MediaExtractor: Send + Sync + std::fmt::Debug {
    /// Every embedded subtitle track a file format holds by itself, each in a file of its own.
    async fn extract_subtitles(
        &self,
        file: &FileLocation,
        hash: &ContentHash,
    ) -> Result<Extraction, AnalyzeError>;

    /// The seeking thumbnails of the first video track, packed into a BIF.
    async fn generate_preview_thumbnails(
        &self,
        file: &FileLocation,
        hash: &ContentHash,
    ) -> Result<Extraction, AnalyzeError>;
}

/// What an extraction came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extraction {
    Written,
    /// Kept for this content already, from this file or a copy of it: nothing was read.
    AlreadyKept,
}

/// A file's content hash, as the scan stores it: 40 hex digits, in lower case. What the files
/// made out of a file are kept by, which makes it a path component: nothing else passes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentHash(String);

impl ContentHash {
    #[must_use]
    pub fn parse(hash: &str) -> Option<Self> {
        (hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| Self(hash.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One piece of work on one file. The queue takes them in this order, each in the order its
/// files were queued: every file's headers before anything that reads a whole file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MediaTask {
    /// Tracks, codecs, declared rates and durations, chapters, and the file's hashes: what
    /// makes it playable and described. Always done.
    Headers,
    /// The deep analysis: the rates and span measured over every packet.
    Packets,
    /// The embedded subtitle tracks, extracted to files of their own.
    Subtitles,
    /// The BIF a player seeks with.
    PreviewThumbnails,
}

impl MediaTask {
    /// The tasks a setting turns on and off.
    pub const OPTIONAL: [Self; 3] = [Self::Packets, Self::Subtitles, Self::PreviewThumbnails];

    /// Every task, in the queue's order.
    pub const ALL: [Self; 4] = [
        Self::Headers,
        Self::Packets,
        Self::Subtitles,
        Self::PreviewThumbnails,
    ];

    /// The server's setting that says how many of it run at once.
    #[must_use]
    pub const fn concurrency(self) -> &'static SettingDefinition {
        match self {
            Self::Headers => &settings::HEADERS_CONCURRENCY,
            Self::Packets => &settings::DEEP_ANALYSIS_CONCURRENCY,
            Self::Subtitles => &settings::SUBTITLE_EXTRACTION_CONCURRENCY,
            Self::PreviewThumbnails => &settings::PREVIEW_THUMBNAILS_CONCURRENCY,
        }
    }

    /// The per-library setting that turns it on and off; `None` for what is always done.
    #[must_use]
    pub const fn setting(self) -> Option<&'static SettingDefinition> {
        match self {
            Self::Headers => None,
            Self::Packets => Some(&settings::DEEP_ANALYSIS),
            Self::Subtitles => Some(&settings::SUBTITLE_EXTRACTION),
            Self::PreviewThumbnails => Some(&settings::PREVIEW_THUMBNAILS),
        }
    }
}

/// Where an optional task is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSwitch {
    pub task: MediaTask,
    pub switch: Switch,
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

/// One file of a version, as found on disk: stored as it is, and read afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPart {
    pub path: String,
    pub size: u64,
    pub modified_at: i64,
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
    /// What the file name gives: `fr`, `fre` and `pt-BR` read as `fr`, `fr` and `pt-BR`.
    pub language: Option<LanguageTag>,
    pub forced: bool,
    pub hearing_impaired: bool,
}

/// A version of an item: one file, or several stacked (`cd1`, `cd2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMedia {
    pub parts: Vec<NewPart>,
}

/// An item found in the library, with every version of it found together.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// A task left on a file, as the analysis takes it from storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedFile {
    pub part: PartId,
    pub section: SectionId,
    /// As recorded, what [`LibraryFiles::stored_path`] gave.
    ///
    /// [`LibraryFiles::stored_path`]: crate::files::LibraryFiles::stored_path
    pub path: String,
    /// The file as the scan last saw it. Storage refuses a result read from it once it
    /// changed: the scan has queued it again.
    pub size: u64,
    pub modified_at: i64,
    pub task: MediaTask,
    /// The duration its headers declared, once read.
    pub duration_ms: Option<u64>,
    /// Once its headers are read, and when the tools state it.
    pub content_hash: Option<ContentHash>,
}

/// Which queued tasks the analysis may take.
#[derive(Debug, Clone, Default)]
pub struct QueueFilter {
    /// Only this library's.
    pub section: Option<SectionId>,
    /// Only this task.
    pub task: Option<MediaTask>,
    /// Files being worked on already.
    pub busy: Vec<PartId>,
    /// Libraries whose files are not read for now: being walked by a scan, or paused.
    pub held: Vec<SectionId>,
    /// Where each optional task is on; one that is off stays queued, and is taken once it is
    /// turned on.
    pub switches: Vec<TaskSwitch>,
}

/// What became of a result handed to storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    Written,
    /// The file is gone, changed since it was queued, or was read that far already: nothing
    /// was written.
    Stale,
}

/// A library's files, and how far they are read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnalysisProgress {
    pub files: u64,
    /// Whose headers are read.
    pub read: u64,
    /// Whose headers are read and that have none of the tasks counted left.
    pub complete: u64,
}

/// Storage port for what a scan writes. Each write is whole or absent, even dropped halfway:
/// a cancelled scan is dropped wherever it waits.
#[async_trait::async_trait]
pub trait ScanRepository: Send + Sync + std::fmt::Debug {
    /// The section's parts whose recorded path starts with `path_prefix`.
    async fn known_parts(
        &self,
        section: SectionId,
        path_prefix: &str,
    ) -> Result<Vec<KnownPart>, ScanError>;

    /// Writes a new item with its versions, its show and season when they are new, queues its
    /// files to be read, and returns its key. `now` is the scan's time, which becomes the
    /// item's `addedAt`.
    async fn add_item(
        &self,
        section: SectionId,
        item: NewItem,
        now: i64,
    ) -> Result<RatingKey, ScanError>;

    /// Records that a part's file changed — size, time and sidecars — keeping its id and its
    /// item, as the reference reuses a path it already knows, and queues it to be read again.
    /// Its tracks stay as they were until then.
    async fn update_part(&self, part: PartId, file: NewPart, now: i64) -> Result<(), ScanError>;

    /// Queues these files to be read again from their headers, as a forced scan asks, in
    /// place of whatever task they had left.
    async fn queue_again(&self, parts: &[PartId], now: i64) -> Result<(), ScanError>;

    /// Up to `limit` queued tasks, of distinct files, each a file's first task the filter
    /// lets through: by [`MediaTask`]'s order, then each in the order it was queued.
    async fn queued_files(
        &self,
        limit: usize,
        filter: &QueueFilter,
    ) -> Result<Vec<QueuedFile>, ScanError>;

    /// The part's first task left, when it has one.
    async fn queued_file(&self, part: PartId) -> Result<Option<QueuedFile>, ScanError>;

    /// Writes what a file's headers say — tracks, codecs, durations, hashes, chapters — in
    /// place of the tracks it had inside, its sidecars kept, and queues what follows: its
    /// packets, its subtitle tracks when it has any, its thumbnails when it has a video track.
    /// `None` is a file that could not be read as media: it stays listed, its tracks inside
    /// and their description gone, and is not queued again until it changes.
    async fn store_headers(
        &self,
        file: &QueuedFile,
        analysis: Option<&Analysis>,
        now: i64,
    ) -> Result<Stored, ScanError>;

    /// Writes the rates and duration measured over a file's packets onto the tracks its
    /// headers gave, which keep their ids, and takes the task off the queue. `None` keeps what
    /// the headers declared: the measurement could not finish.
    async fn store_measurement(
        &self,
        file: &QueuedFile,
        measured: Option<&DeepAnalysis>,
        now: i64,
    ) -> Result<Stored, ScanError>;

    /// Takes a task whose result is kept outside storage off the queue: an extraction, done or
    /// failed.
    async fn finish_task(&self, file: &QueuedFile) -> Result<Stored, ScanError>;

    /// How far the library's files are read, `counted` the optional tasks that count for
    /// [`AnalysisProgress::complete`].
    async fn analysis_progress(
        &self,
        section: SectionId,
        counted: &[MediaTask],
    ) -> Result<AnalysisProgress, ScanError>;

    /// Counts a change of the library's contents, as a scan that changed it does: what the
    /// analysis wrote changes what its listings show.
    async fn contents_changed(&self, section: SectionId) -> Result<(), ScanError>;

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
