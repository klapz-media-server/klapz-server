//! A file's tracks as Jellyfin's `MediaStream`s, labelled as the reference labels them.
//!
//! The rules are the reference's own (`ProbeResultNormalizer`, `MediaStream`), read off its
//! answers on the 35 files both servers scanned (`tools/compare_jellyfin.py`). It publishes what
//! ffprobe printed, which the official API keeps as `stated`: a scanned file's tracks are shaped
//! from it; an imported file's, which states nothing, from the values Plex's analysis stored.
//! Covers and data tracks are streams to Jellyfin too, listed among the tracks by index.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::language;

/// What the official API is asked of a track and of what else a file carries. A macro, so that
/// the item fragment can join it into one constant.
macro_rules! track_fields {
    () => {
        r"
fragment JellyfinTrack on MediaStream {
    id kind position codec language channels bitrate isDefault isForced isExternal title
    profile bitDepth estimatedBitrate
    stated { language name handlerName timeBase codecTag }
    video { width height frameRate level refFrames scan isAnamorphic chromaSubsampling
            hasHdr10Plus averageFrameRate realFrameRate displayAspectRatio sampleAspectRatio
            isCropped pixelFormat isAvc nalLengthSize
            color { space primaries transfer }
            dolbyVision { version profile level baseLayerCompatibility hasBaseLayer
                          hasEnhancementLayer hasRpu } }
    audio { samplingRate channelLayout }
    subtitle { canvasWidth canvasHeight }
    traits { isHearingImpaired isOriginal }
}
fragment JellyfinSideStream on SideStream {
    kind position codec codecTag profile language timeBase mimeType filename isDefault
    isForced
    picture { width height pixelFormat bitDepth colorSpace level realFrameRate
              averageFrameRate sampleAspectRatio displayAspectRatio isInterlaced }
}
"
    };
}
pub(crate) use track_fields as TRACK_FIELDS;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub kind: String,
    pub position: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<i32>,
    pub bitrate: Option<i64>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub title: Option<String>,
    pub profile: Option<String>,
    pub bit_depth: Option<i32>,
    pub estimated_bitrate: Option<i64>,
    #[serde(default)]
    pub stated: Stated,
    pub video: Option<VideoAnalysis>,
    pub audio: Option<AudioAnalysis>,
    pub subtitle: Option<SubtitleAnalysis>,
    pub traits: Traits,
}

