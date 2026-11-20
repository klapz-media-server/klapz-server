//! A file's tracks as Jellyfin's `MediaStream`s, labelled as the reference labels them.
//!
//! The rules were read off the reference's answers on the 35 files both servers scanned
//! (`tools/compare_jellyfin.py`): each label lists a track's attributes — language, codec or
//! profile, layout, default — and a track named in its file keeps its name first, followed by
//! those attributes its name does not already hold.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::language;

/// What the official API is asked of a track. A macro, so that the item fragment can join it
/// into one constant.
macro_rules! track_fields {
    () => {
        r"
fragment JellyfinTrack on MediaStream {
    id kind position codec language channels bitrate isDefault isForced isExternal title
    profile bitDepth
    video { width height frameRate level refFrames scan isAnamorphic chromaSubsampling
            hasHdr10Plus
            color { space primaries transfer }
            dolbyVision { profile baseLayerCompatibility } }
    audio { samplingRate channelLayout }
    traits { isHearingImpaired isOriginal }
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
    pub video: Option<VideoAnalysis>,
    pub audio: Option<AudioAnalysis>,
    pub traits: Traits,
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
    pub profile: i32,
    pub base_layer_compatibility: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioAnalysis {
    pub sampling_rate: Option<i32>,
    pub channel_layout: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Traits {
    pub is_hearing_impaired: bool,
    pub is_original: bool,
}

/// A file's tracks in the order Jellyfin lists them, by their index in the file.
pub fn in_order(tracks: &[Track]) -> impl Iterator<Item = &Track> {
    let mut ordered: Vec<&Track> = tracks.iter().collect();
    ordered.sort_by_key(|track| (track.is_external, track.position));
    ordered.into_iter()
}

/// Jellyfin's codec name, where it differs from the one stored: ffprobe's, but DTS as the
/// Plex importer named it.
fn codec(track: &Track) -> Option<String> {
    let codec = track.codec.as_deref()?;
    Some(
        match codec {
            "dca" => "dts",
            "srt" => "subrip",
            "pgs" => "PGSSUB",
            "vobsub" => "dvd_subtitle",
            other => other,
        }
        .to_owned(),
    )
}

fn is_text_subtitle(codec: &str) -> bool {
    matches!(
        codec,
        "srt" | "subrip" | "ass" | "ssa" | "vtt" | "webvtt" | "mov_text" | "smi" | "text"
    )
}

/// The profile as ffprobe writes it, which the reference publishes: Plex's importer kept
/// some in lower case or by a short name (`lc`, `ma`).
fn profile(codec: Option<&str>, profile: &str) -> String {
    match (codec, profile.to_ascii_lowercase().as_str()) {
        (_, "lc") => "LC".to_owned(),
        (_, "he-aac") => "HE-AAC".to_owned(),
        (_, "he-aacv2") => "HE-AACv2".to_owned(),
        (Some("dca" | "dts"), "ma") => "DTS-HD MA".to_owned(),
        (Some("dca" | "dts"), "hra") => "DTS-HD HRA".to_owned(),
        (Some("dca" | "dts"), "es") => "DTS-ES".to_owned(),
        (Some("dca" | "dts"), "x") => "DTS-HD MA + DTS:X".to_owned(),
        _ => capitalized(profile),
    }
}

/// `main 10` as ffprobe writes it, `Main 10`.
fn capitalized(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// An audio codec as labels name it.
fn friendly_audio_codec(codec: &str) -> String {
    match codec {
        "ac3" => "Dolby Digital".to_owned(),
        "eac3" => "Dolby Digital+".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

/// The reference's label for a picture's size: the smallest of its classes that holds both
/// sides, interlaced pictures with an `i`.
fn resolution_label(width: i32, height: i32, interlaced: bool) -> String {
    const CLASSES: [(i32, i32, &str); 12] = [
        (256, 144, "144"),
        (426, 240, "240"),
        (640, 360, "360"),
        (682, 384, "384"),
        (720, 404, "404"),
        (854, 480, "480"),
        (960, 544, "540"),
        (1024, 576, "576"),
        (1280, 720, "720"),
        (2560, 1080, "1080"),
        (4096, 3072, "4K"),
        (8192, 6144, "8K"),
    ];
    let class = CLASSES
        .iter()
        .find(|(max_width, max_height, _)| width <= *max_width && height <= *max_height)
        .map_or("8K", |(_, _, class)| class);
    if class.ends_with('K') {
        class.to_owned()
    } else {
        format!("{class}{}", if interlaced { 'i' } else { 'p' })
    }
}

/// Jellyfin's dynamic range and its kind, and how a label names the kind.
fn video_range(video: &VideoAnalysis) -> (&'static str, &'static str, String) {
    if let Some(dolby_vision) = &video.dolby_vision {
        let compatibility = dolby_vision.base_layer_compatibility.unwrap_or(0);
        let (kind, base) = match (compatibility, video.has_hdr10_plus) {
            (1 | 6, true) => ("DOVIWithHDR10Plus", Some("HDR10")),
            (1 | 6, false) => ("DOVIWithHDR10", Some("HDR10")),
            (4, _) => ("DOVIWithHLG", Some("HLG")),
            (2, _) => ("DOVIWithSDR", Some("SDR")),
            _ => ("DOVI", None),
        };
        let label = match base {
            Some(base) => format!(
                "Dolby Vision Profile {}.{compatibility} ({base})",
                dolby_vision.profile
            ),
            None => format!(
                "Dolby Vision Profile {}.{compatibility}",
                dolby_vision.profile
            ),
        };
        return ("HDR", kind, label);
    }
    match (video.color.transfer.as_deref(), video.has_hdr10_plus) {
        (Some("smpte2084"), true) => ("HDR", "HDR10Plus", "HDR10+".to_owned()),
        (Some("smpte2084"), false) => ("HDR", "HDR10", "HDR10".to_owned()),
        (Some("arib-std-b67"), _) => ("HDR", "HLG", "HLG".to_owned()),
        _ => ("SDR", "SDR", "SDR".to_owned()),
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

fn language_attribute(code: &str) -> Option<String> {
    (code != "und").then(|| language::english_name(code))
}

fn display_title(track: &Track, kind: &str, language: &str) -> String {
    let codec = codec(track).unwrap_or_default();
    match kind {
        "Video" => {
            let video = track.video.clone().unwrap_or_default();
            let interlaced = video.scan.as_deref() == Some("INTERLACED");
            let mut attributes = Vec::new();
            if let (Some(width), Some(height)) = (video.width, video.height) {
                attributes.push(resolution_label(width, height, interlaced));
            }
            if !codec.is_empty() {
                attributes.push(codec.to_ascii_uppercase());
            }
            attributes.push(video_range(&video).2);
            label(track.title.as_deref(), &attributes, " ")
        }
        "Audio" => {
            let mut attributes = Vec::new();
            attributes.extend(language_attribute(language));
            match track
                .profile
                .as_deref()
                .map(|named| profile(track.codec.as_deref(), named))
            {
                Some(named) if named != "LC" => attributes.push(named),
                _ if !codec.is_empty() => attributes.push(friendly_audio_codec(&codec)),
                _ => {}
            }
            let layout = track
                .audio
                .as_ref()
                .and_then(|audio| audio.channel_layout.as_deref())
                .map(channel_layout);
            match (layout, track.channels) {
                (Some(layout), _) => attributes.push(capitalized(&layout)),
                (None, Some(channels)) => attributes.push(format!("{channels} ch")),
                (None, None) => {}
            }
            if track.is_default {
                attributes.push("Default".to_owned());
            }
            if track.is_external {
                attributes.push("External".to_owned());
            }
            label(track.title.as_deref(), &attributes, " - ")
        }
        _ => {
            let mut attributes = Vec::new();
            attributes.extend(language_attribute(language));
            if track.traits.is_hearing_impaired {
                attributes.push("Hearing Impaired".to_owned());
            }
            if track.is_default {
                attributes.push("Default".to_owned());
            }
            if track.is_forced {
                attributes.push("Forced".to_owned());
            }
            if track.is_external {
                attributes.push("External".to_owned());
            }
            if !codec.is_empty() {
                attributes.push(codec.to_ascii_uppercase());
            }
            label(track.title.as_deref(), &attributes, " - ")
        }
    }
}

/// ffmpeg's layout, as the reference publishes it: `5.1(side)` is `5.1`.
fn channel_layout(layout: &str) -> String {
    layout.strip_suffix("(side)").unwrap_or(layout).to_owned()
}

/// The audio's spatial format, which the reference reads from the profile.
fn spatial_format(profile: Option<&str>) -> &'static str {
    let profile = profile.unwrap_or_default().to_ascii_lowercase();
    if profile.contains("atmos") {
        "DolbyAtmos"
    } else if profile.contains("dts:x") || profile == "x" {
        "DTSX"
    } else {
        "None"
    }
}

/// A picture's shape as the reference writes it: the common ratios by name, others as a
/// number to one.
fn aspect_ratio(width: i32, height: i32) -> String {
    if height <= 0 {
        return String::new();
    }
    let ratio = f64::from(width) / f64::from(height);
    let close = |to: f64| (ratio - to).abs() < 0.01;
    if close(4.0 / 3.0) {
        "4:3".to_owned()
    } else if close(16.0 / 9.0) {
        "16:9".to_owned()
    } else {
        let rounded = format!("{ratio:.2}");
        format!("{}:1", rounded.trim_end_matches('0').trim_end_matches('.'))
    }
}

/// ffmpeg's pixel format for a chroma subsampling and a bit depth: `yuv420p`, `yuv420p10le`.
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

/// One of our tracks as a Jellyfin `MediaStream`.
#[allow(
    clippy::too_many_lines,
    reason = "one field after another, in the reference's order"
)]
pub fn media_stream(track: &Track) -> Value {
    let kind = match track.kind.as_str() {
        "VIDEO" => "Video",
        "AUDIO" => "Audio",
        _ => "Subtitle",
    };
    let language = track
        .language
        .as_deref()
        .map_or("und", language::three_letters);
    let video = track.video.as_ref();
    let mut stream = Map::new();
    let mut put = |key: &str, value: Value| {
        stream.insert(key.to_owned(), value);
    };
    if let Some(codec) = codec(track) {
        put("Codec", json!(codec));
    }
    put("Language", json!(language));
    if let Some(title) = track.title.as_deref().filter(|title| !title.is_empty()) {
        put("Title", json!(title));
    }
    let (range, range_type) = video.map_or(("Unknown", "Unknown"), |video| {
        let (range, kind, _) = video_range(video);
        (range, kind)
    });
    put("VideoRange", json!(range));
    put("VideoRangeType", json!(range_type));
    put(
        "AudioSpatialFormat",
        json!(if kind == "Audio" {
            spatial_format(track.profile.as_deref())
        } else {
            "None"
        }),
    );
    if kind != "Video" {
        put("LocalizedDefault", json!("Default"));
        put("LocalizedExternal", json!("External"));
        if language != "und" {
            put("LocalizedLanguage", json!(language::english_name(language)));
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
    put("DisplayTitle", json!(display_title(track, kind, language)));
    put(
        "IsInterlaced",
        json!(video.is_some_and(|video| video.scan.as_deref() == Some("INTERLACED"))),
    );
    if kind == "Video" {
        put("IsAVC", json!(track.codec.as_deref() == Some("h264")));
    }
    if let Some(layout) = track
        .audio
        .as_ref()
        .and_then(|audio| audio.channel_layout.as_deref())
    {
        put("ChannelLayout", json!(channel_layout(layout)));
    }
    if let Some(bitrate) = track.bitrate {
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
    if let Some(video) = video {
        if let Some(height) = video.height {
            put("Height", json!(height));
        }
        if let Some(width) = video.width {
            put("Width", json!(width));
        }
        if let Some(rate) = video.frame_rate {
            put("AverageFrameRate", json!(rate));
            put("RealFrameRate", json!(rate));
            put("ReferenceFrameRate", json!(rate));
        }
    }
    if let Some(named) = &track.profile {
        put("Profile", json!(profile(track.codec.as_deref(), named)));
    }
    put("Type", json!(kind));
    put("Index", json!(track.position));
    put("IsExternal", json!(track.is_external));
    let text = track.codec.as_deref().is_some_and(is_text_subtitle);
    put("IsTextSubtitleStream", json!(kind == "Subtitle" && text));
    put("SupportsExternalStream", json!(kind == "Subtitle" && text));
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
        if let (Some(width), Some(height)) = (video.width, video.height) {
            put("AspectRatio", json!(aspect_ratio(width, height)));
        }
        if let Some(format) = pixel_format(video.chroma_subsampling.as_deref(), track.bit_depth) {
            put("PixelFormat", json!(format));
        }
        if let Some(level) = video.level {
            put("Level", json!(level));
        }
        put("IsAnamorphic", json!(video.is_anamorphic.unwrap_or(false)));
        if let Some(dolby_vision) = &video.dolby_vision {
            put("VideoDoViTitle", json!(video_range(video).2));
            put("DvProfile", json!(dolby_vision.profile));
        }
    }
    Value::Object(stream)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn track(kind: &str, codec: &str) -> Track {
        Track {
            id: open_plex_api::GlobalId::encode("MediaStream", 1),
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
            video: None,
            audio: None,
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
            is_anamorphic: None,
            chroma_subsampling: Some("4:2:0".to_owned()),
            has_hdr10_plus: false,
            color: Color::default(),
            dolby_vision: None,
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
            (3840, 2160, "PROGRESSIVE", "4K H264 SDR"),
        ] {
            let mut sdr = track("VIDEO", "h264");
            let mut analysis = video(width, height);
            analysis.scan = Some(scan.to_owned());
            sdr.video = Some(analysis);
            assert_eq!(
                media_stream(&sdr)["DisplayTitle"],
                label,
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn a_named_video_keeps_its_name_first() {
        let mut named = track("VIDEO", "hevc");
        named.title = Some("E20 - 1080p".to_owned());
        named.video = Some(video(1440, 1080));
        assert_eq!(
            media_stream(&named)["DisplayTitle"],
            "E20 - 1080p - HEVC - SDR"
        );
    }

    #[test]
    fn hdr_and_dolby_vision_are_named_as_the_reference_names_them() {
        let mut hdr = track("VIDEO", "hevc");
        let mut analysis = video(1920, 1080);
        analysis.color.transfer = Some("smpte2084".to_owned());
        hdr.video = Some(analysis.clone());
        let stream = media_stream(&hdr);
        assert_eq!(stream["VideoRange"], "HDR");
        assert_eq!(stream["VideoRangeType"], "HDR10");
        analysis.dolby_vision = Some(DolbyVision {
            profile: 8,
            base_layer_compatibility: Some(1),
        });
        hdr.video = Some(analysis.clone());
        let stream = media_stream(&hdr);
        assert_eq!(stream["VideoRangeType"], "DOVIWithHDR10");
        assert_eq!(
            stream["DisplayTitle"],
            "1080p HEVC Dolby Vision Profile 8.1 (HDR10)"
        );
        analysis.has_hdr10_plus = true;
        hdr.video = Some(analysis);
        assert_eq!(media_stream(&hdr)["VideoRangeType"], "DOVIWithHDR10Plus");
    }

    #[test]
    fn an_audio_is_labelled_by_language_codec_layout_and_default() {
        let mut english = audio("en", "opus", "stereo");
        english.is_default = true;
        let stream = media_stream(&english);
        assert_eq!(stream["DisplayTitle"], "English - OPUS - Stereo - Default");
        assert_eq!(stream["Language"], "eng");
        assert_eq!(stream["LocalizedLanguage"], "English");
        assert_eq!(stream["LocalizedOriginal"], "Original");
        assert!(stream.get("IsAVC").is_none());
        assert!(stream.get("LocalizedForced").is_none());
    }

    #[test]
    fn a_profile_or_a_friendly_codec_names_the_audio() {
        let mut lc = audio("fr", "aac", "stereo");
        lc.profile = Some("lc".to_owned());
        assert_eq!(media_stream(&lc)["DisplayTitle"], "French - AAC - Stereo");
        assert_eq!(media_stream(&lc)["Profile"], "LC");
        let mut he = audio("de", "aac", "stereo");
        he.profile = Some("he-aac".to_owned());
        assert_eq!(
            media_stream(&he)["DisplayTitle"],
            "German - HE-AAC - Stereo"
        );
        let surround = audio("fr", "eac3", "5.1(side)");
        let stream = media_stream(&surround);
        assert_eq!(stream["DisplayTitle"], "French - Dolby Digital+ - 5.1");
        assert_eq!(stream["ChannelLayout"], "5.1");
        let mut master = audio("en", "dca", "5.1");
        master.profile = Some("ma".to_owned());
        let stream = media_stream(&master);
        assert_eq!(stream["Codec"], "dts");
        assert_eq!(stream["Profile"], "DTS-HD MA");
        assert_eq!(stream["DisplayTitle"], "English - DTS-HD MA - 5.1");
        let mut atmos = audio("en", "eac3", "5.1");
        atmos.profile = Some("dolby digital plus + dolby atmos".to_owned());
        assert_eq!(media_stream(&atmos)["AudioSpatialFormat"], "DolbyAtmos");
    }

    #[test]
    fn a_named_audio_keeps_its_name_and_the_attributes_it_lacks() {
        let mut named = audio("fr", "aac", "stereo");
        named.title = Some("Stereo".to_owned());
        named.is_default = true;
        assert_eq!(
            media_stream(&named)["DisplayTitle"],
            "Stereo - French - AAC - Default"
        );
        let mut held = audio("und", "aac", "5.1");
        held.title = Some("aac 5.1".to_owned());
        held.is_default = true;
        assert_eq!(media_stream(&held)["DisplayTitle"], "aac 5.1 - Default");
    }

    #[test]
    fn an_audio_without_a_layout_counts_its_channels() {
        let mut pcm = track("AUDIO", "pcm_s16le");
        pcm.channels = Some(1);
        assert_eq!(media_stream(&pcm)["DisplayTitle"], "PCM_S16LE - 1 ch");
    }

    #[test]
    fn a_subtitle_is_labelled_as_the_reference_and_text_is_fetchable() {
        let mut sdh = track("SUBTITLE", "srt");
        sdh.language = Some("fr".to_owned());
        sdh.title = Some("SDH".to_owned());
        sdh.traits.is_hearing_impaired = true;
        let stream = media_stream(&sdh);
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
        assert_eq!(
            media_stream(&forced)["DisplayTitle"],
            "French - Default - ASS"
        );
        let image = media_stream(&track("SUBTITLE", "pgs"));
        assert_eq!(image["Codec"], "PGSSUB");
        assert_eq!(image["IsTextSubtitleStream"], false);
        assert_eq!(image["DisplayTitle"], "PGSSUB");
    }

    #[test]
    fn a_picture_is_described_as_the_reference_describes_it() {
        assert_eq!(aspect_ratio(720, 480), "1.5:1");
        assert_eq!(aspect_ratio(1920, 1080), "16:9");
        assert_eq!(aspect_ratio(640, 480), "4:3");
        assert_eq!(aspect_ratio(1920, 800), "2.4:1");
        assert_eq!(
            pixel_format(Some("4:2:0"), Some(8)),
            Some("yuv420p".to_owned())
        );
        assert_eq!(
            pixel_format(Some("4:2:0"), Some(10)),
            Some("yuv420p10le".to_owned())
        );
        assert_eq!(pixel_format(None, Some(8)), None);
    }

    #[test]
    fn a_video_is_never_left_without_its_anamorphic_flag() {
        let mut plain = track("VIDEO", "h264");
        plain.video = Some(video(1920, 1080));
        assert_eq!(media_stream(&plain)["IsAnamorphic"], false);
        assert_eq!(media_stream(&plain)["PixelFormat"], "yuv420p");
        assert!(media_stream(&plain).get("LocalizedDefault").is_none());
    }

    #[test]
    fn tracks_are_listed_by_their_index_in_the_file() {
        let mut audio = track("AUDIO", "opus");
        audio.position = 1;
        let mut external = track("SUBTITLE", "srt");
        external.is_external = true;
        let tracks = [external, audio, track("VIDEO", "h264")];
        let kinds: Vec<&str> = in_order(&tracks).map(|track| track.kind.as_str()).collect();
        assert_eq!(kinds, ["VIDEO", "AUDIO", "SUBTITLE"]);
    }
}
