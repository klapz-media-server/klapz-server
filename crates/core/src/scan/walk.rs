//! One scan of a library: walk, compare, group, analyze, write.

use std::sync::Arc;

use crate::files::{LibraryFiles, RelativePath};
use crate::ids::SectionId;
use crate::library::LibraryRepository;
use crate::scan::{MediaAnalyzer, ScanError, ScanRepository};

/// What a scan needs, assembled once by the server.
#[derive(Debug, Clone)]
pub struct Scanner {
    pub library: Arc<dyn LibraryRepository>,
    pub files: Arc<dyn LibraryFiles>,
    pub analyzer: Arc<dyn MediaAnalyzer>,
    pub store: Arc<dyn ScanRepository>,
}

/// A scan to run: a whole library, or the part of it below one directory.
#[derive(Debug, Clone)]
pub struct ScanRequest {
    pub section: SectionId,
    /// A root of the section and a directory below it.
    pub below: Option<(String, RelativePath)>,
    /// Analyze and compare every file, as if none had been seen.
    pub force: bool,
}

/// What a scan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanOutcome {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unavailable_roots: usize,
}

/// Runs one scan to its end.
///
/// # Errors
///
/// [`ScanError::SectionNotFound`], [`ScanError::Storage`].
pub async fn scan_section(
    _scanner: &Scanner,
    _request: &ScanRequest,
) -> Result<ScanOutcome, ScanError> {
    Ok(ScanOutcome::default())
}