/// What the file states of a track as ffprobe printed it; empty for an imported file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stated {
    pub language: Option<String>,
    pub name: Option<String>,
    pub handler_name: Option<String>,
    pub time_base: Option<String>,
    pub codec_tag: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoAnalysis {
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub frame_rate: Option<f64>,
    pub level: Option<i32>,
    pub ref_frames: Option<i32>,
    pub scan: Option<String>,
    pub is_anamorphic: Option<bool>,
    /// `4:2:0`, `4:2:2`…
    pub chroma_subsampling: Option<String>,
    #[serde(default)]
    pub has_hdr10_plus: bool,
    pub average_frame_rate: Option<String>,
    pub real_frame_rate: Option<String>,
    pub display_aspect_ratio: Option<String>,
    pub sample_aspect_ratio: Option<String>,
    pub is_cropped: Option<bool>,
    pub pixel_format: Option<String>,
    pub is_avc: Option<bool>,
    pub nal_length_size: Option<i32>,
    pub color: Color,
    pub dolby_vision: Option<DolbyVision>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Color {
    pub space: Option<String>,
    pub primaries: Option<String>,
    pub transfer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DolbyVision {
    /// `1.0`.
    pub version: String,
    pub profile: i32,
    pub level: i32,
    pub base_layer_compatibility: Option<i32>,
    pub has_base_layer: bool,
    pub has_enhancement_layer: bool,
    pub has_rpu: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioAnalysis {
    pub sampling_rate: Option<i32>,
    pub channel_layout: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleAnalysis {
    pub canvas_width: Option<i32>,
    pub canvas_height: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Traits {
    pub is_hearing_impaired: bool,
    pub is_original: bool,
}

/// A cover, a data track or an attached file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideStream {
    /// `COVER`, `DATA` or `ATTACHMENT`.
    pub kind: String,
    pub position: i32,
    pub codec: Option<String>,
    pub codec_tag: Option<String>,
    pub profile: Option<String>,
    pub language: Option<String>,
    pub time_base: Option<String>,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub picture: Option<SidePicture>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidePicture {
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub pixel_format: Option<String>,
    pub bit_depth: Option<i32>,
    pub color_space: Option<String>,
    pub level: Option<i32>,
    pub real_frame_rate: Option<String>,
    pub average_frame_rate: Option<String>,
    pub sample_aspect_ratio: Option<String>,
    pub display_aspect_ratio: Option<String>,
    pub is_interlaced: bool,
}

/// Jellyfin's codec name: ffprobe's, picture subtitles by the reference's own names. Plex's
/// importer renamed some (`dca`, `srt`, `pgs`, `vobsub`) and kept PCM's sample format as the
/// profile.
fn codec(track: &Track) -> Option<String> {
    let codec = track.codec.as_deref()?;
    if codec == "pcm" {
        if let Some(format) = track.profile.as_deref().filter(|p| p.starts_with("pcm_")) {
            return Some(format.to_owned());
        }
    }
    Some(
        match codec {
            "dca" => "dts",
            "srt" => "subrip",
            "pgs" | "hdmv_pgs_subtitle" => "PGSSUB",
            "dvb_subtitle" => "DVBSUB",
            "dvb_teletext" => "DVBTXT",
            "vobsub" | "dvd_subtitle" => "DVDSUB",
            other => other,
        }
        .to_owned(),
    )
}

/// Whether the reference reads a subtitle codec as text: any but the picture formats.
fn is_text_format(codec: &str) -> bool {
    let lower = codec.to_ascii_lowercase();
    lower.contains("microdvd")
        || !(lower.contains("pgs")
            || lower.contains("dvdsub")
            || lower.contains("vobsub")
            || lower.contains("dvbsub")
            || lower == "sup"
            || lower == "sub")
}

/// Whether the reference extracts a subtitle for a client to fetch: text, PGS and DVD
/// pictures, which it writes out as they are.
fn is_extractable(codec: &str) -> bool {
    let lower = codec.to_ascii_lowercase();
    is_text_format(codec)
        || lower.contains("pgs")
        || lower == "sup"
        || lower.contains("dvdsub")
        || lower.contains("vobsub")
}

/// The profile as ffprobe writes it, which the reference publishes: Plex's importer kept
/// some in lower case or by a short name (`lc`, `ma`). PCM's is its codec.
fn profile(codec: Option<&str>, profile: &str) -> Option<String> {
    Some(match (codec, profile.to_ascii_lowercase().as_str()) {
        (Some("pcm"), _) => return None,
        (_, "lc") => "LC".to_owned(),
        (_, "he-aac") => "HE-AAC".to_owned(),
        (_, "he-aacv2") => "HE-AACv2".to_owned(),
        (Some("dca" | "dts"), "ma") => "DTS-HD MA".to_owned(),
        (Some("dca" | "dts"), "hra") => "DTS-HD HRA".to_owned(),
        (Some("dca" | "dts"), "es") => "DTS-ES".to_owned(),
        (Some("dca" | "dts"), "x" | "ma + dts:x") => "DTS-HD MA + DTS:X".to_owned(),
        (Some("dca" | "dts"), "dts") => "DTS".to_owned(),
        (Some("mpeg4"), "advanced simple" | "simple") => {
            format!("{} Profile", capitalized(profile))
        }
        _ => capitalized(profile),
    })
}

/// `main 10` as ffprobe writes it, `Main 10`.
fn capitalized(text: &str) -> String {
    text.split(' ')
        .map(first_to_upper)
        .collect::<Vec<_>>()
        .join(" ")
}

/// .NET's `FirstToUpper`, which the reference applies to a language in a label.
fn first_to_upper(text: &str) -> String {
    let mut letters = text.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// An audio codec as labels name it.
fn friendly_audio_codec(codec: &str) -> String {
    match codec {
        "ac3" => "Dolby Digital".to_owned(),
        "eac3" => "Dolby Digital+".to_owned(),
        "dca" => "DTS".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

/// The reference's label for a picture's size (`GetResolutionText`).
fn resolution_label(width: i32, height: i32, interlaced: bool) -> Option<String> {
    const CLASSES: [(i32, i32, &str); 12] = [
        (256, 144, "144"),
        (426, 240, "240"),
        (640, 360, "360"),
        (682, 384, "384"),
        (720, 404, "404"),
        (854, 480, "480"),
        (960, 544, "540"),
        (1024, 576, "576"),
        (1280, 962, "720"),
        (2560, 1440, "1080"),
        (4096, 3072, "4K"),
        (8192, 6144, "8K"),
    ];
    let (_, _, class) = CLASSES
        .iter()
        .find(|(max_width, max_height, _)| width <= *max_width && height <= *max_height)?;
    Some(if class.ends_with('K') {
        (*class).to_owned()
    } else {
        format!("{class}{}", if interlaced { 'i' } else { 'p' })
    })
}

/// What the reference reads of a track's Dolby Vision record.
struct DolbyVisionFlags {
    profile: i32,
    compatibility: i32,
    rpu: bool,
    base_layer: bool,
}

impl DolbyVisionFlags {
    fn of(video: &VideoAnalysis) -> Option<Self> {
        video.dolby_vision.as_ref().map(|record| Self {
            profile: record.profile,
            compatibility: record.base_layer_compatibility.unwrap_or(0),
            rpu: record.has_rpu,
            base_layer: record.has_base_layer,
        })
    }

    /// `VideoDoViTitle`: `Dolby Vision Profile 8.1 (HDR10)`.
    fn title(&self) -> Option<String> {
        if !(self.rpu && self.base_layer && matches!(self.profile, 4 | 5 | 7 | 8 | 9 | 10)) {
            return None;
        }
        let title = if self.compatibility > 0 {
            format!(
                "Dolby Vision Profile {}.{}",
                self.profile, self.compatibility
            )
        } else {
            format!("Dolby Vision Profile {}", self.profile)
        };
        Some(match self.compatibility {
            1 | 6 => format!("{title} (HDR10)"),
            2 => format!("{title} (SDR)"),
            4 => format!("{title} (HLG)"),
            _ => title,
        })
    }
}

/// Jellyfin's dynamic range and its kind (`GetVideoColorRange`).
fn video_range(video: &VideoAnalysis, codec_tag: Option<&str>) -> (&'static str, &'static str) {
    let transfer = video.color.transfer.as_deref().unwrap_or_default();
    let pq = transfer.eq_ignore_ascii_case("smpte2084");
    let hlg = transfer.eq_ignore_ascii_case("arib-std-b67");
    let base = if pq || hlg { "HDR" } else { "SDR" };
    let dolby_vision = DolbyVisionFlags::of(video);
    let flagged = dolby_vision.as_ref().is_some_and(|flags| {
        matches!(flags.profile, 5 | 7 | 8 | 10)
            && flags.rpu
            && flags.base_layer
            && matches!(flags.compatibility, 0 | 1 | 2 | 4 | 6)
    });
    let tagged = codec_tag.is_some_and(|tag| {
        ["dovi", "dvh1", "dvhe", "dav1"]
            .iter()
            .any(|dolby| tag.eq_ignore_ascii_case(dolby))
    });
    if flagged || tagged {
        let profile = dolby_vision.as_ref().map_or(0, |flags| flags.profile);
        let compatibility = dolby_vision.as_ref().map_or(0, |flags| flags.compatibility);
        let set = match (profile, compatibility) {
            (5, _) | (10, 0) => ("HDR", "DOVI"),
            (8 | 10, 1) => ("HDR", "DOVIWithHDR10"),
            (8 | 10, 4) => ("HDR", "DOVIWithHLG"),
            (8 | 10, 2) => ("SDR", "DOVIWithSDR"),
            (8 | 10, _) => (base, "DOVIInvalid"),
            (7, _) => ("HDR", "DOVIWithEL"),
            _ => ("SDR", "SDR"),
        };
        let expected = match set.1 {
            "DOVIWithHDR10" | "DOVIWithEL" => Some("smpte2084"),
            "DOVIWithHLG" => Some("arib-std-b67"),
            _ => None,
        };
        let is = |value: &Option<String>, wanted: &str| {
            value
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(wanted))
        };
        if let Some(expected) = expected {
            if !(is(&video.color.space, "bt2020nc")
                && is(&video.color.transfer, expected)
                && is(&video.color.primaries, "bt2020"))
            {
                return (base, "DOVIInvalid");
            }
        }
        if video.has_hdr10_plus {
            return match set.1 {
                "DOVIWithHDR10" => ("HDR", "DOVIWithHDR10Plus"),
                "DOVIWithEL" => ("HDR", "DOVIWithELHDR10Plus"),
                _ => set,
            };
        }
        return set;
    }
    match (pq, hlg, video.has_hdr10_plus) {
        (true, _, true) => ("HDR", "HDR10Plus"),
        (true, _, false) => ("HDR", "HDR10"),
        (_, true, _) => ("HDR", "HLG"),
        _ => ("SDR", "SDR"),
    }
}

/// A track's name first, then each attribute it does not already hold; without a name, the
/// attributes alone.
fn label(title: Option<&str>, attributes: &[String], separator: &str) -> String {
    match title.filter(|title| !title.is_empty()) {
        Some(title) => {
            let lowered = title.to_lowercase();
            let mut label = title.to_owned();
            for attribute in attributes {
                if !lowered.contains(&attribute.to_lowercase()) {
                    label.push_str(" - ");
                    label.push_str(attribute);
                }
            }
            label
        }
        None => attributes.join(separator),
    }
}

/// ffmpeg's layout, as the reference publishes it: `5.1(side)` is `5.1`.
fn channel_layout(layout: &str) -> &str {
    layout.split_once('(').map_or(layout, |(left, _)| left)
}

/// The audio's spatial format, which the reference reads from the profile.
fn spatial_format(profile: Option<&str>) -> &'static str {
    let profile = profile.unwrap_or_default().to_ascii_lowercase();
    if profile.contains("dolby atmos") {
        "DolbyAtmos"
    } else if profile.contains("dts:x") {
        "DTSX"
    } else {
        "None"
    }
}

fn ratio(text: &str, separator: char) -> Option<(i64, i64)> {
    let (numerator, denominator) = text.split_once(separator)?;
    Some((
        numerator.trim().parse().ok()?,
        denominator.trim().parse().ok()?,
    ))
}

/// A picture's shape as the reference writes it (`GetAspectRatio`): the display ratio ffprobe
/// states, else the size, snapped to the common ratios by name; otherwise the stated ratio.
fn aspect_ratio(display: Option<&str>, width: Option<i32>, height: Option<i32>) -> Option<String> {
    let (across, down) = match display.and_then(|display| ratio(display, ':')) {
        Some((across, down)) if across > 0 && down > 0 => (across, down),
        _ => match (width, height) {
            (Some(width), Some(height)) if width > 0 && height > 0 => {
                (i64::from(width), i64::from(height))
            }
            _ => return display.map(ToOwned::to_owned),
        },
    };
    #[allow(
        clippy::cast_precision_loss,
        reason = "a picture's sides are far below 2^52"
    )]
    let shape = across as f64 / down as f64;
    let close = |to: f64, within: f64| (shape - to).abs() <= within;
    let named = if close(1.777_777_778, 0.03) {
        "16:9"
    } else if close(1.333_333_333_3, 0.05) {
        "4:3"
    } else if close(1.41, 0.005) {
        "1.41:1"
    } else if close(1.5, 0.005) {
        "1.5:1"
    } else if close(1.6, 0.005) {
        "1.6:1"
    } else if close(1.666_666_666_67, 0.005) {
        "5:3"
    } else if close(1.85, 0.02) {
        "1.85:1"
    } else if close(2.35, 0.025) {
        "2.35:1"
    } else if close(2.4, 0.025) {
        "2.40:1"
    } else {
        return display.map(ToOwned::to_owned);
    };
    Some(named.to_owned())
}

/// Whether the reference calls a picture anamorphic, from the ratios ffprobe states: pixels
/// more than 1% off square, unless the container crops the picture.
fn is_anamorphic(video: &VideoAnalysis) -> bool {
    if video.is_cropped == Some(true) {
        return false;
    }
    let sample = video.sample_aspect_ratio.as_deref();
    let display = video.display_aspect_ratio.as_deref();
    match (sample, display) {
        (Some(sample), _) if is_near_square(sample) => false,
        (Some(sample), _) if sample != "0:1" => true,
        (None, None) | (_, Some("0:1")) => false,
        (_, display) => display != aspect_ratio(None, video.width, video.height).as_deref(),
    }
}

fn is_near_square(sample: &str) -> bool {
    #[allow(
        clippy::cast_precision_loss,
        reason = "a sample aspect ratio's terms are far below 2^52"
    )]
    match ratio(sample, ':') {
        Some((numerator, denominator)) if denominator > 0 => {
            (numerator as f64 / denominator as f64 - 1.0).abs() <= 0.01
        }
        _ => sample == "1:1",
    }
}

/// A frame rate as the reference holds it: a 32-bit float of ffprobe's ratio, written with the
/// digits that float needs (`23.976025`). `0/0`, no rate, is none.
fn frame_rate(text: &str) -> Option<f64> {
    let (numerator, denominator) = ratio(text, '/')?;
    if denominator == 0 {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "the reference divides 32-bit floats, which is the precision it publishes"
    )]
    let rate = numerator as f32 / denominator as f32;
    format!("{rate}").parse().ok()
}

/// A rate known only as measured, from an imported file, written as a 32-bit float as well.
fn measured_rate(rate: f64) -> f64 {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the reference holds frame rates as 32-bit floats"
    )]
    let narrow = rate as f32;
    format!("{narrow}").parse().unwrap_or(rate)
}

