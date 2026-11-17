//! The analysis: container, tracks and chapters, from ffprobe.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::engine::{hashes, input, mp4, run, ScanError, Streaming, Tools};
use crate::report::{
    Analysis, Attachment, AudioTrack, Chapter, DeepAnalysis, Disposition, DolbyVision,
    MeasuredTrack, Track, TrackKind, VideoTrack,
};

/// Reads what the file declares; with `deep`, also every packet's size, which measures each
/// track's real bitrate (the reference's deep analysis: demuxed, not decoded).
///
/// # Errors
///
/// [`ScanError::Unreadable`] when ffprobe cannot open the media, [`ScanError::File`] when the
/// file cannot be read at all.
pub fn probe(tools: &Tools, path: &Path, deep: bool) -> Result<Analysis, ScanError> {
    let file = hashes::identify(path)?;
    let probed = ffprobe(tools, path)?;
    let duration_ms = probed.format.duration.as_deref().and_then(milliseconds);
    let container = probed.format.format_name.unwrap_or_default();
    let mut analysis = Analysis {
        file,
        duration_ms,
        bitrate: probed.format.bit_rate.as_deref().and_then(integer),
        optimized_for_streaming: None,
        has_64bit_offsets: None,
        packet_length: None,
        tracks: probed.streams.iter().filter_map(track).collect(),
        chapters: chapters(probed.chapters),
        deep: None,
        container,
    };
    if analysis.container.split(',').any(|name| name == "mp4") {
        // The layout is a nicety; a file ffprobe could read but this walk cannot stays analyzed.
        if let Ok(layout) = mp4::layout(path) {
            analysis.optimized_for_streaming = Some(layout.optimized_for_streaming);
            analysis.has_64bit_offsets = Some(layout.has_64bit_offsets);
        }
    }
    if analysis.container == "mpegts" {
        analysis.packet_length = transport_packet_length(path);
    }
    if deep {
        analysis.deep = Some(measure_packets(tools, path, duration_ms)?);
    }
    Ok(analysis)
}

/// The duration ffprobe reads from the container, for commands sized by it.
pub(crate) fn duration_ms(tools: &Tools, path: &Path) -> Result<Option<u64>, ScanError> {
    let output = run(
        "ffprobe",
        Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_entries",
                "format=duration",
                "-i",
            ])
            .arg(input(path)),
    )?;
    let probed: Probed = parse(&output)?;
    Ok(probed.format.duration.as_deref().and_then(milliseconds))
}

/// The tracks of the file, for commands that pick one.
pub(crate) fn tracks(tools: &Tools, path: &Path) -> Result<Vec<Track>, ScanError> {
    let output = run(
        "ffprobe",
        Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_streams",
                "-i",
            ])
            .arg(input(path)),
    )?;
    let probed: Probed = parse(&output)?;
    Ok(probed.streams.iter().filter_map(track).collect())
}

pub(crate) fn chapters_of(tools: &Tools, path: &Path) -> Result<Vec<Chapter>, ScanError> {
    let output = run(
        "ffprobe",
        Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_chapters",
                "-i",
            ])
            .arg(input(path)),
    )?;
    let probed: Probed = parse(&output)?;
    Ok(chapters(probed.chapters))
}

fn ffprobe(tools: &Tools, path: &Path) -> Result<Probed, ScanError> {
    let output = run(
        "ffprobe",
        Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
                "-show_chapters",
                "-i",
            ])
            .arg(input(path)),
    )?;
    parse(&output)
}

fn parse(output: &[u8]) -> Result<Probed, ScanError> {
    serde_json::from_slice(output).map_err(|error| ScanError::Output {
        tool: "ffprobe",
        message: error.to_string(),
    })
}

fn measure_packets(
    tools: &Tools,
    path: &Path,
    duration_ms: Option<u64>,
) -> Result<DeepAnalysis, ScanError> {
    let mut child = Streaming::spawn(
        "ffprobe",
        Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "packet=stream_index,size",
                "-of",
                "csv=p=0",
                "-i",
            ])
            .arg(input(path)),
    )?;
    let mut totals: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
    for line in BufReader::new(child.stdout()?).lines() {
        let line = line.map_err(|error| ScanError::Output {
            tool: "ffprobe",
            message: error.to_string(),
        })?;
        let mut fields = line.trim().split(',');
        let (Some(index), Some(size)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(index), Ok(size)) = (index.parse::<u32>(), size.parse::<u64>()) else {
            continue;
        };
        let total = totals.entry(index).or_default();
        total.0 += 1;
        total.1 += size;
    }
    child.finish()?;
    let rate = |bytes: u64| match duration_ms {
        Some(ms) if ms > 0 => bytes.saturating_mul(8_000) / ms,
        _ => 0,
    };
    let tracks: Vec<MeasuredTrack> = totals
        .into_iter()
        .map(|(index, (packets, bytes))| MeasuredTrack {
            index,
            packets,
            bytes,
            bitrate: rate(bytes),
        })
        .collect();
    Ok(DeepAnalysis {
        bitrate: rate(tracks.iter().map(|track| track.bytes).sum()),
        tracks,
    })
}

