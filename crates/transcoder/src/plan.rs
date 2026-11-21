//! What must happen to each track of a file for a client to play it when it is not played
//! directly.
//!
//! The rules are the reference's, measured with `/decision` for Plex Web. Over HLS or DASH,
//! H.264 video is copied unless the client caps the bitrate below the file's, and AAC or MP3 audio
//! is copied unless the client refuses that. Other audio is converted to AAC, and other video is
//! re-encoded to H.264 — except VP9 and AV1 over DASH, copied as H.264 is: the reference never
//! sees them there, its DASH being Plex Web's.
//!
//! An adaptive stream has several rungs, best first: [`plan_ladder`].

use crate::session::Protocol;
use crate::TranscodeError;

/// The bitrate the reference converts audio to when no quality limit applies.
pub const AUDIO_BITRATE_KBPS: u32 = 256;

/// The reference cuts a server-adapted stream in 3 seconds at every picture size.
const SERVER_ADAPTIVE_SEGMENT_SECONDS: u32 = 3;

/// A rung the player chooses is cut where the copied video is, on the source's key frames, and
/// those are a few seconds apart: 3 on the test episodes, 3.9 on the test film. Five seconds
/// holds at least one of them almost always, and is the copy's own length in DASH.
pub const CLIENT_ADAPTIVE_SEGMENT_SECONDS: u32 = 5;

/// The length of a segment of copied video, as the reference asks for it.
const COPIED_DASH_SEGMENT_SECONDS: u32 = 5;
const COPIED_HLS_SEGMENT_SECONDS: u32 = 10;

/// The bitrates of Plex Web's quality menu an adaptive stream steps down through, one per
/// picture size of the measured ladder: more rungs would only lengthen a climb back up.
const ADAPTIVE_RUNG_LIMITS: [u32; 5] = [8000, 4000, 2000, 720, 320];

/// One track, as a conversion has to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Its position in the source file.
    pub index: i32,
    pub codec: String,
    /// As stored: `fr`, `en`.
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioAction {
    Copy,
    ConvertToAac,
}

/// Where an encode puts its key frames, which is where its segments can be cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFrames {
    /// Every this many seconds, the length of a segment.
    Every(u32),
    /// Where the source has them, and nowhere else: segments are then cut where those of the
    /// copied video are, and a player can move between the two.
    AsSource,
}

/// The video a quality limit asks the process to produce.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoEncode {
    pub width: u32,
    pub height: u32,
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub crf: u32,
    /// The source's: the reference never changes it. Absent when the file was not measured.
    pub frame_rate: Option<f64>,
    pub key_frames: KeyFrames,
    /// The codec's headers stay the same whatever the bitrate, so that the initialization
    /// segment of one rung decodes the segments of another: the server moves between rungs
    /// under one representation.
    pub shared_headers: bool,
}

impl VideoEncode {
    #[must_use]
    pub const fn segment_seconds(&self) -> u32 {
        match self.key_frames {
            KeyFrames::Every(seconds) => seconds,
            KeyFrames::AsSource => CLIENT_ADAPTIVE_SEGMENT_SECONDS,
        }
    }

    /// Larger pictures cost more to encode, and the reference cuts them finer to compensate.
    const fn fixed_segment_seconds(width: u32) -> u32 {
        match width {
            0..=720 => 8,
            721..=1280 => 5,
            _ => 1,
        }
    }

    pub(crate) const fn x264_options(&self) -> &'static str {
        match self.width {
            0..=560 => "subme=3:me_range=4:rc_lookahead=10:me=hex",
            561..=720 => "subme=2:me_range=4:rc_lookahead=20:me=hex",
            721..=1280 => "subme=1:me_range=4:rc_lookahead=10:me=hex:8x8dct=0:partitions=none",
            _ => "subme=0:me_range=4:rc_lookahead=10:me=hex:8x8dct=0:partitions=none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoAction {
    Copy,
    Encode(VideoEncode),
}

/// What to do with a file's selected tracks when it is not played directly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamPlan {
    pub video: VideoAction,
    pub audio: AudioAction,
}

impl StreamPlan {
    /// The bitrate of converted audio: a quality limit lowers it.
    #[must_use]
    pub const fn audio_kbps(&self) -> u32 {
        match self.video {
            VideoAction::Copy => AUDIO_BITRATE_KBPS,
            VideoAction::Encode(encode) => encode.audio_kbps,
        }
    }
}