/// A file's tracks and what else it carries, as the reference's `MediaStream`s.
pub struct FileStreams<'a> {
    pub tracks: &'a [Track],
    pub side_streams: &'a [SideStream],
    /// Whether the scan read the file, so that its tracks state what ffprobe printed.
    pub scanned: bool,
}

impl FileStreams<'_> {
    /// Every stream by index; sidecar subtitles last.
    pub fn media_streams(&self) -> Vec<Value> {
        let mut streams: Vec<(bool, i32, Value)> = self
            .tracks
            .iter()
            .map(|track| (track.is_external, track.position, self.media_stream(track)))
            .collect();
        streams.extend(
            self.side_streams
                .iter()
                .filter(|side| side.kind != "ATTACHMENT")
                .map(|side| (false, side.position, side_stream(side))),
        );
        streams.sort_by_key(|(external, position, _)| (*external, *position));
        // A teletext stream is a track per page, all at its index, which is how a Jellyfin
        // player names the subtitle it wants: the first page stands for the stream.
        streams.dedup_by(|later, first| !later.0 && !first.0 && later.1 == first.1);
        streams.into_iter().map(|(_, _, stream)| stream).collect()
    }

    /// The reference's `MediaAttachments`: attached files and covers, by index.
    pub fn media_attachments(&self) -> Vec<Value> {
        self.side_streams
            .iter()
            .filter(|side| side.kind != "DATA")
            .map(|side| {
                let mut attachment = Map::new();
                if let Some(codec) = &side.codec {
                    attachment.insert("Codec".to_owned(), json!(codec));
                }
                if let Some(tag) = side
                    .codec_tag
                    .as_deref()
                    .filter(|tag| !tag.trim().is_empty())
                {
                    attachment.insert("CodecTag".to_owned(), json!(tag));
                }
                attachment.insert("Index".to_owned(), json!(side.position));
                if let Some(filename) = &side.filename {
                    attachment.insert("FileName".to_owned(), json!(filename));
                }
                if let Some(mime_type) = &side.mime_type {
                    attachment.insert("MimeType".to_owned(), json!(mime_type));
                }
                Value::Object(attachment)
            })
            .collect()
    }

    /// The language a track is published with: what the file states, as a terminological
    /// code (`fre` is `fra`); an imported track's from Plex's analysis.
    fn language(&self, track: &Track) -> Option<String> {
        if self.scanned && !track.is_external {
            return track
                .stated
                .language
                .as_deref()
                .map(|stated| language::three_letters(stated).to_owned());
        }
        track
            .language
            .as_deref()
            .map(|stored| language::three_letters(stored).to_owned())
    }

    /// The track's name, and for sound and subtitles an MP4 handler's when it names one.
    fn title(track: &Track, kind: &str) -> Option<String> {
        let handler = |default: &str| {
            track
                .stated
                .handler_name
                .clone()
                .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case(default))
        };
        let title = track.title.clone().filter(|title| !title.is_empty());
        // MP4 keeps a track's name apart from a title; the reference reads it for sound and
        // subtitles, then their handler's.
        let named = || track.stated.name.clone().filter(|name| !name.is_empty());
        let title = match kind {
            "Audio" => title.or_else(named).or_else(|| handler("SoundHandler")),
            "Subtitle" => title.or_else(named).or_else(|| handler("SubtitleHandler")),
            _ => title,
        };
        title.filter(|title| !title.eq_ignore_ascii_case("cc"))
    }

    fn bitrate(&self, track: &Track) -> Option<i64> {
        if self.scanned {
            track.estimated_bitrate
        } else {
            track.bitrate
        }
    }

    /// One of our tracks as a Jellyfin `MediaStream`.
    #[allow(
        clippy::too_many_lines,
        reason = "one field after another, in the reference's order"
    )]
    fn media_stream(&self, track: &Track) -> Value {
        let kind = match track.kind.as_str() {
            "VIDEO" => "Video",
            "AUDIO" => "Audio",
            _ => "Subtitle",
        };
        let language = self.language(track);
        let title = Self::title(track, kind);
        let codec = codec(track);
        let video = track.video.as_ref();
        let codec_tag = track
            .stated
            .codec_tag
            .as_deref()
            .filter(|tag| !tag.trim().is_empty() && !tag.contains("[0]"));
        let profile = track
            .profile
            .as_deref()
            .and_then(|named| profile(track.codec.as_deref(), named));
        let mut stream = Map::new();
        let mut put = |key: &str, value: Value| {
            stream.insert(key.to_owned(), value);
        };
        if let Some(codec) = &codec {
            put("Codec", json!(codec));
        }
        if let Some(tag) = codec_tag {
            put("CodecTag", json!(tag));
        }
        if let Some(language) = &language {
            put("Language", json!(language));
        }
        if let Some(time_base) = &track.stated.time_base {
            put("TimeBase", json!(time_base));
        }
        if let Some(title) = &title {
            put("Title", json!(title));
        }
        let (range, range_type) = video.map_or(("Unknown", "Unknown"), |video| {
            video_range(video, track.stated.codec_tag.as_deref())
        });
        put("VideoRange", json!(range));
        put("VideoRangeType", json!(range_type));
        let dolby_vision = video.and_then(DolbyVisionFlags::of);
        let dolby_vision_title = dolby_vision.as_ref().and_then(DolbyVisionFlags::title);
        if let Some(title) = &dolby_vision_title {
            put("VideoDoViTitle", json!(title));
        }
        put(
            "AudioSpatialFormat",
            json!(if kind == "Audio" {
                spatial_format(profile.as_deref())
            } else {
                "None"
            }),
        );
        let localized = language.as_deref().and_then(language::localized_name);
        if kind != "Video" {
            put("LocalizedDefault", json!("Default"));
            put("LocalizedExternal", json!("External"));
            if let Some(name) = localized {
                put("LocalizedLanguage", json!(name));
            }
        }
        if kind == "Audio" {
            put("LocalizedOriginal", json!("Original"));
        }
        if kind == "Subtitle" {
            put("LocalizedUndefined", json!("Undefined"));
            put("LocalizedForced", json!("Forced"));
            put("LocalizedHearingImpaired", json!("Hearing Impaired"));
        }
        put(
            "DisplayTitle",
            json!(display_title(
                track,
                kind,
                &Labels {
                    codec: codec.as_deref(),
                    language: language.as_deref(),
                    localized,
                    title: title.as_deref(),
                    profile: profile.as_deref(),
                    range,
                    dolby_vision: dolby_vision_title.as_deref(),
                }
            )),
        );
        if let Some(size) = video.and_then(|video| video.nal_length_size) {
            put("NalLengthSize", json!(size.to_string()));
        }
        let interlaced = video.is_some_and(|video| video.scan.as_deref() == Some("INTERLACED"));
        put("IsInterlaced", json!(interlaced));
        if let Some(avc) = video.and_then(|video| video.is_avc) {
            put("IsAVC", json!(avc));
        }
        if let Some(layout) = track
            .audio
            .as_ref()
            .and_then(|audio| audio.channel_layout.as_deref())
        {
            put("ChannelLayout", json!(channel_layout(layout)));
        }
        if let Some(bitrate) = self.bitrate(track) {
            put("BitRate", json!(bitrate));
        }
        if let Some(depth) = track.bit_depth {
            put("BitDepth", json!(depth));
        }
        if let Some(frames) = video.and_then(|video| video.ref_frames) {
            put("RefFrames", json!(frames));
        }
        if let Some(channels) = track.channels {
            put("Channels", json!(channels));
        }
        if let Some(rate) = track.audio.as_ref().and_then(|audio| audio.sampling_rate) {
            put("SampleRate", json!(rate));
        }
        put("IsDefault", json!(track.is_default));
        put("IsForced", json!(track.is_forced));
        put("IsHearingImpaired", json!(track.traits.is_hearing_impaired));
        put("IsOriginal", json!(track.traits.is_original));
        let (width, height) = match (video, &track.subtitle) {
            (Some(video), _) => (video.width, video.height),
            (None, Some(canvas)) => (canvas.canvas_width, canvas.canvas_height),
            (None, None) => (None, None),
        };
        if let Some(height) = height {
            put("Height", json!(height));
        }
        if let Some(width) = width {
            put("Width", json!(width));
        }
        if let Some(video) = video {
            let average = match &video.average_frame_rate {
                Some(stated) => frame_rate(stated),
                None => video.frame_rate.map(measured_rate),
            };
            let real = match &video.real_frame_rate {
                Some(stated) => frame_rate(stated),
                None => video.frame_rate.map(measured_rate),
            };
            if let Some(rate) = average {
                put("AverageFrameRate", json!(rate));
            }
            if let Some(rate) = real {
                put("RealFrameRate", json!(rate));
            }
            // The reference falls back to the real rate past 1000, a rate no video has.
            if let Some(rate) = average.filter(|rate| *rate < 1000.0).or(real) {
                put("ReferenceFrameRate", json!(rate));
            }
        }
        if let Some(named) = &profile {
            put("Profile", json!(named));
        }
        put("Type", json!(kind));
        if let Some(video) = video {
            if let Some(shape) = aspect_ratio(
                video.display_aspect_ratio.as_deref(),
                video.width,
                video.height,
            ) {
                put("AspectRatio", json!(shape));
            }
        }
        put("Index", json!(track.position));
        put("IsExternal", json!(track.is_external));
        let text = kind == "Subtitle" && codec.as_deref().is_some_and(is_text_format);
        put("IsTextSubtitleStream", json!(text));
        put(
            "SupportsExternalStream",
            json!(kind == "Subtitle" && codec.as_deref().is_some_and(is_extractable)),
        );
        if let Some(video) = video {
            for (key, value) in [
                ("ColorSpace", &video.color.space),
                ("ColorTransfer", &video.color.transfer),
                ("ColorPrimaries", &video.color.primaries),
            ] {
                if let Some(value) = value {
                    put(key, json!(value));
                }
            }
            let pixel_format = video
                .pixel_format
                .clone()
                .or_else(|| pixel_format(video.chroma_subsampling.as_deref(), track.bit_depth));
            if let Some(format) = pixel_format {
                put("PixelFormat", json!(format));
            }
            // ffprobe's `-99` is a level it does not know, which the reference publishes.
            put("Level", json!(video.level.unwrap_or(-99)));
            let anamorphic = if video.sample_aspect_ratio.is_some()
                || video.display_aspect_ratio.is_some()
                || video.is_cropped.is_some()
            {
                is_anamorphic(video)
            } else {
                video.is_anamorphic.unwrap_or(false)
            };
            put("IsAnamorphic", json!(anamorphic));
            if let Some(record) = &video.dolby_vision {
                let (major, minor) = record
                    .version
                    .split_once('.')
                    .unwrap_or((&record.version, "0"));
                put("DvVersionMajor", json!(major.parse::<i32>().unwrap_or(0)));
                put("DvVersionMinor", json!(minor.parse::<i32>().unwrap_or(0)));
                put("DvProfile", json!(record.profile));
                put("DvLevel", json!(record.level));
                put("RpuPresentFlag", json!(i32::from(record.has_rpu)));
                put(
                    "ElPresentFlag",
                    json!(i32::from(record.has_enhancement_layer)),
                );
                put("BlPresentFlag", json!(i32::from(record.has_base_layer)));
                put(
                    "DvBlSignalCompatibilityId",
                    json!(record.base_layer_compatibility.unwrap_or(0)),
                );
            }
        }
        Value::Object(stream)
    }
}

