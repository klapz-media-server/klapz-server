//! A scanned file's analysis, written in the vocabulary the importer reads out of Plex's own.
//!
//! The scanner reports what ffprobe says (`matroska,webm`, `subrip`, `fre`, `High`); Plex's
//! analysis read the same headers with the same libav and stored them under other names
//! (`mkv`, `srt`, `fr`, `high`). A scanned film must be indistinguishable from an imported
//! one, so the renaming happens here, once, measured against what the reference stored for
//! the test files (`docs/scanner/analysis.md`, §1).

use open_plex_core::scan::NewSidecar;
use open_plex_scanner::report::{Analysis, DolbyVision, Track, TrackKind, VideoTrack};

use crate::entity::{media_stream, stream_dolby_vision};

/// What a version states about itself, from the first video track and the default audio.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct MediaSummary {
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub aspect_ratio: Option<f64>,
    pub bitrate: Option<i32>,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i32>,
    pub frame_rate: Option<f64>,
    pub interlaced: bool,
    pub optimized_for_streaming: bool,
}

pub(super) fn media_summary(analysis: &Analysis, path: &str) -> MediaSummary {
    let video = main_video(analysis);
    let picture = video.and_then(|track| track.video.as_ref());
    let audio = main_audio(analysis);
    MediaSummary {
        width: picture.and_then(|picture| i32::try_from(picture.width).ok()),
        height: picture.and_then(|picture| i32::try_from(picture.height).ok()),
        aspect_ratio: picture.and_then(|picture| {
            let ratio = f64::from(picture.width) / f64::from(picture.height)
                * sample_aspect_ratio(picture.sample_aspect_ratio.as_deref());
            ratio.is_finite().then(|| as_stored_float(ratio))
        }),
        bitrate: analysis
            .deep
            .as_ref()
            .map(|deep| deep.bitrate)
            .or(analysis.bitrate)
            .and_then(to_i32),
        container: container(&analysis.container, path),
        video_codec: video.and_then(|track| track.codec.as_deref()).map(codec),
        audio_codec: audio.and_then(|track| track.codec.as_deref()).map(codec),
        audio_channels: audio
            .and_then(|track| track.audio.as_ref()?.channels)
            .and_then(|channels| i32::try_from(channels).ok()),
        frame_rate: picture
            .and_then(|picture| picture.frame_rate)
            .map(as_stored_float),
        interlaced: picture
            .and_then(|picture| scan_type(picture.field_order.as_deref()?))
            .is_some_and(|scan| scan == "interlaced"),
        optimized_for_streaming: analysis.optimized_for_streaming.unwrap_or(false),
    }
}

/// The video track a version is described by: not a cover picture stored as a track, which
/// the reference counts among a library's codecs but never names a version's.
fn main_video(analysis: &Analysis) -> Option<&Track> {
    analysis
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video && !track.disposition.attached_picture)
}

fn main_audio(analysis: &Analysis) -> Option<&Track> {
    let mut audio = analysis
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio);
    let first = audio.clone().next();
    audio.find(|track| track.disposition.default).or(first)
}

/// The reference keeps a version's frame rate and aspect ratio as 32-bit floats: 24000/1001 is
/// stored as 23.976024627685547.
#[allow(
    clippy::cast_possible_truncation,
    reason = "narrowing to f32 is the point: it reproduces the reference's stored precision"
)]
fn as_stored_float(value: f64) -> f64 {
    f64::from(value as f32)
}

fn sample_aspect_ratio(ratio: Option<&str>) -> f64 {
    let parsed = ratio.and_then(|ratio| {
        let (numerator, denominator) = ratio.split_once(':')?;
        let numerator: f64 = numerator.parse().ok()?;
        let denominator: f64 = denominator.parse().ok()?;
        (numerator > 0.0 && denominator > 0.0).then_some(numerator / denominator)
    });
    parsed.unwrap_or(1.0)
}

