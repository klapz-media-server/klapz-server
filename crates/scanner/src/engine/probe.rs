//! The analysis: container, tracks and chapters, from ffprobe.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::engine::{hashes, input, run, ScanError, Streaming, Tools};
use crate::report::{
    Analysis, Attachment, AudioTrack, Chapter, Cropping, DeepAnalysis, Disposition, DolbyVision,
    FileIdentity, MeasuredTrack, SequenceHeader, Statistics, SubtitleTrack, Track, TrackKind,
    VideoTrack,
};

/// Reads what the file declares: its headers and the probe window, which the reference reads
/// when a scan finds a file. [`measure`] is the rest of its deep analysis.
///
/// # Errors
///
/// [`ScanError::Unreadable`] when ffprobe cannot open the media, [`ScanError::File`] when the
/// file cannot be read at all.
pub async fn probe(tools: &Tools, path: &Path) -> Result<Analysis, ScanError> {
    let file = hashes::identify(path)?;
    let output = ffprobe(tools, path).await?;
    analysis_from_ffprobe(&output, file)
}

/// What ffprobe's `-show_format -show_streams -show_chapters` JSON says of a file: everything
/// of an analysis but the deep analysis.
///
/// ffmpeg-plex states what a stock ffprobe does not: an MP4's layout (`moov_before_mdat`,
/// `co64`) and an MPEG-TS packet's length (`ts_packetsize`) with each stream, H.264's coded
/// size, reference frames and scaling matrix from its sequence header, and no language for a
/// Matroska track declaring `und`. An older build leaves them out, and so does the analysis.
///
/// # Errors
///
/// [`ScanError::Output`] when the output is not ffprobe's JSON.
pub fn analysis_from_ffprobe(output: &[u8], file: FileIdentity) -> Result<Analysis, ScanError> {
    let probed = parse(output)?;
    let container = probed.format.format_name.unwrap_or_default();
    let is = |name: &str| container.split(',').any(|format| format == name);
    // The demuxer's own figures, which ffprobe repeats on every stream.
    let first = probed.streams.first();
    let exported =
        |flag: fn(&Stream) -> Option<&str>| first.and_then(flag).map(|value| value == "true");
    let mut tracks: Vec<Track> = probed.streams.iter().filter_map(track).collect();
    if is("matroska") {
        read_absent_languages_as_english(&mut tracks);
    }
    Ok(Analysis {
        file,
        duration_ms: probed.format.duration.as_deref().and_then(milliseconds),
        duration_us: probed
            .format
            .duration
            .as_deref()
            .and_then(microseconds)
            .and_then(|us| u64::try_from(us).ok()),
        bitrate: probed.format.bit_rate.as_deref().and_then(integer),
        optimized_for_streaming: if is("mp4") {
            exported(|stream| stream.moov_before_mdat.as_deref())
        } else {
            None
        },
        has_64bit_offsets: if is("mp4") {
            exported(|stream| stream.co64.as_deref())
        } else {
            None
        },
        packet_length: if is("mpegts") {
            first
                .and_then(|stream| stream.ts_packetsize.as_deref())
                .and_then(|length| length.parse().ok())
                .filter(|length| *length > 0)
        } else {
            None
        },
        tracks,
        chapters: chapters(probed.chapters),
        deep: None,
        container,
    })
}

/// The duration ffprobe reads from the container, for commands sized by it.
pub(crate) async fn duration_ms(tools: &Tools, path: &Path) -> Result<Option<u64>, ScanError> {
    let output = run(
        "ffprobe",
        tools
            .ffprobe()
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
    )
    .await?;
    let probed: Probed = parse(&output)?;
    Ok(probed.format.duration.as_deref().and_then(milliseconds))
}

/// The tracks of the file, for commands that pick one.
pub(crate) async fn tracks(tools: &Tools, path: &Path) -> Result<Vec<Track>, ScanError> {
    let output = run(
        "ffprobe",
        tools
            .ffprobe()
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_streams",
                "-i",
            ])
            .arg(input(path)),
    )
    .await?;
    let probed: Probed = parse(&output)?;
    Ok(probed.streams.iter().filter_map(track).collect())
}