/// What a label is made of, beside the track itself.
struct Labels<'a> {
    codec: Option<&'a str>,
    language: Option<&'a str>,
    localized: Option<&'a str>,
    title: Option<&'a str>,
    profile: Option<&'a str>,
    range: &'a str,
    dolby_vision: Option<&'a str>,
}

impl Labels<'_> {
    /// The language as labels show it: its name, else the code with its first letter raised.
    fn language(&self) -> Option<String> {
        let code = self.language?;
        Some(first_to_upper(self.localized.unwrap_or(code)))
    }
}

/// `DisplayTitle`, as the reference composes it for each kind.
fn display_title(track: &Track, kind: &str, labels: &Labels<'_>) -> String {
    let mut attributes = Vec::new();
    match kind {
        "Video" => {
            let video = track.video.clone().unwrap_or_default();
            let interlaced = video.scan.as_deref() == Some("INTERLACED");
            if let (Some(width), Some(height)) = (video.width, video.height) {
                attributes.extend(resolution_label(width, height, interlaced));
            }
            if let Some(codec) = labels.codec {
                attributes.push(codec.to_ascii_uppercase());
            }
            match labels.dolby_vision {
                Some(title) => attributes.push(title.to_owned()),
                None => attributes.push(labels.range.to_owned()),
            }
            label(labels.title, &attributes, " ")
        }
        "Audio" => {
            // The codes that name no language are left out of labels.
            if !matches!(labels.language, None | Some("und" | "mul" | "zxx" | "mis")) {
                attributes.extend(labels.language());
            }
            match (labels.profile, labels.codec) {
                (Some(profile), _) if !profile.eq_ignore_ascii_case("lc") => {
                    attributes.push(profile.to_owned());
                }
                (_, Some(codec)) => attributes.push(friendly_audio_codec(codec)),
                _ => {}
            }
            let layout = track
                .audio
                .as_ref()
                .and_then(|audio| audio.channel_layout.as_deref())
                .map(channel_layout);
            match (layout, track.channels) {
                (Some(layout), _) => attributes.push(first_to_upper(layout)),
                (None, Some(channels)) => attributes.push(format!("{channels} ch")),
                (None, None) => {}
            }
            if track.is_default {
                attributes.push("Default".to_owned());
            }
            if track.is_external {
                attributes.push("External".to_owned());
            }
            if track.traits.is_original {
                attributes.push("Original".to_owned());
            }
            label(labels.title, &attributes, " - ")
        }
        _ => {
            attributes.push(labels.language().unwrap_or_else(|| "Undefined".to_owned()));
            if track.traits.is_hearing_impaired {
                attributes.push("Hearing Impaired".to_owned());
            }
            if track.is_default {
                attributes.push("Default".to_owned());
            }
            if track.is_forced {
                attributes.push("Forced".to_owned());
            }
            if let Some(codec) = labels.codec {
                attributes.push(codec.to_ascii_uppercase());
            }
            if track.is_external {
                attributes.push("External".to_owned());
            }
            label(labels.title, &attributes, " - ")
        }
    }
}

