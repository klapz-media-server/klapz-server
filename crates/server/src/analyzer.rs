//! Media analysis: ffprobe on one file, as a child of the server, never more than a few at once.

use std::time::Duration;

use klapz_core::files::FileLocation;
use klapz_core::scan::{AnalyzeError, MediaAnalyzer};
use klapz_scanner::report::Analysis;
use klapz_scanner::{ScanError, Tools};
use tokio::sync::Semaphore;

/// Files analyzed at once. Each is a process reading a file, often over a network share.
const CONCURRENT: usize = 2;

/// Past this, the analysis is dropped and its process killed: a file that keeps a probe busy
/// this long is hostile or broken.
const TIMEOUT: Duration = Duration::from_secs(600);

/// How long `ffprobe -version` may take at startup.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct MediaReader {
    tools: Tools,
    permits: Semaphore,
}

impl MediaReader {
    /// The ffmpeg the server streams with, and `KLAPZ_FFPROBE`.
    pub fn from_env() -> Self {
        Self {
            tools: Tools::from_env(),
            permits: Semaphore::new(CONCURRENT),
        }
    }

    /// Whether a scan could analyse anything: ffprobe starts. What is wrong otherwise.
    pub async fn check(&self) -> Result<(), String> {
        match tokio::time::timeout(CHECK_TIMEOUT, self.tools.check()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err("ffprobe -version did not end".to_owned()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the analysis ran past {0:?}")]
struct TimedOut(Duration);

#[async_trait::async_trait]
impl MediaAnalyzer for MediaReader {
    /// The deep analysis, as the reference's tracks end up stored: rates measured over every
    /// packet. It reads the whole file, so when it cannot finish, the headers alone still make
    /// the item.
    async fn analyze(&self, file: &FileLocation) -> Result<Analysis, AnalyzeError> {
        let FileLocation::Local(path) = file;
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| AnalyzeError::Unavailable("the analyzer is closed".to_owned()))?;
        match self.probe(path, true).await {
            Ok(analysis) => Ok(analysis),
            // The headers alone would fail the same way.
            Err(error @ AnalyzeError::Unavailable(_)) => Err(error),
            Err(error) => {
                tracing::warn!(%error, "deep analysis failed, reading the headers alone");
                self.probe(path, false).await
            }
        }
    }
}

impl MediaReader {
    async fn probe(&self, path: &std::path::Path, deep: bool) -> Result<Analysis, AnalyzeError> {
        tokio::time::timeout(TIMEOUT, klapz_scanner::probe(&self.tools, path, deep))
            .await
            .map_err(|_| AnalyzeError::Failed(Box::new(TimedOut(TIMEOUT))))?
            .map_err(analyze_error)
    }
}

/// Whose fault a failed read is: the file's, recorded against it; the setup's, which stops the
/// scan; or neither, which fails this file alone.
fn analyze_error(error: ScanError) -> AnalyzeError {
    if error.is_media_fault() {
        AnalyzeError::Unreadable(error.to_string())
    } else if error.is_tool_fault() {
        AnalyzeError::Unavailable(error.to_string())
    } else {
        AnalyzeError::Failed(Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn a_missing_ffmpeg_stops_scans_rather_than_blaming_the_file() {
        let reader = MediaReader {
            tools: Tools {
                ffmpeg: "klapz-test-no-such-ffmpeg".into(),
                ffprobe: None,
            },
            permits: Semaphore::new(1),
        };
        assert!(reader.check().await.is_err());
        let file = std::env::temp_dir().join(format!("klapz-analyzer-{}", std::process::id()));
        std::fs::write(&file, b"not media").unwrap();
        let analyzed = reader.analyze(&FileLocation::Local(file.clone())).await;
        std::fs::remove_file(&file).unwrap();
        assert!(
            matches!(analyzed, Err(AnalyzeError::Unavailable(_))),
            "{analyzed:?}"
        );
    }
}
