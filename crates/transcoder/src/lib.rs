//! Converting media for a client.
//!
//! The engine, as `klapz-api` is the official API: it speaks in images, sizes and formats,
//! and knows nothing of HTTP or Plex. `klapz-api-plex` binds Plex's routes onto it, and
//! keeps what is only Plex's: parameter names, status codes, and a PNG labelled `image/jpeg`.

pub mod adapt;
mod backdrop;
mod convert;
pub mod media;
pub mod plan;
pub mod session;
mod source;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{AcquireError, OnceCell, Semaphore};

pub use backdrop::{CornerColors, Rgb, MAX_PIXELS as MAX_BACKDROP_PIXELS};

/// Images being fetched and converted at once. Each one holds a blocking thread and up to
/// 16 MiB of source, and a client controls how many it asks for.
const MAX_CONCURRENT: usize = 8;

/// Whether an image must fill its box or fit inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Entirely inside the box.
    Contain,
    /// Covers the box, overflowing one side.
    Cover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Jpeg,
    Png,
    Ppm,
}

#[derive(Debug, Clone)]
pub struct ImageRequest {
    /// Where the image is, as a URL. Only some hosts are ever fetched.
    pub source: String,
    /// The box. Signed because a side may legitimately be out of range as long as the other
    /// one decides the scale; a result that cannot be drawn is [`TranscodeError::UnusableSize`].
    pub width: i32,
    pub height: i32,
    pub fit: Fit,
    /// Whether an image smaller than its box may be enlarged.
    pub upscale: bool,
    /// `None` keeps the source's format. A transparent source stays PNG whatever is asked,
    /// unless a format that is not JPEG was requested.
    pub format: Option<OutputFormat>,
    /// JPEG quality, 1 to 100.
    pub quality: Option<u8>,
}

#[derive(Debug)]
pub struct ConvertedImage {
    pub bytes: Vec<u8>,
    /// What the bytes are, which a request for JPEG does not guarantee.
    pub format: OutputFormat,
}

#[derive(Debug, thiserror::Error)]
pub enum TranscodeError {
    #[error("the source is not a usable URL")]
    InvalidSource,
    #[error("the source is on a host this server does not fetch from")]
    ForbiddenSource,
    #[error("the source could not be fetched")]
    UnavailableSource,
    #[error("the source image is larger than the server accepts")]
    SourceTooLarge,
    #[error("the source is not an image this server reads")]
    Undecodable,
    #[error("the requested size yields no usable image")]
    UnusableSize,
    #[error("could not encode the image: {0}")]
    Encode(image::ImageError),
    #[error("could not build the HTTP client: {0}")]
    Client(reqwest::Error),
    #[error("the worker converting the image died: {0}")]
    Worker(tokio::task::JoinError),
    #[error("the transcoder is shutting down: {0}")]
    ShuttingDown(AcquireError),
    #[error("the file does not exist on this machine")]
    MissingFile,
    #[error("the file is outside every library")]
    OutsideLibrary,
    #[error("the file is too large to be a subtitle")]
    SidecarTooLarge,
    #[error("the video would have to be converted")]
    VideoNeedsTranscoding,
    /// An image subtitle, which the process overlays rather than draws, or no fonts to draw
    /// a text one with.
    #[error("this subtitle cannot be burned in")]
    SubtitleCannotBeBurned,
    #[error("too many transcode sessions are running")]
    TooManySessions,
    #[error("no such transcode session")]
    UnknownSession,
    #[error("no such segment in this session")]
    UnknownSegment,
    #[error("could not start the transcoder: {0}")]
    Spawn(std::io::Error),
    #[error("could not prepare the session's directory: {0}")]
    Io(std::io::Error),
}

/// How to run the transcoder process and reach this server from it.
#[derive(Debug, Clone)]
pub struct Config {
    /// The patched ffmpeg, whose options the command lines use.
    pub binary: PathBuf,
    /// Where its separately distributed codecs are, for Plex's own build.
    pub external_libs: Option<PathBuf>,
    /// This server, as the process can reach it: `http://127.0.0.1:32500`.
    pub callback_base: String,
    /// The fonts subtitles are drawn with: Plex's own, `Resources/Fonts` beside its binary.
    /// Without them nothing is burned in.
    pub fonts: Option<PathBuf>,
    /// Where sessions write their output. Emptied at startup, so it must be this server's
    /// alone: a second server sharing it would delete the files of the first one's streams.
    pub sessions_directory: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("ffmpeg"),
            external_libs: None,
            callback_base: "http://127.0.0.1:32500".to_owned(),
            fonts: None,
            sessions_directory: std::env::temp_dir().join("klapz-transcode"),
        }
    }
}

#[derive(Debug)]
pub struct Transcoder {
    slots: Semaphore,
    client: OnceCell<reqwest::Client>,
    config: Config,
    sessions: Mutex<HashMap<String, Arc<session::Session>>>,
}

impl Default for Transcoder {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

impl Transcoder {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            slots: Semaphore::new(MAX_CONCURRENT),
            client: OnceCell::new(),
            config,
            sessions: Mutex::default(),
        }
    }

    /// Fetches an image and converts it.
    ///
    /// # Errors
    ///
    /// See [`TranscodeError`]: the source may be refused, missing or not an image, or the
    /// requested size impossible.
    pub async fn convert_image(
        &self,
        request: ImageRequest,
    ) -> Result<ConvertedImage, TranscodeError> {
        let url = source::remote_url(&request.source)?;
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(TranscodeError::ShuttingDown)?;
        let client = self
            .client
            .get_or_try_init(|| async { source::client() })
            .await?;
        let bytes = source::fetch(client, url).await?;

        tokio::task::spawn_blocking(move || convert::convert(&bytes, &request))
            .await
            .map_err(TranscodeError::Worker)?
    }

    /// Fetches an image and draws its backdrop's four corner colors from it.
    ///
    /// # Errors
    ///
    /// As [`Transcoder::convert_image`] for the source; [`TranscodeError::UnusableSize`] for
    /// an image too small or too white to have colors.
    pub async fn corner_colors(&self, source: &str) -> Result<CornerColors, TranscodeError> {
        let url = source::remote_url(source)?;
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(TranscodeError::ShuttingDown)?;
        let client = self
            .client
            .get_or_try_init(|| async { source::client() })
            .await?;
        let bytes = source::fetch(client, url).await?;

        tokio::task::spawn_blocking(move || backdrop::corner_colors(&bytes))
            .await
            .map_err(TranscodeError::Worker)?
    }

    /// The gradient of four corner colors, as a PNG of at most [`MAX_BACKDROP_PIXELS`].
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnusableSize`] for an empty or oversized image.
    pub async fn backdrop(
        &self,
        colors: CornerColors,
        width: u32,
        height: u32,
        noise: bool,
    ) -> Result<Vec<u8>, TranscodeError> {
        // Up to 25 MB of pixels each: held to the same few at once as conversions.
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(TranscodeError::ShuttingDown)?;
        tokio::task::spawn_blocking(move || backdrop::gradient(colors, width, height, noise))
            .await
            .map_err(TranscodeError::Worker)?
    }
}