/// Plex's container names, from ffprobe's `format_name`.
///
/// ffprobe names one demuxer for the whole ISO family, which Plex splits by extension into
/// `mov` and `mp4`; the rest are its demuxer's first name (`matroska` excepted, measured as
/// `mkv`), which is the vocabulary of Plex's transcoder profiles.
pub(super) fn container(format_name: &str, path: &str) -> Option<String> {
    let first = format_name
        .split(',')
        .next()
        .filter(|name| !name.is_empty())?;
    let name = match first {
        "matroska" => "mkv",
        "mov" if extension(path).eq_ignore_ascii_case("mov") => "mov",
        "mov" => "mp4",
        other => other,
    };
    Some(name.to_owned())
}

/// The French dub a release names, for a file whose one audio track carries no language: `VFQ`
/// is Quebec's (`fr-CA`), `VFF`, `VFI`, `FRENCH` and `TRUEFRENCH` France's (`fr-FR`), and `VFQ`
/// wins when a name gives both. The repo owner's rule: the reference leaves such a track
/// without a language.
fn dub_language(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let words = stem
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty());
    let mut france = false;
    for word in words {
        if word.eq_ignore_ascii_case("vfq") {
            return Some("fr-CA");
        }
        france |= ["vff", "vfi", "french", "truefrench"]
            .iter()
            .any(|dub| word.eq_ignore_ascii_case(dub));
    }
    france.then_some("fr-FR")
}

fn extension(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').map_or("", |(_, extension)| extension)
}

/// Plex's codec names where they differ from ffprobe's: libav's older names, which its
/// transcoder profiles use (`dca`, `pgs`, `vobsub`, `pcm`), and `srt`, measured.
pub(super) fn codec(ffprobe: &str) -> String {
    let name = match ffprobe {
        "subrip" => "srt",
        "dts" => "dca",
        "hdmv_pgs_subtitle" => "pgs",
        "dvd_subtitle" => "vobsub",
        pcm if pcm.starts_with("pcm_") => "pcm",
        other => other,
    };
    name.to_owned()
}

/// A track's language as the reference stores it: the ISO 639-1 code when the language has
/// one (`fre` and `fra` are `fr`), else the tag as written (`pt-BR`, `mul`). `und` is no
/// language.
pub(super) fn language(tag: &str) -> Option<String> {
    let tag = tag.trim();
    if tag.is_empty() || tag.eq_ignore_ascii_case("und") {
        return None;
    }
    let lower = tag.to_ascii_lowercase();
    let short = super::languages::ALPHA2
        .binary_search_by_key(&lower.as_str(), |(alpha3, _)| alpha3)
        .ok()
        .map(|found| super::languages::ALPHA2[found].1);
    Some(short.map_or_else(|| tag.to_owned(), ToOwned::to_owned))
}

/// ffprobe's profile, lower-cased as the reference stores it, DTS's named by their extension
/// alone (`ma` for DTS-HD Master Audio), as Plex's analysis writes them.
pub(super) fn profile(ffprobe: &str) -> String {
    let lower = ffprobe.to_lowercase();
    let name = match lower.as_str() {
        "dts-hd ma" => "ma",
        "dts-hd hra" => "hra",
        "dts-es" => "es",
        other => other,
    };
    name.to_owned()
}

/// The chroma subsampling a pixel format names: `yuv420p10le` is `4:2:0`.
pub(super) fn chroma_subsampling(pixel_format: &str) -> Option<&'static str> {
    [
        ("420", "4:2:0"),
        ("422", "4:2:2"),
        ("444", "4:4:4"),
        ("411", "4:1:1"),
    ]
    .into_iter()
    .find(|(digits, _)| pixel_format.contains(digits))
    .map(|(_, ratio)| ratio)
}

/// `progressive` or `interlaced`, from ffprobe's field order; nothing when it does not know.
pub(super) fn scan_type(field_order: &str) -> Option<&'static str> {
    match field_order {
        "progressive" => Some("progressive"),
        "tt" | "bb" | "tb" | "bt" => Some("interlaced"),
        _ => None,
    }
}

/// One track's row, and its Dolby Vision record when it has one.
pub(super) struct TrackRows {
    pub stream: media_stream::Model,
    pub dolby_vision: Option<stream_dolby_vision::Model>,
}