/// A cover or a data track as the reference lists it among the streams.
fn side_stream(side: &SideStream) -> Value {
    let cover = side.kind == "COVER";
    let mut stream = Map::new();
    let mut put = |key: &str, value: Value| {
        stream.insert(key.to_owned(), value);
    };
    if let Some(codec) = &side.codec {
        put("Codec", json!(codec));
    }
    if let Some(tag) = side
        .codec_tag
        .as_deref()
        .filter(|tag| !tag.trim().is_empty() && !tag.contains("[0]"))
    {
        put("CodecTag", json!(tag));
    }
    if let Some(language) = &side.language {
        put("Language", json!(language::three_letters(language)));
    }
    let picture = side.picture.clone().unwrap_or_default();
    if let (true, Some(space)) = (cover, &picture.color_space) {
        put("ColorSpace", json!(space));
    }
    if let Some(time_base) = &side.time_base {
        put("TimeBase", json!(time_base));
    }
    put("VideoRange", json!("Unknown"));
    put("VideoRangeType", json!("Unknown"));
    put("AudioSpatialFormat", json!("None"));
    put("IsInterlaced", json!(cover && picture.is_interlaced));
    if cover {
        if let Some(depth) = picture.bit_depth {
            put("BitDepth", json!(depth));
        }
    }
    put("IsDefault", json!(side.is_default));
    put("IsForced", json!(side.is_forced));
    put("IsHearingImpaired", json!(false));
    put("IsOriginal", json!(false));
    if cover {
        if let Some(height) = picture.height {
            put("Height", json!(height));
        }
        if let Some(width) = picture.width {
            put("Width", json!(width));
        }
        let average = picture.average_frame_rate.as_deref().and_then(frame_rate);
        let real = picture.real_frame_rate.as_deref().and_then(frame_rate);
        if let Some(rate) = average {
            put("AverageFrameRate", json!(rate));
        }
        if let Some(rate) = real {
            put("RealFrameRate", json!(rate));
        }
        if let Some(rate) = average.filter(|rate| *rate < 1000.0).or(real) {
            put("ReferenceFrameRate", json!(rate));
        }
        if let Some(profile) = &side.profile {
            put("Profile", json!(profile));
        }
    }
    put("Type", json!(if cover { "EmbeddedImage" } else { "Data" }));
    if cover {
        if let Some(shape) = aspect_ratio(
            picture.display_aspect_ratio.as_deref(),
            picture.width,
            picture.height,
        ) {
            put("AspectRatio", json!(shape));
        }
    }
    put("Index", json!(side.position));
    put("IsExternal", json!(false));
    put("IsTextSubtitleStream", json!(false));
    put("SupportsExternalStream", json!(false));
    if cover {
        if let Some(format) = &picture.pixel_format {
            put("PixelFormat", json!(format));
        }
        put("Level", json!(picture.level.unwrap_or(-99)));
        let anamorphic = is_anamorphic(&VideoAnalysis {
            width: picture.width,
            height: picture.height,
            sample_aspect_ratio: picture.sample_aspect_ratio.clone(),
            display_aspect_ratio: picture.display_aspect_ratio.clone(),
            ..VideoAnalysis::default()
        });
        put("IsAnamorphic", json!(anamorphic));
    }
    Value::Object(stream)
}

/// ffmpeg's pixel format for a chroma subsampling and a bit depth, for an imported track, which
/// states none: `yuv420p`, `yuv420p10le`.
fn pixel_format(subsampling: Option<&str>, bit_depth: Option<i32>) -> Option<String> {
    let base = match subsampling? {
        "4:2:0" => "yuv420p",
        "4:2:2" => "yuv422p",
        "4:4:4" => "yuv444p",
        _ => return None,
    };
    Some(match bit_depth {
        Some(depth) if depth > 8 => format!("{base}{depth}le"),
        _ => base.to_owned(),
    })
}

/// How the reference ranks a subtitle for a user who set no preference, as its streams carry
/// it (`GetStreamScore`): five digits after a leading 1, each 2 for a flag the stream has —
/// forced, default, extractable, text, external.
fn subtitle_score(stream: &Value) -> i64 {
    let flag = |key: &str| if stream[key] == true { 2 } else { 1 };
    [
        "IsForced",
        "IsDefault",
        "SupportsExternalStream",
        "IsTextSubtitleStream",
        "IsExternal",
    ]
    .iter()
    .fold(1, |score, key| score * 10 + flag(key))
}