/// A file's measurements, as far as they are known.
#[derive(Debug, Clone, Copy)]
pub struct StreamSource<'a> {
    pub video_codec: &'a str,
    pub audio_codec: &'a str,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub bits_per_second: Option<i32>,
    pub frame_rate: Option<f64>,
}

/// How a stream's quality follows the link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Adaptation {
    /// One rung for the whole stream.
    #[default]
    Fixed,
    /// One representation, whose rung the server changes by measuring its deliveries. Always an
    /// encode, as the reference's automatic quality is.
    Server,
    /// Every rung announced, and the player chooses.
    Client,
}

/// What a client asks for beyond being able to play the file.
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    /// In kilobits per second. For an adaptive stream, a ceiling on every rung.
    pub max_video_kbps: Option<u32>,
    /// Convert audio even when it could be copied.
    pub convert_audio: bool,
    pub adaptation: Adaptation,
    /// A subtitle drawn into the picture, which only an encode can do.
    pub burn_subtitle: bool,
}

/// The rungs a stream may be sent at, best first: one for a fixed quality.
#[derive(Debug, Clone, PartialEq)]
pub struct Ladder {
    pub adaptation: Adaptation,
    /// Never empty. Every rung converts the audio the same way: a player moving between rungs
    /// keeps the audio it has.
    pub rungs: Vec<StreamPlan>,
}

impl Ladder {
    /// The best rung, where a stream starts.
    #[must_use]
    pub fn top(&self) -> &StreamPlan {
        &self.rungs[0]
    }

    /// Every rung of an adaptive stream shares its segment length, so that a segment's number
    /// says the same time on all of them.
    #[must_use]
    pub fn segment_seconds(&self, protocol: Protocol) -> u32 {
        let encoded = self.rungs.iter().find_map(|rung| match rung.video {
            VideoAction::Encode(encode) => Some(encode.segment_seconds()),
            VideoAction::Copy => None,
        });
        match (encoded, self.adaptation, protocol) {
            (Some(seconds), _, _) => seconds,
            (None, Adaptation::Client, _) => CLIENT_ADAPTIVE_SEGMENT_SECONDS,
            (None, _, Protocol::Dash) => COPIED_DASH_SEGMENT_SECONDS,
            (None, _, Protocol::Hls) => COPIED_HLS_SEGMENT_SECONDS,
        }
    }

    /// Whether a rung copies the video, which ties every rung to the source's key frames.
    #[must_use]
    pub fn copies(&self) -> bool {
        self.rungs
            .iter()
            .any(|rung| rung.video == VideoAction::Copy)
    }
}

struct Rung {
    limit_kbps: u32,
    width: u32,
    video_kbps: u32,
    audio_kbps: u32,
    crf: u32,
}

const fn rung(limit_kbps: u32, width: u32, video_kbps: u32, audio_kbps: u32, crf: u32) -> Rung {
    Rung {
        limit_kbps,
        width,
        video_kbps,
        audio_kbps,
        crf,
    }
}

/// What the reference produced at each bitrate Plex Web offers, the same for a 1080p and a 720p
/// source. Its own values vary from film to film, by a content analysis we have no equivalent
/// of; these are the ones measured on files it could really read.
const LADDER: [Rung; 7] = [
    rung(200, 160, 98, 91, 20),
    rung(320, 320, 196, 106, 24),
    rung(720, 480, 300, 125, 23),
    rung(1500, 720, 1042, 128, 22),
    rung(2000, 720, 1297, 162, 21),
    rung(3000, 720, 1818, 232, 17),
    rung(4000, 1280, 3419, 119, 23),
];

/// Above the shared rungs, a source wider than 1280. The last has no limit to reach: it is what
/// the reference aims for when none is given, and more than any limit Plex Web offers.
const WIDE_SOURCE_RUNGS: [Rung; 5] = [
    rung(8000, 1920, 7393, 109, 23),
    rung(10_000, 1920, 9299, 130, 22),
    rung(12_000, 1920, 11_205, 151, 21),
    rung(20_000, 1920, 18_303, 233, 17),
    rung(u32::MAX, 1920, 20_000, 256, 16),
];

/// Above the shared rungs, a source at most 1280 wide: it tops out at 9 Mbps whatever the limit.
const NARROW_SOURCE_RUNGS: [Rung; 3] = [
    rung(8000, 1280, 7162, 211, 18),
    rung(9000, 1280, 8081, 233, 17),
    rung(10_000, 1280, 9000, 256, 16),
];

