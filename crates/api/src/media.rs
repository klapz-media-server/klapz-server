//! GraphQL types for the media chain.
//!
//! Named for what these are rather than after Plex's Media/Part/Stream, whose names leak its
//! implementation. The Plex compatibility layer maps back to them.

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, Object, Result, ID};
use open_plex_core::metadata::{
    AudioAnalysis, ColorDescription, DolbyVision, Loudness, MediaFile, MediaStream, MediaVersion,
    ScanType, StreamKind, SubtitleAnalysis, TrackTraits, VideoAnalysis,
};

use open_plex_core::tracks::{selected_tracks, SelectedTracks};
use open_plex_core::StreamId;

use open_plex_core::stream::{reads_as_is, FileFormat, PlayerFormat};

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

    /// The first video track of the version's files: the one `videoCodec` and the
    /// dimensions describe.
    async fn primary_video(&self, context: &Context<'_>) -> Result<Option<Track>> {
        Ok(self
            .tracks(context)
            .await?
            .into_iter()
            .find(|track| track.kind == StreamKind::Video)
            .map(Track))
    }

    /// The audio track `audioCodec` and `audioChannels` describe: the first matching both,
    /// else the default one, else the first. Not always the default — a file can mark one
    /// track default and have been summarised from another.
    async fn primary_audio(&self, context: &Context<'_>) -> Result<Option<Track>> {
        let tracks = self.tracks(context).await?;
        Ok(self.0.primary_audio(&tracks).cloned().map(Track))
    }
}