/// Sets the score the reference gives the subtitles its default mode would play — external,
/// default or forced ones — for a user with no language preference, which is every user here:
/// none is kept.
pub fn score_subtitles(streams: &mut [Value]) {
    for stream in streams.iter_mut() {
        let candidate = stream["Type"] == "Subtitle"
            && (stream["IsExternal"] == true
                || stream["IsDefault"] == true
                || stream["IsForced"] == true);
        if candidate {
            let score = subtitle_score(stream);
            stream["Score"] = json!(score);
        }
    }
}

/// The audio the reference plays by default for a user without preferences: the best-scored
/// default track, else the best-scored one, ties in index order.
pub fn default_audio_index(streams: &[Value]) -> Option<i64> {
    let audio: Vec<&Value> = streams
        .iter()
        .filter(|stream| stream["Type"] == "Audio")
        .collect();
    let score = |stream: &Value| {
        let flag = |key: &str| i64::from(stream[key] == true);
        flag("IsForced") * 100 + flag("IsDefault") * 10 + flag("IsExternal")
    };
    let mut ranked = audio.clone();
    ranked.sort_by_key(|stream| std::cmp::Reverse(score(stream)));
    ranked
        .iter()
        .find(|stream| stream["IsDefault"] == true)
        .or_else(|| ranked.first())
        .and_then(|stream| stream["Index"].as_i64())
}

