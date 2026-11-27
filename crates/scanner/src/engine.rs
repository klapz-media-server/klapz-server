//! Reading one file through ffprobe or ffmpeg, each run as a child of the server.
//!
//! Every child is killed when the future running it is dropped, so a caller bounds a read by
//! dropping it: a timeout, a cancelled scan.

mod artwork;
mod bif;
mod fingerprint;
mod identity;
mod loudness;
mod probe;
mod subtitles;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout, Command};

pub use artwork::{artwork, chapter_thumbs};
pub use bif::preview_index;
pub use fingerprint::fingerprint;
pub use identity::identify;
pub use loudness::loudness;
pub use probe::{analysis_from_ffprobe, measure, probe};
pub use subtitles::{extract_subtitle, extract_subtitles};

use crate::report;

/// The external programs a run may start.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    /// A separate ffprobe. Without one, ffmpeg-plex's own runs as `ffmpeg -probe`.
    pub ffprobe: Option<PathBuf>,
}

impl Tools {
    /// The ffmpeg the server streams with ([`crate::transcoder::binary`]), and `KLAPZ_FFPROBE`.
    pub fn from_env() -> Self {
        Self {
            ffmpeg: crate::transcoder::binary(),
            ffprobe: std::env::var_os("KLAPZ_FFPROBE").map(PathBuf::from),
        }
    }

    fn ffprobe(&self) -> Command {
        if let Some(ffprobe) = &self.ffprobe {
            return Command::new(ffprobe);
        }
        let mut command = Command::new(&self.ffmpeg);
        command.arg("-probe");
        command
    }

    /// Whether ffprobe runs at all, without a file: what a server checks when it starts, so
    /// that a missing tool is told once rather than file after file.
    ///
    /// # Errors
    ///
    /// [`ScanError::Spawn`] when it cannot start, [`ScanError::ProbeUnsupported`] when the
    /// ffmpeg has no ffprobe of its own.
    pub async fn check(&self) -> Result<(), ScanError> {
        run("ffprobe", self.ffprobe().arg("-version"))
            .await
            .map(drop)
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
    /// The ffmpeg run as ffprobe refuses `-probe`: a stock build, or an ffmpeg-plex older than
    /// v8.1.3-15, which carry no ffprobe of their own.
    #[error(
        "{program} has no ffprobe of its own (it refuses -probe): use ffmpeg-plex v8.1.3-15 or \
         later as KLAPZ_TRANSCODER, or set KLAPZ_FFPROBE to an ffprobe"
    )]
    ProbeUnsupported { program: String },
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
    /// Whether the media is at fault rather than the tools or their environment, which the
    /// caller records against the file instead of retrying.
    pub fn is_media_fault(&self) -> bool {
        matches!(
            self,
            Self::Unreadable { .. } | Self::NoSuchTrack(_) | Self::Unsupported(_)
        )
    }

    /// Whether the tools cannot run at all, so that every file would fail the same way: the
    /// caller fixes its setup rather than blame or retry the file.
    pub fn is_tool_fault(&self) -> bool {
        matches!(self, Self::Spawn { .. } | Self::ProbeUnsupported { .. })
    }
}

/// What a tool's failure says: about the tool when ffmpeg refused to be ffprobe, about the
/// media otherwise. ffmpeg names the option it does not know and stops before reading anything.
fn failure(tool: &'static str, program: &OsStr, stderr: &[u8]) -> ScanError {
    let message = last_lines(stderr);
    if message.contains("Unrecognized option 'probe'") {
        ScanError::ProbeUnsupported {
            program: program.to_string_lossy().into_owned(),
        }
    } else {
        ScanError::Unreadable { tool, message }
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
async fn run(tool: &'static str, command: &mut Command) -> Result<Vec<u8>, ScanError> {
    output(tool, command).await.map(|output| output.stdout)
}

/// Runs a tool to completion, returning what it wrote on both outputs: ffmpeg's filters and
/// progress report on standard error.
async fn output(
    tool: &'static str,
    command: &mut Command,
) -> Result<std::process::Output, ScanError> {
    let output = command
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|source| ScanError::Spawn { tool, source })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(failure(
            tool,
            command.as_std().get_program(),
            &output.stderr,
        ))
    }
}

/// A child whose standard output is read as it comes, for an output too large to hold.
struct Streaming {
    tool: &'static str,
    program: OsString,
    child: Child,
}

impl Streaming {
    fn spawn(tool: &'static str, command: &mut Command) -> Result<Self, ScanError> {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| ScanError::Spawn { tool, source })?;
        Ok(Self {
            tool,
            program: command.as_std().get_program().to_owned(),
            child,
        })
    }

    fn stdout(&mut self) -> Result<ChildStdout, ScanError> {
        self.child.stdout.take().ok_or(ScanError::Output {
            tool: self.tool,
            message: "standard output was not captured".to_owned(),
        })
    }

    /// Waits for the end, after standard output was read to its end.
    async fn finish(mut self) -> Result<(), ScanError> {
        let mut stderr = Vec::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            // What the tool said is only for the error message; a failed read loses nothing.
            let _ = pipe.read_to_end(&mut stderr).await;
        }
        let status = self.child.wait().await.map_err(|source| ScanError::Spawn {
            tool: self.tool,
            source,
        })?;
        if status.success() {
            Ok(())
        } else {
            Err(failure(self.tool, &self.program, &stderr))
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

/// A directory the caller made for this run's files. It must exist: nothing here creates a
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ffmpeg_refusing_probe_is_the_tools_fault_not_the_media_s() {
        let stderr = b"ffmpeg version 8.0\nUnrecognized option 'probe'.\nError splitting the argument list: Option not found\n";
        let error = failure("ffprobe", OsStr::new("C:/bin/ffmpeg.exe"), stderr);
        assert!(
            matches!(&error, ScanError::ProbeUnsupported { program } if program == "C:/bin/ffmpeg.exe")
        );
        assert!(error.is_tool_fault());
        assert!(!error.is_media_fault());
        assert!(error.to_string().contains("KLAPZ_FFPROBE"));

        let corrupt = failure(
            "ffprobe",
            OsStr::new("ffmpeg"),
            b"file:x.mkv: Invalid data found when processing input\n",
        );
        assert!(corrupt.is_media_fault());
        assert!(!corrupt.is_tool_fault());
    }
}
