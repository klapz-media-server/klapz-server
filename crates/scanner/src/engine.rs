//! The commands: each reads one file through ffprobe or ffmpeg and returns its report.

mod artwork;
mod bif;
mod fingerprint;
mod hashes;
mod loudness;
mod mp4;
mod probe;
mod subtitles;

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub use artwork::{artwork, chapter_thumbs};
pub use bif::preview_index;
pub use fingerprint::fingerprint;
pub use hashes::identify;
pub use loudness::loudness;
pub use probe::probe;
pub use subtitles::extract_subtitle;

use crate::report;

/// The external programs a run may start.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Tools {
    /// `OPENPLEX_TRANSCODER`, the ffmpeg the server streams with, and `OPENPLEX_FFPROBE`; each
    /// found on `PATH` when unset.
    pub fn from_env() -> Self {
        let tool = |variable: &str, default: &str| {
            std::env::var_os(variable).map_or_else(|| PathBuf::from(default), PathBuf::from)
        };
        Self {
            ffmpeg: tool("OPENPLEX_TRANSCODER", "ffmpeg"),
            ffprobe: tool("OPENPLEX_FFPROBE", "ffprobe"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("cannot read {path}: {source}")]
    File {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot start {tool}: {source}")]
    Spawn {
        tool: &'static str,
        source: std::io::Error,
    },
    /// The tool ran and refused the file: corrupt, truncated, or not media.
    #[error("{tool} could not read the media: {message}")]
    Unreadable { tool: &'static str, message: String },
    #[error("{tool} printed something unexpected: {message}")]
    Output { tool: &'static str, message: String },
    #[error("the file has no track {0} of the kind asked for")]
    NoSuchTrack(u32),
    #[error("{0}")]
    Unsupported(String),
    #[error("output directory {path}: {source}")]
    OutputDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl ScanError {
    /// Whether the media is at fault rather than the scanner or its environment, which the
    /// caller records against the file instead of retrying.
    pub fn is_media_fault(&self) -> bool {
        matches!(
            self,
            Self::Unreadable { .. } | Self::NoSuchTrack(_) | Self::Unsupported(_)
        )
    }
}

/// The input argument for ffmpeg and ffprobe. The `file:` protocol keeps a name such as
/// `concat:a|b` or `http:x` from being read as another protocol, and a name starting with `-`
/// from being read as an option.
fn input(path: &Path) -> OsString {
    let mut argument = OsString::from("file:");
    argument.push(path.as_os_str());
    argument
}

/// Runs a tool to completion, returning its standard output.
fn run(tool: &'static str, command: &mut Command) -> Result<Vec<u8>, ScanError> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|source| ScanError::Spawn { tool, source })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(ScanError::Unreadable {
            tool,
            message: last_lines(&output.stderr),
        })
    }
}

/// A child whose standard output is read as it comes. Dropped early, it is killed rather
/// than left running.
struct Streaming {
    tool: &'static str,
    child: Child,
}

impl Streaming {
    fn spawn(tool: &'static str, command: &mut Command) -> Result<Self, ScanError> {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| ScanError::Spawn { tool, source })?;
        Ok(Self { tool, child })
    }

    fn stdout(&mut self) -> Result<std::process::ChildStdout, ScanError> {
        self.child.stdout.take().ok_or(ScanError::Output {
            tool: self.tool,
            message: "standard output was not captured".to_owned(),
        })
    }

    /// Waits for the end, after standard output was read to its end.
    fn finish(mut self) -> Result<(), ScanError> {
        let mut stderr = Vec::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            // What the tool said is only for the error message; a failed read loses nothing.
            let _ = pipe.read_to_end(&mut stderr);
        }
        let status = self.child.wait().map_err(|source| ScanError::Spawn {
            tool: self.tool,
            source,
        })?;
        if status.success() {
            Ok(())
        } else {
            Err(ScanError::Unreadable {
                tool: self.tool,
                message: last_lines(&stderr),
            })
        }
    }
}

impl Drop for Streaming {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The end of a tool's error output, where it says why it stopped.
fn last_lines(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// A directory the caller made for this run's files. It must exist: the scanner creates no
/// directory of its own, so it cannot be steered into making one somewhere unexpected.
fn output_directory(path: &Path) -> Result<PathBuf, ScanError> {
    let fail = |source| ScanError::OutputDirectory {
        path: path.to_owned(),
        source,
    };
    let directory = path.canonicalize().map_err(fail)?;
    if directory.is_dir() {
        Ok(directory)
    } else {
        Err(fail(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a directory",
        )))
    }
}

/// The produced file `name` in `directory`, with its size, after a tool wrote it.
fn produced(
    directory: &Path,
    name: String,
    role: report::FileRole,
) -> Result<report::ProducedFile, ScanError> {
    let path = directory.join(&name);
    let metadata = std::fs::metadata(&path).map_err(|source| ScanError::File { path, source })?;
    Ok(report::ProducedFile {
        name,
        size: metadata.len(),
        role,
    })
}

/// Seconds as ffmpeg reads them, to the millisecond.
fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}
