use sea_orm::entity::prelude::*;

/// A video, audio or subtitle track.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags the source records one by one, not states to fold into an enum"
)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media_stream")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub media_part_id: i64,
    pub kind: String,
    /// The index the transcoder addresses this track by, so it keeps its ffmpeg meaning.
    pub position: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub is_default: bool,
    pub is_forced: bool,
    /// Set for a sidecar subtitle file, null when the track is embedded.
    pub external_path: Option<String>,
    /// The name the file gives the track, often a release tag. Empty when the file writes
    /// an empty name, which 796 video tracks here do; null when it writes none.
    pub title: Option<String>,
    /// The container's own number for the track: a Matroska track number, an MPEG-TS PID.
    pub stream_identifier: Option<i64>,
    /// The codec profile: `high`, `main 10`, `ma` for DTS-HD Master Audio.
    pub profile: Option<String>,
    pub bit_depth: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Dimensions as encoded, before cropping: 1088 for most 1080-line H.264.
    pub coded_width: Option<i32>,
    pub coded_height: Option<i32>,
    pub frame_rate: Option<f64>,
    /// The codec level times ten for H.264 (`41` is 4.1), times thirty for HEVC.
    pub level: Option<i32>,
    pub ref_frames: Option<i32>,
    /// `progressive` or `interlaced`.
    pub scan_type: Option<String>,
    pub chroma_subsampling: Option<String>,
    pub chroma_location: Option<String>,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_primaries: Option<String>,
    /// The transfer characteristic: `smpte2084` is PQ, the HDR10 curve.
    pub color_transfer: Option<String>,
    /// Null when the analysis did not say, which is not the same as false.
    pub has_scaling_matrix: Option<bool>,
    pub is_anamorphic: Option<bool>,
    /// Sample aspect ratio, as a ratio of integers: `16:15`.
    pub pixel_aspect_ratio: Option<String>,
    /// The codec tag a legacy container carries: `XVID`, `DX50`.
    pub fourcc: Option<String>,
    pub has_closed_captions: bool,
    pub has_separate_fields: bool,
    pub has_hdr10_plus: bool,
    /// Hertz.
    pub sampling_rate: Option<i32>,
    pub channel_layout: Option<String>,
    /// The format of a sidecar subtitle file: `srt`, `ssa`.
    pub subtitle_format: Option<String>,
    /// Captions carried inside the video bitstream (EIA-608) rather than as a track.
    pub is_embedded_in_video: bool,
    pub is_original: bool,
    pub is_dub: bool,
    pub is_commentary: bool,
    pub is_hearing_impaired: bool,
    pub is_visual_impaired: bool,
    pub has_header_compression: bool,
    /// Kilobits per second the analysis found the track needs, one per buffer size from the
    /// smallest, joined by commas as the source writes them.
    pub required_bandwidths: Option<String>,
    /// The columns below are what the scan read as ffprobe printed it, which the importer has
    /// no source for. The language as the file states it: null when it states none.
    pub declared_language: Option<String>,
    /// An MP4 track's own name, from its `udta`.
    pub name_tag: Option<String>,
    pub handler_name: Option<String>,
    /// `num/den`.
    pub time_base: Option<String>,
    /// As ffprobe prints it: `[0][0][0][0]` where the container names none.
    pub codec_tag: Option<String>,
    /// What the track's header declares, bits per second.
    pub declared_bitrate: Option<i64>,
    /// What the muxer measured and tagged, bits per second.
    pub tagged_bitrate: Option<i64>,
    /// `num/den`, as ffprobe states them.
    pub average_frame_rate: Option<String>,
    pub real_frame_rate: Option<String>,
    pub display_aspect_ratio: Option<String>,
    /// As ffprobe states it, after the container's crop; `pixel_aspect_ratio` is the
    /// reference's, before it, and written only for pixels that are not square.
    pub sample_aspect_ratio: Option<String>,
    /// Whether the container asks for lines or columns to be hidden.
    pub is_cropped: Option<bool>,
    /// ffmpeg's pixel format: `yuv420p10le`.
    pub pixel_format: Option<String>,
    pub is_avc: Option<bool>,
    pub nal_length_size: Option<i32>,
    /// The frame a picture subtitle is drawn for.
    pub canvas_width: Option<i32>,
    pub canvas_height: Option<i32>,
    /// A teletext track's subtitle pages, as ffprobe-plex prints them:
    /// `888:fra:hearing_impaired,889:fra:subtitles`.
    pub teletext_pages: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::media_part::Entity",
        from = "Column::MediaPartId",
        to = "super::media_part::Column::Id",
        on_delete = "Cascade"
    )]
    MediaPart,
    #[sea_orm(has_one = "super::stream_dolby_vision::Entity")]
    DolbyVision,
    #[sea_orm(has_one = "super::stream_loudness::Entity")]
    Loudness,
}

impl Related<super::media_part::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MediaPart.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