/// A track's language as tagged; else Matroska's default for its video, which muxers leave
/// unwritten, and the dub its file's name gives for a file's one audio track.
fn track_language(analysis: &Analysis, track: &Track, path: &str) -> Option<String> {
    if let Some(tagged) = track.language.as_deref().and_then(language) {
        return Some(tagged);
    }
    match track.kind {
        TrackKind::Video if analysis.container.starts_with("matroska") => Some("en".to_owned()),
        TrackKind::Audio => {
            let audio = analysis
                .tracks
                .iter()
                .filter(|other| other.kind == TrackKind::Audio)
                .count();
            (audio == 1)
                .then(|| dub_language(path))
                .flatten()
                .map(ToOwned::to_owned)
        }
        _ => None,
    }
}

/// The rows of a file's tracks, numbered from `first_id`. Attachments and data tracks have no
/// row: the reference keeps video, audio and subtitles only.
pub(super) fn track_rows(
    analysis: &Analysis,
    path: &str,
    part: i64,
    first_id: i64,
) -> Vec<TrackRows> {
    let avi = analysis.container == "avi";
    analysis
        .tracks
        .iter()
        .filter_map(|track| Some((track, kind_name(track.kind)?)))
        .zip(first_id..)
        .map(|((track, kind), id)| {
            let bitrate = track_bitrate(analysis, track);
            let language = track_language(analysis, track, path);
            let picture = track.video.as_ref();
            let coded = picture.and_then(|picture| coding(track, picture));
            let sound = track.audio.as_ref();
            let disposition = track.disposition;
            // Written only for pixels that are not square, as the reference writes it.
            let sample_aspect = picture
                .and_then(|picture| picture.sample_aspect_ratio.as_deref())
                .filter(|ratio| !is_square(ratio));
            let stream = media_stream::Model {
                id,
                media_part_id: part,
                kind: kind.to_owned(),
                position: i32::try_from(track.index).unwrap_or(i32::MAX),
                codec: track.codec.as_deref().map(codec),
                language,
                channels: sound
                    .and_then(|sound| sound.channels)
                    .and_then(|channels| i32::try_from(channels).ok()),
                bitrate: bitrate.and_then(to_i32),
                is_default: disposition.default,
                is_forced: disposition.forced,
                external_path: None,
                title: track.title.clone(),
                stream_identifier: None,
                profile: track.profile.as_deref().map(profile),
                bit_depth: picture
                    .and_then(|picture| picture.bit_depth)
                    .or_else(|| sound.and_then(|sound| sound.bit_depth))
                    .and_then(|depth| i32::try_from(depth).ok()),
                width: picture.and_then(|picture| i32::try_from(picture.width).ok()),
                height: picture.and_then(|picture| i32::try_from(picture.height).ok()),
                coded_width: coded.map(|coded| coded.width),
                coded_height: coded.map(|coded| coded.height),
                frame_rate: picture
                    .and_then(|picture| picture.frame_rate)
                    .map(|rate| (rate * 1000.0).round() / 1000.0),
                level: picture.and_then(|picture| picture.level),
                ref_frames: coded.map(|coded| coded.ref_frames),
                scan_type: picture
                    .and_then(|picture| scan_type(picture.field_order.as_deref()?))
                    .map(ToOwned::to_owned),
                chroma_subsampling: picture
                    .and_then(|picture| chroma_subsampling(picture.pixel_format.as_deref()?))
                    .map(ToOwned::to_owned),
                chroma_location: picture.and_then(|picture| picture.chroma_location.clone()),
                color_range: picture.and_then(|picture| picture.color_range.clone()),
                color_space: picture.and_then(|picture| picture.color_space.clone()),
                color_primaries: picture.and_then(|picture| picture.color_primaries.clone()),
                color_transfer: picture.and_then(|picture| picture.color_transfer.clone()),
                has_scaling_matrix: coded.and_then(|coded| coded.has_scaling_matrix),
                is_anamorphic: sample_aspect.map(|_| true),
                pixel_aspect_ratio: sample_aspect.map(ToOwned::to_owned),
                // The reference keeps a FourCC for the legacy containers that carry one.
                fourcc: if avi && picture.is_some() {
                    track.codec_tag.clone()
                } else {
                    None
                },
                has_closed_captions: picture.is_some_and(|picture| picture.closed_captions),
                has_separate_fields: false,
                has_hdr10_plus: false,
                sampling_rate: sound
                    .and_then(|sound| sound.sample_rate)
                    .and_then(|rate| i32::try_from(rate).ok()),
                channel_layout: sound.and_then(|sound| sound.channel_layout.clone()),
                subtitle_format: None,
                is_embedded_in_video: false,
                is_original: disposition.original,
                is_dub: disposition.dub,
                is_commentary: disposition.comment,
                is_hearing_impaired: disposition.hearing_impaired,
                is_visual_impaired: disposition.visual_impaired,
                has_header_compression: false,
                required_bandwidths: None,
            };
            TrackRows {
                dolby_vision: picture
                    .and_then(|picture| picture.dolby_vision)
                    .map(|record| dolby_vision_row(id, record)),
                stream,
            }
        })
        .collect()
}