impl Version {
    /// Every track of every file, files in order, through the loaders the file and track
    /// fields share, so a page of versions still costs two batched queries.
    async fn tracks(&self, context: &Context<'_>) -> Result<Vec<MediaStream>> {
        let files = context
            .data::<DataLoader<FilesLoader>>()?
            .load_one(self.0.id)
            .await?
            .unwrap_or_default();
        let mut by_file = context
            .data::<DataLoader<StreamsLoader>>()?
            .load_many(files.iter().map(|file| file.id))
            .await?;
        Ok(files
            .iter()
            .flat_map(|file| by_file.remove(&file.id).unwrap_or_default())
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

    /// Absolute path on the server's filesystem. Empty for a file a provider streams.
    ///
    /// Reference data: it tells a caller where the media actually lives, which is what makes
    /// the library auditable from outside. It is not an input — playback addresses a file by
    /// `id`, and no endpoint accepts a path from a client.
    async fn path(&self) -> &str {
        &self.0.path
    }

    /// Where a metadata provider streams the file from, relative to the server that
    /// imported it: the trailers the reference's provider supplied, as
    /// `/services/iva/assets/…`. Null for a file of the library. This server streams none.
    async fn stream_url(&self) -> Option<&str> {
        self.0.stream_url.as_deref()
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

    /// The SHA-1 the scanner computed over the file's contents, in hex: two copies of one
    /// file share it. Null when the scanner computed none.
    async fn content_hash(&self) -> Option<&str> {
        self.0.content_hash.as_deref()
    }

    /// The file's `OpenSubtitles` hash, in hex — its size plus the 64-bit sums of its first and
    /// last 64 KiB — which subtitle services find a file by. Null when the scanner computed
    /// none.
    async fn open_subtitles_hash(&self) -> Option<&str> {
        self.0.open_subtitles_hash.as_deref()
    }

    /// Whether the file embeds a cover image.
    async fn has_thumbnail(&self) -> bool {
        self.0.layout.has_thumbnail
    }

    /// Whether an MP4 file uses 64-bit chunk offsets, which some older players cannot read.
    /// Null for a file that is not MP4.
    async fn has_64bit_offsets(&self) -> Option<bool> {
        self.0.layout.has_64bit_offsets
    }

    /// Whether an MP4 file's index precedes its data, so it plays before it has fully
    /// arrived. Null for a file that is not MP4.
    async fn optimized_for_streaming(&self) -> Option<bool> {
        self.0.layout.optimized_for_streaming
    }

    /// The packet size of an MPEG-TS file in bytes: 188, or 192 for Blu-ray's M2TS.
    async fn packet_length(&self) -> Option<i32> {
        self.0.layout.packet_length
    }

    /// Whether chapter names are carried as a text track.
    async fn has_chapter_text_stream(&self) -> bool {
        self.0.layout.has_chapter_text_stream
    }

    /// The tracks inside this file, plus any sidecar subtitle files.
    async fn streams(&self, context: &Context<'_>) -> Result<Vec<Track>> {
        Ok(self.tracks(context).await?.into_iter().map(Track).collect())
    }

    /// Whether a player reads this file as it is, so that `startStream` can send it untouched
    /// with `directPlay`: one of the formats the player declares holds the file's container,
    /// its video codec and the codec of the audio track it would play — `audioTrack`, or the
    /// caller's — and the version weighs no more than `maxBitrate`. An empty list in a format
    /// admits anything; names are compared without regard to case.
    async fn readable_as_is(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "What the player reads as it is, each format on its own.")] formats: Vec<
            PlayerFormatInput,
        >,
        #[graphql(desc = "The most the player takes, in bits per second.")] max_bitrate: Option<
            i64,
        >,
        #[graphql(desc = "The audio track it would play, as returned in `MediaStream.id`.")]
        audio_track: Option<ID>,
    ) -> Result<bool> {
        let Some(version) = crate::schema::readable_version(context, self.0.version_id).await?
        else {
            return Ok(false);
        };
        let (tracks, selected) = self.selection(context).await?;
        let audio = match &audio_track {
            Some(id) => {
                let wanted = GlobalId::decode(id)?.key;
                tracks
                    .iter()
                    .find(|track| track.id.get() == wanted && track.kind == StreamKind::Audio)
                    .ok_or_else(|| async_graphql::Error::new("no such audio track in this file"))?
                    .codec
                    .as_deref()
            }
            None => selected
                .audio
                .and_then(|id| tracks.iter().find(|track| track.id == id))
                .and_then(|track| track.codec.as_deref()),
        };
        let video = tracks
            .iter()
            .find(|track| track.kind == StreamKind::Video)
            .and_then(|track| track.codec.as_deref())
            .or(version.video_codec.as_deref());
        let formats: Vec<PlayerFormat> = formats.into_iter().map(Into::into).collect();
        Ok(reads_as_is(
            &formats,
            FileFormat {
                container: version.container.as_deref(),
                video_codec: video,
                audio_codec: audio,
                bitrate: version.bitrate.map(i64::from),
            },
            max_bitrate,
        ))
    }

    /// The audio track the caller plays this file with: the one it chose with `chooseTracks`,
    /// else the file's default, else its first.
    async fn selected_audio(&self, context: &Context<'_>) -> Result<Option<Track>> {
        let (tracks, selected) = self.selection(context).await?;
        Ok(pick(tracks, selected.audio))
    }

    /// The subtitle the caller plays this file with: the one it chose, none if it chose none,
    /// else the forced subtitle in the selected audio's language. Null for no subtitle.
    async fn selected_subtitle(&self, context: &Context<'_>) -> Result<Option<Track>> {
        let (tracks, selected) = self.selection(context).await?;
        Ok(pick(tracks, selected.subtitle))
    }
}

impl File {
    async fn tracks(&self, context: &Context<'_>) -> Result<Vec<MediaStream>> {
        let loader = context.data::<DataLoader<StreamsLoader>>()?;
        Ok(loader.load_one(self.0.id).await?.unwrap_or_default())
    }

