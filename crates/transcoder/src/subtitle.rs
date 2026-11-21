//! A text subtitle extracted whole, in one pass of the transcoder over the file.
//!
//! A player gets the whole track at once, on the media's timeline, whatever the stream it goes
//! with is doing: a seek or a change of quality restarts the stream's process but not this.
//! It is extracted once per session, from its start.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncReadExt as _;
use tokio::process::Command;
use tokio::sync::{watch, Semaphore};
use tokio::task::AbortHandle;

use crate::plan::Track;
use crate::TranscodeError;

/// The most an extracted subtitle may weigh. A film's run to some hundred kilobytes; ASS
/// karaoke a few megabytes.
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// A text format a player reads subtitles in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Ass,
    WebVtt,
    Srt,
}

impl SubtitleFormat {
    /// The format a track already is, for those whose packets go into a file of it untouched.
    fn of_codec(codec: &str) -> Option<Self> {
        match codec {
            "ass" => Some(Self::Ass),
            "webvtt" | "vtt" => Some(Self::WebVtt),
            "srt" | "subrip" => Some(Self::Srt),
            _ => None,
        }
    }

    /// The name of ffmpeg's encoder for it.
    const fn encoder(self) -> &'static str {
        match self {
            Self::Ass => "ass",
            Self::WebVtt => "webvtt",
            Self::Srt => "subrip",
        }
    }

    /// The name of ffmpeg's muxer for it.
    const fn muxer(self) -> &'static str {
        match self {
            Self::Ass => "ass",
            Self::WebVtt => "webvtt",
            Self::Srt => "srt",
        }
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Ass => "ass",
            Self::WebVtt => "vtt",
            Self::Srt => "srt",
        }
    }

    /// As players and DASH manifests name it.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Ass => "text/x-ssa",
            Self::WebVtt => "text/vtt",
            Self::Srt => "application/x-subrip",
        }
    }
}