/// A track's rate: measured over every packet when a deep analysis ran, and then nothing else,
/// a rate of 0 included (Soul's forced subtitles: ten packets, 318 bytes, no rate stored).
/// Otherwise as its header declares it; without that, the main video takes what the
/// container's rate leaves, and audio and subtitles the rate their muxer tagged — Bull's AAC
/// declares none and the reference stored its tagged 96000.
fn track_bitrate(analysis: &Analysis, track: &Track) -> Option<u64> {
    if let Some(deep) = &analysis.deep {
        return deep
            .tracks
            .iter()
            .find(|measured| measured.index == track.index)
            .map(|measured| measured.bitrate)
            .filter(|rate| *rate > 0);
    }
    track
        .bitrate
        .filter(|rate| *rate > 0)
        .or_else(|| match track.kind {
            TrackKind::Video
                if main_video(analysis).map(|main| main.index) == Some(track.index) =>
            {
                video_bitrate_fallback(analysis)
            }
            TrackKind::Video => None,
            _ => track.tagged_bitrate.filter(|rate| *rate > 0),
        })
}

/// How a video track was coded, as the reference stores it.
#[derive(Debug, Clone, Copy)]
struct Coding {
    width: i32,
    height: i32,
    ref_frames: i32,
    has_scaling_matrix: Option<bool>,
}

/// H.264's from its sequence header, unknown without one, since ffprobe's cropped size would be
/// stored wrong; any other codec's size as ffprobe reports it, with the one reference frame the
/// reference stores for every such track (39,297 of 39,297 in its library).
fn coding(track: &Track, picture: &VideoTrack) -> Option<Coding> {
    if track.codec.as_deref() == Some("h264") {
        let header = picture.sequence_header?;
        return Some(Coding {
            width: i32::try_from(header.coded_width).ok()?,
            height: i32::try_from(header.coded_height).ok()?,
            ref_frames: i32::try_from(header.ref_frames).ok()?,
            has_scaling_matrix: Some(header.has_scaling_matrix),
        });
    }
    Some(Coding {
        width: i32::try_from(picture.coded_width?).ok()?,
        height: i32::try_from(picture.coded_height?).ok()?,
        ref_frames: 1,
        has_scaling_matrix: None,
    })
}

fn is_square(sample_aspect_ratio: &str) -> bool {
    match sample_aspect_ratio.split_once(':') {
        Some((numerator, denominator)) => numerator == denominator || numerator == "0",
        None => true,
    }
}

/// The video's rate when its header declares none: what the container's rate leaves once the
/// audio tracks' declared rates are taken out (Soul: 7726375 − 256000 − 768000 = 6702375,
/// stored exactly by the reference).
fn video_bitrate_fallback(analysis: &Analysis) -> Option<u64> {
    let audio: u64 = analysis
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
        .filter_map(|track| track.bitrate)
        .sum();
    analysis.bitrate.map(|total| total.saturating_sub(audio))
}