/// The highest measured rung within the limit, or the lowest there is.
fn rung_for(limit_kbps: u32, source_width: u32) -> &'static Rung {
    let upper: &[Rung] = if source_width > 1280 {
        &WIDE_SOURCE_RUNGS
    } else {
        &NARROW_SOURCE_RUNGS
    };
    upper
        .iter()
        .rev()
        .chain(LADDER.iter().rev())
        .find(|rung| rung.limit_kbps <= limit_kbps)
        .unwrap_or(&LADDER[0])
}

/// Never enlarges, but the bitrate is the rung's even above the source's, as on the reference.
/// The scaler keeps both sides divisible by 4, so what is announced is what the process writes.
fn encode_within(
    limit_kbps: u32,
    source: &StreamSource<'_>,
    key_frames: Option<KeyFrames>,
) -> Result<VideoEncode, TranscodeError> {
    let side = |value: Option<i32>| value.and_then(|value| u32::try_from(value).ok());
    let (Some(source_width), Some(source_height)) = (side(source.width), side(source.height))
    else {
        return Err(TranscodeError::VideoNeedsTranscoding);
    };
    if source_width == 0 || source_height == 0 {
        return Err(TranscodeError::VideoNeedsTranscoding);
    }
    let rung = rung_for(limit_kbps, source_width);
    let divisible = |value: u32| (value - value % 4).max(4);
    let width = divisible(rung.width.min(source_width));
    let height = u64::from(source_height) * u64::from(width) / u64::from(source_width);
    Ok(VideoEncode {
        width,
        height: divisible(u32::try_from(height).unwrap_or(u32::MAX)),
        video_kbps: rung.video_kbps,
        audio_kbps: rung.audio_kbps,
        crf: rung.crf,
        frame_rate: source.frame_rate,
        key_frames: key_frames
            .unwrap_or(KeyFrames::Every(VideoEncode::fixed_segment_seconds(width))),
        shared_headers: false,
    })
}

/// The reference copies the video only when the limit covers the file's peak bandwidth, which
/// its analysis measures and ours does not: this compares with the average, and so copies a
/// little more often. A file of unknown bitrate cannot be shown to fit.
fn fits(limit_kbps: u32, source: &StreamSource<'_>) -> bool {
    source
        .bits_per_second
        .is_some_and(|bits| i64::from(limit_kbps) * 1000 >= i64::from(bits))
}

/// The rung a stream starts on: its only one at a fixed quality.
///
/// # Errors
///
/// As [`plan_ladder`].
pub fn plan_stream(
    source: &StreamSource<'_>,
    protocol: Protocol,
    limits: Limits,
) -> Result<StreamPlan, TranscodeError> {
    Ok(*plan_ladder(source, protocol, limits)?.top())
}

/// # Errors
///
/// [`TranscodeError::VideoNeedsTranscoding`] when the video has to be encoded and its picture
/// size is unknown.
pub fn plan_ladder(
    source: &StreamSource<'_>,
    protocol: Protocol,
    limits: Limits,
) -> Result<Ladder, TranscodeError> {
    let copyable = copyable(source, protocol);
    let videos = match limits.adaptation {
        Adaptation::Fixed => vec![fixed_video(source, limits, copyable)?],
        Adaptation::Server => server_rungs(source, limits.max_video_kbps.unwrap_or(u32::MAX))?,
        Adaptation::Client => client_rungs(source, limits, copyable)?,
    };
    let top = videos[0];
    let audio = if matches!(source.audio_codec, "aac" | "mp3")
        && !limits.convert_audio
        && top == VideoAction::Copy
    {
        AudioAction::Copy
    } else {
        AudioAction::ConvertToAac
    };
    let audio_kbps = StreamPlan { video: top, audio }.audio_kbps();
    let rungs = videos
        .into_iter()
        .map(|video| StreamPlan {
            video: match video {
                VideoAction::Encode(encode) => VideoAction::Encode(VideoEncode {
                    audio_kbps,
                    ..encode
                }),
                VideoAction::Copy => VideoAction::Copy,
            },
            audio,
        })
        .collect();
    Ok(Ladder {
        adaptation: limits.adaptation,
        rungs,
    })
}

/// Whether the protocol carries the video as it is: HLS's MPEG-TS segments only H.264 here,
/// DASH's fragmented MP4 VP9 and AV1 too. A player that cannot decode those asks for encodes
/// beside the copy, and drops the copy.
fn copyable(source: &StreamSource<'_>, protocol: Protocol) -> bool {
    match protocol {
        Protocol::Hls => source.video_codec == "h264",
        Protocol::Dash => matches!(source.video_codec, "h264" | "vp9" | "av1"),
    }
}

