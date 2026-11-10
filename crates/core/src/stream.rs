//! Sending a file to a player: as it is, or converted on the way.
//!
//! The caller names the file, its tracks and what the player can take; the engine behind the
//! port decides how each track travels and runs the conversion. A started stream is reached
//! at addresses of its own, opened by a secret of its own rather than by the caller's token: a
//! player hands those addresses to components that never see the account.

use crate::metadata::MediaStream;
use crate::playback::{PlaybackOutput, StreamDecision};

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

/// What the player asks for beyond being able to read the result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamLimits {
    /// Bits per second.
    pub max_video_bitrate: Option<u32>,
    /// Convert audio even when it could be copied.
    pub convert_audio: bool,
    /// Automatic quality.
    pub adaptive: bool,
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

#[async_trait::async_trait]
pub trait StreamEngine: Send + Sync + std::fmt::Debug {
    /// Starts a stream, or keeps the one under the same key if it produces the same thing:
    /// players ask for the same start more than once.
    async fn start(&self, order: StreamOrder) -> Result<StartedStream, StreamError>;

    /// Stops the stream stored under `key`. Whether there was one.
    fn stop(&self, key: &str) -> bool;
}