fn dolby_vision_row(stream: i64, record: DolbyVision) -> stream_dolby_vision::Model {
    stream_dolby_vision::Model {
        stream_id: stream,
        version: format!("{}.{}", record.version_major, record.version_minor),
        profile: i32::try_from(record.profile).unwrap_or(i32::MAX),
        level: i32::try_from(record.level).unwrap_or(i32::MAX),
        base_layer_compatibility: i32::try_from(record.bl_compatibility_id).ok(),
        has_base_layer: record.bl_present,
        has_enhancement_layer: record.el_present,
        has_rpu: record.rpu_present,
    }
}

/// A subtitle file beside the media, as the reference records one: an embedded track's row
/// with no index, and the file's location as a URL.
pub(super) fn sidecar_row(sidecar: &NewSidecar, part: i64, id: i64) -> media_stream::Model {
    let codec = sidecar.codec.to_ascii_lowercase();
    media_stream::Model {
        id,
        media_part_id: part,
        kind: "subtitle".to_owned(),
        position: 0,
        codec: Some(codec.clone()),
        language: sidecar.language.as_deref().and_then(language),
        channels: None,
        bitrate: None,
        is_default: false,
        is_forced: sidecar.forced,
        external_path: Some(file_url(&sidecar.path)),
        title: None,
        stream_identifier: None,
        profile: None,
        bit_depth: None,
        width: None,
        height: None,
        coded_width: None,
        coded_height: None,
        frame_rate: None,
        level: None,
        ref_frames: None,
        scan_type: None,
        chroma_subsampling: None,
        chroma_location: None,
        color_range: None,
        color_space: None,
        color_primaries: None,
        color_transfer: None,
        has_scaling_matrix: None,
        is_anamorphic: None,
        pixel_aspect_ratio: None,
        fourcc: None,
        has_closed_captions: false,
        has_separate_fields: false,
        has_hdr10_plus: false,
        sampling_rate: None,
        channel_layout: None,
        subtitle_format: Some(codec),
        is_embedded_in_video: false,
        is_original: false,
        is_dub: false,
        is_commentary: false,
        is_hearing_impaired: sidecar.hearing_impaired,
        is_visual_impaired: false,
        has_header_compression: false,
        required_bandwidths: None,
    }
}

/// The `file://` URL the reference stores for a sidecar, escaping what `core::sidecar` decodes
/// (spaces and `%`): `C:\Films\a b.srt` is `file:///C:/Films/a%20b.srt`.
pub(super) fn file_url(path: &str) -> String {
    let forward = path
        .replace('\\', "/")
        .replace('%', "%25")
        .replace(' ', "%20");
    if forward.starts_with('/') {
        format!("file://{forward}")
    } else {
        format!("file:///{forward}")
    }
}

/// Whether the file carries a cover: an attached picture, or an image attachment.
pub(super) fn has_thumbnail(analysis: &Analysis) -> bool {
    analysis.tracks.iter().any(|track| {
        track.disposition.attached_picture
            || track
                .attachment
                .as_ref()
                .and_then(|attachment| attachment.mime_type.as_deref())
                .is_some_and(|mime| mime.starts_with("image/"))
    })
}

const fn kind_name(kind: TrackKind) -> Option<&'static str> {
    match kind {
        TrackKind::Video => Some("video"),
        TrackKind::Audio => Some("audio"),
        TrackKind::Subtitle => Some("subtitle"),
        TrackKind::Attachment | TrackKind::Data => None,
    }
}