pub(crate) async fn chapters_of(tools: &Tools, path: &Path) -> Result<Vec<Chapter>, ScanError> {
    let output = run(
        "ffprobe",
        tools
            .ffprobe()
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_chapters",
                "-i",
            ])
            .arg(input(path)),
    )
    .await?;
    let probed: Probed = parse(&output)?;
    Ok(chapters(probed.chapters))
}

async fn ffprobe(tools: &Tools, path: &Path) -> Result<Vec<u8>, ScanError> {
    run(
        "ffprobe",
        tools
            .ffprobe()
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
    )
    .await
}

fn parse(output: &[u8]) -> Result<Probed, ScanError> {
    serde_json::from_slice(output).map_err(|error| ScanError::Output {
        tool: "ffprobe",
        message: error.to_string(),
    })
}

/// Every packet's size and time, which measures each track's real bitrate: the reference's
/// deep analysis, demuxed and not decoded, so it reads the whole file. `duration_ms`, what the
/// headers declare, stands in for a span the packets do not give.
///
/// # Errors
///
/// As [`probe`].
pub async fn measure(
    tools: &Tools,
    path: &Path,
    duration_ms: Option<u64>,
) -> Result<DeepAnalysis, ScanError> {
    let mut child = Streaming::spawn(
        "ffprobe",
        tools
            .ffprobe()
            .args([
                "-v",
                "error",
                "-show_entries",
                "packet=stream_index,pts_time,dts_time,duration_time,size",
                "-of",
                "compact=p=0",
                "-i",
            ])
            .arg(input(path)),
    )?;
    let mut totals: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
    let mut span = Span::default();
    let mut lines = BufReader::new(child.stdout()?).lines();
    while let Some(line) = lines.next_line().await.map_err(|error| ScanError::Output {
        tool: "ffprobe",
        message: error.to_string(),
    })? {
        let Some(packet) = Packet::parse(&line) else {
            continue;
        };
        let total = totals.entry(packet.stream).or_default();
        total.0 += 1;
        total.1 += packet.size;
        if let Some(start) = packet.start_us {
            span.cover(start, start + packet.duration_us.unwrap_or(0));
        }
    }
    child.finish().await?;
    // The reference divides by the time the packets cover, not by the container's duration:
    // Soul's last video packet ends at 6031.567 s where its header says 6031.568, and every
    // rate it stored is the truncated quotient by the former.
    let measured_us = span.length_us();
    let span_us = measured_us.or_else(|| duration_ms.map(|ms| ms.saturating_mul(1000)));
    let rate = |bytes: u64| match span_us {
        Some(us) if us > 0 => {
            let bits = u128::from(bytes) * 8_000_000;
            u64::try_from(bits / u128::from(us)).unwrap_or(u64::MAX)
        }
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
        duration_ms: measured_us.map(|us| us / 1000),
        bitrate: rate(tracks.iter().map(|track| track.bytes).sum()),
        tracks,
    })
}

/// One line of `-show_entries packet=… -of compact=p=0`: `stream_index=0|pts_time=…|…`.
#[derive(Debug, PartialEq)]
struct Packet {
    stream: u32,
    size: u64,
    /// The presentation time, or the decoding time for a packet that has none.
    start_us: Option<i64>,
    duration_us: Option<i64>,
}

impl Packet {
    fn parse(line: &str) -> Option<Self> {
        let field = |name: &str| {
            line.trim().split('|').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                (key == name).then_some(value)
            })
        };
        Some(Self {
            stream: field("stream_index")?.parse().ok()?,
            size: field("size")?.parse().ok()?,
            start_us: field("pts_time")
                .and_then(microseconds)
                .or_else(|| field("dts_time").and_then(microseconds)),
            duration_us: field("duration_time").and_then(microseconds),
        })
    }
}

/// The earliest start and the latest end seen among the packets.
#[derive(Debug, Default)]
struct Span {
    covered: Option<(i64, i64)>,
}

impl Span {
    fn cover(&mut self, start: i64, end: i64) {
        self.covered = Some(match self.covered {
            Some((first, last)) => (first.min(start), last.max(end)),
            None => (start, end),
        });
    }

    fn length_us(&self) -> Option<u64> {
        let (first, last) = self.covered?;
        u64::try_from(last - first)
            .ok()
            .filter(|length| *length > 0)
    }
}

