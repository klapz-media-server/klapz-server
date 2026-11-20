//! Sending a file to a player: as it is, or converted on the way.
//!
//! The caller names the file, its tracks and what the player can take; the engine behind the
//! port decides how each track travels and runs the conversion. A started stream is reached
//! at addresses of its own, opened by a secret of its own rather than by the caller's token: a
//! player hands those addresses to components that never see the account.

use crate::metadata::MediaStream;
use crate::playback::{PlaybackOutput, StreamDecision};
use crate::sidecar::{SidecarError, SidecarSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamProtocol {
    Dash,
    Hls,
}

/// How a file travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The file as it is, which the player says it reads.
    Original,
    Converted(StreamProtocol),
}

/// One track, as an engine has to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamTrack {
    /// The index the transcoder addresses the track by.
    pub position: i32,
    pub codec: String,
    pub language: Option<String>,
    /// Bits per second, when measured.
    pub bitrate: Option<i32>,
}

impl StreamTrack {
    pub fn of(stream: &MediaStream) -> Self {
        Self {
            position: stream.position,
            codec: stream.codec.clone().unwrap_or_default(),
            language: stream.language.clone(),
            bitrate: stream.bitrate,
        }
    }
}

/// A file and the measurements of the version it belongs to.
#[derive(Debug, Clone)]
pub struct StreamSource {
    /// As stored. The engine opens it only once it is found to lie within one of `roots`.
    pub path: String,
    /// The roots of the file's library.
    pub roots: Vec<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Bits per second.
    pub bitrate: Option<i32>,
    pub frame_rate: Option<f64>,
    pub duration_ms: Option<i64>,
    pub video: StreamTrack,
    pub audio: StreamTrack,
}

/// How a subtitle reaches the player of a conversion.
#[derive(Debug, Clone, PartialEq)]
pub enum SubtitleOutput {
    /// Drawn into the picture. `scale` is 1.0 for the normal size.
    Burn { track: StreamTrack, scale: f64 },
    /// As text beside the stream, for the player to draw.
    Text { track: StreamTrack },
}

/// How a converted stream's quality follows the link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StreamQuality {
    /// One quality for the whole stream.
    #[default]
    Fixed,
    /// One quality announced, which the server changes by measuring how fast the player
    /// receives it.
    ServerAdaptive,
    /// Every quality announced, and the player chooses.
    ClientAdaptive,
}

/// What the player asks for beyond being able to read the result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamLimits {
    /// Bits per second: for an adaptive stream, a ceiling on every quality.
    pub max_video_bitrate: Option<u32>,
    /// Convert audio even when it could be copied.
    pub convert_audio: bool,
    pub quality: StreamQuality,
}

#[derive(Debug, Clone)]
pub struct StreamOrder {
    /// The name the stream is stored under: a new start under it replaces it.
    pub key: String,
    /// What is being watched, across changes of quality: the streams of the same playback
    /// are replaced too.
    pub playback: Option<String>,
    pub source: StreamSource,
    pub delivery: Delivery,
    pub limits: StreamLimits,
    pub subtitle: Option<SubtitleOutput>,
    pub position_ms: i64,
}

#[derive(Debug, Clone)]
pub struct StartedStream {
    /// Where the player reads it: a path on this server, its secret included. A manifest for a
    /// conversion, the file itself otherwise.
    pub url: String,
    /// Where the player reads text subtitles, when it asked for them.
    pub subtitles_url: Option<String>,
    pub video: StreamDecision,
    pub audio: StreamDecision,
    pub output: PlaybackOutput,
    /// The keys of the streams of the same playback this one replaced.
    pub replaced: Vec<String>,
}

/// A format a player declares it reads as it is: containers, and the video and audio codecs
/// it reads in them. An empty list admits anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerFormat {
    pub containers: Vec<String>,
    pub video_codecs: Vec<String>,
    pub audio_codecs: Vec<String>,
}

/// What a player has to read of a file to play it as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileFormat<'a> {
    pub container: Option<&'a str>,
    pub video_codec: Option<&'a str>,
    /// The audio track it would play.
    pub audio_codec: Option<&'a str>,
    /// Bits per second, of the whole version.
    pub bitrate: Option<i64>,
}

impl PlayerFormat {
    /// Whether this format holds the file: its container, and each of its codecs it has.
    /// Names are compared without regard to case; a container named by several names, as
    /// ffmpeg names MP4 (`mov,mp4,m4a`), is held when one of them is.
    pub fn reads(&self, file: FileFormat<'_>) -> bool {
        let admits = |list: &[String], value: Option<&str>| {
            list.is_empty()
                || value.is_some_and(|value| {
                    value.split(',').any(|name| {
                        list.iter()
                            .any(|held| held.eq_ignore_ascii_case(name.trim()))
                    })
                })
        };
        // A file without such a track needs no codec for it.
        let codec_admitted =
            |list: &[String], codec: Option<&str>| codec.is_none() || admits(list, codec);
        admits(&self.containers, file.container)
            && codec_admitted(&self.video_codecs, file.video_codec)
            && codec_admitted(&self.audio_codecs, file.audio_codec)
    }
}