pub(super) fn to_i32(value: u64) -> Option<i32> {
    i32::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;

    #[test]
    fn names_containers_as_the_reference() {
        assert_eq!(container("matroska,webm", "a.mkv").as_deref(), Some("mkv"));
        assert_eq!(
            container("mov,mp4,m4a,3gp,3g2,mj2", "C:\\x\\a.MOV").as_deref(),
            Some("mov")
        );
        assert_eq!(
            container("mov,mp4,m4a,3gp,3g2,mj2", "/x/a.m4v").as_deref(),
            Some("mp4")
        );
        assert_eq!(container("mpegts", "a.ts").as_deref(), Some("mpegts"));
        assert_eq!(container("avi", "a.avi").as_deref(), Some("avi"));
        assert_eq!(container("", "a.avi"), None);
    }

    #[test]
    fn a_release_name_tells_which_french_dub_it_carries() {
        assert_eq!(
            dub_language(r"D:\Films\Le Film.2020.VFQ.1080p.mkv"),
            Some("fr-CA")
        );
        assert_eq!(dub_language("/films/Le.Film.2020.vff.mkv"), Some("fr-FR"));
        assert_eq!(dub_language("Le Film [VFI] 2020.mkv"), Some("fr-FR"));
        assert_eq!(
            dub_language("Bull.2016.S05E01.FRENCH.720p.WEB.H264-FREAMON.mkv"),
            Some("fr-FR")
        );
        assert_eq!(dub_language("Film.FRENCH.VFQ.mkv"), Some("fr-CA"));
        assert_eq!(
            dub_language("Soul.2020.TRUEFRENCH.1080p.mkv"),
            Some("fr-FR")
        );
        // A whole word only, and the file's name only.
        assert_eq!(dub_language("Frenchie.2019.mkv"), None);
        assert_eq!(dub_language("/VFQ/Film.2020.mkv"), None);
        assert_eq!(dub_language("Film.2020.vfq"), None);
    }

    #[test]
    fn names_codecs_as_the_reference() {
        assert_eq!(codec("subrip"), "srt");
        assert_eq!(codec("dts"), "dca");
        assert_eq!(codec("hdmv_pgs_subtitle"), "pgs");
        assert_eq!(codec("pcm_s24le"), "pcm");
        assert_eq!(codec("h264"), "h264");
    }

    #[test]
    fn shortens_languages_that_have_a_two_letter_code() {
        assert_eq!(language("fre").as_deref(), Some("fr"));
        assert_eq!(language("fra").as_deref(), Some("fr"));
        assert_eq!(language("ger").as_deref(), Some("de"));
        assert_eq!(language("eng").as_deref(), Some("en"));
        assert_eq!(language("fr").as_deref(), Some("fr"));
        assert_eq!(language("pt-BR").as_deref(), Some("pt-BR"));
        assert_eq!(language("mul").as_deref(), Some("mul"));
        assert_eq!(language("und"), None);
        assert_eq!(language(""), None);
    }

    #[test]
    fn the_language_table_is_sorted_for_its_search() {
        assert!(super::super::languages::ALPHA2
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn lower_cases_profiles_and_names_dts_by_its_extension() {
        assert_eq!(profile("High"), "high");
        assert_eq!(profile("Main 10"), "main 10");
        assert_eq!(profile("LC"), "lc");
        assert_eq!(
            profile("Dolby Digital Plus + Dolby Atmos"),
            "dolby digital plus + dolby atmos"
        );
        assert_eq!(profile("DTS-HD MA"), "ma");
        assert_eq!(profile("DTS"), "dts");
    }

    #[test]
    fn reads_subsampling_and_scan_type() {
        assert_eq!(chroma_subsampling("yuv420p"), Some("4:2:0"));
        assert_eq!(chroma_subsampling("yuv420p10le"), Some("4:2:0"));
        assert_eq!(chroma_subsampling("yuv422p"), Some("4:2:2"));
        assert_eq!(chroma_subsampling("rgb24"), None);
        assert_eq!(scan_type("progressive"), Some("progressive"));
        assert_eq!(scan_type("tt"), Some("interlaced"));
        assert_eq!(scan_type("unknown"), None);
    }

    #[test]
    fn writes_sidecar_locations_as_core_reads_them() {
        assert_eq!(
            file_url("C:\\Films\\a b%.fr.srt"),
            "file:///C:/Films/a%20b%25.fr.srt"
        );
        assert_eq!(file_url("/mnt/films/a.srt"), "file:///mnt/films/a.srt");
    }

    #[test]
    fn stores_frame_rates_and_ratios_at_the_reference_precision() {
        assert_eq!(as_stored_float(24000.0 / 1001.0), 23.976_024_627_685_547);
        assert_eq!(sample_aspect_ratio(Some("1:1")), 1.0);
        assert_eq!(sample_aspect_ratio(Some("0:1")), 1.0);
        assert_eq!(sample_aspect_ratio(Some("4:3")), 4.0 / 3.0);
        assert!(is_square("1:1") && is_square("0:1"));
        assert!(!is_square("16:15"));
    }
}
