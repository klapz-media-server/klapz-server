//! What must happen to each track of a file for a client to play it when it is not played
//! directly.
//!
//! The rules are the reference's, measured with `/decision` for Plex Web. Over HLS or DASH,
//! H.264 video is copied unless the client caps the bitrate below the file's, and AAC or MP3 audio
//! is copied unless the client refuses that. Other audio is converted to AAC. Other video needs
//! a conversion this server does not do yet.

use crate::TranscodeError;

/// The bitrate the reference converts audio to when no quality limit applies.
pub const AUDIO_BITRATE_KBPS: u32 = 256;

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
}

impl VideoEncode {
    /// Larger pictures cost more to encode, and the reference cuts them finer to compensate.
    #[must_use]
    pub const fn segment_seconds(&self) -> u32 {
        match self.width {
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

/// What a client asks for beyond being able to play the file.
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    /// In kilobits per second.
    pub max_video_kbps: Option<u32>,
    /// Convert audio even when it could be copied.
    pub convert_audio: bool,
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

/// What the reference produced for 1920-wide H.264 films at each bitrate Plex Web offers. Its
/// own values vary a little from film to film, by a content analysis we have no equivalent of.
const LADDER: [Rung; 11] = [
    rung(200, 160, 98, 91, 20),
    rung(320, 320, 196, 106, 24),
    rung(720, 480, 300, 125, 23),
    rung(1500, 720, 1042, 128, 22),
    rung(2000, 720, 1297, 162, 21),
    rung(3000, 1280, 2566, 98, 24),
    rung(4000, 1280, 3551, 122, 23),
    rung(8000, 1920, 7417, 112, 23),
    rung(10_000, 1920, 9379, 135, 22),
    rung(12_000, 1920, 11_205, 151, 21),
    rung(20_000, 1920, 18_303, 233, 17),
];

/// The highest rung within the limit, or the lowest there is.
fn rung_for(limit_kbps: u32) -> &'static Rung {
    LADDER
        .iter()
        .rev()
        .find(|rung| rung.limit_kbps <= limit_kbps)
        .unwrap_or(&LADDER[0])
}

/// Never enlarges. The scaler keeps both sides divisible by 4, so what is announced is what the
/// process writes.
fn encode_within(
    limit_kbps: u32,
    source: &StreamSource<'_>,
) -> Result<VideoEncode, TranscodeError> {
    let side = |value: Option<i32>| value.and_then(|value| u32::try_from(value).ok());
    let (Some(source_width), Some(source_height)) = (side(source.width), side(source.height))
    else {
        return Err(TranscodeError::VideoNeedsTranscoding);
    };
    if source_width == 0 || source_height == 0 {
        return Err(TranscodeError::VideoNeedsTranscoding);
    }
    let rung = rung_for(limit_kbps);
    let divisible = |value: u32| (value - value % 4).max(4);
    let width = divisible(rung.width.min(source_width));
    let height = u64::from(source_height) * u64::from(width) / u64::from(source_width);
    let source_kbps = side(source.bits_per_second).map_or(u32::MAX, |bits| bits / 1000);
    Ok(VideoEncode {
        width,
        height: divisible(u32::try_from(height).unwrap_or(u32::MAX)),
        video_kbps: rung.video_kbps.min(source_kbps),
        audio_kbps: rung.audio_kbps,
        crf: rung.crf,
        frame_rate: source.frame_rate,
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

/// # Errors
///
/// [`TranscodeError::VideoNeedsTranscoding`] when the video cannot be copied and this server
/// cannot convert it.
pub fn plan_stream(
    source: &StreamSource<'_>,
    limits: Limits,
) -> Result<StreamPlan, TranscodeError> {
    if source.video_codec != "h264" {
        return Err(TranscodeError::VideoNeedsTranscoding);
    }
    let video = match limits.max_video_kbps {
        Some(limit) if !fits(limit, source) => VideoAction::Encode(encode_within(limit, source)?),
        _ => VideoAction::Copy,
    };
    let audio = if matches!(source.audio_codec, "aac" | "mp3")
        && !limits.convert_audio
        && video == VideoAction::Copy
    {
        AudioAction::Copy
    } else {
        AudioAction::ConvertToAac
    };
    Ok(StreamPlan { video, audio })
}

/// The transcoder's name for the decoder of a codec, where it differs from the codec's. Plex's
/// build decodes E-AC-3 through Media Foundation on Windows.
pub(crate) fn audio_decoder(codec: &str) -> &str {
    match codec {
        "eac3" => "eac3_mf",
        other => other,
    }
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
            convert_audio: false,
        }
    }

    fn encode_of(plan: StreamPlan) -> VideoEncode {
        match plan.video {
            VideoAction::Encode(encode) => encode,
            VideoAction::Copy => panic!("the video was copied"),
        }
    }

    #[test]
    fn copies_what_hls_can_carry() {
        for codec in ["aac", "mp3"] {
            let plan = plan_stream(&source("h264", codec), Limits::default()).unwrap();
            assert_eq!(plan.audio, AudioAction::Copy);
            assert_eq!(plan.video, VideoAction::Copy);
        }
    }

    #[test]
    fn converts_the_audio_hls_cannot_carry() {
        for codec in ["eac3", "ac3", "dca", "truehd"] {
            assert_eq!(
                plan_stream(&source("h264", codec), Limits::default())
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
            max_video_kbps: None,
            convert_audio: true,
        };
        assert_eq!(
            plan_stream(&source("h264", "aac"), limits).unwrap().audio,
            AudioAction::ConvertToAac
        );
    }

    #[test]
    fn refuses_video_it_would_have_to_convert() {
        for codec in ["hevc", "av1", "mpeg4", "mpeg2video"] {
            assert!(matches!(
                plan_stream(&source(codec, "aac"), Limits::default()),
                Err(TranscodeError::VideoNeedsTranscoding)
            ));
        }
    }

    #[test]
    fn copies_when_the_limit_covers_the_file() {
        let plan = plan_stream(&source("h264", "eac3"), limited(12_000)).unwrap();
        assert_eq!(plan.video, VideoAction::Copy);
        assert_eq!(plan.audio_kbps(), AUDIO_BITRATE_KBPS);
    }

    /// Film 513702, 1920x1040, asked for 2 Mbps. The reference writes 1333k and 167k here, and
    /// 388 for the height: the ladder holds typical values.
    #[test]
    fn encodes_down_to_the_rung_within_the_limit() {
        let plan = plan_stream(&source("h264", "eac3"), limited(2000)).unwrap();
        let encode = encode_of(plan);
        assert_eq!((encode.width, encode.height), (720, 388));
        assert_eq!(encode.video_kbps, 1297);
        assert_eq!(plan.audio_kbps(), 162);
        assert_eq!(plan.audio, AudioAction::ConvertToAac);
        assert_eq!(encode.frame_rate, Some(25.0));
    }

    #[test]
    fn a_limit_between_rungs_takes_the_lower() {
        let encode = encode_of(plan_stream(&source("h264", "aac"), limited(5000)).unwrap());
        assert_eq!(encode.width, 1280);
        assert_eq!(encode.video_kbps, 3551);
    }

    #[test]
    fn a_limit_below_the_ladder_takes_its_lowest_rung() {
        let encode = encode_of(plan_stream(&source("h264", "aac"), limited(1)).unwrap());
        assert_eq!(encode.width, 160);
    }

    #[test]
    fn converts_copyable_audio_along_with_the_video() {
        let plan = plan_stream(&source("h264", "aac"), limited(720)).unwrap();
        assert_eq!(plan.audio, AudioAction::ConvertToAac);
        assert_eq!(plan.audio_kbps(), 125);
    }

    #[test]
    fn never_enlarges_nor_raises_the_bitrate() {
        let small = StreamSource {
            width: Some(560),
            height: Some(304),
            bits_per_second: Some(1_100_000),
            ..source("h264", "aac")
        };
        // A limit under the file's bitrate, on the rung above its width.
        let encode = encode_of(plan_stream(&small, limited(1000)).unwrap());
        assert_eq!((encode.width, encode.height), (480, 260));

        let wide_limit = StreamSource {
            bits_per_second: Some(3_000_000),
            ..small
        };
        let encode = encode_of(plan_stream(&wide_limit, limited(2000)).unwrap());
        assert_eq!((encode.width, encode.height), (560, 304));
    }

    #[test]
    fn a_file_of_unknown_bitrate_is_encoded_under_a_limit() {
        let unknown = StreamSource {
            bits_per_second: None,
            ..source("h264", "aac")
        };
        assert!(matches!(
            plan_stream(&unknown, limited(100_000)).unwrap().video,
            VideoAction::Encode(_)
        ));
    }

    #[test]
    fn cannot_encode_a_picture_of_unknown_size() {
        let unknown = StreamSource {
            width: None,
            ..source("h264", "aac")
        };
        assert!(matches!(
            plan_stream(&unknown, limited(720)),
            Err(TranscodeError::VideoNeedsTranscoding)
        ));
    }

    #[test]
    fn cuts_larger_pictures_finer() {
        let encode =
            |limit| encode_of(plan_stream(&source("h264", "aac"), limited(limit)).unwrap());
        assert_eq!(encode(2000).segment_seconds(), 8);
        assert_eq!(encode(3000).segment_seconds(), 5);
        assert_eq!(encode(8000).segment_seconds(), 1);
    }
}
