use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::report::FileIdentity;
use crate::ScanError;

/// Size and modification time: what tells whether a file is one already known, without
/// reading it. Its hashes are ffprobe's, which [`crate::analysis_from_ffprobe`] adds.
///
/// # Errors
///
/// [`ScanError::File`] when the file's metadata cannot be read.
pub fn identify(path: &Path) -> Result<FileIdentity, ScanError> {
    let metadata = std::fs::metadata(path).map_err(|source| ScanError::File {
        path: path.to_owned(),
        source,
    })?;
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0);
    Ok(FileIdentity {
        size: metadata.len(),
        modified_at,
        content_hash: None,
        open_subtitles_hash: None,
    })
}
