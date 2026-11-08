//! What must happen to each track of a file for a client to play it over HLS.
//!
//! The rules are the reference's, measured with `/decision` on one file of each of 16
//! container and codec combinations, for Plex Web: over HLS (MPEG-TS) H.264 video is copied, and
//! AAC or MP3 audio is copied. Other audio is converted to AAC. Other video needs a full video
//! transcode, which this server does not do yet.

use crate::TranscodeError;

/// The bitrate the reference converts audio to when no quality limit applies.
pub const AUDIO_BITRATE_KBPS: u32 = 256;

/// One track, as a remux has to know it.
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

/// What to do with a file's selected tracks when it is not played directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPlan {
    pub audio: AudioAction,
}

/// # Errors
///
/// [`TranscodeError::VideoNeedsTranscoding`] when the video cannot be copied.
pub fn plan_direct_stream(
    video_codec: &str,
    audio_codec: &str,
) -> Result<StreamPlan, TranscodeError> {
    if video_codec != "h264" {
        return Err(TranscodeError::VideoNeedsTranscoding);
    }
    Ok(StreamPlan {
        audio: if matches!(audio_codec, "aac" | "mp3") {
            AudioAction::Copy
        } else {
            AudioAction::ConvertToAac
        },
    })
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

    #[test]
    fn copies_what_hls_can_carry() {
        assert_eq!(
            plan_direct_stream("h264", "aac").unwrap().audio,
            AudioAction::Copy
        );
        assert_eq!(
            plan_direct_stream("h264", "mp3").unwrap().audio,
            AudioAction::Copy
        );
    }

    #[test]
    fn converts_the_audio_hls_cannot_carry() {
        for codec in ["eac3", "ac3", "dca", "truehd"] {
            assert_eq!(
                plan_direct_stream("h264", codec).unwrap().audio,
                AudioAction::ConvertToAac,
                "{codec}"
            );
        }
    }

    #[test]
    fn refuses_video_it_would_have_to_transcode() {
        for codec in ["hevc", "av1", "mpeg4", "mpeg2video"] {
            assert!(matches!(
                plan_direct_stream(codec, "aac"),
                Err(TranscodeError::VideoNeedsTranscoding)
            ));
        }
    }
}