    async fn selection(&self, context: &Context<'_>) -> Result<(Vec<MediaStream>, SelectedTracks)> {
        let tracks = self.tracks(context).await?;
        let choice = crate::watch::track_choice_of(context, self.0.id).await?;
        let selected = selected_tracks(&tracks, choice);
        Ok((tracks, selected))
    }
}

fn pick(tracks: Vec<MediaStream>, id: Option<StreamId>) -> Option<Track> {
    let id = id?;
    tracks.into_iter().find(|track| track.id == id).map(Track)
}

/// The last path segment, or the whole string when it has no separator.
///
/// Splits on both `/` and `\`: the separator belongs to the machine that scanned the
/// library, not to the one answering the request.
pub(crate) fn filename_of(path: &str) -> &str {
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

    /// The file a subtitle kept apart from its media lives in, and its text. Null for a track
    /// inside the media.
    async fn sidecar(&self) -> Option<crate::sidecar::SidecarFile> {
        crate::sidecar::SidecarFile::of(&self.0)
    }

    /// The offset the caller set on this subtitle with `setSubtitleOffset`, in milliseconds,
    /// for its player to apply. 0 when it set none, and on a video or audio track.
    async fn offset_ms(&self, context: &Context<'_>) -> Result<i32> {
        crate::sidecar::offset_of(context, self.0.id).await
    }

    /// Where an external track's file is, as the scanner recorded it: a `file://` URL. Null
    /// for a track inside the media file.
    ///
    /// Reference data, as `MediaFile.path` is: no endpoint accepts it back.
    async fn external_url(&self) -> Option<&str> {
        self.0.external_path.as_deref()
    }

    /// The name the file gives the track, often a release tag: `VFF AC3 5.1 @448kb`. Empty
    /// when the file writes an empty name, null when it writes none.
    async fn title(&self) -> Option<&str> {
        self.0.title.as_deref()
    }

    /// The container's own number for the track: a Matroska track number, an MPEG-TS PID
    /// (13 bits at most, so always within an `Int`).
    async fn stream_identifier(&self) -> Option<i32> {
        self.0
            .stream_identifier
            .and_then(|identifier| i32::try_from(identifier).ok())
    }

    /// The codec profile: `high` or `main 10` for video, `lc` for AAC, `ma` for DTS-HD
    /// Master Audio, `dolby digital plus + dolby atmos`.
    async fn profile(&self) -> Option<&str> {
        self.0.profile.as_deref()
    }

    /// Bits per sample.
    async fn bit_depth(&self) -> Option<i32> {
        self.0.bit_depth
    }

    /// The bandwidth, in bits per second, the track needs to play without stalling, for each
    /// of the buffer sizes the analysis tried, from the smallest: a larger buffer absorbs
    /// more of the peaks. Empty when the track was never analysed that far.
    ///
    /// Floats holding whole numbers: the analysis writes `2147483647` kilobits for a buffer
    /// that no bandwidth fills, which in bits exceeds an `Int`.
    async fn required_bandwidths(&self) -> Vec<f64> {
        self.0
            .required_bandwidths
            .iter()
            .map(|kilobits| f64::from(*kilobits) * 1000.0)
            .collect()
    }

    /// What the file says the track is for, beyond its language.
    async fn traits(&self) -> Traits {
        Traits(self.0.traits)
    }

    /// What the analysis measured about a video track. Null on any other kind.
    async fn video(&self) -> Option<Video> {
        self.0.video.as_deref().cloned().map(Video)
    }

    /// What the analysis measured about an audio track. Null on any other kind.
    async fn audio(&self) -> Option<Audio> {
        self.0.audio.clone().map(Audio)
    }

    /// What the analysis says about a subtitle track. Null on any other kind.
    async fn subtitle(&self) -> Option<Subtitle> {
        self.0.subtitle.clone().map(Subtitle)
    }
}

/// What a file says a track is for.
#[derive(Debug, Clone, Copy)]
pub struct Traits(pub TrackTraits);

#[Object(name = "TrackTraits")]
impl Traits {
    /// In the work's original language.
    async fn is_original(&self) -> bool {
        self.0.original
    }

    /// A dub into another language.
    async fn is_dub(&self) -> bool {
        self.0.dub
    }

    /// A commentary track.
    async fn is_commentary(&self) -> bool {
        self.0.commentary
    }

    /// Meant for the deaf and hard of hearing: subtitles describing sounds as well.
    async fn is_hearing_impaired(&self) -> bool {
        self.0.hearing_impaired
    }

    /// Audio description, for the blind and partially sighted.
    async fn is_visual_impaired(&self) -> bool {
        self.0.visual_impaired
    }

    /// Whether the track uses Matroska header stripping, which some players cannot decode.
    async fn has_header_compression(&self) -> bool {
        self.0.header_compression
    }
}

/// Whether a picture is drawn whole or in alternating fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum Scan {
    /// Every frame drawn whole.
    Progressive,
    /// Alternating fields of odd and even lines.
    Interlaced,
}

impl From<ScanType> for Scan {
    fn from(scan: ScanType) -> Self {
        match scan {
            ScanType::Progressive => Self::Progressive,
            ScanType::Interlaced => Self::Interlaced,
        }
    }
}

/// What the analysis measured about a video track.
#[derive(Debug, Clone)]
pub struct Video(pub VideoAnalysis);

#[Object(name = "VideoAnalysis")]
impl Video {
    /// Displayed width in pixels.
    async fn width(&self) -> Option<i32> {
        self.0.width
    }

