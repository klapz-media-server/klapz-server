//! Scanning a library: starting a scan, and stopping it.

use async_graphql::{Context, Enum, InputObject, Result, SimpleObject, ID};
use klapz_core::scan::{ScanError, ScanRequest};
use klapz_core::SectionId;

use crate::library::{Library, LIBRARY_TYPE};
use crate::node::GlobalId;
use crate::schema::{self, ApiContext};

/// A library to scan.
#[derive(Debug, InputObject)]
pub struct ScanLibraryInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The library, as returned in `Library.id`.
    pub library_id: ID,
    /// Analyze every file again, as if none had been seen. By default a directory whose files
    /// kept their names, sizes and modification times is skipped.
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
    /// Whether a scan was running and was asked to stop. It stops at once, matching included,
    /// even halfway through reading a file, which the next scan finds new.
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
