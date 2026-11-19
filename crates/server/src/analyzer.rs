//! Media analysis by the scanner binary: one process per file, never more than a few at once.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use open_plex_core::files::FileLocation;
use open_plex_core::scan::{AnalyzeError, MediaAnalyzer};
use open_plex_scanner::report::{Analysis, Output, Report, VERSION};
use tokio::process::Command;
use tokio::sync::Semaphore;

/// Files analyzed at once. Each is a process reading a file, often over a network share.
const CONCURRENT: usize = 2;

/// Past this, the process is killed: a file that keeps a probe busy this long is hostile or
/// broken.
const TIMEOUT: Duration = Duration::from_secs(600);

/// The scanner's exit status when the media is at fault.
const EXIT_MEDIA: i32 = 3;

#[derive(Debug)]
pub struct ScannerProcess {
    binary: PathBuf,
    permits: Semaphore,
}

impl ScannerProcess {
    /// `OPENPLEX_SCANNER`, or `open-plex-scanner` beside this server's own executable, where
    /// Cargo puts both.
    pub fn from_env() -> Self {
        let binary = std::env::var_os("OPENPLEX_SCANNER").map_or_else(
            || {
                let name = format!("open-plex-scanner{}", std::env::consts::EXE_SUFFIX);
                std::env::current_exe()
                    .ok()
                    .and_then(|server| server.parent().map(|directory| directory.join(&name)))
                    .unwrap_or_else(|| PathBuf::from(name))
            },
            PathBuf::from,
        );
        Self {
            binary,
            permits: Semaphore::new(CONCURRENT),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ScannerFailure {
    #[error("cannot start the scanner {0}: {1}")]
    Spawn(String, #[source] std::io::Error),
    #[error("the scanner ran past {0:?}")]
    Timeout(Duration),
    #[error("the scanner failed: {0}")]
    Status(String),
    #[error("the scanner's report cannot be read: {0}")]
    Report(String),
}

fn failed(failure: ScannerFailure) -> AnalyzeError {
    AnalyzeError::Failed(Box::new(failure))
}

#[async_trait::async_trait]
impl MediaAnalyzer for ScannerProcess {
    /// The deep analysis, as the reference's tracks end up stored: rates measured over every
    /// packet. It reads the whole file, so when it cannot finish, the headers alone still make
    /// the item.
    async fn analyze(&self, file: &FileLocation) -> Result<Analysis, AnalyzeError> {
        let FileLocation::Local(path) = file;
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| failed(ScannerFailure::Status("the analyzer is closed".to_owned())))?;
        match self.probe(&["probe", "--deep", "--"], path).await {
            Ok(analysis) => Ok(analysis),
            Err(error) => {
                tracing::warn!(%error, "deep analysis failed, reading the headers alone");
                self.probe(&["probe", "--"], path).await
            }
        }
    }
}

impl ScannerProcess {
    async fn probe(&self, arguments: &[&str], path: &Path) -> Result<Analysis, AnalyzeError> {
        let child = Command::new(&self.binary)
            .args(arguments)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                failed(ScannerFailure::Spawn(
                    self.binary.display().to_string(),
                    error,
                ))
            })?;
        let output = tokio::time::timeout(TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| failed(ScannerFailure::Timeout(TIMEOUT)))?
            .map_err(|error| failed(ScannerFailure::Status(error.to_string())))?;
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        match output.status.code() {
            Some(0) => {}
            Some(EXIT_MEDIA) => return Err(AnalyzeError::Unreadable(stderr)),
            _ => return Err(failed(ScannerFailure::Status(stderr))),
        }
        let report: Report = serde_json::from_slice(&output.stdout)
            .map_err(|error| failed(ScannerFailure::Report(error.to_string())))?;
        match report {
            Report {
                version: VERSION,
                output: Output::Probe(analysis),
            } => Ok(analysis),
            Report { version, .. } => Err(failed(ScannerFailure::Report(format!(
                "version {version} of another command, where version {VERSION} of probe was expected"
            )))),
        }
    }
}
