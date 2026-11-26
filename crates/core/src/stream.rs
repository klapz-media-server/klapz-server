//! Sending a file to a player: as it is, or converted on the way.
//!
//! The caller names the file, its tracks and what the player can take; the engine behind the
//! port decides how each track travels and runs the conversion. A started stream is reached
//! at addresses of its own, opened by a secret of its own rather than by the caller's token: a
//! player hands those addresses to components that never see the account.

use crate::metadata::MediaStream;
use crate::playback::{PlaybackOutput, StreamDecision};
use crate::sidecar::{SidecarError, SidecarSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamProtocol {
    Dash,
    Hls,
}

/// How a file travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The file as it is, which the player says it reads.
    Original,
    Converted(StreamProtocol),
}

/// One track, as an engine has to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamTrack {
    /// The index the transcoder addresses the track by.
    pub position: i32,
    pub codec: String,
    pub language: Option<String>,
    /// Bits per second, when measured.
    pub bitrate: Option<i32>,
    /// The page a teletext track's subtitles are decoded from, when it declares one.
    pub teletext_page: Option<u16>,
}

impl StreamTrack {
    pub fn of(stream: &MediaStream) -> Self {
        Self {
            position: stream.position,
            codec: stream.codec.clone().unwrap_or_default(),
            language: stream.language.clone(),
            bitrate: stream.bitrate,
            teletext_page: stream
                .subtitle
                .as_ref()
                .and_then(|subtitle| subtitle.teletext_pages.as_deref())
                .and_then(crate::teletext::subtitle_page)
                .map(|page| page.number),
        }
    }
}

/// A file and the measurements of the version it belongs to.
#[derive(Debug, Clone)]
pub struct StreamSource {
    /// As stored. The engine opens it only once it is found to lie within one of `roots`.
    pub path: String,
    /// The roots of the file's library.
    pub roots: Vec<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Bits per second.
    pub bitrate: Option<i32>,
    pub frame_rate: Option<f64>,
    pub duration_ms: Option<i64>,
    pub video: StreamTrack,
    pub audio: StreamTrack,
    /// The subtitle tracks inside the file, sidecars aside: the engine may prepare those it
    /// delivers beside the stream before the player asks, since it changes track by starting anew.
    pub subtitles: Vec<StreamTrack>,
}

/// How a subtitle reaches the player of a conversion.
#[derive(Debug, Clone, PartialEq)]
pub enum SubtitleOutput {
    /// Drawn into the picture. `scale` is 1.0 for the normal size.
    Burn { track: StreamTrack, scale: f64 },
    /// Beside the stream, whole, for the player to draw.
    Text {
        track: StreamTrack,
        format: SubtitleFormat,
    },
}

impl SubtitleOutput {
    /// Copied when the track is already in the format it is sent in; burning re-encodes it
    /// into the picture.
    #[must_use]
    pub fn decision(&self) -> StreamDecision {
        match self {
            Self::Text { track, format }
                if SubtitleFormat::of_codec(&track.codec) == Some(*format) =>
            {
                StreamDecision::Copy
            }
            Self::Text { .. } | Self::Burn { .. } => StreamDecision::Transcode,
        }
    }
}

/// A format a player draws subtitles in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Ass,
    WebVtt,
    Srt,
    /// TTML holding pictures, the one format a Blu-ray's, a DVD's or a broadcast's subtitles
    /// reach a player in without being burned in.
    Ttml,
}

/// Whether a subtitle of this codec is pictures rather than text.
fn is_image(codec: &str) -> bool {
    matches!(
        codec,
        "pgs"
            | "hdmv_pgs_subtitle"
            | "vobsub"
            | "dvd_subtitle"
            | "dvdsub"
            | "dvb_subtitle"
            | "dvbsub"
            | "xsub"
    )
}

impl SubtitleFormat {
    fn of_codec(codec: &str) -> Option<Self> {
        match codec {
            "ass" => Some(Self::Ass),
            "webvtt" | "vtt" => Some(Self::WebVtt),
            "srt" | "subrip" => Some(Self::Srt),
            _ => None,
        }
    }