/// 188 bytes, or 192 for the timecoded packets of Blu-ray's M2TS, told by where the sync
/// bytes fall.
fn transport_packet_length(path: &Path) -> Option<u32> {
    use std::io::Read;
    let mut head = [0; 400];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if head[0] == 0x47 && head[188] == 0x47 && head[376] == 0x47 {
        Some(188)
    } else if head[4] == 0x47 && head[196] == 0x47 && head[388] == 0x47 {
        Some(192)
    } else {
        None
    }
}

fn track(stream: &Stream) -> Option<Track> {
    let kind = match stream.codec_type.as_deref()? {
        "video" => TrackKind::Video,
        "audio" => TrackKind::Audio,
        "subtitle" => TrackKind::Subtitle,
        "attachment" => TrackKind::Attachment,
        _ => TrackKind::Data,
    };
    let tag = |name: &str| {
        stream
            .tags
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    // Matroska writes its statistics per language, `BPS-eng`, when the muxer was told one.
    let tagged_bitrate = stream
        .tags
        .iter()
        .find(|(key, _)| {
            key.eq_ignore_ascii_case("BPS") || key.to_ascii_uppercase().starts_with("BPS-")
        })
        .and_then(|(_, value)| integer(value));
    let flag = |name: &str| {
        stream
            .disposition
            .get(name)
            .is_some_and(|value| *value != 0)
    };
    let disposition = Disposition {
        default: flag("default"),
        forced: flag("forced"),
        hearing_impaired: flag("hearing_impaired"),
        visual_impaired: flag("visual_impaired"),
        original: flag("original"),
        dub: flag("dub"),
        comment: flag("comment"),
        attached_picture: flag("attached_pic"),
    };
    let video = (kind == TrackKind::Video).then(|| VideoTrack {
        width: stream.width.unwrap_or(0),
        height: stream.height.unwrap_or(0),
        coded_width: stream.coded_width.filter(|width| *width > 0),
        coded_height: stream.coded_height.filter(|height| *height > 0),
        level: stream.level.filter(|level| *level >= 0),
        pixel_format: stream.pix_fmt.clone(),
        bit_depth: stream
            .bits_per_raw_sample
            .as_deref()
            .and_then(|bits| bits.parse().ok()),
        color_range: stream.color_range.clone(),
        color_space: stream.color_space.clone(),
        color_primaries: stream.color_primaries.clone(),
        color_transfer: stream.color_transfer.clone(),
        chroma_location: stream.chroma_location.clone(),
        frame_rate: stream.r_frame_rate.as_deref().and_then(ratio),
        field_order: stream.field_order.clone(),
        sample_aspect_ratio: stream.sample_aspect_ratio.clone(),
        display_aspect_ratio: stream.display_aspect_ratio.clone(),
        closed_captions: stream.closed_captions.is_some_and(|captions| captions != 0),
        dolby_vision: stream.side_data_list.iter().find_map(dolby_vision),
    });
    let audio = (kind == TrackKind::Audio).then(|| AudioTrack {
        sample_rate: stream
            .sample_rate
            .as_deref()
            .and_then(|rate| rate.parse().ok()),
        channels: stream.channels,
        channel_layout: stream.channel_layout.clone(),
        bit_depth: stream
            .bits_per_raw_sample
            .as_deref()
            .and_then(|bits| bits.parse().ok())
            .filter(|bits| *bits > 0),
    });
    let attachment = (kind == TrackKind::Attachment).then(|| Attachment {
        mime_type: tag("mimetype"),
        filename: tag("filename"),
    });
    Some(Track {
        index: stream.index,
        kind,
        codec: stream.codec_name.clone(),
        codec_tag: stream
            .codec_tag_string
            .clone()
            .filter(|tag| !tag.starts_with("[0]") && tag != "0x0000"),
        profile: stream.profile.clone(),
        language: tag("language").filter(|language| language != "und"),
        title: tag("title"),
        bitrate: stream.bit_rate.as_deref().and_then(integer),
        tagged_bitrate,
        disposition,
        video,
        audio,
        attachment,
    })
}

fn dolby_vision(side_data: &SideData) -> Option<DolbyVision> {
    if side_data.side_data_type.as_deref() != Some("DOVI configuration record") {
        return None;
    }
    Some(DolbyVision {
        version_major: side_data.dv_version_major?,
        version_minor: side_data.dv_version_minor.unwrap_or(0),
        profile: side_data.dv_profile?,
        level: side_data.dv_level.unwrap_or(0),
        rpu_present: side_data.rpu_present_flag == Some(1),
        el_present: side_data.el_present_flag == Some(1),
        bl_present: side_data.bl_present_flag == Some(1),
        bl_compatibility_id: side_data.dv_bl_signal_compatibility_id.unwrap_or(0),
    })
}

fn chapters(probed: Vec<ProbedChapter>) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = probed
        .into_iter()
        .zip(1..)
        .map(|(chapter, index)| Chapter {
            index,
            start_ms: chapter
                .start_time
                .as_deref()
                .and_then(milliseconds)
                .unwrap_or(0),
            end_ms: chapter
                .end_time
                .as_deref()
                .and_then(milliseconds)
                .unwrap_or(0),
            title: chapter
                .tags
                .get("title")
                .cloned()
                .filter(|title| !title.trim().is_empty()),
        })
        .collect();
    // The reference clears the titles when they only number the chapters ("Chapters had
    // generic titles, clearing"): a client then names them itself.
    if chapters
        .iter()
        .all(|chapter| chapter.title.as_deref().is_none_or(is_generic_title))
    {
        for chapter in &mut chapters {
            chapter.title = None;
        }
    }
    chapters
}