/// The subtitle the reference's default mode plays: external first, then default, then forced
/// (`GetDefaultSubtitleStreamIndex`, no preferred language).
pub fn default_subtitle_index(streams: &[Value]) -> Option<i64> {
    let flag = |stream: &Value, key: &str| stream[key] == true;
    let mut subtitles: Vec<&Value> = streams
        .iter()
        .filter(|stream| stream["Type"] == "Subtitle")
        .collect();
    subtitles.sort_by_key(|stream| {
        std::cmp::Reverse((
            flag(stream, "IsExternal"),
            flag(stream, "IsDefault"),
            !flag(stream, "IsForced"),
            flag(stream, "IsForced"),
        ))
    });
    subtitles
        .into_iter()
        .find(|stream| {
            flag(stream, "IsExternal") || flag(stream, "IsDefault") || flag(stream, "IsForced")
        })
        .and_then(|stream| stream["Index"].as_i64())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn track(kind: &str, codec: &str) -> Track {
        Track {
            id: klapz_api::GlobalId::encode("MediaStream", 1),
            kind: kind.to_owned(),
            position: 0,
            codec: Some(codec.to_owned()),
            language: None,
            channels: None,
            bitrate: None,
            is_default: false,
            is_forced: false,
            is_external: false,
            title: None,
            profile: None,
            bit_depth: None,
            estimated_bitrate: None,
            stated: Stated::default(),
            video: None,
            audio: None,
            subtitle: None,
            traits: Traits::default(),
        }
    }

    fn video(width: i32, height: i32) -> VideoAnalysis {
        VideoAnalysis {
            width: Some(width),
            height: Some(height),
            frame_rate: Some(23.976),
            level: Some(41),
            ref_frames: Some(4),
            scan: Some("PROGRESSIVE".to_owned()),
            chroma_subsampling: Some("4:2:0".to_owned()),
            ..VideoAnalysis::default()
        }
    }

    fn audio(language: &str, codec: &str, layout: &str) -> Track {
        let mut audio = track("AUDIO", codec);
        audio.language = Some(language.to_owned());
        audio.audio = Some(AudioAnalysis {
            sampling_rate: Some(48_000),
            channel_layout: Some(layout.to_owned()),
        });
        audio
    }

    /// One track of an imported file, which states nothing.
    fn imported(track: &Track) -> Value {
        FileStreams {
            tracks: std::slice::from_ref(track),
            side_streams: &[],
            scanned: false,
        }
        .media_stream(track)
    }

    /// One track of a scanned file.
    fn scanned(track: &Track) -> Value {
        FileStreams {
            tracks: std::slice::from_ref(track),
            side_streams: &[],
            scanned: true,
        }
        .media_stream(track)
    }

    #[test]
    fn a_video_is_labelled_by_size_codec_and_range_as_the_reference() {
        for (width, height, scan, label) in [
            (720, 480, "PROGRESSIVE", "480p H264 SDR"),
            (720, 404, "PROGRESSIVE", "404p H264 SDR"),
            (704, 396, "PROGRESSIVE", "404p H264 SDR"),
            (384, 284, "PROGRESSIVE", "360p H264 SDR"),
            (1920, 808, "PROGRESSIVE", "1080p H264 SDR"),
            (1440, 1080, "INTERLACED", "1080i H264 SDR"),
            (720, 576, "INTERLACED", "576i H264 SDR"),
            (1920, 1088, "PROGRESSIVE", "1080p H264 SDR"),
            (1280, 960, "PROGRESSIVE", "720p H264 SDR"),
            (3840, 2160, "PROGRESSIVE", "4K H264 SDR"),
        ] {
            let mut sdr = track("VIDEO", "h264");
            let mut analysis = video(width, height);
            analysis.scan = Some(scan.to_owned());
            sdr.video = Some(analysis);
            assert_eq!(imported(&sdr)["DisplayTitle"], label, "{width}x{height}");
        }
    }

    #[test]
    fn a_named_video_keeps_its_name_first() {
        let mut named = track("VIDEO", "hevc");
        named.title = Some("E20 - 1080p".to_owned());
        named.video = Some(video(1440, 1080));
        assert_eq!(imported(&named)["DisplayTitle"], "E20 - 1080p - HEVC - SDR");
    }

    #[test]
    fn hdr_and_dolby_vision_are_named_as_the_reference_names_them() {
        let mut hdr = track("VIDEO", "hevc");
        let mut analysis = video(1920, 1080);
        analysis.color = Color {
            space: Some("bt2020nc".to_owned()),
            primaries: Some("bt2020".to_owned()),
            transfer: Some("smpte2084".to_owned()),
        };
        hdr.video = Some(analysis.clone());
        let stream = imported(&hdr);
        assert_eq!(stream["VideoRange"], "HDR");
        assert_eq!(stream["VideoRangeType"], "HDR10");
        assert_eq!(stream["DisplayTitle"], "1080p HEVC HDR");
        analysis.dolby_vision = Some(DolbyVision {
            version: "1.0".to_owned(),
            profile: 8,
            level: 6,
            base_layer_compatibility: Some(1),
            has_base_layer: true,
            has_enhancement_layer: false,
            has_rpu: true,
        });
        hdr.video = Some(analysis.clone());
        let stream = imported(&hdr);
        assert_eq!(stream["VideoRangeType"], "DOVIWithHDR10");
        assert_eq!(stream["VideoDoViTitle"], "Dolby Vision Profile 8.1 (HDR10)");
        assert_eq!(
            stream["DisplayTitle"],
            "1080p HEVC Dolby Vision Profile 8.1 (HDR10)"
        );
        assert_eq!(stream["DvVersionMajor"], 1);
        assert_eq!(stream["DvLevel"], 6);
        assert_eq!(stream["RpuPresentFlag"], 1);
        assert_eq!(stream["ElPresentFlag"], 0);
        assert_eq!(stream["DvBlSignalCompatibilityId"], 1);
        analysis.has_hdr10_plus = true;
        hdr.video = Some(analysis.clone());
        assert_eq!(imported(&hdr)["VideoRangeType"], "DOVIWithHDR10Plus");
        // A record whose base layer is not the BT.2020 PQ it claims is invalid.
        analysis.color.primaries = Some("bt709".to_owned());
        hdr.video = Some(analysis);
        assert_eq!(imported(&hdr)["VideoRangeType"], "DOVIInvalid");
    }

    #[test]
    fn an_audio_is_labelled_by_language_codec_layout_and_default() {
        let mut english = audio("en", "opus", "stereo");
        english.is_default = true;
        let stream = imported(&english);
        assert_eq!(stream["DisplayTitle"], "English - OPUS - Stereo - Default");
        assert_eq!(stream["Language"], "eng");
        assert_eq!(stream["LocalizedLanguage"], "English");
        assert_eq!(stream["LocalizedOriginal"], "Original");
        assert!(stream.get("IsAVC").is_none());
        assert!(stream.get("LocalizedForced").is_none());
        assert!(stream.get("Level").is_none());
    }

    #[test]
    fn a_profile_or_a_friendly_codec_names_the_audio() {
        let mut lc = audio("fr", "aac", "stereo");
        lc.profile = Some("lc".to_owned());
        assert_eq!(imported(&lc)["DisplayTitle"], "French - AAC - Stereo");
        assert_eq!(imported(&lc)["Profile"], "LC");
        let mut he = audio("de", "aac", "stereo");
        he.profile = Some("he-aac".to_owned());
        assert_eq!(imported(&he)["DisplayTitle"], "German - HE-AAC - Stereo");
        let surround = audio("fr", "eac3", "5.1(side)");
        let stream = imported(&surround);
        assert_eq!(stream["DisplayTitle"], "French - Dolby Digital+ - 5.1");
        assert_eq!(stream["ChannelLayout"], "5.1");
        let mut master = audio("en", "dca", "5.1");
        master.profile = Some("ma".to_owned());
        let stream = imported(&master);
        assert_eq!(stream["Codec"], "dts");
        assert_eq!(stream["Profile"], "DTS-HD MA");
        assert_eq!(stream["DisplayTitle"], "English - DTS-HD MA - 5.1");
        let mut dts_x = audio("en", "dca", "7.1");
        dts_x.profile = Some("ma + dts:x".to_owned());
        assert_eq!(imported(&dts_x)["Profile"], "DTS-HD MA + DTS:X");
        assert_eq!(imported(&dts_x)["AudioSpatialFormat"], "DTSX");
        let mut atmos = audio("en", "eac3", "5.1");
        atmos.profile = Some("dolby digital plus + dolby atmos".to_owned());
        assert_eq!(imported(&atmos)["AudioSpatialFormat"], "DolbyAtmos");
    }

    #[test]
    fn pcm_is_named_by_its_sample_format_and_has_no_profile() {
        let mut pcm = track("AUDIO", "pcm");
        pcm.profile = Some("pcm_s16le".to_owned());
        pcm.channels = Some(1);
        let stream = imported(&pcm);
        assert_eq!(stream["Codec"], "pcm_s16le");
        assert!(stream.get("Profile").is_none());
        assert_eq!(stream["DisplayTitle"], "PCM_S16LE - 1 ch");
    }

    #[test]
    fn a_named_audio_keeps_its_name_and_the_attributes_it_lacks() {
        let mut named = audio("fr", "aac", "stereo");
        named.title = Some("Stereo".to_owned());
        named.is_default = true;
        assert_eq!(
            imported(&named)["DisplayTitle"],
            "Stereo - French - AAC - Default"
        );
        let mut held = audio("und", "aac", "5.1");
        held.title = Some("aac 5.1".to_owned());
        held.is_default = true;
        assert_eq!(imported(&held)["DisplayTitle"], "aac 5.1 - Default");
    }

    #[test]
    fn a_scanned_track_states_what_ffprobe_printed() {
        let mut video_track = track("VIDEO", "h264");
        let mut analysis = video(720, 404);
        analysis.average_frame_rate = Some("30000/1001".to_owned());
        analysis.real_frame_rate = Some("30000/1001".to_owned());
        analysis.display_aspect_ratio = Some("16:9".to_owned());
        analysis.sample_aspect_ratio = Some("1:1".to_owned());
        analysis.pixel_format = Some("yuv420p".to_owned());
        analysis.is_avc = Some(true);
        analysis.nal_length_size = Some(4);
        analysis.level = None;
        video_track.video = Some(analysis);
        video_track.stated = Stated {
            language: Some("und".to_owned()),
            name: None,
            handler_name: None,
            time_base: Some("1/30000".to_owned()),
            codec_tag: Some("avc1".to_owned()),
        };
        video_track.estimated_bitrate = Some(1_510_916);
        video_track.bitrate = Some(1_510_741);
        let stream = scanned(&video_track);
        assert_eq!(stream["Language"], "und");
        assert_eq!(stream["TimeBase"], "1/30000");
        assert_eq!(stream["CodecTag"], "avc1");
        assert_eq!(stream["NalLengthSize"], "4");
        assert_eq!(stream["IsAVC"], true);
        assert_eq!(stream["BitRate"], 1_510_916);
        assert_eq!(stream["AverageFrameRate"], 29.97003);
        assert_eq!(stream["ReferenceFrameRate"], 29.97003);
        assert_eq!(stream["Level"], -99);
        assert_eq!(stream["AspectRatio"], "16:9");
        assert_eq!(stream["IsAnamorphic"], false);
        assert!(
            stream.get("LocalizedLanguage").is_none(),
            "a video has none"
        );
    }

    #[test]
    fn a_scanned_track_without_a_stated_language_has_none() {
        let mut silent = audio("en", "vorbis", "stereo");
        silent.is_default = true;
        silent.stated.time_base = Some("1/1000".to_owned());
        let stream = scanned(&silent);
        assert!(stream.get("Language").is_none());
        assert!(stream.get("LocalizedLanguage").is_none());
        assert_eq!(stream["DisplayTitle"], "VORBIS - Stereo - Default");
    }

    #[test]
    fn a_language_the_reference_cannot_name_is_shown_capitalized() {
        let mut teletext = track("SUBTITLE", "dvb_teletext");
        teletext.stated.language = Some("fra,fra".to_owned());
        let stream = scanned(&teletext);
        assert_eq!(stream["Language"], "fra,fra");
        assert!(stream.get("LocalizedLanguage").is_none());
        assert_eq!(stream["DisplayTitle"], "Fra,fra - DVBTXT");
        let mut undetermined = audio("und", "aac", "stereo");
        undetermined.stated.language = Some("und".to_owned());
        let stream = scanned(&undetermined);
        assert_eq!(stream["LocalizedLanguage"], "Undetermined");
        assert_eq!(stream["DisplayTitle"], "AAC - Stereo");
    }

    #[test]
    fn an_mp4_handler_names_a_sound_unless_it_is_the_muxers() {
        let mut held = audio("und", "aac", "5.1");
        held.stated.name = Some("aac 5.1".to_owned());
        held.stated.handler_name = Some("French (aac 2.0)".to_owned());
        assert_eq!(scanned(&held)["Title"], "aac 5.1");
        let mut named = audio("und", "aac", "stereo");
        named.stated.handler_name = Some("French (aac 2.0)".to_owned());
        assert_eq!(scanned(&named)["Title"], "French (aac 2.0)");
        named.stated.handler_name = Some("SoundHandler".to_owned());
        assert!(scanned(&named).get("Title").is_none());
        let mut video_track = track("VIDEO", "h264");
        video_track.stated.handler_name = Some("VideoHandler".to_owned());
        assert!(scanned(&video_track).get("Title").is_none());
    }

    #[test]
    fn a_subtitle_is_labelled_as_the_reference_and_text_is_fetchable() {
        let mut sdh = track("SUBTITLE", "srt");
        sdh.language = Some("fr".to_owned());
        sdh.title = Some("SDH".to_owned());
        sdh.traits.is_hearing_impaired = true;
        let stream = imported(&sdh);
        assert_eq!(
            stream["DisplayTitle"],
            "SDH - French - Hearing Impaired - SUBRIP"
        );
        assert_eq!(stream["Codec"], "subrip");
        assert_eq!(stream["IsTextSubtitleStream"], true);
        assert_eq!(stream["LocalizedForced"], "Forced");
        let mut forced = track("SUBTITLE", "ass");
        forced.language = Some("fr".to_owned());
        forced.is_default = true;
        assert_eq!(imported(&forced)["DisplayTitle"], "French - Default - ASS");
        let image = imported(&track("SUBTITLE", "pgs"));
        assert_eq!(image["Codec"], "PGSSUB");
        assert_eq!(image["IsTextSubtitleStream"], false);
        assert_eq!(image["SupportsExternalStream"], true);
        assert_eq!(image["DisplayTitle"], "Undefined - PGSSUB");
        let dvd = imported(&track("SUBTITLE", "vobsub"));
        assert_eq!(dvd["Codec"], "DVDSUB");
        assert_eq!(dvd["SupportsExternalStream"], true);
        let broadcast = imported(&track("SUBTITLE", "dvb_subtitle"));
        assert_eq!(broadcast["Codec"], "DVBSUB");
        assert_eq!(broadcast["SupportsExternalStream"], false);
        let teletext = imported(&track("SUBTITLE", "dvb_teletext"));
        assert_eq!(teletext["Codec"], "DVBTXT");
        assert_eq!(teletext["IsTextSubtitleStream"], true);
    }

    #[test]
    fn a_picture_subtitle_states_its_canvas() {
        let mut pgs = track("SUBTITLE", "pgs");
        pgs.subtitle = Some(SubtitleAnalysis {
            canvas_width: Some(1920),
            canvas_height: Some(1080),
        });
        let stream = scanned(&pgs);
        assert_eq!(stream["Width"], 1920);
        assert_eq!(stream["Height"], 1080);
    }

    #[test]
    fn a_picture_is_shaped_as_the_reference_shapes_it() {
        assert_eq!(
            aspect_ratio(None, Some(720), Some(480)).as_deref(),
            Some("1.5:1")
        );
        assert_eq!(
            aspect_ratio(None, Some(1920), Some(1080)).as_deref(),
            Some("16:9")
        );
        assert_eq!(
            aspect_ratio(Some("240:101"), Some(1920), Some(808)).as_deref(),
            Some("2.40:1")
        );
        assert_eq!(
            aspect_ratio(Some("24:13"), Some(1920), Some(1040)).as_deref(),
            Some("1.85:1")
        );
        assert_eq!(
            aspect_ratio(Some("109:80"), Some(654), Some(480)).as_deref(),
            Some("4:3")
        );
        // Nothing near a common ratio: the stated one, as written.
        assert_eq!(
            aspect_ratio(Some("5:4"), Some(1280), Some(1024)).as_deref(),
            Some("5:4")
        );
        assert_eq!(aspect_ratio(None, None, None), None);
    }

    #[test]
    fn pixels_off_square_are_anamorphic_unless_the_picture_is_cropped() {
        let picture = |sample: &str, cropped| VideoAnalysis {
            width: Some(720),
            height: Some(576),
            sample_aspect_ratio: Some(sample.to_owned()),
            display_aspect_ratio: Some("16:9".to_owned()),
            is_cropped: Some(cropped),
            ..VideoAnalysis::default()
        };
        assert!(is_anamorphic(&picture("64:45", false)));
        assert!(!is_anamorphic(&picture("64:45", true)));
        assert!(!is_anamorphic(&picture("3201:3200", false)));
    }

    #[test]
    fn a_frame_rate_is_a_32_bit_float_of_the_stated_ratio() {
        assert_eq!(frame_rate("24000/1001"), Some(23.976_025));
        assert_eq!(frame_rate("203220000/8125199"), Some(25.011_08));
        assert_eq!(frame_rate("25/1"), Some(25.0));
        assert_eq!(frame_rate("0/0"), None);
    }

    #[test]
    fn subtitles_the_default_mode_plays_are_scored_and_the_first_chosen() {
        let subtitle = |index: i64, default: bool, forced: bool, text: bool| {
            json!({
                "Type": "Subtitle", "Index": index, "IsDefault": default, "IsForced": forced,
                "IsExternal": false, "IsTextSubtitleStream": text, "SupportsExternalStream": true,
            })
        };
        let mut streams = vec![
            json!({ "Type": "Audio", "Index": 1, "IsDefault": false, "IsForced": false }),
            json!({ "Type": "Audio", "Index": 2, "IsDefault": true, "IsForced": false }),
            subtitle(3, false, true, true),
            subtitle(4, false, false, true),
            subtitle(5, true, false, false),
        ];
        score_subtitles(&mut streams);
        assert_eq!(streams[2]["Score"], 121_221);
        assert!(streams[3].get("Score").is_none());
        assert_eq!(streams[4]["Score"], 112_211);
        assert_eq!(default_subtitle_index(&streams), Some(5));
        assert_eq!(default_audio_index(&streams), Some(2));
    }

    #[test]
    fn a_cover_and_a_data_track_are_streams_and_attachments() {
        let cover = SideStream {
            kind: "COVER".to_owned(),
            position: 2,
            codec: Some("bmp".to_owned()),
            codec_tag: Some("[0][0][0][0]".to_owned()),
            profile: None,
            language: None,
            time_base: Some("1/90000".to_owned()),
            mime_type: None,
            filename: None,
            is_default: false,
            is_forced: false,
            picture: Some(SidePicture {
                width: Some(640),
                height: Some(360),
                pixel_format: Some("bgra".to_owned()),
                real_frame_rate: Some("90000/1".to_owned()),
                average_frame_rate: Some("0/0".to_owned()),
                ..SidePicture::default()
            }),
        };
        let data = SideStream {
            kind: "DATA".to_owned(),
            position: 3,
            codec: Some("timed_id3".to_owned()),
            codec_tag: Some("ID3 ".to_owned()),
            picture: None,
            ..cover.clone()
        };
        let file = FileStreams {
            tracks: &[],
            side_streams: &[cover, data],
            scanned: true,
        };
        let streams = file.media_streams();
        assert_eq!(streams[0]["Type"], "EmbeddedImage");
        assert_eq!(streams[0]["RealFrameRate"], 90_000.0);
        assert!(streams[0].get("AverageFrameRate").is_none());
        assert_eq!(streams[0]["AspectRatio"], "16:9");
        assert_eq!(streams[0]["Level"], -99);
        assert!(streams[0].get("CodecTag").is_none());
        assert_eq!(streams[1]["Type"], "Data");
        assert_eq!(streams[1]["CodecTag"], "ID3 ");
        assert!(streams[1].get("Level").is_none());
        let attachments = file.media_attachments();
        assert_eq!(attachments.len(), 1, "a data track is no attachment");
        assert_eq!(attachments[0]["CodecTag"], "[0][0][0][0]");
        assert_eq!(attachments[0]["Index"], 2);
    }
}