fn fixed_video(
    source: &StreamSource<'_>,
    limits: Limits,
    copyable: bool,
) -> Result<VideoAction, TranscodeError> {
    Ok(match limits.max_video_kbps {
        Some(limit) if !fits(limit, source) => {
            VideoAction::Encode(encode_within(limit, source, None)?)
        }
        // Measured: 1920 wide, 20 Mbps, CRF 16 for Soul burned at no limit — the top rung.
        limit if limits.burn_subtitle || !copyable => {
            VideoAction::Encode(encode_within(limit.unwrap_or(u32::MAX), source, None)?)
        }
        _ => VideoAction::Copy,
    })
}

/// The reference's automatic quality, then lower bitrates at its picture size: a player reading
/// one representation keeps its initialization segment, which another picture size would not
/// decode, and the codec's headers are kept identical for the same reason.
fn server_rungs(source: &StreamSource<'_>, limit: u32) -> Result<Vec<VideoAction>, TranscodeError> {
    let shared = |encode: VideoEncode| VideoEncode {
        key_frames: KeyFrames::Every(SERVER_ADAPTIVE_SEGMENT_SECONDS),
        shared_headers: true,
        ..encode
    };
    let top = shared(encode_within(limit, source, None)?);
    let mut rungs = vec![VideoAction::Encode(top)];
    for step in ADAPTIVE_RUNG_LIMITS {
        let lower = encode_within(step.min(limit), source, None)?;
        if lower.video_kbps < lowest_kbps(&rungs) {
            rungs.push(VideoAction::Encode(VideoEncode {
                width: top.width,
                height: top.height,
                ..shared(lower)
            }));
        }
    }
    Ok(rungs)
}

/// The copy when the source allows it, the best stream there is, then encodes cut where the
/// copy is. Without a copy, encodes cut on a grid.
fn client_rungs(
    source: &StreamSource<'_>,
    limits: Limits,
    copyable: bool,
) -> Result<Vec<VideoAction>, TranscodeError> {
    let limit = limits.max_video_kbps.unwrap_or(u32::MAX);
    let copies = copyable
        && !limits.burn_subtitle
        && limits
            .max_video_kbps
            .is_none_or(|limit| fits(limit, source));
    let key_frames = if copies {
        KeyFrames::AsSource
    } else {
        KeyFrames::Every(CLIENT_ADAPTIVE_SEGMENT_SECONDS)
    };
    let mut rungs = if copies {
        vec![VideoAction::Copy]
    } else {
        vec![VideoAction::Encode(encode_within(
            limit,
            source,
            Some(key_frames),
        )?)]
    };
    // An encode close to the copy's bitrate would cost a process and give the player nothing.
    let source_kbps = source
        .bits_per_second
        .and_then(|bits| u32::try_from(bits / 1000).ok());
    for step in ADAPTIVE_RUNG_LIMITS
        .into_iter()
        .filter(|step| *step <= limit)
    {
        let encode = encode_within(step, source, Some(key_frames))?;
        let worth = !copies
            || source_kbps
                .is_some_and(|kbps| u64::from(encode.video_kbps) * 4 <= u64::from(kbps) * 3);
        if worth && encode.video_kbps < lowest_kbps(&rungs) {
            rungs.push(VideoAction::Encode(encode));
        }
    }
    Ok(rungs)
}

fn lowest_kbps(rungs: &[VideoAction]) -> u32 {
    rungs
        .iter()
        .filter_map(|rung| match rung {
            VideoAction::Encode(encode) => Some(encode.video_kbps),
            VideoAction::Copy => None,
        })
        .min()
        .unwrap_or(u32::MAX)
}

/// The transcoder's decoder for a video codec, as the reference names it on the command line.
/// None for a codec it has no decoder of that name for, which the transcoder then picks from
/// the file itself. Its native AV1 decoder only drives hardware.
pub(crate) fn video_decoder(codec: &str) -> Option<&'static str> {
    Some(match codec {
        "av1" => "libdav1d",
        "msmpeg4v3" => "msmpeg4",
        "flv1" => "flv",
        "h264" => "h264",
        "hevc" => "hevc",
        "mpeg1video" => "mpeg1video",
        "mpeg2video" => "mpeg2video",
        "mpeg4" => "mpeg4",
        "msmpeg4v1" => "msmpeg4v1",
        "msmpeg4v2" => "msmpeg4v2",
        "vc1" => "vc1",
        "wmv3" => "wmv3",
        "vp8" => "vp8",
        "vp9" => "vp9",
        "rv30" => "rv30",
        "theora" => "theora",
        "prores" => "prores",
        "mjpeg" => "mjpeg",
        "png" => "png",
        _ => return None,
    })
}

