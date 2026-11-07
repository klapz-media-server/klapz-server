//! GraphQL types for the media chain.
//!
//! Named for what these are rather than after Plex's Media/Part/Stream, whose names leak its
//! implementation. The Plex compatibility layer maps back to them.

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, Object, Result, ID};
use open_plex_core::metadata::{MediaFile, MediaStream, MediaVersion, StreamKind};

use crate::loader::{FilesLoader, StreamsLoader};
use crate::node::GlobalId;

pub const VERSION_TYPE: &str = "MediaVersion";
pub const FILE_TYPE: &str = "MediaFile";
pub const STREAM_TYPE: &str = "MediaStream";

/// The kind of a media track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TrackKind {
    /// Picture.
    Video,
    /// Sound.
    Audio,
    /// Subtitles or captions.
    Subtitle,
}

impl From<StreamKind> for TrackKind {
    fn from(kind: StreamKind) -> Self {
        match kind {
            StreamKind::Video => Self::Video,
            StreamKind::Audio => Self::Audio,
            StreamKind::Subtitle => Self::Subtitle,
        }
    }
}

/// A particular encoding of an item — a 1080p cut and a 4K cut are two versions.
#[derive(Debug, Clone)]
pub struct Version(pub MediaVersion);

#[allow(clippy::missing_errors_doc)]
#[Object(name = "MediaVersion")]
impl Version {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this version.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(VERSION_TYPE, self.0.id.get()))
    }

    /// Frame width in pixels.
    async fn width(&self) -> Option<i32> {
        self.0.width
    }

    /// Frame height in pixels.
    async fn height(&self) -> Option<i32> {
        self.0.height
    }

    /// Display aspect ratio as measured, for example 2.3762.
    ///
    /// The exact value, not the standard it is closest to: Plex reports 2.35 here, having
    /// snapped the measurement onto a ladder of cinema ratios. That snap is presentation and
    /// lives in the compatibility layer.
    async fn aspect_ratio(&self) -> Option<f64> {
        self.0.aspect_ratio
    }

    /// Video codec, for example `h264` or `hevc`.
    async fn video_codec(&self) -> Option<&str> {
        self.0.video_codec.as_deref()
    }

    /// Audio codec of the default track, for example `eac3`.
    async fn audio_codec(&self) -> Option<&str> {
        self.0.audio_codec.as_deref()
    }

    /// Channel count of the default audio track.
    async fn audio_channels(&self) -> Option<i32> {
        self.0.audio_channels
    }

    /// Overall bitrate in **bits** per second, as measured.
    ///
    /// Plex publishes kilobits; this is the unmodified measurement, and the compatibility
    /// layer divides. Same reasoning as `frameRate`: the official API gives the number, not
    /// the display form.
    async fn bitrate(&self) -> Option<i32> {
        self.0.bitrate
    }

    /// Container format, for example `mkv`.
    async fn container(&self) -> Option<&str> {
        self.0.container.as_deref()
    }

    /// Runtime in milliseconds.
    async fn duration_ms(&self) -> Option<i64> {
        self.0.duration_ms
    }

    /// Frames per second, for example 23.976.
    ///
    /// A number rather than a label: Plex publishes `24p`, which is a rounded display
    /// string. The compatibility layer derives it; the official API gives the measured value.
    async fn frame_rate(&self) -> Option<f64> {
        self.0.frame_rate
    }

    /// Whether the picture is interlaced.
    async fn interlaced(&self) -> bool {
        self.0.interlaced
    }

    /// Whether the file is laid out for progressive streaming without a remux.
    async fn optimized_for_streaming(&self) -> bool {
        self.0.optimized_for_streaming
    }

    /// The files making up this version.
    ///
    /// Usually one; several for a release split across discs — 14 of 157,414 files here.
    async fn files(&self, context: &Context<'_>) -> Result<Vec<File>> {
        let loader = context.data::<DataLoader<FilesLoader>>()?;
        Ok(loader
            .load_one(self.0.id)
            .await?
            .unwrap_or_default()
            .into_iter()
            .map(File)
            .collect())
    }
}

/// One file on disk belonging to a version.
#[derive(Debug, Clone)]
pub struct File(pub MediaFile);