/// ffprobe's seconds, `-0.042000` or `6031.567000`, in whole microseconds: read as decimals,
/// since a float would put a rate one unit off.
fn microseconds(seconds: &str) -> Option<i64> {
    let (negative, digits) = match seconds.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, seconds),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut micros = whole.parse::<i64>().ok()?.checked_mul(1_000_000)?;
    for (position, digit) in fraction.bytes().take(6).enumerate() {
        let weight = 10_i64.pow(5 - u32::try_from(position).ok()?);
        micros = micros.checked_add(i64::from(digit - b'0') * weight)?;
    }
    Some(if negative { -micros } else { micros })
}

/// Plex's reading of a Matroska track that states no language at all, neither declaring one
/// nor tagged with one: English, as the reference's ffmpeg-plex reported both a track without a
/// `Language` element, English by the format's default, and one declaring `und`. What the file
/// declares stays as it is. Attachments, cover pictures among them, are no track entries and
/// never had one.
fn read_absent_languages_as_english(tracks: &mut [Track]) {
    for track in tracks {
        let entry = track.kind != TrackKind::Attachment && !track.disposition.attached_picture;
        if entry && track.declared_language.is_none() {
            track.language = Some("eng".to_owned());
        }
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
    // Not `BPS-eng`, the name an older mkvmerge gave the same statistic: the reference reads
    // `BPS` alone, and left Soul's subtitles, tagged that way, without a rate.
    let tagged_bitrate = tag("BPS").as_deref().and_then(integer);
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
    let video = (kind == TrackKind::Video).then(|| video_track(stream));
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
    let subtitle = (kind == TrackKind::Subtitle)
        .then(|| {
            Some(SubtitleTrack {
                width: stream.width.filter(|width| *width > 0)?,
                height: stream.height.filter(|height| *height > 0)?,
            })
        })
        .flatten();
    // A Matroska attachment that is an image comes as a cover picture, still named and typed.
    let attached = kind == TrackKind::Attachment
        || (disposition.attached_picture
            && (tag("filename").is_some() || tag("mimetype").is_some()));
    let attachment = attached.then(|| Attachment {
        mime_type: tag("mimetype"),
        filename: tag("filename"),
    });
    Some(Track {
        index: stream.index,
        identifier: stream
            .id
            .as_deref()
            .and_then(|id| u32::from_str_radix(id.strip_prefix("0x")?, 16).ok()),
        kind,
        codec: stream.codec_name.clone(),
        codec_tag: stream
            .codec_tag_string
            .clone()
            .filter(|tag| !tag.starts_with("[0]") && tag != "0x0000"),
        raw_codec_tag: stream.codec_tag_string.clone(),
        time_base: stream.time_base.clone(),
        profile: stream.profile.clone(),
        language: tag("language").filter(|language| language != "und"),
        declared_language: tag("language"),
        title: tag("title"),
        name: tag("name"),
        handler_name: tag("handler_name"),
        bitrate: stream.bit_rate.as_deref().and_then(integer),
        tagged_bitrate,
        statistics: statistics(&tag),
        disposition,
        video,
        audio,
        subtitle,
        attachment,
    })
}

/// mkvmerge's statistics tags, under either of their names.
fn statistics(tag: &dyn Fn(&str) -> Option<String>) -> Option<Statistics> {
    let either = |name: &str| tag(name).or_else(|| tag(&format!("{name}-eng")));
    let statistics = Statistics {
        bitrate: either("BPS").as_deref().and_then(integer),
        bytes: either("NUMBER_OF_BYTES").as_deref().and_then(integer),
        duration_us: either("DURATION").as_deref().and_then(tag_duration_us),
    };
    (statistics != Statistics::default()).then_some(statistics)
}

/// A `DURATION` tag, `00:00:50.040000000`, in microseconds.
fn tag_duration_us(text: &str) -> Option<u64> {
    let mut parts = text.trim().splitn(3, ':');
    let hours: u64 = parts.next()?.parse().ok()?;
    let minutes: u64 = parts.next()?.parse().ok()?;
    let seconds = u64::try_from(microseconds(parts.next()?)?).ok()?;
    hours
        .checked_mul(3_600_000_000)?
        .checked_add(minutes.checked_mul(60_000_000)?)?
        .checked_add(seconds)
}

fn video_track(stream: &Stream) -> VideoTrack {
    VideoTrack {
        width: stream.width.unwrap_or(0),
        height: stream.height.unwrap_or(0),
        coded_width: stream.coded_width.filter(|width| *width > 0),
        coded_height: stream.coded_height.filter(|height| *height > 0),
        sequence_header: sequence_header(stream),
        level: stream.level.filter(|level| *level >= 0),
        pixel_format: stream.pix_fmt.clone(),
        // ffprobe states the depth of H.264 alone; for HEVC, AV1, VP9 and the rest it is the
        // pixel format's, which is where the reference reads it too.
        bit_depth: stream
            .bits_per_raw_sample
            .as_deref()
            .and_then(|bits| bits.parse().ok())
            .or_else(|| pixel_depth(stream.pix_fmt.as_deref()?)),
        color_range: stream.color_range.clone(),
        color_space: stream.color_space.clone(),
        color_primaries: stream.color_primaries.clone(),
        color_transfer: stream.color_transfer.clone(),
        chroma_location: stream.chroma_location.clone(),
        frame_rate: stream
            .avg_frame_rate
            .as_deref()
            .and_then(ratio)
            .or_else(|| stream.r_frame_rate.as_deref().and_then(ratio)),
        average_frame_rate: stream.avg_frame_rate.clone(),
        real_frame_rate: stream.r_frame_rate.clone(),
        is_avc: stream.is_avc.as_deref().map(|framing| framing == "true"),
        nal_length_size: stream
            .nal_length_size
            .as_deref()
            .and_then(|size| size.parse().ok()),
        field_order: stream.field_order.clone(),
        sample_aspect_ratio: stream.sample_aspect_ratio.clone(),
        display_aspect_ratio: stream.display_aspect_ratio.clone(),
        cropping: stream.side_data_list.iter().find_map(cropping),
        closed_captions: stream.closed_captions.is_some_and(|captions| captions != 0),
        dolby_vision: stream.side_data_list.iter().find_map(dolby_vision),
    }
}

/// H.264's sequence parameter set as ffmpeg-plex states it: the coded size before cropping
/// (1088 lines for a 1080p picture), the reference frames and whether a scaling matrix is
/// present, where a stock ffprobe gives the cropped size and no reference count. Its
/// `scaling_matrix_present`, printed for H.264 alone, tells the two apart.
fn sequence_header(stream: &Stream) -> Option<SequenceHeader> {
    if stream.codec_name.as_deref() != Some("h264") {
        return None;
    }
    Some(SequenceHeader {
        coded_width: stream.coded_width.filter(|width| *width > 0)?,
        coded_height: stream.coded_height.filter(|height| *height > 0)?,
        ref_frames: stream.refs?,
        has_scaling_matrix: stream.scaling_matrix_present? != 0,
    })
}

fn cropping(side_data: &SideData) -> Option<Cropping> {
    if side_data.side_data_type.as_deref() != Some("Frame Cropping") {
        return None;
    }
    let cropping = Cropping {
        top: side_data.crop_top.unwrap_or(0),
        bottom: side_data.crop_bottom.unwrap_or(0),
        left: side_data.crop_left.unwrap_or(0),
        right: side_data.crop_right.unwrap_or(0),
    };
    (cropping != Cropping::default()).then_some(cropping)
}

/// The bits per component a pixel format names: `yuv420p10le` 10, `p010le` 10, `yuv420p`,
/// `nv12` and `gray` 8. Nothing for the packed formats, whose digits count whole pixels.
fn pixel_depth(pixel_format: &str) -> Option<u32> {
    let name = pixel_format
        .strip_suffix("le")
        .or_else(|| pixel_format.strip_suffix("be"))
        .unwrap_or(pixel_format);
    // What follows the planar layout (`yuv420p`, `gbrp`) or `gray` is the depth, 8 when bare.
    let depth = if name.starts_with("yuv") || name.starts_with("gbr") {
        name.rsplit_once('p').map(|(_, depth)| depth)
    } else {
        name.strip_prefix("gray")
    };
    if let Some(depth) = depth {
        return if depth.is_empty() {
            Some(8)
        } else {
            depth.parse().ok()
        };
    }
    if name.starts_with("nv") {
        return Some(8);
    }
    // Semi-planar at a higher depth: `p010`, `p016`, `p210`, `p410`.
    name.strip_prefix('p')
        .filter(|digits| digits.len() == 3)
        .and_then(|digits| digits.get(1..)?.parse().ok())
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
            start_ms: chapter_time(chapter.start, chapter.time_base.as_deref())
                .or_else(|| chapter.start_time.as_deref().and_then(milliseconds))
                .unwrap_or(0),
            end_ms: chapter_time(chapter.end, chapter.time_base.as_deref())
                .or_else(|| chapter.end_time.as_deref().and_then(milliseconds))
                .unwrap_or(0),
            title: chapter
                .tags
                .get("title")
                .cloned()
                .filter(|title| !title.trim().is_empty()),
            start_us: chapter
                .start_time
                .as_deref()
                .and_then(microseconds)
                .and_then(|us| u64::try_from(us).ok()),
            declared_title: chapter.tags.get("title").cloned(),
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

/// A chapter's time as the reference stores it: the timestamp times its time base in floating
/// point, kept to six significant digits, in milliseconds — 1462.544417 s is 1462540, 433.224 s
/// 433224. Measured on the 86 chapters of the sample files.
fn chapter_time(timestamp: Option<i64>, time_base: Option<&str>) -> Option<u64> {
    let (numerator, denominator) = time_base?.split_once('/')?;
    let (numerator, denominator): (f64, f64) = (numerator.parse().ok()?, denominator.parse().ok()?);
    if denominator == 0.0 {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "a timestamp past 2^53 is no time a file holds"
    )]
    let seconds = timestamp? as f64 * (numerator / denominator);
    let seconds: f64 = format!("{seconds:.5e}").parse().ok()?;
    milliseconds(&seconds.to_string())
}

/// `Chapter 3`, `chapter.3`, `03`, `00:07:13.224`: the reference's `(?:chapter)?\.? ?(\d+)`
/// and `[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3}`, whole, the second what mkvmerge names chapters
/// it was given no title for.
fn is_generic_title(title: &str) -> bool {
    is_numbered(title) || is_timestamp(title.trim())
}

fn is_timestamp(title: &str) -> bool {
    let bytes = title.as_bytes();
    bytes.len() == 12
        && bytes
            .iter()
            .enumerate()
            .all(|(position, byte)| match position {
                2 | 5 => *byte == b':',
                8 => *byte == b'.',
                _ => byte.is_ascii_digit(),
            })
}

fn is_numbered(title: &str) -> bool {
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
    /// In hex: `0x44`.
    id: Option<String>,
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_tag_string: Option<String>,
    profile: Option<String>,
    level: Option<i32>,
    width: Option<u32>,
    height: Option<u32>,
    coded_width: Option<u32>,
    coded_height: Option<u32>,
    refs: Option<u32>,
    scaling_matrix_present: Option<u8>,
    pix_fmt: Option<String>,
    bits_per_raw_sample: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_primaries: Option<String>,
    color_transfer: Option<String>,
    chroma_location: Option<String>,
    field_order: Option<String>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    sample_aspect_ratio: Option<String>,
    display_aspect_ratio: Option<String>,
    is_avc: Option<String>,
    nal_length_size: Option<String>,
    time_base: Option<String>,
    closed_captions: Option<u8>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    bit_rate: Option<String>,
    /// The demuxer's exported options, which ffprobe prints with every stream, as strings:
    /// `true` or `false` for MP4's, a number of bytes for MPEG-TS's.
    moov_before_mdat: Option<String>,
    co64: Option<String>,
    ts_packetsize: Option<String>,
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
    crop_top: Option<u32>,
    crop_bottom: Option<u32>,
    crop_left: Option<u32>,
    crop_right: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ProbedChapter {
    start: Option<i64>,
    end: Option<i64>,
    time_base: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn probed_track(index: u32, kind: TrackKind, language: Option<&str>) -> Track {
        Track {
            index,
            identifier: None,
            kind,
            codec: None,
            codec_tag: None,
            raw_codec_tag: None,
            time_base: None,
            profile: None,
            language: language.map(ToOwned::to_owned),
            declared_language: language.map(ToOwned::to_owned),
            title: None,
            name: None,
            handler_name: None,
            bitrate: None,
            tagged_bitrate: None,
            statistics: None,
            disposition: Disposition::default(),
            video: None,
            audio: None,
            subtitle: None,
            attachment: None,
        }
    }

    #[test]
    fn plex_reads_a_matroska_track_stating_no_language_as_english() {
        let mut cover = probed_track(3, TrackKind::Video, None);
        cover.disposition.attached_picture = true;
        let mut tagged_und = probed_track(2, TrackKind::Audio, None);
        tagged_und.declared_language = Some("und".to_owned());
        let mut tracks = vec![
            probed_track(0, TrackKind::Video, None),
            probed_track(1, TrackKind::Audio, Some("fre")),
            tagged_und,
            cover,
            probed_track(4, TrackKind::Attachment, None),
        ];
        read_absent_languages_as_english(&mut tracks);
        let read: Vec<Option<&str>> = tracks
            .iter()
            .map(|track| track.language.as_deref())
            .collect();
        // A track tagged `und` states it, and neither the cover nor the attachment is an entry.
        assert_eq!(read, [Some("eng"), Some("fre"), None, None, None]);
        // What the file declares is untouched.
        assert_eq!(tracks[0].declared_language, None);
    }

    fn stream(json: &str) -> Stream {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn reads_h264s_sequence_header_only_where_ffmpeg_plex_states_it() {
        let patched = stream(
            r#"{"index": 0, "codec_name": "h264", "coded_width": 1920, "coded_height": 1088,
                "refs": 4, "scaling_matrix_present": 0}"#,
        );
        assert_eq!(
            sequence_header(&patched),
            Some(SequenceHeader {
                coded_width: 1920,
                coded_height: 1088,
                ref_frames: 4,
                has_scaling_matrix: false,
            })
        );
        // A stock ffprobe: the cropped size, and nothing to tell it from the coded one.
        let stock = stream(
            r#"{"index": 0, "codec_name": "h264", "coded_width": 1920, "coded_height": 1080}"#,
        );
        assert_eq!(sequence_header(&stock), None);
        let hevc = stream(
            r#"{"index": 0, "codec_name": "hevc", "coded_width": 3840, "coded_height": 2160,
                "refs": 1}"#,
        );
        assert_eq!(sequence_header(&hevc), None);
    }

    #[test]
    fn reads_the_layout_the_demuxer_exports() {
        let identity = FileIdentity {
            size: 1,
            modified_at: 0,
            open_subtitles_hash: "0000000000000001".to_owned(),
        };
        let mp4 = br#"{"format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2"},
            "streams": [{"index": 0, "codec_type": "video", "moov_before_mdat": "false",
                         "co64": "true"}]}"#;
        let analysis = analysis_from_ffprobe(mp4, identity.clone()).unwrap();
        assert_eq!(analysis.optimized_for_streaming, Some(false));
        assert_eq!(analysis.has_64bit_offsets, Some(true));
        assert_eq!(analysis.packet_length, None);
        let ts = br#"{"format": {"format_name": "mpegts"},
            "streams": [{"index": 0, "codec_type": "video", "ts_packetsize": "192"}]}"#;
        let analysis = analysis_from_ffprobe(ts, identity).unwrap();
        assert_eq!(analysis.packet_length, Some(192));
        assert_eq!(analysis.optimized_for_streaming, None);
    }

    #[test]
    fn reads_a_duration_tag_to_the_microsecond() {
        assert_eq!(tag_duration_us("00:00:50.048000000"), Some(50_048_000));
        assert_eq!(tag_duration_us("01:02:03.5"), Some(3_723_500_000));
        assert_eq!(tag_duration_us("50.0"), None);
    }

    #[test]
    fn reads_the_depth_a_pixel_format_names() {
        for (format, depth) in [
            ("yuv420p", Some(8)),
            ("yuvj420p", Some(8)),
            ("yuv420p10le", Some(10)),
            ("yuv444p12be", Some(12)),
            ("gbrp10le", Some(10)),
            ("gray", Some(8)),
            ("gray10le", Some(10)),
            ("nv12", Some(8)),
            ("p010le", Some(10)),
            ("rgb24", None),
            ("bgra", None),
        ] {
            assert_eq!(pixel_depth(format), depth, "{format}");
        }
    }

    #[test]
    fn keeps_a_crop_only_when_it_hides_something() {
        let side_data = |top| SideData {
            side_data_type: Some("Frame Cropping".to_owned()),
            dv_version_major: None,
            dv_version_minor: None,
            dv_profile: None,
            dv_level: None,
            rpu_present_flag: None,
            el_present_flag: None,
            bl_present_flag: None,
            dv_bl_signal_compatibility_id: None,
            crop_top: Some(top),
            crop_bottom: Some(top),
            crop_left: Some(0),
            crop_right: Some(0),
        };
        assert_eq!(
            cropping(&side_data(140)),
            Some(Cropping {
                top: 140,
                bottom: 140,
                left: 0,
                right: 0
            })
        );
        assert_eq!(cropping(&side_data(0)), None);
    }

    #[test]
    fn numbered_chapter_titles_are_generic() {
        for title in [
            "Chapter 3",
            "chapter.12",
            "Chapter3",
            "07",
            "Chapter. 1",
            "00:07:13.224",
        ] {
            assert!(is_generic_title(title), "{title}");
        }
        for title in [
            "Chapter",
            "Opening",
            "Chapter 3: The Hall",
            "1.5",
            "Chapitre 01",
            "0:07:13.224",
            "00:07:13",
        ] {
            assert!(!is_generic_title(title), "{title}");
        }
    }

    #[test]
    fn clears_titles_only_when_every_one_is_generic() {
        let probed = |titles: &[&str]| -> Vec<ProbedChapter> {
            titles
                .iter()
                .map(|title| ProbedChapter {
                    start: None,
                    end: None,
                    time_base: None,
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
    fn reads_packet_lines_with_their_times_in_microseconds() {
        assert_eq!(
            Packet::parse("stream_index=5|pts_time=6031.525000|dts_time=6031.483000|duration_time=0.042000|size=31337
"),
            Some(Packet {
                stream: 5,
                size: 31_337,
                start_us: Some(6_031_525_000),
                duration_us: Some(42_000),
            })
        );
        let subtitle = Packet::parse(
            "stream_index=2|pts_time=N/A|dts_time=-0.042000|duration_time=N/A|size=12",
        )
        .unwrap();
        assert_eq!(subtitle.start_us, Some(-42_000));
        assert_eq!(subtitle.duration_us, None);
        assert_eq!(Packet::parse("stream_index=2|size=N/A"), None);
        assert_eq!(microseconds("2467.16"), Some(2_467_160_000));
        assert_eq!(microseconds("1e3"), None);
    }

    #[test]
    fn spans_from_the_first_start_to_the_last_end() {
        let mut span = Span::default();
        assert_eq!(span.length_us(), None);
        span.cover(32_616_000, 32_700_000);
        span.cover(0, 42_000);
        span.cover(6_031_525_000, 6_031_567_000);
        assert_eq!(span.length_us(), Some(6_031_567_000));
    }

    #[test]
    fn keeps_a_chapter_time_to_six_significant_digits() {
        let nanoseconds = Some("1/1000000000");
        assert_eq!(
            chapter_time(Some(1_462_544_417_000), nanoseconds),
            Some(1_462_540)
        );
        assert_eq!(
            chapter_time(Some(247_830_917_000), nanoseconds),
            Some(247_831)
        );
        assert_eq!(
            chapter_time(Some(433_224_000_000), nanoseconds),
            Some(433_224)
        );
        // 1037.745 in floating point is a hair above, so it rounds up.
        assert_eq!(
            chapter_time(Some(1_037_745_000_000), nanoseconds),
            Some(1_037_750)
        );
        assert_eq!(
            chapter_time(Some(1_037_745), Some("1/1000")),
            Some(1_037_750)
        );
        assert_eq!(chapter_time(Some(80), Some("1/1000")), Some(80));
        assert_eq!(chapter_time(Some(1), Some("1/0")), None);
        assert_eq!(chapter_time(None, nanoseconds), None);
    }

    #[test]
    fn reads_ffprobe_seconds_and_rates() {
        assert_eq!(milliseconds("1248.664000"), Some(1_248_664));
        assert_eq!(milliseconds("N/A"), None);
        assert!((ratio("24000/1001").unwrap() - 23.976_023_976).abs() < 1e-9);
        assert_eq!(ratio("0/0"), None);
    }
}