    /// Displayed height in pixels.
    async fn height(&self) -> Option<i32> {
        self.0.height
    }

    /// Width as encoded, before cropping.
    async fn coded_width(&self) -> Option<i32> {
        self.0.coded_width
    }

    /// Height as encoded, before cropping: 1088 for most 1080-line H.264, which codes
    /// whole 16-pixel macroblocks.
    async fn coded_height(&self) -> Option<i32> {
        self.0.coded_height
    }

    /// Frames per second, as measured: 23.976.
    async fn frame_rate(&self) -> Option<f64> {
        self.0.frame_rate
    }

    /// The codec's level indicator as coded: 41 is H.264 level 4.1, 120 is HEVC level 4,
    /// which codes thirty times the level.
    async fn level(&self) -> Option<i32> {
        self.0.level
    }

    /// How many reference frames the decoder must keep.
    async fn ref_frames(&self) -> Option<i32> {
        self.0.ref_frames
    }

    /// Progressive or interlaced. Null when the analysis did not say.
    async fn scan(&self) -> Option<Scan> {
        self.0.scan_type.map(Into::into)
    }

    /// Chroma subsampling: `4:2:0`.
    async fn chroma_subsampling(&self) -> Option<&str> {
        self.0.chroma_subsampling.as_deref()
    }

    /// Where chroma samples sit relative to luma: `left`, `topleft`, `center`.
    async fn chroma_location(&self) -> Option<&str> {
        self.0.chroma_location.as_deref()
    }

    /// How the track's colors are to be read.
    async fn color(&self) -> Color {
        Color(self.0.color.clone())
    }

    /// Whether the encoder used custom quantisation matrices. Null when the analysis did
    /// not say, which is not the same as false.
    async fn has_scaling_matrix(&self) -> Option<bool> {
        self.0.has_scaling_matrix
    }

    /// Whether pixels are not square. Null when the analysis did not say.
    async fn is_anamorphic(&self) -> Option<bool> {
        self.0.anamorphic
    }

    /// The sample aspect ratio, as a ratio of integers: `16:15`. Set on anamorphic tracks.
    async fn pixel_aspect_ratio(&self) -> Option<&str> {
        self.0.pixel_aspect_ratio.as_deref()
    }

    /// The codec tag a legacy container carries: `XVID`, `DX50`.
    async fn fourcc(&self) -> Option<&str> {
        self.0.fourcc.as_deref()
    }

    /// Whether closed captions (EIA-608) ride inside the picture.
    async fn has_closed_captions(&self) -> bool {
        self.0.closed_captions
    }

    /// Whether fields are coded separately, as an interlaced picture can be.
    async fn has_separate_fields(&self) -> bool {
        self.0.separate_fields
    }

    /// Whether the track carries HDR10+ dynamic metadata.
    async fn has_hdr10_plus(&self) -> bool {
        self.0.hdr10_plus
    }

    /// The Dolby Vision configuration. Null on a track without Dolby Vision.
    async fn dolby_vision(&self) -> Option<DolbyVisionObject> {
        self.0.dolby_vision.clone().map(DolbyVisionObject)
    }
}

/// How a video track's colors are to be read, in ffmpeg's vocabulary.
#[derive(Debug, Clone)]
pub struct Color(pub ColorDescription);

#[Object(name = "ColorDescription")]
impl Color {
    /// `tv` for limited range, `pc` for full.
    async fn range(&self) -> Option<&str> {
        self.0.range.as_deref()
    }

    /// The matrix coefficients: `bt709`, `bt2020nc`.
    async fn space(&self) -> Option<&str> {
        self.0.space.as_deref()
    }

    /// The primaries: `bt709`, `bt2020`.
    async fn primaries(&self) -> Option<&str> {
        self.0.primaries.as_deref()
    }

    /// The transfer characteristic: `bt709`; `smpte2084` is PQ, the curve of HDR10 and
    /// Dolby Vision; `arib-std-b67` is HLG.
    async fn transfer(&self) -> Option<&str> {
        self.0.transfer.as_deref()
    }
}

/// A Dolby Vision configuration record.
#[derive(Debug, Clone)]
pub struct DolbyVisionObject(pub DolbyVision);

#[Object(name = "DolbyVision")]
impl DolbyVisionObject {
    /// The record's version, as written: `1.0`.
    async fn version(&self) -> &str {
        &self.0.version
    }

