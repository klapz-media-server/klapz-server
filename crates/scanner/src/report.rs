//! What reading a file finds. Values are measurements as the tools report them — ffprobe's
//! codec and format names, seconds turned into milliseconds, bits per second — and mapping
//! them onto Plex's vocabulary is ingestion's business. Serializable, so that a test replays a
//! captured analysis.

use serde::{Deserialize, Serialize};

/// Enough to recognize a file again, cheaply: what the reference compares before analyzing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIdentity {
    pub size: u64,
    /// Unix seconds.
    pub modified_at: i64,
    /// The SHA-1 of the size in decimal followed by the hex SHA-1s of the first 64 KiB and, for
    /// a larger file, of the last 64 KiB, in 40 hex digits: the hash the reference keeps for a
    /// part. ffprobe's (`format.hashes.content`), absent from an older ffmpeg-plex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// `OpenSubtitles`' hash: the size plus the 64-bit words of the first and last 64 KiB, in
    /// 16 hex digits. ffprobe's (`format.hashes.oshash`), absent from an older ffmpeg-plex and
    /// for a file under 128 KiB, as in the reference implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_subtitles_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub file: FileIdentity,
    /// ffprobe's `format_name`, such as `matroska,webm`.
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The same duration to the microsecond, as ffprobe prints it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_us: Option<u64>,
    /// The container's own bitrate, in bits per second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    /// MP4 only: the index (`moov`) comes before the media data, so playback can start before
    /// the whole file is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimized_for_streaming: Option<bool>,
    /// MP4 only: chunk offsets are 64-bit (`co64`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_64bit_offsets: Option<bool>,
    /// MPEG-TS only: 188 or 192 bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet_length: Option<u32>,
    pub tracks: Vec<Track>,
    pub chapters: Vec<Chapter>,
    /// Present after a deep analysis: rates measured over every packet rather than declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deep: Option<DeepAnalysis>,
}

