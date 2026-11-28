//! Scanning a library: starting a scan, and stopping it; and how far its files are read.

use async_graphql::{Context, Enum, InputObject, Result, SimpleObject, ID};
use klapz_core::metadata::FileAnalysis;
use klapz_core::scan::{LibraryAnalysis, ScanError, ScanRequest};
use klapz_core::{PartId, SectionId};

use crate::library::{Library, LIBRARY_TYPE};
use crate::media::{File, FILE_TYPE};
use crate::node::GlobalId;
use crate::schema::{self, ApiContext};

/// How much of a file's media the server has read. A scan lists a file as soon as it finds
/// it, and reads it afterwards, in the background: every file's headers first, then every
/// file's packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "FileAnalysis")]
pub enum FileAnalysisType {
    /// Nothing yet: no track, codec or duration is known. Playing it reads its headers first.
    Pending,
    /// Its headers: tracks, codecs, and the durations and rates they declare. The rates
    /// measured over its packets are still to come, or not measured at all where the
    /// `analysis.deep` setting is off.
    HeadersRead,
    /// All there is to read, or all that could be read of it.
    Complete,
}

impl From<FileAnalysis> for FileAnalysisType {
    fn from(analysis: FileAnalysis) -> Self {
        match analysis {
            FileAnalysis::Pending => Self::Pending,
            FileAnalysis::HeadersRead => Self::HeadersRead,
            FileAnalysis::Complete => Self::Complete,
        }
    }
}

/// How far a library's files are read.
#[derive(Debug, SimpleObject)]
#[graphql(name = "LibraryAnalysis")]
pub struct LibraryAnalysisObject {
    /// The library's files.
    pub file_count: i32,
    /// Those whose headers are read, measured or not: they show their tracks and durations,
    /// and play without waiting.
    pub read_file_count: i32,
    /// Those whose headers are read and that have nothing left of what the library's settings
    /// turn on — packets measured, subtitles extracted, thumbnails made — or as far as it
    /// could go.
    pub complete_file_count: i32,
    /// Whether some of its files are being read now. False with files left to read when the
    /// analysis waits: for the library's scan, for its next one after `cancelLibraryScan`, or
    /// for what `failure` tells.
    pub reading: bool,
    /// Why its files are not being read: the media tools cannot run, or a file could not be
    /// opened, which holds the library until its next scan. Null when nothing is wrong, and
    /// for a caller who does not administer the library.
    pub failure: Option<String>,
}

impl LibraryAnalysisObject {
    pub(crate) fn new(analysis: LibraryAnalysis, administered: bool) -> Self {
        let count = |files: u64| i32::try_from(files).unwrap_or(i32::MAX);
        Self {
            file_count: count(analysis.progress.files),
            read_file_count: count(analysis.progress.read),
            complete_file_count: count(analysis.progress.complete),
            reading: analysis.reading,
            failure: analysis.failure.filter(|_| administered),
        }
    }
}

/// A library to scan.
#[derive(Debug, InputObject)]
pub struct ScanLibraryInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The library, as returned in `Library.id`.
    pub library_id: ID,
    /// Walk every directory and read every file again, as if none had been seen. By default a
    /// directory whose files kept their names, sizes and modification times is skipped.
    #[graphql(default)]
    pub force: bool,
}

/// What `scanLibrary` did.
#[derive(Debug, SimpleObject)]
pub struct ScanLibraryPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The library, now being scanned. Null when the request was refused.
    pub library: Option<Library>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<ScanUserError>,
}

/// A library whose scan to stop.
#[derive(Debug, InputObject)]
pub struct CancelLibraryScanInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The library, as returned in `Library.id`.
    pub library_id: ID,
}

/// What `cancelLibraryScan` did.
#[derive(Debug, SimpleObject)]
pub struct CancelLibraryScanPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// Whether a scan, or the reading of the library's files, was under way and was asked to
    /// stop. Both stop at once, a scan's matching included; the files left to read wait for
    /// the library's next scan.
    pub cancelled: bool,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<ScanUserError>,
}

/// Why a scan request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ScanErrorCode {
    /// No library the caller administers carries this id.
    LibraryNotFound,
    /// A scan of this library is already running.
    AlreadyRunning,
    /// Music and photo libraries are not scanned by this server.
    UnsupportedKind,
}

/// A request a scan mutation refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct ScanUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: ScanErrorCode,
}

impl ScanUserError {
    fn new(code: ScanErrorCode) -> Self {
        let (field, message) = match code {
            ScanErrorCode::LibraryNotFound => ("libraryId", "no such library"),
            ScanErrorCode::AlreadyRunning => ("libraryId", "this library is already being scanned"),
            ScanErrorCode::UnsupportedKind => ("libraryId", "this kind of library is not scanned"),
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }
}

/// A file whose media to read now.
#[derive(Debug, InputObject)]
pub struct ReadMediaFileInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The file, as returned in `MediaFile.id`.
    pub file_id: ID,
}

