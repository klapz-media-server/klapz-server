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