impl Analysis {
    /// The packets' span when a deep analysis measured one, else the container's duration.
    pub fn measured_duration_ms(&self) -> Option<u64> {
        self.deep
            .as_ref()
            .and_then(|deep| deep.duration_ms)
            .or(self.duration_ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
    Attachment,
    Data,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    /// ffmpeg's stream index, what every other command's `--stream` names.
    pub index: u32,
    /// The container's own number for the track: an MP4 track ID, an MPEG-TS PID. Matroska's
    /// and AVI's are not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<u32>,
    pub kind: TrackKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// The container's codec identifier (a `FourCC`), when it carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_tag: Option<String>,
    /// ffprobe's `codec_tag_string` as printed: `[0][0][0][0]` where the container names none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_codec_tag: Option<String>,
    /// The unit of the track's timestamps, `num/den`: `1/1000` in Matroska.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The language as the file states it: Matroska's `LanguageBCP47` or `Language` element,
    /// English when it writes neither and none when it says `und`, or a `LANGUAGE` tag;
    /// elsewhere ffprobe's tag, `und` included. `language` is ffprobe-plex's, which writes
    /// `eng` for a Matroska track that says `und`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// An MP4 track's own name, from its `udta`: a title some muxers write there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The handler an MP4 track names, often the muxer's (`SoundHandler`), sometimes a title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler_name: Option<String>,
    /// Declared by the stream header, in bits per second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    /// Matroska's `BPS` statistics tag, which the muxer measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagged_bitrate: Option<u64>,
    /// Every statistics tag the muxer wrote, under their names old (`BPS-eng`) or new.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statistics: Option<Statistics>,
    pub disposition: Disposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoTrack>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioTrack>,
    /// An image subtitle's canvas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<SubtitleTrack>,
    /// The subtitle pages a DVB teletext track declares, as ffprobe-plex prints them: each
    /// page, its language and its kind, `888:fra:hearing_impaired,889:fra:subtitles`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teletext_pages: Option<String>,
    /// An attachment's MIME type and file name: a font, or a Matroska cover, which ffprobe
    /// lists as a picture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<Attachment>,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "each is an independent flag of the container, not a state machine"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Disposition {
    pub default: bool,
    pub forced: bool,
    pub hearing_impaired: bool,
    pub visual_impaired: bool,
    pub original: bool,
    pub dub: bool,
    pub comment: bool,
    /// A cover image stored as a video track.
    pub attached_picture: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTrack {
    pub width: u32,
    pub height: u32,
    /// As ffprobe reports it: for H.264, the size before cropping with ffmpeg-plex, the
    /// cropped one with a stock ffprobe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coded_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coded_height: Option<u32>,
    /// H.264 only: what its sequence parameter set says, where ffprobe's figures differ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_header: Option<SequenceHeader>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixel_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_primaries: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_transfer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chroma_location: Option<String>,
    /// `avg_frame_rate` evaluated, else `r_frame_rate`: 23.976023976… for 24000/1001, and the
    /// 25.011 the reference stores for an MP4 whose timestamps drift from its nominal 25.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<f64>,
    /// `avg_frame_rate` as written, `num/den`: `0/0` when ffprobe could not average one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_frame_rate: Option<String>,
    /// `r_frame_rate`, the lowest rate every timestamp falls on, as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_frame_rate: Option<String>,
    /// H.264 only: whether NAL units are prefixed by their length (the `avcC` framing of MP4 and
    /// Matroska) rather than by start codes (Annex B, MPEG-TS's).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_avc: Option<bool>,
    /// H.264 only: the bytes of that length prefix, 0 under Annex B.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nal_length_size: Option<u32>,
    /// `progressive`, `tt`, `bb`, `tb`, `bt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_order: Option<String>,
    /// As `num:den`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_aspect_ratio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_aspect_ratio: Option<String>,
    /// The container's crop, which ffprobe applies before stating the sample aspect ratio.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cropping: Option<Cropping>,
    pub closed_captions: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dolby_vision: Option<DolbyVision>,
}

/// Lines and columns a container asks a player to hide, from Matroska's `PixelCrop*`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cropping {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

/// An H.264 sequence parameter set, as ffmpeg-plex's ffprobe states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceHeader {
    /// In whole macroblocks, before cropping: 1088 for a 1080-line picture.
    pub coded_width: u32,
    pub coded_height: u32,
    /// `max_num_ref_frames`: how many pictures the encoder allowed itself to predict from.
    pub ref_frames: u32,
    /// `seq_scaling_matrix_present_flag`.
    pub has_scaling_matrix: bool,
}

/// The Dolby Vision configuration record, from the stream's side data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DolbyVision {
    pub version_major: u32,
    pub version_minor: u32,
    pub profile: u32,
    pub level: u32,
    pub rpu_present: bool,
    pub el_present: bool,
    pub bl_present: bool,
    pub bl_compatibility_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
    /// ffprobe's `channel_layout`, such as `5.1(side)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_layout: Option<String>,
    /// Lossless formats only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrack {
    pub width: u32,
    pub height: u32,
}

/// What a muxer measured of a track and wrote as tags: mkvmerge's `BPS`, `NUMBER_OF_BYTES` and
/// `DURATION`, or the same with `-eng` appended, as it named them before 2015.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statistics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_us: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    /// From 1, in the file's order.
    pub index: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Absent when the file names none, or only numbers them (*Chapter 3*), which the reference
    /// clears.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Where it starts to the microsecond, as ffprobe states it: `start_ms` keeps the
    /// reference's six significant digits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_us: Option<u64>,
    /// The title as the file writes it, numbered or not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepAnalysis {
    /// The time the packets span, from the first one's start to the last one's end, which the
    /// reference stores as the duration once it has measured it: 6031567 for Soul, whose
    /// header says 6031568.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Every packet's bytes over that span, in bits per second.
    pub bitrate: u64,
    pub tracks: Vec<MeasuredTrack>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredTrack {
    pub index: u32,
    pub packets: u64,
    pub bytes: u64,
    pub bitrate: u64,
}

/// EBU R 128 measurements of one audio track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loudness {
    pub track: u32,
    /// Integrated loudness, LUFS.
    pub integrated: f64,
    /// The relative gate the integrated value was measured above, LUFS.
    pub threshold: f64,
    /// Loudness range, LU.
    pub range: f64,
    pub range_low: f64,
    pub range_high: f64,
    /// True peak, dBFS.
    pub true_peak: f64,
}

/// Chromaprint's raw fingerprint of the start of one audio track, as `fpcalc -raw` prints it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub track: u32,
    /// `fpcalc`'s numbering: 2 is Chromaprint's default, what the reference stores.
    pub algorithm: u32,
    /// The audio actually fingerprinted, which the file may end before the length asked for.
    pub duration_ms: u64,
    /// One value every 4096 / 3 samples at 11,025 Hz, about 123.8 ms.
    pub item_duration_us: u64,
    pub values: Vec<u32>,
}

/// Files written into the directory the caller gave, each named relative to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Produced {
    pub files: Vec<ProducedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProducedFile {
    pub name: String,
    pub size: u64,
    pub role: FileRole,
}

/// What a produced file is, and what it was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileRole {
    /// An embedded subtitle track, extracted as is.
    #[serde(rename_all = "camelCase")]
    Subtitle { track: u32, codec: String },
    /// Roku's preview format: one JPEG every `interval_ms`.
    #[serde(rename_all = "camelCase")]
    PreviewIndex { frames: u32, interval_ms: u32 },
    /// A frame at the chapter's start.
    #[serde(rename_all = "camelCase")]
    ChapterThumb { chapter: u32, at_ms: u64 },
    /// A frame for a poster, fitted inside 720×512.
    #[serde(rename_all = "camelCase")]
    Thumb { at_ms: u64 },
    /// A frame for a backdrop, fitted inside 1280×720.
    #[serde(rename_all = "camelCase")]
    Art { at_ms: u64 },
    /// A picture the file carries: an attached picture or a Matroska image attachment.
    #[serde(rename_all = "camelCase")]
    Cover { track: u32 },
}
