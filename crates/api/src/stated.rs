//! What a file states about itself and its tracks as its prober printed it, and what it carries
//! besides the tracks that play.
//!
//! The rest of the media chain is in the reference's vocabulary, as Plex's analysis stored it;
//! these are ffprobe's own words, which other clients' servers publish (the Jellyfin surface
//! reads them). Only a scanned file states them: an imported one answers null.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, Object, Result};
use open_plex_core::metadata::{
    MediaFile, MetadataError, MetadataRepository, SidePicture, SideStream, SideStreamKind,
    StatedFile, StatedTrack,
};
use open_plex_core::PartId;

use crate::int64::Int64;
use crate::loader::StreamsLoader;

/// What a track states as the prober printed it.
#[derive(Debug, Clone)]
pub struct StatedTrackObject(pub StatedTrack);

#[Object(name = "StatedTrack")]
impl StatedTrackObject {
    /// The language as the file states it, as written: `fre`, `fr-FR`, `und`, `fra,fra`.
    /// Null when it states none — a Matroska track saying `und` among them, which
    /// `MediaStream.language` reads as English.
    async fn language(&self) -> Option<&str> {
        self.0.language.as_deref()
    }

    /// An MP4 track's own name, from its `udta`: a title some muxers write there, which
    /// `MediaStream.title` does not read.
    async fn name(&self) -> Option<&str> {
        self.0.name.as_deref()
    }

    /// The handler an MP4 track names: `SoundHandler`, sometimes a title.
    async fn handler_name(&self) -> Option<&str> {
        self.0.handler_name.as_deref()
    }

    /// The unit of the track's timestamps, `num/den`: `1/1000` in Matroska.
    async fn time_base(&self) -> Option<&str> {
        self.0.time_base.as_deref()
    }

    /// The container's codec tag as ffprobe prints it: `avc1`, or `[0][0][0][0]` where the
    /// container names none.
    async fn codec_tag(&self) -> Option<&str> {
        self.0.codec_tag.as_deref()
    }

    /// Bits per second the track's header declares.
    async fn bitrate(&self) -> Option<i64> {
        self.0.bitrate
    }

    /// Bits per second the muxer measured and tagged: Matroska's `BPS`, else its bytes over its
    /// duration.
    async fn tagged_bitrate(&self) -> Option<i64> {
        self.0.tagged_bitrate
    }
}

/// What a file's container states as the prober printed it.
#[derive(Debug, Clone)]
pub struct StatedFileObject(pub StatedFile);

#[Object(name = "StatedFile")]
impl StatedFileObject {
    /// The demuxer's names for the format: `matroska,webm`, `mov,mp4,m4a,3gp,3g2,mj2`.
    async fn format_name(&self) -> Option<&str> {
        self.0.format_name.as_deref()
    }

    /// The duration the container declares, in microseconds.
    /// `MediaFile.durationMs` may be the time the packets were measured to span instead.
    async fn duration_us(&self) -> Option<Int64> {
        self.0.duration_us.map(Int64)
    }

    /// The bitrate the container declares, in bits per second.
    async fn bitrate(&self) -> Option<i64> {
        self.0.bitrate
    }
}

/// What a file carries besides the tracks a player plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "SideStreamKind")]
pub enum SideStreamKindObject {
    /// A picture stored as a video track: an MP4 cover, a Matroska attachment that is an
    /// image.
    Cover,
    /// Timed metadata: ID3 tags, a `QuickTime` chapter track.
    Data,
    /// A file attached to a Matroska file: a font, an image.
    Attachment,
}

impl From<SideStreamKind> for SideStreamKindObject {
    fn from(kind: SideStreamKind) -> Self {
        match kind {
            SideStreamKind::Cover => Self::Cover,
            SideStreamKind::Data => Self::Data,
            SideStreamKind::Attachment => Self::Attachment,
        }
    }
}

/// A cover, a data track or an attached file inside a media file.
#[derive(Debug, Clone)]
pub struct SideStreamObject(pub SideStream);

#[Object(name = "SideStream")]
impl SideStreamObject {
    /// Which of the three it is.
    async fn kind(&self) -> SideStreamKindObject {
        self.0.kind.into()
    }

    /// ffmpeg's index of the stream in the file, numbered with the tracks.
    async fn position(&self) -> i32 {
        self.0.position
    }

    /// As ffprobe names it: `mjpeg`, `timed_id3`, `ttf`.
    async fn codec(&self) -> Option<&str> {
        self.0.codec.as_deref()
    }

    /// The container's codec tag as ffprobe prints it.
    async fn codec_tag(&self) -> Option<&str> {
        self.0.codec_tag.as_deref()
    }

    /// The codec profile, as ffprobe names it: `Baseline`.
    async fn profile(&self) -> Option<&str> {
        self.0.profile.as_deref()
    }

    /// The language as the file states it.
    async fn language(&self) -> Option<&str> {
        self.0.language.as_deref()
    }

    /// The unit of the stream's timestamps, `num/den`.
    async fn time_base(&self) -> Option<&str> {
        self.0.time_base.as_deref()
    }