/// `Chapter 3`, `chapter.3`, `03`: the reference's `(?:chapter)?\.? ?(\d+)`, whole.
fn is_generic_title(title: &str) -> bool {
    let lower = title.trim().to_ascii_lowercase();
    let rest = lower.strip_prefix("chapter").unwrap_or(&lower);
    let rest = rest.strip_prefix('.').unwrap_or(rest);
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
}

fn milliseconds(seconds: &str) -> Option<u64> {
    let seconds: f64 = seconds.parse().ok()?;
    if seconds.is_finite() && seconds >= 0.0 {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked finite and non-negative; a duration fits u64 milliseconds"
        )]
        Some((seconds * 1000.0).round() as u64)
    } else {
        None
    }
}

fn integer(value: &str) -> Option<u64> {
    value.trim().parse().ok()
}

fn ratio(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator: f64 = numerator.parse().ok()?;
    let denominator: f64 = denominator.parse().ok()?;
    (denominator != 0.0 && numerator > 0.0).then(|| numerator / denominator)
}

#[derive(Debug, Deserialize)]
struct Probed {
    #[serde(default)]
    format: Format,
    #[serde(default)]
    streams: Vec<Stream>,
    #[serde(default)]
    chapters: Vec<ProbedChapter>,
}

#[allow(clippy::struct_field_names, reason = "named as ffprobe prints them")]
#[derive(Debug, Default, Deserialize)]
struct Format {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Stream {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_tag_string: Option<String>,
    profile: Option<String>,
    level: Option<i32>,
    width: Option<u32>,
    height: Option<u32>,
    coded_width: Option<u32>,
    coded_height: Option<u32>,
    pix_fmt: Option<String>,
    bits_per_raw_sample: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_primaries: Option<String>,
    color_transfer: Option<String>,
    chroma_location: Option<String>,
    field_order: Option<String>,
    r_frame_rate: Option<String>,
    sample_aspect_ratio: Option<String>,
    display_aspect_ratio: Option<String>,
    closed_captions: Option<u8>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    bit_rate: Option<String>,
    #[serde(default)]
    disposition: BTreeMap<String, u8>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[allow(clippy::struct_field_names, reason = "named as ffprobe prints them")]
#[derive(Debug, Deserialize)]
struct SideData {
    side_data_type: Option<String>,
    dv_version_major: Option<u32>,
    dv_version_minor: Option<u32>,
    dv_profile: Option<u32>,
    dv_level: Option<u32>,
    rpu_present_flag: Option<u8>,
    el_present_flag: Option<u8>,
    bl_present_flag: Option<u8>,
    dv_bl_signal_compatibility_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ProbedChapter {
    start_time: Option<String>,
    end_time: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn numbered_chapter_titles_are_generic() {
        for title in ["Chapter 3", "chapter.12", "Chapter3", "07", "Chapter. 1"] {
            assert!(is_generic_title(title), "{title}");
        }
        for title in ["Chapter", "Opening", "Chapter 3: The Hall", "1.5"] {
            assert!(!is_generic_title(title), "{title}");
        }
    }

    #[test]
    fn clears_titles_only_when_every_one_is_generic() {
        let probed = |titles: &[&str]| -> Vec<ProbedChapter> {
            titles
                .iter()
                .map(|title| ProbedChapter {
                    start_time: Some("0.000000".to_owned()),
                    end_time: Some("1.000000".to_owned()),
                    tags: BTreeMap::from([("title".to_owned(), (*title).to_owned())]),
                })
                .collect()
        };
        let generic = chapters(probed(&["Chapter 1", "Chapter 2"]));
        assert!(generic.iter().all(|chapter| chapter.title.is_none()));
        let named = chapters(probed(&["Chapter 1", "Credits"]));
        assert_eq!(named[1].title.as_deref(), Some("Credits"));
        assert_eq!(named[0].title.as_deref(), Some("Chapter 1"));
        assert_eq!(named[1].index, 2);
    }

    #[test]
    fn reads_ffprobe_seconds_and_rates() {
        assert_eq!(milliseconds("1248.664000"), Some(1_248_664));
        assert_eq!(milliseconds("N/A"), None);
        assert!((ratio("24000/1001").unwrap() - 23.976_023_976).abs() < 1e-9);
        assert_eq!(ratio("0/0"), None);
    }
}