    /// The profile: 5 (single layer, no fallback), 7 (dual layer, Blu-ray), 8 (single layer
    /// with an HDR10, SDR or HLG fallback).
    async fn profile(&self) -> i32 {
        self.0.profile
    }

    /// The level, which bounds resolution and frame rate.
    async fn level(&self) -> i32 {
        self.0.level
    }

    /// What the base layer plays as without Dolby Vision: 1 HDR10, 2 SDR, 4 HLG, 6 Blu-ray
    /// HDR10. Null when the record does not say.
    async fn base_layer_compatibility(&self) -> Option<i32> {
        self.0.base_layer_compatibility
    }

    /// Whether the track carries a base layer.
    async fn has_base_layer(&self) -> bool {
        self.0.has_base_layer
    }

    /// Whether the track carries an enhancement layer, as profile 7 does.
    async fn has_enhancement_layer(&self) -> bool {
        self.0.has_enhancement_layer
    }

    /// Whether the track carries the reference processing unit, its dynamic metadata.
    async fn has_rpu(&self) -> bool {
        self.0.has_rpu
    }
}

/// What the analysis measured about an audio track.
#[derive(Debug, Clone)]
pub struct Audio(pub AudioAnalysis);

#[Object(name = "AudioAnalysis")]
impl Audio {
    /// Samples per second, in hertz.
    async fn sampling_rate(&self) -> Option<i32> {
        self.0.sampling_rate
    }

    /// The channel layout, in ffmpeg's vocabulary: `stereo`, `5.1(side)`.
    async fn channel_layout(&self) -> Option<&str> {
        self.0.channel_layout.as_deref()
    }

    /// The track's EBU R128 loudness. Null when it was never measured.
    async fn loudness(&self) -> Option<LoudnessObject> {
        self.0.loudness.clone().map(LoudnessObject)
    }
}

/// An EBU R128 loudness measurement.
#[derive(Debug, Clone)]
pub struct LoudnessObject(pub Loudness);

#[Object(name = "Loudness")]
impl LoudnessObject {
    /// The version of the analysis that measured it, as written: `0.02`.
    async fn analysis_version(&self) -> &str {
        &self.0.analysis_version
    }

    /// Integrated loudness over the whole track, in LUFS.
    async fn integrated(&self) -> f64 {
        self.0.integrated
    }

    /// Loudness range, in LU: how much the loudness varies.
    async fn range(&self) -> f64 {
        self.0.range
    }

    /// True peak, in dBTP.
    async fn true_peak(&self) -> f64 {
        self.0.true_peak
    }

    /// The gating threshold the integration used, in LUFS.
    async fn threshold(&self) -> f64 {
        self.0.threshold
    }

    /// The gain, in dB, that brings the track to the target loudness.
    async fn gain_offset(&self) -> f64 {
        self.0.gain_offset
    }
}

/// What the analysis says about a subtitle track.
#[derive(Debug, Clone)]
pub struct Subtitle(pub SubtitleAnalysis);

#[Object(name = "SubtitleAnalysis")]
impl Subtitle {
    /// The format of a sidecar subtitle file: `srt`, `ssa`. Null for an embedded track,
    /// whose codec says it.
    async fn format(&self) -> Option<&str> {
        self.0.format.as_deref()
    }

    /// Whether the captions ride inside the video bitstream (EIA-608) rather than in a
    /// track of their own.
    async fn is_embedded_in_video(&self) -> bool {
        self.0.embedded_in_video
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

/// A format a player reads as it is.
#[derive(Debug, async_graphql::InputObject)]
pub struct PlayerFormatInput {
    /// Containers, as ffmpeg names them: `mkv`, `mp4`, `webm`. Empty for any.
    #[graphql(default)]
    pub containers: Vec<String>,
    /// Video codecs it reads in them: `h264`, `hevc`. Empty for any.
    #[graphql(default)]
    pub video_codecs: Vec<String>,
    /// Audio codecs it reads in them: `aac`, `opus`. Empty for any.
    #[graphql(default)]
    pub audio_codecs: Vec<String>,
}

impl From<PlayerFormatInput> for PlayerFormat {
    fn from(input: PlayerFormatInput) -> Self {
        Self {
            containers: input.containers,
            video_codecs: input.video_codecs,
            audio_codecs: input.audio_codecs,
        }
    }
}