    /// The format a track reaches a player in. Pictures go as TTML, when the player reads it;
    /// text in its own format when the player reads it, else the first text format the player
    /// names, since any text converts into any of these. `None` for a player that reads no
    /// format for the track. Whether a track that is not pictures is text is the engine's to
    /// say.
    pub fn for_track(codec: &str, readable: &[Self]) -> Option<Self> {
        if is_image(codec) {
            return readable.contains(&Self::Ttml).then_some(Self::Ttml);
        }
        Self::of_codec(codec)
            .filter(|own| readable.contains(own))
            .or_else(|| {
                readable
                    .iter()
                    .copied()
                    .find(|format| *format != Self::Ttml)
            })
    }
}

/// How a converted stream's quality follows the link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StreamQuality {
    /// One quality for the whole stream.
    #[default]
    Fixed,
    /// One quality announced, which the server changes by measuring how fast the player
    /// receives it.
    ServerAdaptive,
    /// Every quality announced, and the player chooses.
    ClientAdaptive,
}

/// What the player asks for beyond being able to read the result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamLimits {
    /// Bits per second: for an adaptive stream, a ceiling on every quality.
    pub max_video_bitrate: Option<u32>,
    /// Convert audio even when it could be copied.
    pub convert_audio: bool,
    pub quality: StreamQuality,
    /// The video codecs the player decodes, as tracks name them: only those are sent as they
    /// are. `None`, any the protocol carries.
    pub video_codecs: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct StreamOrder {
    /// The name the stream is stored under: a new start under it replaces it.
    pub key: String,
    /// What is being watched, across changes of quality: the streams of the same playback
    /// are replaced too.
    pub playback: Option<String>,
    pub source: StreamSource,
    pub delivery: Delivery,
    pub limits: StreamLimits,
    pub subtitle: Option<SubtitleOutput>,
    /// The text formats the player draws subtitles in, by preference; empty when it named none.
    pub subtitle_formats: Vec<SubtitleFormat>,
    pub position_ms: i64,
    /// When the player asked for it. Two starts of one playback can reach the engine in either
    /// order, so the later one asked for is the one kept, not the later one to arrive.
    pub asked_at: std::time::Instant,
}

#[derive(Debug, Clone)]
pub struct StartedStream {
    /// Where the player reads it: a path on this server, its secret included. A manifest for a
    /// conversion, the file itself otherwise.
    pub url: String,
    /// Where the player reads the subtitle beside the stream, when it asked for it.
    pub subtitles_url: Option<String>,
    /// The format they are in.
    pub subtitle_format: Option<SubtitleFormat>,
    pub video: StreamDecision,
    pub audio: StreamDecision,
    /// What happens to the subtitle, when one was asked for.
    pub subtitle: Option<StreamDecision>,
    /// Every subtitle track of the file the player can read beside a conversion, the one asked
    /// for among them: it switches between them without starting anew.
    pub subtitle_files: Vec<SubtitleFile>,
    pub output: PlaybackOutput,
    /// The keys of the streams of the same playback this one replaced.
    pub replaced: Vec<String>,
}

/// A subtitle track of the file, served whole beside a conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleFile {
    /// The track's position in the file.
    pub position: i32,
    /// A path on this server, its secret included.
    pub url: String,
    pub format: SubtitleFormat,
    pub decision: StreamDecision,
}

/// A format a player declares it reads as it is: containers, and the video and audio codecs
/// it reads in them. An empty list admits anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerFormat {
    pub containers: Vec<String>,
    pub video_codecs: Vec<String>,
    pub audio_codecs: Vec<String>,
}

/// What a player has to read of a file to play it as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileFormat<'a> {
    pub container: Option<&'a str>,
    pub video_codec: Option<&'a str>,
    /// The audio track it would play.
    pub audio_codec: Option<&'a str>,
    /// Bits per second, of the whole version.
    pub bitrate: Option<i64>,
}

impl PlayerFormat {
    /// Whether this format holds the file: its container, and each of its codecs it has.
    /// Names are compared without regard to case; a container named by several names, as
    /// ffmpeg names MP4 (`mov,mp4,m4a`), is held when one of them is.
    pub fn reads(&self, file: FileFormat<'_>) -> bool {
        let admits = |list: &[String], value: Option<&str>| {
            list.is_empty()
                || value.is_some_and(|value| {
                    value.split(',').any(|name| {
                        list.iter()
                            .any(|held| held.eq_ignore_ascii_case(name.trim()))
                    })
                })
        };
        // A file without such a track needs no codec for it.
        let codec_admitted =
            |list: &[String], codec: Option<&str>| codec.is_none() || admits(list, codec);
        admits(&self.containers, file.container)
            && codec_admitted(&self.video_codecs, file.video_codec)
            && codec_admitted(&self.audio_codecs, file.audio_codec)
    }
}

