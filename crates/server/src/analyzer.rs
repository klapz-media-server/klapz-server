//! Media analysis: ffprobe on one file, as a child of the server. How many run at once is the
//! analysis queue's to bound (`core::scan::BackgroundAnalysis`), not this reader's.

use std::time::Duration;

use klapz_core::files::FileLocation;
use klapz_core::scan::{AnalyzeError, MediaAnalyzer};
use klapz_scanner::report::{Analysis, DeepAnalysis};
use klapz_scanner::{ScanError, Tools};

/// Past this, reading the headers is dropped and its process killed: the reference reads them
/// in milliseconds, so a file that keeps a probe busy this long is hostile, broken, or on a
/// share that stopped answering. A player may be waiting on it.
const HEADERS_TIMEOUT: Duration = Duration::from_secs(60);

/// Past this, measuring the packets is dropped and its process killed: it reads the whole file,
/// which a large one on a slow share takes minutes to.
const MEASURE_TIMEOUT: Duration = Duration::from_secs(600);

/// How long `ffprobe -version` may take at startup.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct MediaReader {
    tools: Tools,
}

impl MediaReader {
    /// The ffmpeg the server streams with, and `KLAPZ_FFPROBE`.
    pub fn from_env() -> Self {
        Self {
            tools: Tools::from_env(),
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
pub(crate) struct TimedOut(pub(crate) Duration);

#[async_trait::async_trait]
impl MediaAnalyzer for MediaReader {
    async fn read_headers(&self, file: &FileLocation) -> Result<Analysis, AnalyzeError> {
        let FileLocation::Local(path) = file;
        tokio::time::timeout(HEADERS_TIMEOUT, klapz_scanner::probe(&self.tools, path))
            .await
            .map_err(|_| AnalyzeError::Failed(Box::new(TimedOut(HEADERS_TIMEOUT))))?
            .map_err(analyze_error)
    }

    async fn measure(
        &self,
        file: &FileLocation,
        duration_ms: Option<u64>,
    ) -> Result<DeepAnalysis, AnalyzeError> {
        let FileLocation::Local(path) = file;
        tokio::time::timeout(
            MEASURE_TIMEOUT,
            klapz_scanner::measure(&self.tools, path, duration_ms),
        )
        .await
        .map_err(|_| AnalyzeError::Failed(Box::new(TimedOut(MEASURE_TIMEOUT))))?
        .map_err(analyze_error)
    }
}

/// Whose fault a failed read is: the file's, recorded against it; the setup's, which holds
/// every read; the file's absence, which holds its library's until its next scan; or neither,
/// which fails this file alone.
pub(crate) fn analyze_error(error: ScanError) -> AnalyzeError {
    if error.is_media_fault() {
        AnalyzeError::Unreadable(error.to_string())
    } else if error.is_tool_fault() {
        AnalyzeError::Unavailable(error.to_string())
    } else if let ScanError::File { .. } = error {
        AnalyzeError::Unreachable(error.to_string())
    } else {
        AnalyzeError::Failed(Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn a_missing_ffmpeg_holds_the_analysis_rather_than_blaming_the_file() {
        let reader = MediaReader {
            tools: Tools {
                ffmpeg: "klapz-test-no-such-ffmpeg".into(),
                ffprobe: None,
            },
        };
        assert!(reader.check().await.is_err());
        let file = std::env::temp_dir().join(format!("klapz-analyzer-{}", std::process::id()));
        std::fs::write(&file, b"not media").unwrap();
        let read = reader
            .read_headers(&FileLocation::Local(file.clone()))
            .await;
        std::fs::remove_file(&file).unwrap();
        assert!(
            matches!(read, Err(AnalyzeError::Unavailable(_))),
            "{read:?}"
        );
    }

    #[tokio::test]
    async fn a_file_that_is_gone_is_unreachable_rather_than_not_media() {
        let reader = MediaReader {
            tools: Tools {
                ffmpeg: "klapz-test-no-such-ffmpeg".into(),
                ffprobe: None,
            },
        };
        let file = std::env::temp_dir().join(format!("klapz-analyzer-gone-{}", std::process::id()));
        let read = reader.read_headers(&FileLocation::Local(file)).await;
        assert!(
            matches!(read, Err(AnalyzeError::Unreachable(_))),
            "{read:?}"
        );
    }
}