/// The command extracting `track` of `source` as `format` onto standard output: copied when it
/// already is that format, converted otherwise. The file is named `file:…`, so that no name is
/// read as an option or a protocol.
fn extraction_arguments(source: &Path, track: &Track, format: SubtitleFormat) -> Vec<String> {
    let codec = if SubtitleFormat::of_codec(&track.codec) == Some(format) {
        "copy"
    } else {
        format.encoder()
    };
    [
        "-nostdin",
        "-loglevel",
        "error",
        "-analyzeduration",
        "20000000",
        "-probesize",
        "20000000",
        "-i",
        &format!("file:{}", source.display()),
        "-map",
        &format!("0:{}", track.index),
        "-codec:0",
        codec,
        "-f",
        format.muxer(),
        "pipe:1",
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

/// Where a session's extraction stands.
#[derive(Debug, Clone)]
enum Extracted {
    Pending,
    Done(Arc<[u8]>),
    Failed,
}

/// A session's subtitle, extracted once, from the session's start, by a task of its own: reading
/// a whole file over a network share takes longer than a player waits for a request, and a
/// request given up must not take the work done so far with it. Every request waits for the
/// same result. Dropped with its session, it stops the process.
#[derive(Debug)]
pub(crate) struct Extraction {
    result: watch::Receiver<Extracted>,
    task: AbortHandle,
}

impl Extraction {
    pub(crate) fn start(
        config: crate::Config,
        slots: Arc<Semaphore>,
        source: PathBuf,
        track: Track,
        format: SubtitleFormat,
    ) -> Self {
        let (sender, result) = watch::channel(Extracted::Pending);
        let task = tokio::spawn(async move {
            let outcome = match slots.acquire_owned().await {
                Ok(_slot) => match extract(&config, &source, &track, format).await {
                    Ok(bytes) => Extracted::Done(bytes.into()),
                    Err(error) => {
                        tracing::warn!(%error, track = track.index, "could not extract a subtitle");
                        Extracted::Failed
                    }
                },
                Err(_) => Extracted::Failed,
            };
            // Nobody left to read it is no failure.
            let _ = sender.send(outcome);
        });
        Self {
            result,
            task: task.abort_handle(),
        }
    }

    /// The subtitle, once extracted.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::SubtitleExtraction`] when the extraction failed or was stopped.
    pub(crate) async fn bytes(&self) -> Result<Arc<[u8]>, TranscodeError> {
        let mut result = self.result.clone();
        let outcome = result
            .wait_for(|extracted| !matches!(extracted, Extracted::Pending))
            .await
            .map_err(|_| TranscodeError::SubtitleExtraction)?
            .clone();
        match outcome {
            Extracted::Done(bytes) => Ok(bytes),
            Extracted::Pending | Extracted::Failed => Err(TranscodeError::SubtitleExtraction),
        }
    }
}

impl Drop for Extraction {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Runs the transcoder over the whole file and returns the track it extracted.
///
/// # Errors
///
/// [`TranscodeError::Spawn`] when the process cannot start,
/// [`TranscodeError::SubtitleExtraction`] when it fails or writes more than a subtitle weighs.
async fn extract(
    config: &crate::Config,
    source: &Path,
    track: &Track,
    format: SubtitleFormat,
) -> Result<Vec<u8>, TranscodeError> {
    let mut command = Command::new(&config.binary);
    command
        .args(extraction_arguments(source, track, format))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // A player that goes away before the end stops the read of the file with it.
        .kill_on_drop(true);
    if let Some(codecs) = &config.external_libs {
        command.env(
            "FFMPEG_EXTERNAL_LIBS",
            crate::session::external_libs_value(codecs),
        );
    }
    let mut child = command.spawn().map_err(TranscodeError::Spawn)?;
    let mut bytes = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        stdout
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(TranscodeError::Io)?;
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(TranscodeError::SubtitleExtraction);
    }
    let status = child.wait().await.map_err(TranscodeError::Io)?;
    if !status.success() {
        return Err(TranscodeError::SubtitleExtraction);
    }
    Ok(match format {
        SubtitleFormat::WebVtt => without_timestamp_map(bytes),
        SubtitleFormat::Ass | SubtitleFormat::Srt => bytes,
    })
}

/// The file without the `X-TIMESTAMP-MAP` header Plex's build writes, which ties cues to the
/// clock of HLS's MPEG-TS segments: a player honoring it beside a whole file shifts every cue by
/// the 10 seconds it states.
fn without_timestamp_map(bytes: Vec<u8>) -> Vec<u8> {
    const HEADER: &[u8] = b"\nX-TIMESTAMP-MAP=";
    let Some(start) = bytes
        .windows(HEADER.len())
        .position(|window| window == HEADER)
    else {
        return bytes;
    };
    // Only within the header block, which the first blank line ends.
    if bytes[..start].windows(2).any(|pair| pair == b"\n\n") {
        return bytes;
    }
    let end = bytes[start + 1..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(bytes.len(), |at| start + 1 + at);
    [&bytes[..start], &bytes[end..]].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(codec: &str) -> Track {
        Track {
            index: 3,
            codec: codec.to_owned(),
            language: Some("fr".to_owned()),
        }
    }

    #[tokio::test]
    async fn every_reader_gets_the_one_outcome_of_an_extraction() {
        let config = crate::Config {
            binary: PathBuf::from("no-such-transcoder-anywhere"),
            ..crate::Config::default()
        };
        let extraction = Extraction::start(
            config,
            Arc::new(Semaphore::new(1)),
            PathBuf::from("film.mkv"),
            track("subrip"),
            SubtitleFormat::WebVtt,
        );
        let (first, second) = tokio::join!(extraction.bytes(), extraction.bytes());
        assert!(matches!(first, Err(TranscodeError::SubtitleExtraction)));
        assert!(matches!(second, Err(TranscodeError::SubtitleExtraction)));
        // Read again once over, it answers at once.
        assert!(extraction.bytes().await.is_err());
    }

    #[test]
    fn drops_the_hls_timestamp_map_from_a_whole_webvtt_file() {
        // As Plex's build writes I Am Groot's French track.
        let written =
            b"WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000\n\n\n00:37.416 --> 00:38.625\nX\n";
        assert_eq!(
            without_timestamp_map(written.to_vec()),
            b"WEBVTT\n\n\n00:37.416 --> 00:38.625\nX\n"
        );
        // A cue's text is no header.
        let cue = b"WEBVTT\n\n00:01.000 --> 00:02.000\nX-TIMESTAMP-MAP=\n";
        assert_eq!(without_timestamp_map(cue.to_vec()), cue);
    }

    #[test]
    fn copies_a_track_already_in_the_format_asked() {
        let joined = extraction_arguments(
            Path::new("/m/film.mkv"),
            &track("subrip"),
            SubtitleFormat::Srt,
        )
        .join(" ");
        assert!(
            joined.ends_with("-i file:/m/film.mkv -map 0:3 -codec:0 copy -f srt pipe:1"),
            "{joined}"
        );
        let ass =
            extraction_arguments(Path::new("f"), &track("ass"), SubtitleFormat::Ass).join(" ");
        assert!(ass.contains("-codec:0 copy -f ass"), "{ass}");
    }

    #[test]
    fn converts_a_track_into_another_format() {
        let joined = extraction_arguments(Path::new("f"), &track("subrip"), SubtitleFormat::WebVtt)
            .join(" ");
        assert!(
            joined.contains("-codec:0 webvtt -f webvtt pipe:1"),
            "{joined}"
        );
        // SSA is not ASS to the muxer: it is converted.
        let ssa =
            extraction_arguments(Path::new("f"), &track("ssa"), SubtitleFormat::Ass).join(" ");
        assert!(ssa.contains("-codec:0 ass -f ass"), "{ssa}");
        let mov =
            extraction_arguments(Path::new("f"), &track("mov_text"), SubtitleFormat::Srt).join(" ");
        assert!(mov.contains("-codec:0 subrip -f srt"), "{mov}");
    }
}