/// The names a file's container answers to: its own, and `webm` for a Matroska file holding
/// nothing but `WebM`'s codecs — VP8, VP9 or AV1 pictures, Vorbis or Opus sound, no subtitle,
/// cover or data track — which a player of `WebM` reads as one of its own. `other_streams`
/// counts what the file carries besides its tracks, attached files apart.
pub fn container_names(
    container: &str,
    tracks: &[crate::metadata::MediaStream],
    other_streams: usize,
) -> String {
    use crate::metadata::StreamKind;
    let webm = container.eq_ignore_ascii_case("mkv")
        && other_streams == 0
        && !tracks.is_empty()
        && tracks.iter().all(|track| match track.kind {
            StreamKind::Video => matches!(track.codec.as_deref(), Some("vp8" | "vp9" | "av1")),
            StreamKind::Audio => matches!(track.codec.as_deref(), Some("vorbis" | "opus")),
            StreamKind::Subtitle => false,
        });
    if webm {
        format!("{container},webm")
    } else {
        container.to_owned()
    }
}

/// Whether a player reads a file as it is, so that it can be sent untouched: one of the formats
/// it declares holds the file, and the file weighs no more than the most the player takes. A
/// file of unknown weight is not held back for it.
pub fn reads_as_is(
    formats: &[PlayerFormat],
    file: FileFormat<'_>,
    max_bitrate: Option<i64>,
) -> bool {
    let light_enough = match (file.bitrate, max_bitrate) {
        (Some(bitrate), Some(max)) => bitrate <= max,
        _ => true,
    };
    light_enough && formats.iter().any(|format| format.reads(file))
}

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("the file is not on this machine")]
    MissingFile,
    #[error("the video would have to be converted to a codec this server does not produce")]
    VideoNotConvertible,
    #[error("this subtitle cannot be delivered this way")]
    SubtitleNotDeliverable,
    #[error("too many streams are running")]
    TooManyStreams,
    #[error("a later start of the same playback replaced this one")]
    Superseded,
    #[error("the stream could not be started")]
    Engine(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Why a conversion was not restarted the more conservative way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FallbackRefusal {
    /// Nothing converts under the key: no stream, a file sent as it is, or a process gone.
    #[error("no conversion is running under this key")]
    NoConversion,
    /// The conversion already runs the most conservative way the engine has.
    #[error("this conversion has nothing to fall back to")]
    Exhausted,
}

#[async_trait::async_trait]
pub trait StreamEngine: Send + Sync + std::fmt::Debug {
    /// Starts a stream, or keeps the one under the same key if it produces the same thing:
    /// players ask for the same start more than once.
    async fn start(&self, order: StreamOrder) -> Result<StartedStream, StreamError>;

    /// Stops the stream stored under `key`: its process, its addresses and what it wrote.
    /// Whether there was one.
    fn stop(&self, key: &str) -> bool;

    /// A sign of life for the stream under `key`. Whether there is one.
    fn touch(&self, key: &str) -> bool;

    /// Restarts the conversion under `key` the more conservative way, under the same
    /// addresses: from hardware to software, from HEVC to H.264. What a player asks for when
    /// it cannot decode what it receives.
    ///
    /// # Errors
    ///
    /// No conversion under `key`, or none more conservative than the one running.
    fn fall_back(&self, key: &str) -> Result<(), FallbackRefusal>;

    /// The keys of the streams nobody has asked anything of for the engine's idle limit, and
    /// of those whose conversion is gone.
    fn idle(&self) -> Vec<String>;

    /// Stops the stream under `key` if it is still one `idle` would list. Whether it did.
    fn stop_if_idle(&self, key: &str) -> bool;

    /// Stops every stream, and returns once their processes have exited.
    async fn stop_all(&self);

    /// The content of a sidecar subtitle, provided it lies within its library and beside its
    /// media.
    ///
    /// # Errors
    ///
    /// [`SidecarError::Missing`] for a file that is not there or not where it may be,
    /// [`SidecarError::TooLarge`] past what a subtitle can weigh.
    async fn read_sidecar(&self, sidecar: &SidecarSource) -> Result<Vec<u8>, SidecarError>;

    /// Deletes a sidecar subtitle's file, under the same conditions as reading it. A file
    /// already gone is not an error.
    ///
    /// # Errors
    ///
    /// [`SidecarError::Missing`] for a file not where a sidecar may be, an I/O failure.
    async fn remove_sidecar(&self, sidecar: &SidecarSource) -> Result<(), SidecarError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(kind: crate::metadata::StreamKind, codec: &str) -> crate::metadata::MediaStream {
        crate::metadata::MediaStream {
            id: crate::ids::StreamId::new(1),
            file_id: crate::ids::PartId::new(1),
            kind,
            position: 0,
            codec: Some(codec.to_owned()),
            language: None,
            channels: None,
            bitrate: None,
            is_default: false,
            is_forced: false,
            external_path: None,
            title: None,
            stream_identifier: None,
            profile: None,
            bit_depth: None,
            traits: crate::metadata::TrackTraits::default(),
            video: None,
            audio: None,
            subtitle: None,
            required_bandwidths: Vec::new(),
            stated: crate::metadata::StatedTrack::default(),
        }
    }

    #[test]
    fn a_text_subtitle_keeps_its_format_when_the_player_reads_it() {
        use SubtitleFormat::{Ass, Srt, WebVtt};
        assert_eq!(SubtitleFormat::for_track("ass", &[WebVtt, Ass]), Some(Ass));
        assert_eq!(
            SubtitleFormat::for_track("subrip", &[WebVtt, Ass]),
            Some(WebVtt)
        );
        assert_eq!(SubtitleFormat::for_track("subrip", &[Ass, Srt]), Some(Srt));
        assert_eq!(SubtitleFormat::for_track("mov_text", &[Srt]), Some(Srt));
        assert_eq!(SubtitleFormat::for_track("ass", &[]), None);
    }

    #[test]
    fn pictures_go_as_ttml_and_text_never_does() {
        use SubtitleFormat::{Ass, Ttml, WebVtt};
        assert_eq!(
            SubtitleFormat::for_track("hdmv_pgs_subtitle", &[WebVtt, Ttml]),
            Some(Ttml)
        );
        assert_eq!(SubtitleFormat::for_track("pgs", &[WebVtt, Ass]), None);
        assert_eq!(
            SubtitleFormat::for_track("subrip", &[Ttml, WebVtt]),
            Some(WebVtt)
        );
        assert_eq!(SubtitleFormat::for_track("subrip", &[Ttml]), None);
    }

    #[test]
    fn a_teletext_track_reaches_the_engine_with_the_page_it_is_decoded_from() {
        use crate::ids::{PartId, StreamId};
        use crate::metadata::{StatedTrack, StreamKind, SubtitleAnalysis, TrackTraits};

        let stored = |pages: Option<&str>| MediaStream {
            id: StreamId::new(1),
            file_id: PartId::new(1),
            kind: StreamKind::Subtitle,
            position: 0,
            codec: Some("dvb_teletext".to_owned()),
            language: None,
            channels: None,
            bitrate: None,
            is_default: false,
            is_forced: false,
            external_path: None,
            title: None,
            stream_identifier: None,
            profile: None,
            bit_depth: None,
            traits: TrackTraits::default(),
            video: None,
            audio: None,
            subtitle: Some(SubtitleAnalysis {
                teletext_pages: pages.map(ToOwned::to_owned),
                ..SubtitleAnalysis::default()
            }),
            required_bandwidths: Vec::new(),
            stated: StatedTrack::default(),
        };
        let arte = stored(Some("888:fra:hearing_impaired,889:fra:subtitles"));
        assert_eq!(StreamTrack::of(&arte).teletext_page, Some(889));
        // Read before its pages were known.
        assert_eq!(StreamTrack::of(&stored(None)).teletext_page, None);
    }

    #[test]
    fn a_subtitle_is_copied_only_when_sent_in_its_own_format() {
        let sent = |codec: &str, format| {
            let track = StreamTrack {
                position: 2,
                codec: codec.to_owned(),
                language: None,
                bitrate: None,
                teletext_page: None,
            };
            SubtitleOutput::Text { track, format }.decision()
        };
        assert_eq!(sent("ass", SubtitleFormat::Ass), StreamDecision::Copy);
        assert_eq!(sent("subrip", SubtitleFormat::Srt), StreamDecision::Copy);
        assert_eq!(
            sent("subrip", SubtitleFormat::WebVtt),
            StreamDecision::Transcode
        );
        assert_eq!(
            sent("hdmv_pgs_subtitle", SubtitleFormat::Ttml),
            StreamDecision::Transcode
        );
        let burned = SubtitleOutput::Burn {
            track: StreamTrack {
                position: 2,
                codec: "ass".to_owned(),
                language: None,
                bitrate: None,
                teletext_page: None,
            },
            scale: 1.0,
        };
        assert_eq!(burned.decision(), StreamDecision::Transcode);
    }

    #[test]
    fn a_matroska_file_of_webm_codecs_answers_to_webm_too() {
        use crate::metadata::StreamKind;
        let noob = [
            track(StreamKind::Video, "vp9"),
            track(StreamKind::Audio, "vorbis"),
        ];
        assert_eq!(container_names("mkv", &noob, 0), "mkv,webm");
        // A cover or a subtitle is no part of WebM.
        assert_eq!(container_names("mkv", &noob, 1), "mkv");
        let subtitled = [
            track(StreamKind::Video, "av1"),
            track(StreamKind::Subtitle, "srt"),
        ];
        assert_eq!(container_names("mkv", &subtitled, 0), "mkv");
        let h264 = [
            track(StreamKind::Video, "h264"),
            track(StreamKind::Audio, "opus"),
        ];
        assert_eq!(container_names("mkv", &h264, 0), "mkv");
        assert_eq!(container_names("mp4", &noob, 0), "mp4");
        let webm_player = PlayerFormat {
            containers: vec!["webm".to_owned()],
            ..PlayerFormat::default()
        };
        let names = container_names("mkv", &noob, 0);
        assert!(webm_player.reads(FileFormat {
            container: Some(&names),
            ..FileFormat::default()
        }));
    }

    fn format(containers: &[&str], video: &[&str], audio: &[&str]) -> PlayerFormat {
        let owned = |list: &[&str]| list.iter().map(|name| (*name).to_owned()).collect();
        PlayerFormat {
            containers: owned(containers),
            video_codecs: owned(video),
            audio_codecs: owned(audio),
        }
    }

    fn mkv(video: &'static str, audio: &'static str) -> FileFormat<'static> {
        FileFormat {
            container: Some("mkv"),
            video_codec: Some(video),
            audio_codec: Some(audio),
            bitrate: Some(8_000_000),
        }
    }

    #[test]
    fn a_format_holds_a_file_whose_container_and_codecs_it_names() {
        let browser = format(&["mkv", "mp4"], &["h264"], &["aac", "opus"]);
        assert!(browser.reads(mkv("h264", "aac")));
        assert!(!browser.reads(mkv("hevc", "aac")));
        assert!(!browser.reads(mkv("h264", "dca")));
        assert!(!format(&["mp4"], &[], &[]).reads(mkv("h264", "aac")));
    }

    #[test]
    fn an_empty_list_admits_anything_and_a_missing_track_needs_nothing() {
        assert!(format(&[], &[], &[]).reads(mkv("hevc", "dca")));
        let silent = FileFormat {
            audio_codec: None,
            ..mkv("h264", "aac")
        };
        assert!(format(&["MKV"], &["H264"], &["aac"]).reads(silent));
    }

    #[test]
    fn a_container_of_several_names_is_held_by_one_of_them() {
        let file = FileFormat {
            container: Some("mov,mp4,m4a,3gp,3g2,mj2"),
            ..mkv("h264", "aac")
        };
        assert!(format(&["mp4"], &[], &[]).reads(file));
        assert!(!format(&["webm"], &[], &[]).reads(file));
    }

    #[test]
    fn a_player_reads_as_is_what_one_format_holds_under_its_bitrate() {
        let formats = [
            format(&["webm"], &["vp9"], &["opus"]),
            format(&["mkv"], &["h264"], &["aac"]),
        ];
        assert!(reads_as_is(&formats, mkv("h264", "aac"), Some(10_000_000)));
        assert!(!reads_as_is(&formats, mkv("h264", "aac"), Some(4_000_000)));
        assert!(reads_as_is(&formats, mkv("h264", "aac"), None));
        assert!(!reads_as_is(&[], mkv("h264", "aac"), None));
        let unweighed = FileFormat {
            bitrate: None,
            ..mkv("h264", "aac")
        };
        assert!(reads_as_is(&formats, unweighed, Some(1)));
    }
}