/// Whether a player reads a file as it is, so that it can be sent untouched: one of the formats
/// it declares holds the file, and the file weighs no more than the most the player takes. A
/// file of unknown weight is not held back for it.
pub fn reads_as_is(
    formats: &[PlayerFormat],
    file: FileFormat<'_>,
    max_bitrate: Option<i64>,
) -> bool {
    let light_enough = match (file.bitrate, max_bitrate) {
        (Some(bitrate), Some(max)) => bitrate <= max,
        _ => true,
    };
    light_enough && formats.iter().any(|format| format.reads(file))
}

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("the file is not on this machine")]
    MissingFile,
    #[error("the video would have to be converted to a codec this server does not produce")]
    VideoNotConvertible,
    #[error("this subtitle cannot be delivered this way")]
    SubtitleNotDeliverable,
    #[error("too many streams are running")]
    TooManyStreams,
    #[error("the stream could not be started")]
    Engine(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Why a conversion was not restarted the more conservative way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FallbackRefusal {
    /// Nothing converts under the key: no stream, a file sent as it is, or a process gone.
    #[error("no conversion is running under this key")]
    NoConversion,
    /// The conversion already runs the most conservative way the engine has.
    #[error("this conversion has nothing to fall back to")]
    Exhausted,
}

#[async_trait::async_trait]
pub trait StreamEngine: Send + Sync + std::fmt::Debug {
    /// Starts a stream, or keeps the one under the same key if it produces the same thing:
    /// players ask for the same start more than once.
    async fn start(&self, order: StreamOrder) -> Result<StartedStream, StreamError>;

    /// Stops the stream stored under `key`: its process, its addresses and what it wrote.
    /// Whether there was one.
    fn stop(&self, key: &str) -> bool;

    /// A sign of life for the stream under `key`. Whether there is one.
    fn touch(&self, key: &str) -> bool;

    /// Restarts the conversion under `key` the more conservative way, under the same
    /// addresses: from hardware to software, from HEVC to H.264. What a player asks for when
    /// it cannot decode what it receives.
    ///
    /// # Errors
    ///
    /// No conversion under `key`, or none more conservative than the one running.
    fn fall_back(&self, key: &str) -> Result<(), FallbackRefusal>;

    /// The keys of the streams nobody has asked anything of for the engine's idle limit, and
    /// of those whose conversion is gone.
    fn idle(&self) -> Vec<String>;

    /// Stops the stream under `key` if it is still one `idle` would list. Whether it did.
    fn stop_if_idle(&self, key: &str) -> bool;

    /// Stops every stream, and returns once their processes have exited.
    async fn stop_all(&self);

    /// The content of a sidecar subtitle, provided it lies within its library and beside its
    /// media.
    ///
    /// # Errors
    ///
    /// [`SidecarError::Missing`] for a file that is not there or not where it may be,
    /// [`SidecarError::TooLarge`] past what a subtitle can weigh.
    async fn read_sidecar(&self, sidecar: &SidecarSource) -> Result<Vec<u8>, SidecarError>;

    /// Deletes a sidecar subtitle's file, under the same conditions as reading it. A file
    /// already gone is not an error.
    ///
    /// # Errors
    ///
    /// [`SidecarError::Missing`] for a file not where a sidecar may be, an I/O failure.
    async fn remove_sidecar(&self, sidecar: &SidecarSource) -> Result<(), SidecarError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(containers: &[&str], video: &[&str], audio: &[&str]) -> PlayerFormat {
        let owned = |list: &[&str]| list.iter().map(|name| (*name).to_owned()).collect();
        PlayerFormat {
            containers: owned(containers),
            video_codecs: owned(video),
            audio_codecs: owned(audio),
        }
    }

    fn mkv(video: &'static str, audio: &'static str) -> FileFormat<'static> {
        FileFormat {
            container: Some("mkv"),
            video_codec: Some(video),
            audio_codec: Some(audio),
            bitrate: Some(8_000_000),
        }
    }

    #[test]
    fn a_format_holds_a_file_whose_container_and_codecs_it_names() {
        let browser = format(&["mkv", "mp4"], &["h264"], &["aac", "opus"]);
        assert!(browser.reads(mkv("h264", "aac")));
        assert!(!browser.reads(mkv("hevc", "aac")));
        assert!(!browser.reads(mkv("h264", "dca")));
        assert!(!format(&["mp4"], &[], &[]).reads(mkv("h264", "aac")));
    }

    #[test]
    fn an_empty_list_admits_anything_and_a_missing_track_needs_nothing() {
        assert!(format(&[], &[], &[]).reads(mkv("hevc", "dca")));
        let silent = FileFormat {
            audio_codec: None,
            ..mkv("h264", "aac")
        };
        assert!(format(&["MKV"], &["H264"], &["aac"]).reads(silent));
    }

    #[test]
    fn a_container_of_several_names_is_held_by_one_of_them() {
        let file = FileFormat {
            container: Some("mov,mp4,m4a,3gp,3g2,mj2"),
            ..mkv("h264", "aac")
        };
        assert!(format(&["mp4"], &[], &[]).reads(file));
        assert!(!format(&["webm"], &[], &[]).reads(file));
    }

    #[test]
    fn a_player_reads_as_is_what_one_format_holds_under_its_bitrate() {
        let formats = [
            format(&["webm"], &["vp9"], &["opus"]),
            format(&["mkv"], &["h264"], &["aac"]),
        ];
        assert!(reads_as_is(&formats, mkv("h264", "aac"), Some(10_000_000)));
        assert!(!reads_as_is(&formats, mkv("h264", "aac"), Some(4_000_000)));
        assert!(reads_as_is(&formats, mkv("h264", "aac"), None));
        assert!(!reads_as_is(&[], mkv("h264", "aac"), None));
        let unweighed = FileFormat {
            bitrate: None,
            ..mkv("h264", "aac")
        };
        assert!(reads_as_is(&formats, unweighed, Some(1)));
    }
}