/// The transcoder's decoder for an audio codec, as [`video_decoder`]. Plex's build decodes
/// E-AC-3 through Media Foundation on Windows. `pcm` names no decoder: the sample format that
/// says which is only in the file.
pub(crate) fn audio_decoder(codec: &str) -> Option<&'static str> {
    Some(match codec {
        "eac3" => "eac3_mf",
        "aac" => "aac",
        "ac3" => "ac3",
        "dca" => "dca",
        "truehd" => "truehd",
        "mp2" => "mp2",
        "mp3" => "mp3",
        "flac" => "flac",
        "vorbis" => "vorbis",
        "opus" => "opus",
        "cook" => "cook",
        "pcm_bluray" => "pcm_bluray",
        "adpcm_ms" => "adpcm_ms",
        "adpcm_ima_wav" => "adpcm_ima_wav",
        _ => return None,
    })
}

/// The ISO 639-2 code the transcoder labels a track with, from the code we store.
pub(crate) fn language_tag(language: &str) -> Option<&'static str> {
    Some(match language {
        "fr" => "fra",
        "en" => "eng",
        "de" => "deu",
        "es" => "spa",
        "it" => "ita",
        "ja" => "jpn",
        "pt" => "por",
        "ru" => "rus",
        "zh" => "zho",
        "ko" => "kor",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn source<'a>(video_codec: &'a str, audio_codec: &'a str) -> StreamSource<'a> {
        StreamSource {
            video_codec,
            audio_codec,
            width: Some(1920),
            height: Some(1040),
            bits_per_second: Some(9_834_000),
            frame_rate: Some(25.0),
        }
    }

    fn limited(kbps: u32) -> Limits {
        Limits {
            max_video_kbps: Some(kbps),
            ..Limits::default()
        }
    }

    fn encode_of(plan: StreamPlan) -> VideoEncode {
        match plan.video {
            VideoAction::Encode(encode) => encode,
            VideoAction::Copy => panic!("the video was copied"),
        }
    }

    #[test]
    fn burning_a_subtitle_encodes_at_the_top_rung_or_within_the_limit() {
        let burn = Limits {
            burn_subtitle: true,
            ..Limits::default()
        };
        let encode = encode_of(plan_stream(&soul(), Protocol::Hls, burn).unwrap());
        assert_eq!(
            (encode.width, encode.video_kbps, encode.crf),
            (1920, 20_000, 16)
        );
        assert_eq!(encode.segment_seconds(), 1);
        let limited = Limits {
            max_video_kbps: Some(2000),
            ..burn
        };
        // A limit the file fits would copy; burning encodes anyway.
        assert_eq!(
            encode_of(plan_stream(&soul(), Protocol::Hls, limited).unwrap()).width,
            720
        );
    }

    #[test]
    fn copies_what_hls_can_carry() {
        for codec in ["aac", "mp3"] {
            let plan =
                plan_stream(&source("h264", codec), Protocol::Hls, Limits::default()).unwrap();
            assert_eq!(plan.audio, AudioAction::Copy);
            assert_eq!(plan.video, VideoAction::Copy);
        }
    }

    #[test]
    fn converts_the_audio_hls_cannot_carry() {
        for codec in ["eac3", "ac3", "dca", "truehd"] {
            assert_eq!(
                plan_stream(&source("h264", codec), Protocol::Hls, Limits::default())
                    .unwrap()
                    .audio,
                AudioAction::ConvertToAac,
                "{codec}"
            );
        }
    }

    #[test]
    fn a_client_can_refuse_copied_audio() {
        let limits = Limits {
            convert_audio: true,
            ..Limits::default()
        };
        assert_eq!(
            plan_stream(&source("h264", "aac"), Protocol::Hls, limits)
                .unwrap()
                .audio,
            AudioAction::ConvertToAac
        );
    }

    #[test]
    fn encodes_video_it_cannot_copy_at_the_top_rung_or_within_the_limit() {
        for codec in ["hevc", "av1", "mpeg4", "mpeg2video"] {
            let plan =
                plan_stream(&source(codec, "aac"), Protocol::Hls, Limits::default()).unwrap();
            let encode = encode_of(plan);
            assert_eq!((encode.width, encode.video_kbps), (1920, 20_000), "{codec}");
            assert_eq!(plan.audio, AudioAction::ConvertToAac, "{codec}");
            // A limit the file fits would copy H.264.
            let limited = encode_of(
                plan_stream(&source(codec, "aac"), Protocol::Hls, limited(12_000)).unwrap(),
            );
            assert_eq!(limited.video_kbps, 11_205, "{codec}");
        }
    }

    #[test]
    fn copies_vp9_and_av1_over_dash_alone() {
        for codec in ["vp9", "av1"] {
            let plan = plan_stream(&source(codec, "aac"), Protocol::Dash, Limits::default());
            assert_eq!(plan.unwrap().video, VideoAction::Copy, "{codec}");
            let plan = plan_stream(&source(codec, "aac"), Protocol::Hls, Limits::default());
            assert!(
                matches!(plan.unwrap().video, VideoAction::Encode(_)),
                "{codec}"
            );
        }
        let hevc = plan_stream(&source("hevc", "aac"), Protocol::Dash, Limits::default());
        assert!(matches!(hevc.unwrap().video, VideoAction::Encode(_)));
    }

    #[test]
    fn a_player_chooses_between_a_vp9_copy_and_encodes_cut_where_it_is() {
        let vp9 = StreamSource {
            video_codec: "vp9",
            ..soul()
        };
        let ladder = plan_ladder(&vp9, Protocol::Dash, chosen(Limits::default())).unwrap();
        assert_eq!(sizes(&ladder)[0], None);
        assert!(ladder.rungs[1..]
            .iter()
            .all(|rung| encode_of(*rung).key_frames == KeyFrames::AsSource));
    }

    #[test]
    fn a_player_chooses_between_encodes_of_video_it_cannot_copy() {
        let hevc = StreamSource {
            video_codec: "hevc",
            ..soul()
        };
        let ladder = plan_ladder(&hevc, Protocol::Hls, chosen(Limits::default())).unwrap();
        assert!(!ladder.copies());
        assert_eq!(sizes(&ladder)[0], Some((1920, 1080, 20_000)));
        for rung in &ladder.rungs {
            assert_eq!(encode_of(*rung).key_frames, KeyFrames::Every(5));
        }
    }

    #[test]
    fn copies_when_the_limit_covers_the_file() {
        let plan = plan_stream(&source("h264", "eac3"), Protocol::Hls, limited(12_000)).unwrap();
        assert_eq!(plan.video, VideoAction::Copy);
        assert_eq!(plan.audio_kbps(), AUDIO_BITRATE_KBPS);
    }

    /// Film 513702, 1920x1040, asked for 2 Mbps. The reference writes 1333k and 167k here, and
    /// 388 for the height: the ladder holds typical values.
    #[test]
    fn encodes_down_to_the_rung_within_the_limit() {
        let plan = plan_stream(&source("h264", "eac3"), Protocol::Hls, limited(2000)).unwrap();
        let encode = encode_of(plan);
        assert_eq!((encode.width, encode.height), (720, 388));
        assert_eq!(encode.video_kbps, 1297);
        assert_eq!(plan.audio_kbps(), 162);
        assert_eq!(plan.audio, AudioAction::ConvertToAac);
        assert_eq!(encode.frame_rate, Some(25.0));
    }

    #[test]
    fn a_limit_between_rungs_takes_the_lower() {
        let encode =
            encode_of(plan_stream(&source("h264", "aac"), Protocol::Hls, limited(5000)).unwrap());
        assert_eq!(encode.width, 1280);
        assert_eq!(encode.video_kbps, 3419);
    }

    #[test]
    fn a_limit_below_the_ladder_takes_its_lowest_rung() {
        let encode =
            encode_of(plan_stream(&source("h264", "aac"), Protocol::Hls, limited(1)).unwrap());
        assert_eq!(encode.width, 160);
    }

    #[test]
    fn converts_copyable_audio_along_with_the_video() {
        let plan = plan_stream(&source("h264", "aac"), Protocol::Hls, limited(720)).unwrap();
        assert_eq!(plan.audio, AudioAction::ConvertToAac);
        assert_eq!(plan.audio_kbps(), 125);
    }

    #[test]
    fn never_enlarges_the_picture() {
        let small = StreamSource {
            width: Some(560),
            height: Some(304),
            bits_per_second: Some(1_100_000),
            ..source("h264", "aac")
        };
        // A limit under the file's bitrate, on the rung above its width.
        let encode = encode_of(plan_stream(&small, Protocol::Hls, limited(1000)).unwrap());
        assert_eq!((encode.width, encode.height), (480, 260));

        let wide_limit = StreamSource {
            bits_per_second: Some(3_000_000),
            ..small
        };
        let encode = encode_of(plan_stream(&wide_limit, Protocol::Hls, limited(2000)).unwrap());
        assert_eq!((encode.width, encode.height), (560, 304));
    }

    #[test]
    fn a_file_of_unknown_bitrate_is_encoded_under_a_limit() {
        let unknown = StreamSource {
            bits_per_second: None,
            ..source("h264", "aac")
        };
        assert!(matches!(
            plan_stream(&unknown, Protocol::Hls, limited(100_000))
                .unwrap()
                .video,
            VideoAction::Encode(_)
        ));
    }

    fn adaptive(limit: Option<u32>) -> Limits {
        Limits {
            max_video_kbps: limit,
            adaptation: Adaptation::Server,
            ..Limits::default()
        }
    }

    fn soul() -> StreamSource<'static> {
        StreamSource {
            width: Some(1920),
            height: Some(1080),
            bits_per_second: Some(7_726_000),
            ..source("h264", "eac3")
        }
    }

    fn bull() -> StreamSource<'static> {
        StreamSource {
            width: Some(1280),
            height: Some(720),
            bits_per_second: Some(3_209_000),
            ..source("h264", "aac")
        }
    }

    /// The sizes, bitrates and CRFs of the reference encoding these two files, except the heights
    /// of 270 and 90 it announces: our sides are floored to multiples of 4.
    #[test]
    fn automatic_quality_follows_the_rungs_measured_for_each_source() {
        let wide = [
            (None, 1920, 1080, 20_000, 256, 16),
            (Some(20_000), 1920, 1080, 18_303, 233, 17),
            (Some(12_000), 1920, 1080, 11_205, 151, 21),
            (Some(10_000), 1920, 1080, 9299, 130, 22),
            (Some(8000), 1920, 1080, 7393, 109, 23),
            (Some(4000), 1280, 720, 3419, 119, 23),
            (Some(3000), 720, 404, 1818, 232, 17),
            (Some(2000), 720, 404, 1297, 162, 21),
            (Some(1500), 720, 404, 1042, 128, 22),
            (Some(720), 480, 268, 300, 125, 23),
            (Some(320), 320, 180, 196, 106, 24),
            (Some(200), 160, 88, 98, 91, 20),
        ];
        let narrow = [
            (None, 1280, 720, 9000, 256, 16),
            (Some(20_000), 1280, 720, 9000, 256, 16),
            (Some(10_000), 1280, 720, 9000, 256, 16),
            (Some(9000), 1280, 720, 8081, 233, 17),
            (Some(8000), 1280, 720, 7162, 211, 18),
            (Some(4000), 1280, 720, 3419, 119, 23),
            (Some(3000), 720, 404, 1818, 232, 17),
            (Some(1500), 720, 404, 1042, 128, 22),
        ];
        for (file, expected) in [(soul(), &wide[..]), (bull(), &narrow[..])] {
            for &(limit, width, height, video, audio, crf) in expected {
                let plan = plan_stream(&file, Protocol::Hls, adaptive(limit)).unwrap();
                let encode = encode_of(plan);
                assert_eq!(
                    (
                        encode.width,
                        encode.height,
                        encode.video_kbps,
                        plan.audio_kbps(),
                        encode.crf
                    ),
                    (width, height, video, audio, crf),
                    "{limit:?}"
                );
            }
        }
    }

    #[test]
    fn automatic_quality_encodes_what_it_could_have_copied() {
        let plan = plan_stream(&bull(), Protocol::Hls, adaptive(Some(20_000))).unwrap();
        assert!(matches!(plan.video, VideoAction::Encode(_)));
        assert_eq!(plan.audio, AudioAction::ConvertToAac);
    }

    #[test]
    fn automatic_quality_cuts_every_size_in_three_seconds() {
        for limit in [None, Some(4000), Some(720)] {
            assert_eq!(
                encode_of(plan_stream(&soul(), Protocol::Hls, adaptive(limit)).unwrap())
                    .segment_seconds(),
                3
            );
        }
    }

    #[test]
    fn cannot_encode_a_picture_of_unknown_size() {
        let unknown = StreamSource {
            width: None,
            ..source("h264", "aac")
        };
        assert!(matches!(
            plan_stream(&unknown, Protocol::Hls, limited(720)),
            Err(TranscodeError::VideoNeedsTranscoding)
        ));
    }

    #[test]
    fn cuts_larger_pictures_finer() {
        let encode = |limit| {
            encode_of(plan_stream(&source("h264", "aac"), Protocol::Hls, limited(limit)).unwrap())
        };
        assert_eq!(encode(2000).segment_seconds(), 8);
        assert_eq!(encode(4000).segment_seconds(), 5);
        assert_eq!(encode(8000).segment_seconds(), 1);
    }

    fn chosen(limits: Limits) -> Limits {
        Limits {
            adaptation: Adaptation::Client,
            ..limits
        }
    }

    /// Each rung's video as (width, height, kbps), `None` for the copy.
    fn sizes(ladder: &Ladder) -> Vec<Option<(u32, u32, u32)>> {
        ladder
            .rungs
            .iter()
            .map(|rung| match rung.video {
                VideoAction::Copy => None,
                VideoAction::Encode(encode) => {
                    Some((encode.width, encode.height, encode.video_kbps))
                }
            })
            .collect()
    }

    #[test]
    fn a_player_chooses_between_the_copy_and_encodes_well_below_it() {
        // Soul averages 7.7 Mbps: an encode at 7.4 would be no lighter than the copy.
        let ladder = plan_ladder(&soul(), Protocol::Hls, chosen(Limits::default())).unwrap();
        assert_eq!(
            sizes(&ladder),
            [
                None,
                Some((1280, 720, 3419)),
                Some((720, 404, 1297)),
                Some((480, 268, 300)),
                Some((320, 180, 196)),
            ]
        );
        for rung in &ladder.rungs {
            assert_eq!(rung.audio, AudioAction::ConvertToAac);
            assert_eq!(rung.audio_kbps(), AUDIO_BITRATE_KBPS);
            if let VideoAction::Encode(encode) = rung.video {
                assert_eq!(encode.key_frames, KeyFrames::AsSource);
                assert!(!encode.shared_headers);
            }
        }
        assert_eq!(ladder.segment_seconds(Protocol::Hls), 5);
        assert_eq!(ladder.segment_seconds(Protocol::Dash), 5);

        // Bull's AAC is copied on every rung, since the copy keeps it.
        let ladder = plan_ladder(&bull(), Protocol::Hls, chosen(Limits::default())).unwrap();
        assert_eq!(
            sizes(&ladder),
            [
                None,
                Some((720, 404, 1297)),
                Some((480, 268, 300)),
                Some((320, 180, 196))
            ]
        );
        assert!(ladder
            .rungs
            .iter()
            .all(|rung| rung.audio == AudioAction::Copy));
    }

    #[test]
    fn without_a_copy_a_player_chooses_between_encodes_on_a_grid() {
        let burn = chosen(Limits {
            burn_subtitle: true,
            ..Limits::default()
        });
        let ladder = plan_ladder(&soul(), Protocol::Hls, burn).unwrap();
        assert_eq!(ladder.rungs.len(), 6);
        assert_eq!(sizes(&ladder)[0], Some((1920, 1080, 20_000)));
        assert!(!ladder.copies());
        for rung in &ladder.rungs {
            assert_eq!(encode_of(*rung).key_frames, KeyFrames::Every(5));
        }
        // A limit the file does not fit leaves no copy, and caps every rung.
        let capped = plan_ladder(&soul(), Protocol::Hls, chosen(limited(2000))).unwrap();
        assert_eq!(
            sizes(&capped),
            [
                Some((720, 404, 1297)),
                Some((480, 268, 300)),
                Some((320, 180, 196))
            ]
        );
    }

    #[test]
    fn the_server_lowers_the_bitrate_at_one_picture_size() {
        let ladder = plan_ladder(&soul(), Protocol::Hls, adaptive(None)).unwrap();
        let kbps: Vec<u32> = sizes(&ladder)
            .into_iter()
            .map(|size| {
                let (width, height, kbps) = size.unwrap();
                assert_eq!((width, height), (1920, 1080));
                kbps
            })
            .collect();
        assert_eq!(kbps, [20_000, 7393, 3419, 1297, 300, 196]);
        for rung in &ladder.rungs {
            let encode = encode_of(*rung);
            assert!(encode.shared_headers);
            assert_eq!(encode.segment_seconds(), 3);
            // The audio stays as it started, so that its headers do too.
            assert_eq!(rung.audio_kbps(), 256);
        }
        // Under a limit, it starts on the limit's rung and keeps its size.
        let capped = plan_ladder(&bull(), Protocol::Hls, adaptive(Some(4000))).unwrap();
        assert_eq!(
            sizes(&capped),
            [
                Some((1280, 720, 3419)),
                Some((1280, 720, 1297)),
                Some((1280, 720, 300)),
                Some((1280, 720, 196))
            ]
        );
    }
}