/// What `readMediaFile` did.
#[derive(Debug, SimpleObject)]
pub struct ReadMediaFilePayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The file, as it reads now. Null when the request was refused.
    pub file: Option<File>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<ReadMediaFileUserError>,
}

/// Why a file could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ReadMediaFileErrorCode {
    /// No file the caller can see carries this id.
    FileNotFound,
    /// The file could not be read now: it cannot be opened, the media tools cannot run, or it
    /// took too long. The analysis reads it later.
    NotAnalyzed,
}

/// A request `readMediaFile` refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct ReadMediaFileUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: ReadMediaFileErrorCode,
}

impl ReadMediaFileUserError {
    fn new(code: ReadMediaFileErrorCode) -> Self {
        let message = match code {
            ReadMediaFileErrorCode::FileNotFound => "no such file",
            ReadMediaFileErrorCode::NotAnalyzed => {
                "this file cannot be read yet; it will be once the server reaches it"
            }
        };
        Self {
            field: vec!["input".to_owned(), "fileId".to_owned()],
            message: message.to_owned(),
            code,
        }
    }
}

/// Reads a file's headers now, when that is what it waits for: what playing it needs. Whether
/// nothing is left to wait for.
pub(crate) async fn read_before_playing(context: &Context<'_>, file: PartId) -> Result<bool> {
    Ok(context.data::<ApiContext>()?.scans.read_now(file).await?)
}

pub(crate) async fn read_media_file(
    context: &Context<'_>,
    input: ReadMediaFileInput,
) -> Result<ReadMediaFilePayload> {
    let refused = |code| {
        Ok(ReadMediaFilePayload {
            client_mutation_id: input.client_mutation_id.clone(),
            file: None,
            user_errors: vec![ReadMediaFileUserError::new(code)],
        })
    };
    let readable = match GlobalId::decode(&input.file_id)
        .ok()
        .filter(|decoded| decoded.type_name == FILE_TYPE)
    {
        Some(decoded) => schema::readable_file(context, PartId::new(decoded.key)).await?,
        None => None,
    };
    let Some(file) = readable else {
        return refused(ReadMediaFileErrorCode::FileNotFound);
    };
    if !read_before_playing(context, file.id).await? {
        return refused(ReadMediaFileErrorCode::NotAnalyzed);
    }
    let api = context.data::<ApiContext>()?;
    let Some(file) = api.metadata.file(file.id).await? else {
        return refused(ReadMediaFileErrorCode::FileNotFound);
    };
    Ok(ReadMediaFilePayload {
        client_mutation_id: input.client_mutation_id,
        file: Some(File(file)),
        user_errors: Vec::new(),
    })
}

/// The library `id` names, when the caller administers it: scanning writes to it, as an edit
/// does. Anything else answers as if it did not exist.
async fn administered(context: &Context<'_>, id: &ID) -> Result<Option<Library>> {
    let Some(section) = GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == LIBRARY_TYPE)
        .map(|decoded| SectionId::new(decoded.key))
    else {
        return Ok(None);
    };
    if !crate::access::can_edit(context, section) {
        return Ok(None);
    }
    schema::load_library(context, section).await
}

pub(crate) async fn scan_library(
    context: &Context<'_>,
    input: ScanLibraryInput,
) -> Result<ScanLibraryPayload> {
    let refused = |code| {
        Ok(ScanLibraryPayload {
            client_mutation_id: input.client_mutation_id.clone(),
            library: None,
            user_errors: vec![ScanUserError::new(code)],
        })
    };
    let Some(library) = administered(context, &input.library_id).await? else {
        return refused(ScanErrorCode::LibraryNotFound);
    };
    if !library.is_scannable() {
        return refused(ScanErrorCode::UnsupportedKind);
    }
    let request = ScanRequest {
        section: library.section_id(),
        below: None,
        force: input.force,
    };
    match context.data::<ApiContext>()?.scans.start(request).await {
        Ok(()) => Ok(ScanLibraryPayload {
            client_mutation_id: input.client_mutation_id,
            library: Some(library),
            user_errors: Vec::new(),
        }),
        Err(ScanError::AlreadyRunning) => refused(ScanErrorCode::AlreadyRunning),
        Err(ScanError::UnsupportedKind) => refused(ScanErrorCode::UnsupportedKind),
        Err(ScanError::SectionNotFound(_)) => refused(ScanErrorCode::LibraryNotFound),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn cancel_library_scan(
    context: &Context<'_>,
    input: CancelLibraryScanInput,
) -> Result<CancelLibraryScanPayload> {
    let Some(library) = administered(context, &input.library_id).await? else {
        return Ok(CancelLibraryScanPayload {
            client_mutation_id: input.client_mutation_id,
            cancelled: false,
            user_errors: vec![ScanUserError::new(ScanErrorCode::LibraryNotFound)],
        });
    };
    let cancelled = context
        .data::<ApiContext>()?
        .scans
        .cancel(library.section_id());
    Ok(CancelLibraryScanPayload {
        client_mutation_id: input.client_mutation_id,
        cancelled,
        user_errors: Vec::new(),
    })
}