#[allow(clippy::missing_errors_doc)]
#[Object(name = "MediaFile")]
impl File {
    /// Globally unique, opaque identifier. Playback and download endpoints take it too.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(FILE_TYPE, self.0.id.get()))
    }

    /// Position within a multi-file release, starting at zero.
    async fn position(&self) -> i32 {
        self.0.position
    }

    /// Absolute path on the server's filesystem.
    ///
    /// Reference data: it tells a caller where the media actually lives, which is what makes
    /// the library auditable from outside. It is not an input — playback addresses a file by
    /// `id`, and no endpoint accepts a path from a client.
    async fn path(&self) -> &str {
        &self.0.path
    }

    /// Last segment of `path`, without its directories.
    ///
    /// Derived rather than stored: it is what a caller displays, and deriving it keeps one
    /// source of truth for where the file is. Both separators are honored because a library
    /// scanned on Windows stores `\` while the same server may serve a path scanned on Linux.
    async fn filename(&self) -> &str {
        filename_of(&self.0.path)
    }

    /// File size in bytes.
    async fn size_bytes(&self) -> Option<i64> {
        self.0.size_bytes
    }

    /// Runtime in milliseconds.
    async fn duration_ms(&self) -> Option<i64> {
        self.0.duration_ms
    }

    /// The tracks inside this file, plus any sidecar subtitle files.
    async fn streams(&self, context: &Context<'_>) -> Result<Vec<Track>> {
        let loader = context.data::<DataLoader<StreamsLoader>>()?;
        Ok(loader
            .load_one(self.0.id)
            .await?
            .unwrap_or_default()
            .into_iter()
            .map(Track)
            .collect())
    }
}

/// The last path segment, or the whole string when it has no separator.
///
/// Splits on both `/` and `\`: the separator belongs to the machine that scanned the
/// library, not to the one answering the request.
fn filename_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A track: a video, audio or subtitle stream.
#[derive(Debug, Clone)]
pub struct Track(pub MediaStream);

#[allow(clippy::missing_errors_doc)]
#[Object(name = "MediaStream")]
impl Track {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this track.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(STREAM_TYPE, self.0.id.get()))
    }

    /// Which kind of track this is.
    async fn kind(&self) -> TrackKind {
        self.0.kind.into()
    }

    /// Index of the track inside the file, as the transcoder addresses it.
    async fn position(&self) -> i32 {
        self.0.position
    }

    /// Codec, for example `h264`, `eac3` or `srt`.
    async fn codec(&self) -> Option<&str> {
        self.0.codec.as_deref()
    }

    /// Language tag as stored, for example `fr` or `en`.
    async fn language(&self) -> Option<&str> {
        self.0.language.as_deref()
    }

    /// Channel count, for audio tracks.
    async fn channels(&self) -> Option<i32> {
        self.0.channels
    }

    /// Bitrate in **bits** per second, as measured. See `MediaVersion.bitrate`.
    async fn bitrate(&self) -> Option<i32> {
        self.0.bitrate
    }

    /// Whether the file marks this track as the default for its kind.
    async fn is_default(&self) -> bool {
        self.0.is_default
    }

    /// Whether the subtitle track is forced — shown even when subtitles are off, for foreign
    /// dialogue.
    async fn is_forced(&self) -> bool {
        self.0.is_forced
    }

    /// Whether the track lives in a separate file next to the media rather than inside it.
    async fn is_external(&self) -> bool {
        self.0.external_path.is_some()
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::filename_of;

    #[test]
    fn takes_the_last_segment_whatever_the_separator() {
        assert_eq!(
            filename_of("/mnt/acd/Plex/Movies/VF/Avatar.mkv"),
            "Avatar.mkv"
        );
        assert_eq!(filename_of(r"D:\Media\Films\Avatar.mkv"), "Avatar.mkv");
        // A path scanned on one platform and served on another mixes both separators.
        assert_eq!(filename_of(r"/mnt/share\Avatar.mkv"), "Avatar.mkv");
        assert_eq!(filename_of("Avatar.mkv"), "Avatar.mkv");
        assert_eq!(filename_of(""), "");
        // A trailing separator names no file; reporting empty beats reporting the directory.
        assert_eq!(filename_of("/mnt/movies/"), "");
    }
}