    /// An attached file's MIME type: `application/x-truetype-font`.
    async fn mime_type(&self) -> Option<&str> {
        self.0.mime_type.as_deref()
    }

    /// An attached file's name, as the file stores it.
    async fn filename(&self) -> Option<&str> {
        self.0.filename.as_deref()
    }

    /// A cover's picture. Null for data and attached files.
    async fn picture(&self) -> Option<SidePictureObject> {
        self.0.picture.clone().map(SidePictureObject)
    }

    /// Whether the file flags it default.
    async fn is_default(&self) -> bool {
        self.0.is_default
    }

    /// Whether the file flags it forced.
    async fn is_forced(&self) -> bool {
        self.0.is_forced
    }
}

/// A cover's picture, in ffprobe's terms.
#[derive(Debug, Clone)]
pub struct SidePictureObject(pub SidePicture);

#[Object(name = "SidePicture")]
impl SidePictureObject {
    /// In pixels.
    async fn width(&self) -> Option<i32> {
        self.0.width
    }

    /// In pixels.
    async fn height(&self) -> Option<i32> {
        self.0.height
    }

    /// ffmpeg's pixel format: `yuvj420p`, `bgra`.
    async fn pixel_format(&self) -> Option<&str> {
        self.0.pixel_format.as_deref()
    }

    /// Bits per component, where ffprobe states it.
    async fn bit_depth(&self) -> Option<i32> {
        self.0.bit_depth
    }

    /// The color matrix: `bt470bg`.
    async fn color_space(&self) -> Option<&str> {
        self.0.color_space.as_deref()
    }

    /// The codec's level as ffprobe states it, `-99` for none.
    async fn level(&self) -> Option<i32> {
        self.0.level
    }

    /// The lowest rate every timestamp falls on, `num/den`: a cover's is its time base.
    async fn real_frame_rate(&self) -> Option<&str> {
        self.0.real_frame_rate.as_deref()
    }

    /// The average frame rate, `num/den`; `0/0` for a still picture.
    async fn average_frame_rate(&self) -> Option<&str> {
        self.0.average_frame_rate.as_deref()
    }

    /// The sample aspect ratio, `num:den`.
    async fn sample_aspect_ratio(&self) -> Option<&str> {
        self.0.sample_aspect_ratio.as_deref()
    }

    /// The display aspect ratio, `num:den`.
    async fn display_aspect_ratio(&self) -> Option<&str> {
        self.0.display_aspect_ratio.as_deref()
    }

    /// Whether the picture is interlaced.
    async fn is_interlaced(&self) -> bool {
        self.0.interlaced
    }
}

/// Loads the side streams of several files in one call.
#[derive(Debug, Clone)]
pub struct SideStreamsLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl SideStreamsLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<PartId> for SideStreamsLoader {
    type Value = Vec<SideStream>;
    type Error = Arc<MetadataError>;

    async fn load(&self, keys: &[PartId]) -> Result<HashMap<PartId, Self::Value>, Self::Error> {
        let streams = self
            .repository
            .side_streams_for(keys)
            .await
            .map_err(Arc::new)?;
        let mut grouped: HashMap<PartId, Self::Value> = HashMap::new();
        for stream in streams {
            grouped.entry(stream.file_id).or_default().push(stream);
        }
        Ok(grouped)
    }
}

/// Loads files by id, for what a track needs of its file. Storage has no batch for it; the
/// loader still asks once per file however many of its tracks ask.
#[derive(Debug, Clone)]
pub struct FileLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl FileLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<PartId> for FileLoader {
    type Value = MediaFile;
    type Error = Arc<MetadataError>;

    async fn load(&self, keys: &[PartId]) -> Result<HashMap<PartId, Self::Value>, Self::Error> {
        let mut files = HashMap::new();
        for key in keys {
            if let Some(file) = self.repository.file(*key).await.map_err(Arc::new)? {
                files.insert(*key, file);
            }
        }
        Ok(files)
    }
}

/// A file's side streams.
pub(crate) async fn side_streams_of(
    context: &Context<'_>,
    file: PartId,
) -> Result<Vec<SideStreamObject>> {
    let loader = context.data::<DataLoader<SideStreamsLoader>>()?;
    Ok(loader
        .load_one(file)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(SideStreamObject)
        .collect())
}

/// A track's rate as its file states it, else estimated (`core::bitrate`).
pub(crate) async fn estimated_bitrate(
    context: &Context<'_>,
    track: &open_plex_core::MediaStream,
) -> Result<Option<i64>> {
    let siblings = context
        .data::<DataLoader<StreamsLoader>>()?
        .load_one(track.file_id)
        .await?
        .unwrap_or_default();
    let file = context
        .data::<DataLoader<FileLoader>>()?
        .load_one(track.file_id)
        .await?;
    let rates =
        open_plex_core::bitrate::estimate(&siblings, file.and_then(|file| file.stated.bitrate));
    Ok(siblings
        .iter()
        .zip(rates)
        .find(|(sibling, _)| sibling.id == track.id)
        .and_then(|(_, rate)| rate))
}
