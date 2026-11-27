//! A stream in progress: one supervised transcoder process at a time, and what it wrote.
//!
//! The command line is the reference's, taken from its log. The process reads the source and
//! reports its segments over HTTP on loopback, as it does on Plex, at URLs holding an
//! unguessable per-session id: this also keeps the client's token out of the process list.
//!
//! Every process a session starts is a run, writing into a directory of its own: a seek or a
//! change of rung starts another, and what earlier runs wrote stays and is served. An adaptive
//! stream is a ladder of rungs. When the server moves between them they share one
//! representation, and the newest run started at or before a segment is the one that answers
//! for it. When the player chooses, each rung is a representation of its own, and ffmpeg-plex
//! cuts and numbers its segments on the media's timeline (`absolute_segments`), its audio
//! where each key frame is shown: segment N is the same in every run, whatever rung and
//! wherever it started, and any run of the rung that wrote it may answer for it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::adapt::{self, Delivery, ServerAdaptation};
use crate::plan::{
    audio_decoder, video_decoder, Adaptation, AudioAction, H264Preset, KeyFrames, Ladder,
    StreamPlan, Track, VideoAction, VideoEncode,
};
use crate::subtitle::{self, Extraction, SubtitleFormat};
use crate::{TranscodeError, Transcoder};

/// Each session is a process and a directory of segments, and a client controls how many.
const MAX_SESSIONS: usize = 4;

/// A client that stopped asking has gone away without saying so.
const IDLE_LIFETIME: Duration = Duration::from_secs(300);

/// How old a directory no session owns must be to be removed: a session creates its directory
/// a moment before it is registered.
const ORPHAN_AGE: Duration = Duration::from_secs(60);

/// How a session's directory begins, below the transcoder's.
const SESSION_DIRECTORY: &str = "transcode-";

/// How long a request for a playlist or a segment waits for the process to reach it.
const WAIT_FOR_OUTPUT: Duration = Duration::from_secs(20);

/// How often a wait looks for a file. The process's announcements do not follow its files: it
/// can write several chunks between two of them, and none after the last.
const FILE_POLL: Duration = Duration::from_millis(100);

/// How long a request for a chunk is held once the session's process has ended. Refused at once,
/// a player retries the chunk in a tight loop. Still watched meanwhile, since the other track's
/// seek can restart the process.
const HOLD_BEFORE_REFUSAL: Duration = Duration::from_secs(5);

/// Enough for a player to start without running dry.
const SEGMENTS_BEFORE_PLAYLIST: usize = 3;

/// How far ahead of what it has written a process is left to catch up by itself, at most half
/// this long at the read rate. A request beyond this is a seek: the new run's burst answers it.
const SEEK_MARGIN_SECONDS: u32 = 30;

/// How many segments in a row the source may leave without a key frame. None of them can start
/// a file, and each is answered with the one it is shown in, once a later one proves it.
const SEGMENTS_WITHOUT_KEY_FRAME: u32 = 4;

/// How far ahead of real time a run reads its source. Unbounded, a copy writes a film in
/// minutes, gigabytes nobody may watch, and fills the disk. Ours: the reference has none.
const READ_RATE: &str = "2";

/// The media a run reads at full speed when it starts, to fill the player's buffer quickly.
const READ_BURST_SECONDS: u32 = SEEK_MARGIN_SECONDS;

/// How the result reaches the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Hls,
    Dash,
}

/// A track of a DASH stream: the video of one rendition, or the audio all of them share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashTrack {
    Video(usize),
    Audio,
}

impl DashTrack {
    /// The process's number for it, in its file names.
    const fn stream(self) -> u32 {
        match self {
            Self::Video(_) => 0,
            Self::Audio => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamRequest {
    /// The client's session identifier, which later requests name it by.
    pub session: String,
    /// What is being watched, across changes of quality: a new stream under the same name
    /// replaces the one before it.
    pub playback: Option<String>,
    /// When the player asked for it, which decides which of two starts of a playback replaces
    /// the other.
    pub asked_at: Instant,
    /// The file, already checked to lie within a library.
    pub source: PathBuf,
    pub video: Track,
    pub audio: Track,
    /// The stream starts on its top rung.
    pub ladder: Ladder,
    /// Where to start, in seconds.
    pub offset: Option<f64>,
    pub protocol: Protocol,
    /// The media's full duration in milliseconds, for the predicted HLS playlist; 0 if
    /// unknown.
    pub total_ms: u64,
    /// The subtitle the player gets, if any. Drawing it in needs the plan to encode the video.
    pub subtitle: Option<SubtitleDelivery>,
    /// The file's subtitle tracks and the format each would reach the player in: those that
    /// convert into it are extracted from the start, for the player to switch to.
    pub text_subtitles: Vec<(Track, SubtitleFormat)>,
    /// How hard x264 works on every rung it encodes.
    pub h264_preset: H264Preset,
}

/// How a subtitle reaches the player.
#[derive(Debug, Clone, PartialEq)]
pub enum SubtitleDelivery {
    /// Drawn into the picture. `scale` is the client's size, 1.0 for its 100%.
    Burn { track: Track, scale: f64 },
    /// Beside the stream, extracted whole in a format the player reads, for it to draw: text,
    /// or pictures as TTML.
    Text {
        track: Track,
        format: SubtitleFormat,
    },
}

impl SubtitleDelivery {
    const fn track(&self) -> &Track {
        match self {
            Self::Burn { track, .. } | Self::Text { track, .. } => track,
        }
    }

    /// Text the process can draw with libass or convert to another text format.
    fn is_text(&self) -> bool {
        subtitle::is_text(&self.track().codec)
    }

    /// Pictures, laid over the video rather than drawn.
    fn is_image(&self) -> bool {
        subtitle::is_image(&self.track().codec)
    }

    /// Whether this delivery can be made: text drawn with fonts and pictures laid over the
    /// video, or either converted into a format that holds it.
    fn deliverable(&self, fonts: bool) -> bool {
        match self {
            Self::Burn { .. } => (self.is_text() && fonts) || self.is_image(),
            Self::Text { track, format } => format.holds(&track.codec),
        }
    }
}

/// What an encode draws into its picture.
#[derive(Debug, Clone, Copy)]
enum Burn<'a> {
    /// A text subtitle, drawn by libass at the client's scale.
    Text {
        track: &'a Track,
        scale: f64,
        fonts: &'a Path,
    },
    /// An image subtitle, its canvas stretched to the picture and laid over it, as the
    /// reference's software path does (`[%s]scale=%d:%d[%i];` then `[%i]overlay` in its binary;
    /// the command itself was not captured).
    Image { track: &'a Track },
}

impl Burn<'_> {
    /// How many filter outputs the video's chain names, which the audio's numbering follows:
    /// two to scale, then one to draw text or two to overlay a picture.
    const fn video_labels(burn: Option<Self>) -> u32 {
        match burn {
            None => 2,
            Some(Self::Text { .. }) => 3,
            Some(Self::Image { .. }) => 4,
        }
    }
}

impl StreamRequest {
    /// Whether this asks for what a running session already produces. A player may change quality
    /// under the same session id.
    fn produces_the_same_as(&self, other: &Self) -> bool {
        self.source == other.source
            && self.video == other.video
            && self.audio == other.audio
            && self.ladder == other.ladder
            && self.protocol == other.protocol
            && self.subtitle == other.subtitle
            && self.h264_preset == other.h264_preset
    }

    fn segment_seconds(&self) -> u32 {
        self.ladder.segment_seconds(self.protocol)
    }

    fn client_adapted(&self) -> bool {
        self.ladder.adaptation == Adaptation::Client
    }

    /// Which representation a rung is written as: its own when the player chooses, the one
    /// they all share otherwise.
    fn rendition_of(&self, rung: usize) -> usize {
        if self.client_adapted() {
            rung
        } else {
            0
        }
    }
}

/// One segment, once the process has announced it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub number: u32,
    pub seconds: f64,
}

/// The state of a session's output.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub segments: Vec<Segment>,
    /// The process is done: nothing will be added.
    pub ended: bool,
    /// The media's full duration in milliseconds; 0 when unknown.
    pub total_ms: u64,
    /// The nominal length of a segment, in seconds.
    pub segment_seconds: u32,
}

/// What a client's status display reads about a running conversion: the reference's
/// `TranscodeSession`, decisions and live progress together.
#[derive(Debug, Clone)]
pub struct SessionStatus {
    pub protocol: &'static str,
    pub container: &'static str,
    pub source_video_codec: String,
    pub source_audio_codec: String,
    pub video_decision: &'static str,
    pub audio_decision: &'static str,
    pub video_codec: String,
    pub audio_codec: String,
    /// The converted audio's channel count; `None` when the audio is copied and unknown.
    pub audio_channels: Option<u32>,
    /// The rung of the latest video segment the player received: its encode, `None` when the
    /// video is copied.
    pub video_encode: Option<VideoEncode>,
    /// The converted audio's bitrate; `None` when the audio is copied.
    pub audio_kbps: Option<u32>,
    pub duration_ms: u64,
    /// How far content is available, and from where, in seconds.
    pub max_offset_seconds: f64,
    pub min_offset_seconds: f64,
    /// Per cent of the whole produced so far.
    pub progress: f64,
    /// Media seconds produced per wall-clock second.
    pub speed: f64,
    pub remaining_seconds: i64,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy)]
struct Announced {
    start: f64,
    seconds: f64,
    finished: bool,
}

/// One process's output.
#[derive(Debug)]
struct Run {
    generation: u64,
    rung: usize,
    rendition: usize,
    /// The segment it was started at, as the player numbers them.
    start_chunk: u32,
    directory: PathBuf,
    /// HLS files, as the process announced them.
    segments: BTreeMap<u32, Announced>,
    ended: bool,
    /// Its process exited successfully: it went as far as the media goes.
    completed: bool,
}

impl Run {
    /// The file holding segment `number`, `stream` for DASH, once whole: a DASH file is renamed
    /// from its temporary name when complete, an HLS one announced finished. When the source has
    /// no key frame within the segment, none starts there and the file it is shown in answers,
    /// once a file after it proves that none will.
    fn written(&self, protocol: Protocol, stream: u32, number: u32) -> Option<PathBuf> {
        let path = |number: u32| match protocol {
            Protocol::Dash => self.directory.join(chunk_name(stream, number + 1)),
            Protocol::Hls => self.directory.join(hls_name(number)),
        };
        let whole = |number: u32| match protocol {
            Protocol::Dash => path(number).exists(),
            Protocol::Hls => self
                .segments
                .get(&number)
                .is_some_and(|segment| segment.finished),
        };
        if number < self.start_chunk {
            return None;
        }
        if whole(number) {
            return Some(path(number));
        }
        let skipped = (number + 1..=number.saturating_add(SEGMENTS_WITHOUT_KEY_FRAME)).any(whole);
        if !skipped {
            return None;
        }
        // A run opens at the key frame before its start, so the file showing the segment it was
        // started at can be numbered before it.
        (self.start_chunk.saturating_sub(SEGMENTS_WITHOUT_KEY_FRAME)..number)
            .rev()
            .find(|n| whole(*n))
            .map(path)
    }

    /// Whether it ran to the end of the media from at or before segment `number` and wrote
    /// nothing from there on.
    fn ended_before(&self, protocol: Protocol, stream: u32, number: u32) -> bool {
        // A segment without a key frame is written with a later one, never further than this. An
        // HLS file is looked for on disk too, in case its announcement never came.
        let on_disk = (number..=number.saturating_add(SEGMENTS_WITHOUT_KEY_FRAME)).any(|n| {
            let name = match protocol {
                Protocol::Dash => chunk_name(stream, n + 1),
                Protocol::Hls => hls_name(n),
            };
            self.directory.join(name).exists()
        });
        let wrote_from = on_disk
            || (protocol == Protocol::Hls
                && self.segments.range(number..).any(|(_, s)| s.finished));
        self.completed && self.start_chunk <= number && !wrote_from
    }

    fn latest_finished(&self) -> Option<u32> {
        self.segments
            .iter()
            .rev()
            .find(|(_, segment)| segment.finished)
            .map(|(number, _)| *number)
    }
}

#[derive(Debug, Default)]
struct Progress {
    /// Oldest first: the last is the one running, or the one that ran last.
    runs: Vec<Run>,
    /// The manifest the process wrote for DASH, once it holds the tracks, by rendition.
    manifests: HashMap<usize, String>,
}

impl Progress {
    fn current(&self) -> Option<&Run> {
        self.runs.last()
    }
}

/// The process currently working for a session.
#[derive(Debug)]
struct Process {
    generation: u64,
    /// When it started, for the transcode speed a status reports.
    started: Instant,
    stop: Arc<Notify>,
    task: Option<JoinHandle<()>>,
}

/// What decides a session's rung.
#[derive(Debug)]
struct Switching {
    /// The server's choice, for a stream it adapts.
    server: Option<ServerAdaptation>,
    /// When the process was last restarted for a rung the player asked for.
    last_client_switch: Option<Instant>,
    /// The rung of the latest video segment served.
    served_rung: usize,
    /// The latest video segment asked for: where a player switching rungs is.
    last_video_chunk: Option<u32>,
    /// The latest video segment sent whole, and when its sending began: it is judged once the
    /// player asks for the next one.
    sent: Option<(Delivery, Instant)>,
}

#[derive(Debug)]
pub(crate) struct Session {
    callback: String,
    /// The process's way back in, up to the session: `…/transcoder`.
    callback_base: String,
    request: StreamRequest,
    directory: PathBuf,
    progress: Mutex<Progress>,
    changed: Notify,
    closed: AtomicBool,
    process: Mutex<Option<Process>>,
    /// Held while one process replaces another: the video and the audio of one seek both
    /// arrive here, and only the first may restart anything.
    restarting: tokio::sync::Mutex<()>,
    last_used: Mutex<Instant>,
    switching: Mutex<Switching>,
    /// The text subtitle, extracted apart from the start.
    subtitle: Option<Arc<Extraction>>,
}

/// Removed with the last reference, which the process's supervising task holds until it exits.
impl Drop for Session {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.directory) {
            tracing::warn!(%error, "could not remove a transcode session's directory");
        }
    }
}

impl Session {
    fn touch(&self) {
        *self
            .last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
    }

    fn with_progress<T>(&self, read: impl FnOnce(&mut Progress) -> T) -> T {
        read(&mut self.progress.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn with_process<T>(&self, read: impl FnOnce(&mut Option<Process>) -> T) -> T {
        read(&mut self.process.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn with_switching<T>(&self, read: impl FnOnce(&mut Switching) -> T) -> T {
        read(
            &mut self
                .switching
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    fn seconds(&self) -> f64 {
        f64::from(self.request.segment_seconds())
    }

    fn current_generation(&self) -> Option<u64> {
        self.with_progress(|progress| progress.current().map(|run| run.generation))
    }

    /// The running run's rendition, rung and starting segment.
    fn current(&self) -> Option<(usize, usize, u32)> {
        self.with_progress(|progress| {
            progress
                .current()
                .map(|run| (run.rendition, run.rung, run.start_chunk))
        })
    }

    /// Whether the latest process has ended, or none was started.
    fn ended(&self) -> bool {
        self.with_progress(|progress| progress.current().is_none_or(|run| run.ended))
    }

    /// Waits until `ready` holds or the process has had long enough.
    async fn wait_until(&self, ready: impl Fn(&Progress) -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT_FOR_OUTPUT;
        loop {
            // Taken before checking, so a change arriving in between is not missed.
            let change = self.changed.notified();
            if self.with_progress(|progress| {
                ready(progress) || progress.current().is_none_or(|run| run.ended)
            }) {
                return;
            }
            if tokio::time::timeout_at(deadline, change).await.is_err() {
                return;
            }
        }
    }

    /// Waits for part of the output to exist. Whether it did in time.
    async fn wait_for<T>(&self, mut found: impl FnMut() -> Option<T>) -> Option<T> {
        let began = tokio::time::Instant::now();
        loop {
            if let Some(found) = found() {
                return Some(found);
            }
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            let limit = if self.ended() {
                HOLD_BEFORE_REFUSAL
            } else {
                WAIT_FOR_OUTPUT
            };
            if began.elapsed() >= limit {
                return None;
            }
            tokio::time::sleep(FILE_POLL).await;
        }
    }

    /// The file holding chunk `number` of a stream copied or converted at one rendition. When
    /// the server moves between rungs, only the newest run started at or before it may answer:
    /// an older one wrote another rung there.
    fn written_chunk(&self, stream: u32, number: u32) -> Option<PathBuf> {
        let only_newest = self.request.ladder.adaptation == Adaptation::Server;
        self.with_progress(|progress| {
            progress
                .runs
                .iter()
                .rev()
                .filter(|run| run.start_chunk <= number)
                .take(if only_newest { 1 } else { usize::MAX })
                .map(|run| run.directory.join(chunk_name(stream, number + 1)))
                .find(|path| path.exists())
        })
    }

    /// Whether the running process will reach this chunk soon without being told to.
    fn will_reach(&self, stream: u32, number: u32) -> bool {
        let Some((directory, first)) = self.with_progress(|progress| {
            progress
                .current()
                .filter(|run| !run.ended)
                .map(|run| (run.directory.clone(), run.start_chunk))
        }) else {
            return false;
        };
        if number < first {
            return false;
        }
        // A chunk being written carries a `.tmp` suffix until it is whole.
        let in_progress = directory
            .join(format!("{}.tmp", chunk_name(stream, number + 1)))
            .exists();
        let margin = SEEK_MARGIN_SECONDS.div_ceil(self.request.segment_seconds());
        number == first
            || in_progress
            || (number.saturating_sub(margin).max(first)..number)
                .any(|earlier| directory.join(chunk_name(stream, earlier + 1)).exists())
    }

    /// The finished HLS segment `number`, by the same rule as [`Self::written_chunk`].
    fn hls_written(&self, number: u32) -> Option<PathBuf> {
        let only_newest = self.request.ladder.adaptation == Adaptation::Server;
        self.with_progress(|progress| {
            progress
                .runs
                .iter()
                .rev()
                .filter(|run| run.start_chunk <= number)
                .take(if only_newest { 1 } else { usize::MAX })
                .find(|run| run.segments.get(&number).is_some_and(|s| s.finished))
                .map(|run| run.directory.join(hls_name(number)))
        })
    }

    /// Whether segment `number` lies past the end of the media: the manifests are laid from the
    /// stated duration, and can count one more segment than the frames the process found. Every
    /// rung cuts on the same timeline, so any run that reached the end tells.
    fn past_the_end(&self, stream: u32, number: u32) -> bool {
        let protocol = self.request.protocol;
        self.with_progress(|progress| {
            progress
                .runs
                .iter()
                .any(|run| run.ended_before(protocol, stream, number))
        })
    }

    /// Whether the running process will reach this HLS segment soon without being told to.
    fn hls_will_reach(&self, number: u32) -> bool {
        let margin = SEEK_MARGIN_SECONDS.div_ceil(self.request.segment_seconds());
        self.with_progress(|progress| {
            progress
                .current()
                .filter(|run| !run.ended && run.start_chunk <= number)
                .is_some_and(|run| {
                    let frontier = run
                        .latest_finished()
                        .map_or(run.start_chunk, |latest| latest.max(run.start_chunk));
                    number <= frontier.saturating_add(margin)
                })
        })
    }

    /// Segment `number` of a rendition the player chose, `track` `None` for HLS, from the newest
    /// run of that rendition started at or before it that wrote it. The audio is the same on
    /// every rung, so any run's will do.
    fn chosen_file(
        &self,
        track: Option<DashTrack>,
        rendition: usize,
        number: u32,
    ) -> Option<PathBuf> {
        let protocol = self.request.protocol;
        self.with_progress(|progress| {
            progress
                .runs
                .iter()
                .rev()
                .filter(|run| run.start_chunk <= number)
                .filter(|run| match track {
                    Some(DashTrack::Audio) => true,
                    Some(DashTrack::Video(wanted)) => run.rendition == wanted,
                    None => run.rendition == rendition,
                })
                .find_map(|run| run.written(protocol, track.map_or(0, DashTrack::stream), number))
        })
    }

    /// Whether the running process, at this rendition, will write segment `number` soon.
    fn chosen_will_reach(&self, track: Option<DashTrack>, rendition: usize, number: u32) -> bool {
        let running = self
            .current()
            .is_some_and(|(running, _, _)| running == rendition);
        running
            && match track {
                Some(track) => self.will_reach(track.stream(), number),
                None => self.hls_will_reach(number),
            }
    }

    fn output(&self, rendition: usize) -> Output {
        let total_ms = self.request.total_ms;
        let segment_seconds = self.request.segment_seconds();
        let segments = if self.request.client_adapted() {
            // Every segment some run of the rendition completed, at its nominal length.
            let mut segments = Vec::new();
            let mut number = 0;
            while self.chosen_file(None, rendition, number).is_some() {
                segments.push(Segment {
                    number,
                    seconds: f64::from(segment_seconds),
                });
                number += 1;
            }
            segments
        } else {
            self.with_progress(|progress| {
                let mut merged: BTreeMap<u32, f64> = BTreeMap::new();
                for run in &progress.runs {
                    merged.extend(
                        run.segments
                            .iter()
                            .filter(|(_, segment)| segment.finished)
                            .map(|(number, segment)| (*number, segment.seconds)),
                    );
                }
                merged
                    .into_iter()
                    .map(|(number, seconds)| Segment { number, seconds })
                    .collect()
            })
        };
        Output {
            segments,
            ended: self.ended(),
            total_ms,
            segment_seconds,
        }
    }

    /// The rung of a video segment just served, for the status a timeline reports.
    fn served(&self, rung: usize, number: u32) {
        self.with_switching(|switching| {
            switching.served_rung = rung;
            switching.last_video_chunk = Some(number);
        });
    }

    /// The rung the newest run answering for a segment was started at.
    fn rung_answering(&self, number: u32) -> usize {
        self.with_progress(|progress| {
            progress
                .runs
                .iter()
                .rev()
                .find(|run| run.start_chunk <= number)
                .or_else(|| progress.current())
                .map_or(0, |run| run.rung)
        })
    }
}

fn chunk_name(stream: u32, file: u32) -> String {
    format!("chunk-stream{stream}-{file:05}.m4s")
}

fn init_name(stream: u32) -> String {
    format!("init-stream{stream}.m4s")
}

fn hls_name(number: u32) -> String {
    format!("media-{number:05}.ts")
}

/// Reads the transcoder's announcement, which restates the latest segments each time:
/// `media-00000.ts,0.000000,8.000000,7.914667,8.000000`, with a leading `#` while unfinished.
fn parse_announcement(body: &str) -> Vec<(u32, Announced)> {
    body.lines()
        .filter_map(|line| {
            let (finished, line) = line
                .strip_prefix('#')
                .map_or((true, line), |rest| (false, rest));
            let mut fields = line.split(',');
            let number = fields
                .next()?
                .strip_prefix("media-")?
                .strip_suffix(".ts")?
                .parse()
                .ok()?;
            let start: f64 = fields.next()?.parse().ok()?;
            let end: f64 = fields.next()?.parse().ok()?;
            Some((
                number,
                Announced {
                    start,
                    seconds: end - start,
                    finished,
                },
            ))
        })
        .collect()
}

/// `absolute`: on the media's grid, for a rung the player chooses, which ffmpeg-plex alone
/// can do; a build without the option refuses it.
fn hls_output(callback: &str, seconds: u32, first_file: u32, absolute: bool) -> Vec<String> {
    let segment_time = seconds.to_string();
    [
        "-segment_format",
        "mpegts",
        "-f",
        "ssegment",
        "-individual_header_trailer",
        "0",
        "-segment_time",
        &segment_time,
        "-segment_start_number",
        &first_file.to_string(),
        "-segment_time_delta",
        "0.0625",
        "-segment_list",
        &format!("{callback}/manifest?X-Plex-Http-Pipeline=infinite"),
        "-segment_list_type",
        "csv",
        "-segment_list_size",
        // Every segment each time, where the reference keeps the last 5: a remux runs far ahead
        // of its announcements, and the first ones to arrive already listed segment 5 onward.
        // Not 0, the usual "unlimited", which this build exits on without writing anything.
        "100000",
        "-segment_list_separate_stream_times",
        "1",
        "-segment_list_unfinished",
        "1",
    ]
    .iter()
    .chain(if absolute {
        ["-segment_absolute", "1"].iter()
    } else {
        [].iter()
    })
    .chain(&[
        "-max_delay",
        "5000000",
        "-avoid_negative_ts",
        "disabled",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "media-%05d.ts",
    ])
    .map(ToString::to_string)
    .collect()
}

/// This build keeps the chunks on a grid of `seconds` even when copying video, so their number
/// says where they are in time: 494 chunks of 5 seconds for 2467 seconds. The grid starts where
/// the run does, though, unless `absolute` puts it on the media's, as for [`hls_output`].
fn dash_output(callback: &str, seconds: u32, first_file: u32, absolute: bool) -> Vec<String> {
    let mut args = strings(&["-f", "dash", "-seg_duration", &seconds.to_string()]);
    if absolute {
        args.extend(strings(&["-absolute_segments", "1"]));
    }
    args.extend(strings(&[
        "-dash_segment_type",
        "mp4",
        "-init_seg_name",
        "init-stream$RepresentationID$.m4s",
        "-media_seg_name",
        "chunk-stream$RepresentationID$-$Number%05d$.m4s",
        "-window_size",
        "5",
        "-delete_removed",
        "false",
        "-skip_to_segment",
        &first_file.to_string(),
        "-manifest_name",
        &format!("{callback}/manifest?X-Plex-Http-Pipeline=infinite"),
        "-avoid_negative_ts",
        "disabled",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "dash",
    ]));
    args
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// What one process of a session is asked to do.
struct RunPlan<'a> {
    plan: &'a StreamPlan,
    /// Where it starts in the source, in seconds.
    offset: Option<f64>,
    /// The number of its first segment's file.
    first_file: u32,
    callback: &'a str,
}

fn input_arguments(request: &StreamRequest, run: &RunPlan<'_>) -> Vec<String> {
    let StreamRequest {
        video,
        audio,
        protocol,
        ..
    } = request;
    let plan = run.plan;
    let encodes = plan.video != VideoAction::Copy;
    let client = request.client_adapted();
    let mut args = Vec::new();
    if let Some(decoder) = video_decoder(&video.codec) {
        args.extend(strings(&[&format!("-codec:{}", video.index), decoder]));
    }
    if plan.audio == AudioAction::ConvertToAac {
        if let Some(decoder) = audio_decoder(&audio.codec) {
            args.extend(strings(&[&format!("-codec:{}", audio.index), decoder]));
        }
    }
    // The reference always states it for copied DASH, otherwise only when there is one.
    let start = match protocol {
        _ if client => Some(run.offset.unwrap_or(0.0)),
        _ if encodes => run.offset.filter(|seconds| *seconds > 0.0),
        Protocol::Dash => Some(run.offset.unwrap_or(0.0)),
        Protocol::Hls => run.offset,
    };
    if let Some(start) = start {
        args.extend(strings(&["-ss", &start.to_string()]));
    }
    // Rungs cut where the copy is all start on the key frame before, as the copy has to.
    let inaccurate = if client {
        request.ladder.copies()
    } else {
        !encodes
    };
    if inaccurate {
        args.push("-noaccurate_seek".to_owned());
    }
    args.extend(strings(&[
        "-readrate",
        READ_RATE,
        "-readrate_initial_burst",
        &READ_BURST_SECONDS.to_string(),
        "-analyzeduration",
        "20000000",
        "-probesize",
        "20000000",
        "-i",
        &format!("{}/source", run.callback),
        "-start_at_zero",
        "-copyts",
    ]));
    if !encodes || start.is_none_or(|start| start <= 0.0) {
        args.extend(strings(&["-fps_mode", "cfr"]));
    }
    args.extend(strings(&[
        "-y",
        "-nostats",
        "-loglevel",
        "quiet",
        "-loglevel_plex",
        "error",
        "-progressurl",
        &format!("{}/progress", run.callback),
    ]));
    args
}

/// The level every rung of a server-adapted stream declares, high enough for 1080p at 60 frames
/// and the top rung's 20 Mbps: left to x264, it follows the bitrate.
const SHARED_LEVEL: &str = "4.2";

fn video_encoder_options(encode: &VideoEncode, preset: H264Preset) -> Vec<String> {
    let mut args = strings(&[
        "-codec:0",
        "libx264",
        "-crf:0",
        &encode.crf.to_string(),
        "-maxrate:0",
        &format!("{}k", encode.video_kbps),
        "-bufsize:0",
        &format!("{}k", encode.video_kbps * 2),
    ]);
    if let Some(rate) = encode.frame_rate {
        args.extend(strings(&["-r:0", &rate.to_string()]));
    }
    let mut x264 = encode.x264_options().to_owned();
    if encode.key_frames == KeyFrames::AsSource {
        // x264's own key frames would cut a piece where the copy has none.
        x264.push_str(":scenecut=0:keyint=infinite");
    }
    if encode.shared_headers {
        // Otherwise the picture parameters follow the CRF, and the level the bitrate.
        x264.push_str(":stitchable=1");
    }
    args.extend(strings(&["-preset:0", preset.name(), "-x264opts:0", &x264]));
    if encode.shared_headers {
        args.extend(strings(&["-level:0", SHARED_LEVEL]));
    }
    let key_frames = match encode.key_frames {
        KeyFrames::Every(seconds) => format!("expr:gte(t,n_forced*{seconds})"),
        KeyFrames::AsSource => "source".to_owned(),
    };
    args.extend(strings(&["-force_key_frames:0", &key_frames]));
    args
}

/// Escapes a value for an option of a filter inside a filtergraph: once for the option
/// parser, once again for the graph's. The reference's own command escapes this way, `C\\\:`
/// for a drive's colon.
fn filter_value(text: &str) -> String {
    let once = |text: &str| {
        let mut escaped = String::with_capacity(text.len() * 2);
        for character in text.chars() {
            if matches!(
                character,
                '\\' | ':' | '=' | ',' | ' ' | '\'' | '[' | ']' | ';'
            ) {
                escaped.push('\\');
            }
            escaped.push(character);
        }
        escaped
    };
    once(&once(text))
}

/// The reference's libass style, as it writes it: Noto Sans Medium, white with a dark outline.
const SUBTITLE_STYLE: &str = "ScaledBorderAndShadow=yes,FontName=Noto Sans Medium,Bold=500,\
PrimaryColour=&H00FFFFFF,OutlineColour=&H00020713,BackColour=&HCC000000";

/// The filter that draws a subtitle onto the picture labelled `[1]`, into `[2]`.
fn burn_filter(subtitle: &Track, scale: f64, fonts: &Path) -> String {
    // The reference names the directory in Windows' verbatim form and the font without it.
    let font = fonts.join("NotoSans-Medium.otf");
    let font = font.to_string_lossy();
    let font = font.trim_start_matches(r"\\?\");
    let language = subtitle
        .language
        .as_deref()
        .map_or_else(String::new, |language| format!(":language={language}"));
    format!(
        "[1]inlineass=font_scale={scale:.6}:font_path={}:fonts_dir={}{language}:overrides={}:\
outline=2.6:shadow=1.7:font_size=54[2]",
        filter_value(font),
        filter_value(&fonts.to_string_lossy()),
        filter_value(SUBTITLE_STYLE),
    )
}

fn video_arguments(
    video: &Track,
    action: VideoAction,
    burn: Option<Burn<'_>>,
    preset: H264Preset,
) -> Vec<String> {
    let mut args = match action {
        VideoAction::Copy => strings(&["-map", &format!("0:{}", video.index)]),
        VideoAction::Encode(encode) => {
            let scale = format!(
                "[0:{}]scale=w={}:h={}:force_divisible_by=4[0];[0]format=pix_fmts=yuv420p|nv12[1]",
                video.index, encode.width, encode.height
            );
            match burn {
                Some(Burn::Text {
                    track,
                    scale: size,
                    fonts,
                }) => strings(&[
                    "-map_inlineass",
                    &format!("0:{}", track.index),
                    "-filter_complex",
                    &format!("{scale};{}", burn_filter(track, size, fonts)),
                    "-map",
                    "[2]",
                ]),
                Some(Burn::Image { track }) => strings(&[
                    "-filter_complex",
                    &format!(
                        "{scale};[0:{}]scale={}:{}[2];[1][2]overlay[3]",
                        track.index, encode.width, encode.height
                    ),
                    "-map",
                    "[3]",
                ]),
                None => strings(&["-filter_complex", &scale, "-map", "[1]"]),
            }
        }
    };
    if let Some(tag) = &video.language_code {
        args.extend(strings(&["-metadata:s:0", &format!("language={tag}")]));
    }
    match action {
        VideoAction::Copy => args.extend(strings(&["-codec:0", "copy"])),
        VideoAction::Encode(encode) => args.extend(video_encoder_options(&encode, preset)),
    }
    args
}

/// `native_encoder`: the reference converts audio beside copied video with Media Foundation's
/// encoder and beside an encode with ffmpeg's own; rungs a player moves between must all use
/// the same, and use ffmpeg's.
fn audio_arguments(
    audio: &Track,
    plan: &StreamPlan,
    burn: Option<Burn<'_>>,
    native_encoder: bool,
) -> Vec<String> {
    let converts = plan.audio == AudioAction::ConvertToAac;
    let encodes = plan.video != VideoAction::Copy;
    let mut args = if converts {
        // Filter outputs are numbered across the command, after the video's.
        let label = if encodes { Burn::video_labels(burn) } else { 0 };
        strings(&[
            "-filter_complex",
            &format!(
                "[0:{}] aresample=async=1:ochl='stereo':rematrix_maxval=0.000000dB:osr=48000[{label}]",
                audio.index
            ),
            "-map",
            &format!("[{label}]"),
        ])
    } else {
        strings(&["-map", &format!("0:{}", audio.index)])
    };
    if let Some(tag) = &audio.language_code {
        args.extend(strings(&["-metadata:s:1", &format!("language={tag}")]));
    }
    if converts {
        args.extend(strings(&[
            "-codec:1",
            if encodes || native_encoder {
                "aac"
            } else {
                "aac_mf"
            },
            "-b:1",
            &format!("{}k", plan.audio_kbps()),
        ]));
    } else {
        args.extend(strings(&["-codec:1", "copy"]));
    }
    args
}

fn stream_arguments(
    request: &StreamRequest,
    run: &RunPlan<'_>,
    fonts: Option<&Path>,
) -> Vec<String> {
    let seconds = request.segment_seconds();
    let client = request.client_adapted();
    let burn = match (&request.subtitle, fonts) {
        (Some(subtitle @ SubtitleDelivery::Burn { track, .. }), _) if subtitle.is_image() => {
            Some(Burn::Image { track })
        }
        (Some(SubtitleDelivery::Burn { track, scale }), Some(fonts)) => Some(Burn::Text {
            track,
            scale: *scale,
            fonts,
        }),
        _ => None,
    };
    let mut args = input_arguments(request, run);
    args.extend(video_arguments(
        &request.video,
        run.plan.video,
        burn,
        request.h264_preset,
    ));
    args.extend(audio_arguments(&request.audio, run.plan, burn, client));
    if request.protocol == Protocol::Dash
        && run.plan.audio == AudioAction::Copy
        && request.audio.codec == "aac"
    {
        // AAC from MPEG-TS comes as ADTS, and the dash muxer does not insert this filter
        // itself: the init segment then lacks the AudioSpecificConfig and browsers refuse it
        // (Shaka 3014). AAC already in MP4 or Matroska passes through unchanged.
        args.extend(strings(&["-bsf:1", "aac_adtstoasc"]));
    }
    args.extend(match request.protocol {
        Protocol::Hls => hls_output(run.callback, seconds, run.first_file, client),
        Protocol::Dash => dash_output(run.callback, seconds, run.first_file, client),
    });
    if let Some(Burn::Text {
        track: subtitle, ..
    }) = burn
    {
        // The filter draws only what this output decodes alongside: without it, nothing.
        args.extend(strings(&[
            "-map",
            &format!("0:{}", subtitle.index),
            "-f",
            "null",
            "-codec",
            "ass",
            "nullfile",
        ]));
    }
    args
}

/// Moves a start onto the segment grid and says which segment that is, as the player numbers
/// them. A segment's number has to say where it is in time, so the process must start on one.
fn align_to_chunk(request: &mut StreamRequest) -> u32 {
    let seconds = f64::from(request.segment_seconds());
    let whole = (request.offset.unwrap_or(0.0).max(0.0) / seconds).floor();
    request.offset = Some(whole * seconds);
    // Far beyond any film's length, so the cast loses nothing.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "not negative, and a chunk count far below u32::MAX"
    )]
    let chunk = whole as u32;
    chunk
}

/// The number the file of a run's first segment carries: DASH numbers its chunks from one,
/// HLS its segments from zero. A run on the media's grid numbers its files from where they are
/// shown, but is told its first segment all the same: this build writes an encode's timestamps
/// from zero when told its first DASH segment is the first, wherever it starts.
const fn first_file(protocol: Protocol, chunk: u32) -> u32 {
    match protocol {
        Protocol::Dash => chunk + 1,
        Protocol::Hls => chunk,
    }
}

/// The form Plex's build expects its codec directory in: backslashes, colons and spaces escaped
/// once, and on Windows the extended-length prefix.
pub(crate) fn external_libs_value(directory: &Path) -> String {
    let slash = char::from(92);
    let double = format!("{slash}{slash}");
    let escaped = directory
        .display()
        .to_string()
        .replace(slash, &double)
        .replace(':', &format!("{slash}:"))
        .replace(' ', &format!("{slash} "));
    if cfg!(windows) {
        format!("{double}{double}?{double}{escaped}{double}")
    } else {
        format!("{escaped}/")
    }
}

/// What a wait for a segment finds: its file, or no file once it is known to lie past the end
/// of the media.
fn written_or_past_the_end(
    written: Option<PathBuf>,
    past_the_end: impl FnOnce() -> bool,
) -> Option<Vec<PathBuf>> {
    match written {
        Some(path) => Some(vec![path]),
        None => past_the_end().then(Vec::new),
    }
}

/// Watches a run's process until it exits or is told to stop, then marks the run ended.
fn supervise(session: &Arc<Session>, mut child: tokio::process::Child, generation: u64) {
    let stop = Arc::new(Notify::new());
    session.with_process(|process| {
        *process = Some(Process {
            generation,
            started: Instant::now(),
            stop: Arc::clone(&stop),
            task: None,
        });
    });

    let supervised = Arc::clone(session);
    let task = tokio::spawn(async move {
        let completed = tokio::select! {
            result = child.wait() => match result {
                Ok(status) => status.success(),
                Err(error) => {
                    tracing::warn!(%error, "lost track of a transcoder process");
                    false
                }
            },
            () = stop.notified() => {
                if let Err(error) = child.kill().await {
                    tracing::warn!(%error, "could not kill a transcoder process");
                }
                false
            }
        };
        supervised.with_progress(|progress| {
            if let Some(run) = progress
                .runs
                .iter_mut()
                .find(|run| run.generation == generation)
            {
                run.ended = true;
                run.completed = completed;
            }
        });
        supervised.changed.notify_waiters();
    });
    session.with_process(|process| {
        if let Some(process) = process
            .as_mut()
            .filter(|process| process.generation == generation)
        {
            process.task = Some(task);
        }
    });
}

/// Why a process is replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Restart {
    /// Another place in the same rung.
    Seek,
    /// A rung the player asked for.
    ClientSwitch,
    /// A rung the server chose.
    ServerSwitch,
}

fn remove_unowned(root: &Path, owned: &std::collections::HashSet<String>, settled_after: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if owned.contains(name.as_ref()) || subtitle::is_subtitle_directory(&name) {
            continue;
        }
        let settled = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= settled_after);
        // A process still running holds it: the next sweep tries again.
        if settled && std::fs::remove_dir_all(entry.path()).is_ok() {
            tracing::info!(directory = %entry.path().display(), "removed an orphaned transcode directory");
        }
    }
}

/// Marks a session taken out of the table closed, and tells its process to stop.
fn close(session: &Session) {
    session.closed.store(true, Ordering::Release);
    let stop =
        session.with_process(|process| process.as_ref().map(|process| Arc::clone(&process.stop)));
    if let Some(stop) = stop {
        stop.notify_one();
    }
}

/// The other sessions of the request's playback, which it replaces, unless one of them was
/// asked for after it: a player that started twice in a row keeps its second stream, whichever
/// reached here first.
fn others_of_playback(
    sessions: &HashMap<String, Arc<Session>>,
    request: &StreamRequest,
) -> Result<Vec<String>, TranscodeError> {
    let Some(playback) = request.playback.as_deref() else {
        return Ok(Vec::new());
    };
    let others: Vec<(&String, &Arc<Session>)> = sessions
        .iter()
        .filter(|(id, session)| {
            **id != request.session && session.request.playback.as_deref() == Some(playback)
        })
        .collect();
    if others
        .iter()
        .any(|(_, session)| session.request.asked_at > request.asked_at)
    {
        return Err(TranscodeError::Superseded);
    }
    Ok(others.into_iter().map(|(id, _)| id.clone()).collect())
}

impl Transcoder {
    fn session(&self, id: &str) -> Option<Arc<Session>> {
        let found = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        if let Some(session) = &found {
            session.touch();
        }
        found
    }

    /// Whether a session answers to this callback.
    #[must_use]
    pub fn has_callback(&self, callback: &str) -> bool {
        self.by_callback(callback).is_some()
    }

    fn by_callback(&self, callback: &str) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .find(|session| session.callback == callback)
            .cloned()
    }

    /// Removes what a previous run's sessions left on disk, which a session cleans up itself
    /// unless the server is killed before it can. The files' subtitles stay.
    pub fn remove_leftover_sessions(&self) {
        let entries = match std::fs::read_dir(&self.config.sessions_directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                tracing::warn!(%error, "could not remove old transcode sessions");
                return;
            }
        };
        // The files' subtitles outlive the server; a session's directory never does.
        for entry in entries.flatten() {
            if subtitle::is_subtitle_directory(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let removed = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                std::fs::remove_dir_all(entry.path())
            } else {
                std::fs::remove_file(entry.path())
            };
            if let Err(error) = removed {
                tracing::warn!(%error, path = %entry.path().display(), "could not remove an old transcode session");
            }
        }
    }

    /// Starts extracting every subtitle of the request's file that converts into the format
    /// named for it, the one it asked for first, and returns that one's extraction.
    async fn prepare_subtitles(&self, request: &StreamRequest) -> Option<Arc<Extraction>> {
        let chosen = match &request.subtitle {
            Some(SubtitleDelivery::Text { track, format }) => Some((track.clone(), *format)),
            _ => None,
        };
        let wanted: Vec<(Track, SubtitleFormat)> = chosen
            .iter()
            .cloned()
            .chain(
                request
                    .text_subtitles
                    .iter()
                    .filter(|(track, format)| format.holds(&track.codec))
                    .cloned(),
            )
            .collect();
        if wanted.is_empty() {
            return None;
        }
        let extractions = self
            .subtitles
            .prepare(&self.config, &request.source, &wanted)
            .await;
        chosen.and(extractions.into_iter().next())
    }

    /// Removes the directories no session owns. A process outlives a server killed outright,
    /// until its source stops answering, and holds its directory until then: the emptying at
    /// startup misses it, and this catches it once the process is gone.
    pub async fn remove_orphan_directories(&self) {
        let owned: std::collections::HashSet<String> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|session| format!("{SESSION_DIRECTORY}{}", session.callback))
            .collect();
        let root = self.config.sessions_directory.clone();
        let removal =
            tokio::task::spawn_blocking(move || remove_unowned(&root, &owned, ORPHAN_AGE)).await;
        if let Err(error) = removal {
            tracing::warn!(%error, "could not look for orphaned transcode directories");
        }
        self.subtitles.prune().await;
    }

    /// Starts a session, or does nothing if the client's is already running: a player asks
    /// for the same start more than once. Asking for something else replaces it. Returns the
    /// sessions of the same playback it replaced, which the caller may hold records of.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::TooManySessions`] at the limit, or if the process cannot be started.
    pub async fn start_stream(
        &self,
        mut request: StreamRequest,
    ) -> Result<Vec<String>, TranscodeError> {
        if let Some(subtitle) = &request.subtitle {
            if !subtitle.deliverable(self.config.fonts.is_some()) {
                return Err(TranscodeError::SubtitleCannotBeBurned);
            }
        }
        self.stop_idle_sessions();
        let mut replaced = self.stop_streams_of(&request)?;
        if let Some(running) = self.session(&request.session) {
            if running.request.produces_the_same_as(&request) {
                return Ok(replaced);
            }
            self.stop_session(&request.session);
        }
        if self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
            >= MAX_SESSIONS
        {
            return Err(TranscodeError::TooManySessions);
        }

        let callback = uuid::Uuid::new_v4().simple().to_string();
        let directory = self
            .config
            .sessions_directory
            .join(format!("{SESSION_DIRECTORY}{callback}"));
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(TranscodeError::Io)?;

        let chunk = align_to_chunk(&mut request);
        let server = (request.ladder.adaptation == Adaptation::Server).then(|| {
            ServerAdaptation::new(
                request
                    .ladder
                    .rungs
                    .iter()
                    .map(|rung| match rung.video {
                        VideoAction::Encode(encode) => encode.video_kbps + encode.audio_kbps,
                        VideoAction::Copy => 0,
                    })
                    .collect(),
            )
        });
        let key = request.session.clone();
        let subtitle = self.prepare_subtitles(&request).await;
        let session = Arc::new(Session {
            callback_base: format!("{}/transcoder", self.config.callback_base),
            callback,
            request,
            directory,
            progress: Mutex::default(),
            changed: Notify::new(),
            closed: AtomicBool::new(false),
            process: Mutex::new(None),
            restarting: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
            switching: Mutex::new(Switching {
                server,
                last_client_switch: None,
                served_rung: 0,
                last_video_chunk: None,
                sent: None,
            }),
            subtitle,
        });
        // Registered before the process exists: its first act is to read its source back.
        // Another start of the playback may have registered while this one prepared.
        let registered = {
            let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
            let others = others_of_playback(&sessions, &session.request);
            if let Ok(others) = &others {
                sessions.insert(key.clone(), Arc::clone(&session));
                for id in others {
                    if let Some(other) = sessions.remove(id) {
                        close(&other);
                    }
                }
            }
            others
        };
        // Refused, the session goes with its directory.
        replaced.extend(registered?);

        let offset = session.request.offset;
        if let Err(error) = self.launch(&session, 0, chunk, offset).await {
            self.sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&key);
            return Err(error);
        }
        Ok(replaced)
    }

    /// A player changing quality opens a new session and leaves the old one for us to close.
    fn stop_streams_of(&self, request: &StreamRequest) -> Result<Vec<String>, TranscodeError> {
        let replaced = others_of_playback(
            &self.sessions.lock().unwrap_or_else(PoisonError::into_inner),
            request,
        )?;
        for id in &replaced {
            self.stop_session(id);
        }
        Ok(replaced)
    }

    /// Starts a process for this session at a rung and a segment, writing into a directory of
    /// its own. The session must have none running.
    async fn launch(
        &self,
        session: &Arc<Session>,
        rung: usize,
        chunk: u32,
        offset: Option<f64>,
    ) -> Result<(), TranscodeError> {
        let request = &session.request;
        let plan = request
            .ladder
            .rungs
            .get(rung)
            .ok_or(TranscodeError::UnknownSegment)?;
        let generation = session.with_progress(|progress| {
            progress
                .runs
                .last()
                .map_or(0, |previous| previous.generation + 1)
        });
        let directory = session.directory.join(format!("run-{generation}"));
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(TranscodeError::Io)?;
        let callback = format!(
            "{}/{}/{generation}",
            session.callback_base, session.callback
        );
        let run = RunPlan {
            plan,
            offset,
            first_file: first_file(request.protocol, chunk),
            callback: &callback,
        };
        let mut command = Command::new(&self.config.binary);
        command
            .args(stream_arguments(
                request,
                &run,
                self.config.fonts.as_deref(),
            ))
            .current_dir(&directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(codecs) = &self.config.external_libs {
            command.env("FFMPEG_EXTERNAL_LIBS", external_libs_value(codecs));
        }
        let child = command.spawn().map_err(TranscodeError::Spawn)?;
        tracing::debug!(rung, chunk, generation, "started a transcoder run");

        session.with_progress(|progress| {
            progress.runs.push(Run {
                generation,
                rung,
                rendition: request.rendition_of(rung),
                start_chunk: chunk,
                directory,
                segments: BTreeMap::new(),
                ended: false,
                completed: false,
            });
        });
        supervise(session, child, generation);
        Ok(())
    }

    /// Replaces the session's process with one at `rung` starting at segment `chunk`, unless
    /// another request did so since `seen` was the running run. What is already written stays,
    /// so going back over it costs nothing.
    async fn restart(
        &self,
        session: &Arc<Session>,
        seen: Option<u64>,
        rung: usize,
        chunk: u32,
        reason: Restart,
    ) -> Result<(), TranscodeError> {
        let _exclusive = session.restarting.lock().await;
        if session.closed.load(Ordering::Acquire) || session.current_generation() != seen {
            return Ok(());
        }
        let previous = session.with_process(|process| {
            process
                .as_mut()
                .map(|process| (Arc::clone(&process.stop), process.task.take()))
        });
        if let Some((stop, task)) = previous {
            stop.notify_one();
            if let Some(task) = task {
                if let Err(error) = task.await {
                    tracing::warn!(%error, "a transcoder's supervisor failed");
                }
            }
        }
        if reason == Restart::ClientSwitch {
            session.with_switching(|switching| {
                switching.last_client_switch = Some(Instant::now());
            });
        }
        tracing::debug!(?reason, rung, chunk, "restarting a transcoder");
        let offset = f64::from(chunk) * session.seconds();
        self.launch(session, rung, chunk, Some(offset)).await
    }

    /// Moves a server-adapted session to the rung its measurements chose, if that is not the
    /// one running. `requested` is the segment being asked for.
    async fn follow_server_choice(
        &self,
        session: &Arc<Session>,
        requested: u32,
    ) -> Result<(), TranscodeError> {
        let Some(wanted) = session
            .with_switching(|switching| switching.server.as_ref().map(ServerAdaptation::current))
        else {
            return Ok(());
        };
        let Some((_, running, _)) = session.current() else {
            return Ok(());
        };
        if wanted == running {
            return Ok(());
        }
        let seen = session.current_generation();
        let finished = match session.request.protocol {
            Protocol::Hls => {
                session.with_progress(|progress| progress.current().and_then(Run::latest_finished))
            }
            Protocol::Dash => (requested..u32::MAX)
                .take_while(|number| session.written_chunk(0, *number).is_some())
                .last(),
        };
        let at = adapt::server_restart_at(requested, finished, wanted < running);
        self.restart(session, seen, wanted, at, Restart::ServerSwitch)
            .await
    }

    /// Records a video segment sent whole, for a stream the server adapts. It is judged when
    /// the player asks for the next one.
    pub fn record_delivery(&self, id: &str, delivery: Delivery) {
        let Some(session) = self.session(id) else {
            return;
        };
        let began = Instant::now()
            .checked_sub(delivery.sending)
            .unwrap_or_else(Instant::now);
        session.with_switching(|switching| {
            if switching.server.is_some() {
                switching.sent = Some((delivery, began));
            }
        });
    }

    /// The player asks for another video segment: the one sent before it has arrived.
    fn judge_delivery(session: &Session) {
        let segment = Duration::from_secs(u64::from(session.request.segment_seconds()));
        let moved = session.with_switching(|switching| {
            let (delivery, began) = switching.sent.take()?;
            let delivery = Delivery {
                sending: adapt::delivery_time(delivery.sending, began.elapsed(), segment),
                ..delivery
            };
            tracing::debug!(?delivery, "judged a delivery");
            let moved = switching
                .server
                .as_mut()?
                .record(delivery, Instant::now())?;
            Some((moved, delivery))
        });
        if let Some((rung, delivery)) = moved {
            tracing::info!(rung, ?delivery, "the link moved a stream to another rung");
        }
    }

    /// Stops the sessions nobody has asked about for a while.
    pub fn stop_idle_sessions(&self) {
        for id in self.idle_sessions() {
            self.stop_session_if_idle(&id);
        }
    }

    /// The sessions nobody has asked about for the idle limit.
    #[must_use]
    pub fn idle_sessions(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, session)| session.idle_for() > IDLE_LIFETIME)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Stops the session if nobody has asked about it for the idle limit. Whether it did.
    pub fn stop_session_if_idle(&self, id: &str) -> bool {
        let session = {
            let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
            match sessions.get(id) {
                Some(session) if session.idle_for() > IDLE_LIFETIME => sessions.remove(id),
                _ => None,
            }
        };
        session.as_deref().map(close).is_some()
    }

    /// Whether the session existed. Its process is told to stop; its directory goes once the
    /// process has exited and nothing reads from it any more.
    pub fn stop_session(&self, id: &str) -> bool {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        session.as_deref().map(close).is_some()
    }

    /// Stops every session, and returns once their processes have exited.
    pub async fn stop_all_sessions(&self) {
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, session)| session)
            .collect();
        let mut supervisors = Vec::new();
        for session in &sessions {
            close(session);
            if let Some(task) = session.with_process(|process| process.as_mut()?.task.take()) {
                supervisors.push(task);
            }
        }
        for supervisor in supervisors {
            if let Err(error) = supervisor.await {
                tracing::warn!(%error, "a transcoder process's supervisor failed");
            }
        }
    }

    /// Whether the session exists, without counting as a sign of life.
    pub fn has_session(&self, id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(id)
    }

    /// Whether the session exists; keeps it alive if so.
    pub fn ping_session(&self, id: &str) -> bool {
        self.session(id).is_some()
    }

    /// The segments of a rendition finished so far, once there are enough to start on. A
    /// rendition the process is not writing has nothing to wait for.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`].
    pub async fn session_output(
        &self,
        id: &str,
        rendition: usize,
    ) -> Result<Output, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        if session
            .current()
            .is_some_and(|(running, _, _)| running == rendition)
        {
            session
                .wait_until(|progress| {
                    progress.current().is_some_and(|run| {
                        run.segments
                            .values()
                            .filter(|segment| segment.finished)
                            .count()
                            >= SEGMENTS_BEFORE_PLAYLIST
                    })
                })
                .await;
        }
        Ok(session.output(rendition))
    }

    /// A running session's decisions and live progress, or `None` if it is not known.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a status display: the magnitudes are small and exactness is not needed"
    )]
    pub fn session_status(&self, id: &str) -> Option<SessionStatus> {
        let session = self.session(id)?;
        let request = &session.request;
        let seconds = session.seconds();
        let served = session.with_switching(|switching| switching.served_rung);
        let plan = request
            .ladder
            .rungs
            .get(served)
            .unwrap_or(request.ladder.top());
        let (protocol, container) = match request.protocol {
            Protocol::Dash => ("dash", "mp4"),
            Protocol::Hls => ("hls", "mpegts"),
        };
        let (video_decision, video_codec, video_encode) = match plan.video {
            VideoAction::Copy => ("copy", request.video.codec.clone(), None),
            VideoAction::Encode(encode) => ("transcode", "h264".to_owned(), Some(encode)),
        };
        let (audio_decision, audio_codec, audio_channels, audio_kbps) = match plan.audio {
            AudioAction::Copy => ("copy", request.audio.codec.clone(), None, None),
            AudioAction::ConvertToAac => (
                "transcode",
                "aac".to_owned(),
                Some(2),
                Some(plan.audio_kbps()),
            ),
        };
        let (ended, frontier, started) = session.with_progress(|progress| {
            let run = progress.current();
            let frontier = run.map_or(0.0, |run| {
                let written = match request.protocol {
                    Protocol::Hls => run
                        .segments
                        .values()
                        .filter(|segment| segment.finished)
                        .map(|segment| segment.start + segment.seconds)
                        .fold(0.0, f64::max),
                    Protocol::Dash => {
                        let first = run.start_chunk + 1;
                        let count = (first..u32::MAX)
                            .take_while(|file| run.directory.join(chunk_name(0, *file)).exists())
                            .count();
                        f64::from(run.start_chunk) * seconds + count as f64 * seconds
                    }
                };
                written.max(f64::from(run.start_chunk) * seconds)
            });
            let started = run.map_or(0.0, |run| f64::from(run.start_chunk) * seconds);
            (run.is_none_or(|run| run.ended), frontier, started)
        });
        let total_seconds = request.total_ms as f64 / 1000.0;
        let elapsed = session
            .with_process(|process| {
                process
                    .as_ref()
                    .map(|process| process.started.elapsed().as_secs_f64())
            })
            .unwrap_or(0.0);
        let produced = (frontier - started).max(0.0);
        let speed = if elapsed > 0.0 {
            produced / elapsed
        } else {
            0.0
        };
        let progress = if total_seconds > 0.0 {
            (frontier / total_seconds * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let remaining = if speed > 0.0 {
            ((total_seconds - frontier) / speed).max(0.0) as i64
        } else {
            0
        };
        let round = |value: f64| (value * 10.0).round() / 10.0;
        Some(SessionStatus {
            protocol,
            container,
            source_video_codec: request.video.codec.clone(),
            source_audio_codec: request.audio.codec.clone(),
            video_decision,
            audio_decision,
            video_codec,
            audio_codec,
            audio_channels,
            video_encode,
            audio_kbps,
            duration_ms: request.total_ms,
            max_offset_seconds: frontier,
            min_offset_seconds: 0.0,
            progress: round(progress),
            speed: round(speed),
            remaining_seconds: remaining,
            complete: ended,
        })
    }

    /// A finished HLS segment's file, waiting for it if it is still being written. No file is a
    /// segment past the end of the media, to be answered empty as the reference does.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`], or [`TranscodeError::UnknownSegment`] if it never
    /// appears.
    pub async fn session_segment(
        &self,
        id: &str,
        rendition: usize,
        number: u32,
    ) -> Result<Vec<PathBuf>, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        if session.request.client_adapted() {
            return self
                .chosen_chunk(&session, None, rendition, number)
                .await
                .map(|file| file.into_iter().collect());
        }
        if rendition != 0 {
            return Err(TranscodeError::UnknownSegment);
        }
        if session.past_the_end(0, number) {
            return Ok(Vec::new());
        }
        Self::judge_delivery(&session);
        self.follow_server_choice(&session, number).await?;
        // Not written yet, and the running process is too far behind to reach it soon: restart
        // there, as a far forward seek needs. Without this the request only waits, then refuses a
        // segment the process was never going to produce. A process that has already ended is held
        // then refused rather than restarted, so a player does not drive an endless relaunch.
        if session.hls_written(number).is_none()
            && !session.ended()
            && !session.hls_will_reach(number)
        {
            let seen = session.current_generation();
            let rung = session.current().map_or(0, |(_, rung, _)| rung);
            self.restart(&session, seen, rung, number, Restart::Seek)
                .await?;
        }
        let files = session
            .wait_for(|| {
                written_or_past_the_end(session.hls_written(number), || {
                    session.past_the_end(0, number)
                })
            })
            .await
            .ok_or(TranscodeError::UnknownSegment)?;
        if !files.is_empty() {
            session.served(session.rung_answering(number), number);
        }
        Ok(files)
    }

    /// The manifest the process wrote for a DASH session, waiting for it: its top rung's, which
    /// alone may copy the video and so be the only one to know its codec.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`], or [`TranscodeError::UnknownSegment`] if the process
    /// ended without writing one.
    pub async fn session_manifest(&self, id: &str) -> Result<String, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        session
            .wait_until(|progress| progress.manifests.contains_key(&0))
            .await;
        session
            .with_progress(|progress| progress.manifests.get(&0).cloned())
            .ok_or(TranscodeError::UnknownSegment)
    }

    /// A DASH track's initialization segment, waiting for it.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`], or [`TranscodeError::UnknownSegment`] if it never
    /// appears.
    pub async fn dash_header(&self, id: &str, track: DashTrack) -> Result<PathBuf, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        let stream = track.stream();
        // Unlike a chunk it has no temporary name: it is whole once the first chunk is begun.
        let header = |session: &Session| {
            session.with_progress(|progress| {
                progress
                    .runs
                    .iter()
                    .rev()
                    .filter(|run| match track {
                        DashTrack::Video(rendition) => run.rendition == rendition,
                        DashTrack::Audio => true,
                    })
                    .find(|run| {
                        let first = first_file(Protocol::Dash, run.start_chunk);
                        // On the media's grid, a run started on the key frame before its first
                        // segment writes the end of the one before under its number first.
                        let earliest = if session.request.client_adapted() {
                            first.saturating_sub(1)
                        } else {
                            first
                        };
                        (earliest..=first).any(|file| {
                            let name = chunk_name(stream, file);
                            run.directory.join(&name).exists()
                                || run.directory.join(format!("{name}.tmp")).exists()
                        })
                    })
                    .map(|run| run.directory.join(init_name(stream)))
            })
        };
        if let DashTrack::Video(rendition) = track {
            if rendition >= session.request.ladder.rungs.len()
                || (rendition > 0 && !session.request.client_adapted())
            {
                return Err(TranscodeError::UnknownSegment);
            }
            let running = session.current().map(|(running, _, _)| running);
            if header(&session).is_none() && running != Some(rendition) {
                // A player moving to a rung asks for its header first, from where it is.
                let at = session
                    .with_switching(|switching| switching.last_video_chunk)
                    .map_or_else(
                        || session.current().map_or(0, |(_, _, start)| start),
                        |latest| latest + 1,
                    );
                self.switch_for_player(&session, rendition, at, Instant::now())
                    .await?;
            }
        }
        session
            .wait_for(|| header(&session))
            .await
            .ok_or(TranscodeError::UnknownSegment)
    }

    /// Restarts the process at a rung the player asked for, once the rules allow it.
    async fn switch_for_player(
        &self,
        session: &Arc<Session>,
        rendition: usize,
        chunk: u32,
        asked_at: Instant,
    ) -> Result<(), TranscodeError> {
        loop {
            let Some((running, _, _)) = session.current() else {
                return Ok(());
            };
            if running == rendition || session.closed.load(Ordering::Acquire) {
                return Ok(());
            }
            let last = session.with_switching(|switching| switching.last_client_switch);
            match adapt::client_switch_wait(Instant::now(), asked_at, last, rendition < running) {
                None => {
                    let seen = session.current_generation();
                    return self
                        .restart(session, seen, rendition, chunk, Restart::ClientSwitch)
                        .await;
                }
                Some(wait) => tokio::time::sleep(wait.min(FILE_POLL)).await,
            }
        }
    }

    /// A session's text subtitle, extracted whole in the format chosen at its start, once its
    /// extraction is done; waiting for the session to start too: Plex Web asks for its
    /// subtitles right after the decision, before it starts the stream, and the reference holds
    /// that request until it does.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`] if none starts in time,
    /// [`TranscodeError::UnknownSegment`] for a session with no text subtitle, and what
    /// extracting it meets.
    pub async fn subtitles(&self, id: &str) -> Result<(Vec<u8>, SubtitleFormat), TranscodeError> {
        let began = Instant::now();
        let session = loop {
            if let Some(session) = self.session(id) {
                break session;
            }
            if began.elapsed() >= WAIT_FOR_OUTPUT {
                return Err(TranscodeError::UnknownSession);
            }
            tokio::time::sleep(FILE_POLL).await;
        };
        let (Some(extraction), Some(SubtitleDelivery::Text { format, .. })) =
            (&session.subtitle, &session.request.subtitle)
        else {
            return Err(TranscodeError::UnknownSegment);
        };
        Ok((extraction.bytes().await?, *format))
    }

    /// The subtitle track at `position` of a session's file, whole, in the format it was
    /// prepared in when the session started: what a player switches to without starting anew.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`] for no such session,
    /// [`TranscodeError::UnknownSegment`] for a track it did not prepare, and what extracting it
    /// meets.
    pub async fn subtitle_file(
        &self,
        id: &str,
        position: i32,
    ) -> Result<(Vec<u8>, SubtitleFormat), TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        let Some((track, format)) = session
            .request
            .text_subtitles
            .iter()
            .find(|(track, format)| track.index == position && format.holds(&track.codec))
        else {
            return Err(TranscodeError::UnknownSegment);
        };
        // Started with the session, so this finds it on disk or under way.
        let extraction = self
            .subtitles
            .prepare(
                &self.config,
                &session.request.source,
                &[(track.clone(), *format)],
            )
            .await
            .into_iter()
            .next()
            .ok_or(TranscodeError::UnknownSegment)?;
        Ok((extraction.bytes().await?, *format))
    }

    /// A finished DASH chunk's file. `number` counts from zero, as the manifest given to the
    /// player does. One the running process will not get to soon is a seek, and restarts it
    /// there. No file is a chunk past the end of the media, to be answered empty.
    ///
    /// # Errors
    ///
    /// As [`Self::dash_header`].
    pub async fn dash_chunk(
        &self,
        id: &str,
        track: DashTrack,
        number: u32,
    ) -> Result<Vec<PathBuf>, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        if session.request.client_adapted() {
            let rendition = match track {
                DashTrack::Video(rendition) => rendition,
                DashTrack::Audio => session.current().map_or(0, |(running, _, _)| running),
            };
            return self
                .chosen_chunk(&session, Some(track), rendition, number)
                .await
                .map(|file| file.into_iter().collect());
        }
        if matches!(track, DashTrack::Video(rendition) if rendition != 0) {
            return Err(TranscodeError::UnknownSegment);
        }
        let stream = track.stream();
        if session.past_the_end(stream, number) {
            return Ok(Vec::new());
        }
        if stream == 0 {
            Self::judge_delivery(&session);
        }
        self.follow_server_choice(&session, number).await?;
        if session.written_chunk(stream, number).is_none() && !session.will_reach(stream, number) {
            let seen = session.current_generation();
            let rung = session.current().map_or(0, |(_, rung, _)| rung);
            self.restart(&session, seen, rung, number, Restart::Seek)
                .await?;
        }
        let files = session
            .wait_for(|| {
                written_or_past_the_end(session.written_chunk(stream, number), || {
                    session.past_the_end(stream, number)
                })
            })
            .await
            .ok_or(TranscodeError::UnknownSegment)?;
        if stream == 0 && !files.is_empty() {
            session.served(session.rung_answering(number), number);
        }
        Ok(files)
    }

    /// A segment of a stream whose player chooses the rung, `track` `None` for HLS. Served from
    /// whatever run wrote it; otherwise the running process is left to reach it, moved there
    /// if it is too far, or replaced by one at the rung asked for, once the rules allow. `None`
    /// past the end of the media.
    async fn chosen_chunk(
        &self,
        session: &Arc<Session>,
        track: Option<DashTrack>,
        rendition: usize,
        number: u32,
    ) -> Result<Option<PathBuf>, TranscodeError> {
        if rendition >= session.request.ladder.rungs.len() {
            return Err(TranscodeError::UnknownSegment);
        }
        let video = !matches!(track, Some(DashTrack::Audio));
        let asked_at = Instant::now();
        let began = tokio::time::Instant::now();
        loop {
            if session.closed.load(Ordering::Acquire) {
                return Err(TranscodeError::UnknownSegment);
            }
            if let Some(file) = session.chosen_file(track, rendition, number) {
                if video {
                    session.served(rendition, number);
                }
                return Ok(Some(file));
            }
            if session.past_the_end(track.map_or(0, DashTrack::stream), number) {
                return Ok(None);
            }
            let seen = session.current_generation();
            let Some((running, _, start)) = session.current() else {
                return Err(TranscodeError::UnknownSegment);
            };
            let ended = session.ended();
            // The audio is the same on every rung: whichever runs writes it.
            let rendition = if video { rendition } else { running };
            if running == rendition {
                // A run that started at or before the segment and ended without it has reached
                // the end of the media, or failed: restarting it would only fail again.
                let exhausted = ended && start <= number;
                if !exhausted && !session.chosen_will_reach(track, rendition, number) {
                    self.restart(session, seen, rendition, number, Restart::Seek)
                        .await?;
                    continue;
                }
            } else {
                let last = session.with_switching(|switching| switching.last_client_switch);
                let upgrade = rendition < running;
                if adapt::client_switch_wait(Instant::now(), asked_at, last, upgrade).is_none() {
                    self.restart(session, seen, rendition, number, Restart::ClientSwitch)
                        .await?;
                    continue;
                }
            }
            let limit = if ended {
                HOLD_BEFORE_REFUSAL
            } else {
                WAIT_FOR_OUTPUT
            };
            if began.elapsed() >= limit {
                return Err(TranscodeError::UnknownSegment);
            }
            tokio::time::sleep(FILE_POLL).await;
        }
    }

    /// The source of the session behind a callback URL, for the process to read.
    #[must_use]
    pub fn callback_source(&self, callback: &str) -> Option<PathBuf> {
        Some(self.by_callback(callback)?.request.source.clone())
    }

    /// Records the process's announcement of its segments. `run` is the generation its URL
    /// names: an announcement from a process since replaced must not count for the new one.
    /// Whether the callback is known.
    #[must_use]
    pub fn announce_segments(&self, callback: &str, run: &str, body: &str) -> bool {
        let Some(session) = self.by_callback(callback) else {
            return false;
        };
        let Ok(generation) = run.parse::<u64>() else {
            return true;
        };
        session.with_progress(|progress| {
            let Some(run) = progress
                .runs
                .iter_mut()
                .find(|run| run.generation == generation)
            else {
                return;
            };
            let rendition = run.rendition;
            run.segments.extend(parse_announcement(body));
            if body.contains("<Representation") {
                progress.manifests.insert(rendition, body.to_owned());
            }
        });
        session.changed.notify_waiters();
        true
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// From the reference's log: a file with H.264 at 0 and DTS at 1, video copied and audio
    /// converted. Only the URLs and the length of the segment list differ.
    const CAPTURED: &str = "-codec:0 h264 -codec:1 dca -noaccurate_seek -analyzeduration 20000000 \
-probesize 20000000 -i CB/source -start_at_zero -copyts -fps_mode cfr -y -nostats -loglevel quiet \
-loglevel_plex error -progressurl CB/progress -map 0:0 -metadata:s:0 language=eng -codec:0 copy \
-filter_complex [0:1]_aresample=async=1:ochl='stereo':rematrix_maxval=0.000000dB:osr=48000[0] \
-map [0] -metadata:s:1 language=fra -codec:1 aac_mf -b:1 256k -segment_format mpegts -f ssegment \
-individual_header_trailer 0 -segment_time 10 -segment_start_number 0 -segment_time_delta 0.0625 \
-segment_list CB/manifest?X-Plex-Http-Pipeline=infinite -segment_list_type csv -segment_list_size 100000 \
-segment_list_separate_stream_times 1 -segment_list_unfinished 1 -max_delay 5000000 \
-avoid_negative_ts disabled -map_metadata -1 -map_chapters -1 media-%05d.ts";

    fn track(index: i32, codec: &str, language: &str) -> Track {
        Track {
            teletext_page: None,
            index,
            codec: codec.to_owned(),
            language: Some(language.to_owned()),
            language_code: Some(
                match language {
                    "fr" => "fra",
                    "en" => "eng",
                    other => other,
                }
                .to_owned(),
            ),
        }
    }

    fn fixed(plan: StreamPlan) -> Ladder {
        Ladder {
            adaptation: Adaptation::Fixed,
            rungs: vec![plan],
        }
    }

    fn request(audio: AudioAction) -> StreamRequest {
        StreamRequest {
            session: "s".to_owned(),
            playback: None,
            asked_at: Instant::now(),
            source: PathBuf::from("x"),
            video: track(0, "h264", "en"),
            audio: track(1, "dca", "fr"),
            ladder: fixed(StreamPlan {
                video: VideoAction::Copy,
                audio,
            }),
            offset: None,
            protocol: Protocol::Hls,
            total_ms: 0,
            subtitle: None,
            text_subtitles: Vec::new(),
            h264_preset: H264Preset::default(),
        }
    }

    /// The command of a run of the request's top rung, from where it asks to start, without
    /// the read rate the reference's commands lack.
    fn arguments(request: &StreamRequest, first_file: u32, fonts: Option<&Path>) -> Vec<String> {
        without_read_rate(stream_arguments(
            request,
            &RunPlan {
                plan: request.ladder.top(),
                offset: request.offset,
                first_file,
                callback: "CB",
            },
            fonts,
        ))
    }

    fn without_read_rate(mut args: Vec<String>) -> Vec<String> {
        let at = args.iter().position(|arg| arg == "-readrate").unwrap();
        args.drain(at..at + 4);
        args
    }

    #[test]
    fn reads_the_source_at_twice_real_time_after_a_burst() {
        let args = stream_arguments(
            &request(AudioAction::Copy),
            &RunPlan {
                plan: request(AudioAction::Copy).ladder.top(),
                offset: Some(90.0),
                first_file: 1,
                callback: "CB",
            },
            None,
        )
        .join(" ");
        assert!(args.contains("-readrate 2 -readrate_initial_burst 30 -analyzeduration"));
        assert!(args.find("-readrate") < args.find("-i CB/source"));
    }

    #[test]
    fn writes_the_references_command_line() {
        let args = arguments(&request(AudioAction::ConvertToAac), 0, None);
        // The space inside the filter is what makes it one argument here and several there.
        let joined = args.join(" ").replace("[0:1] aresample", "[0:1]_aresample");
        assert_eq!(joined, CAPTURED);
    }

    /// From the reference's log, for Plex Web in Chrome: H.264 at 0, E-AC-3 at 2.
    const CAPTURED_DASH: &str = "-codec:0 h264 -codec:2 eac3_mf -ss 0 -noaccurate_seek \
-analyzeduration 20000000 -probesize 20000000 -i CB/source -start_at_zero -copyts -fps_mode cfr -y \
-nostats -loglevel quiet -loglevel_plex error -progressurl CB/progress -map 0:0 \
-metadata:s:0 language=eng -codec:0 copy \
-filter_complex [0:2]_aresample=async=1:ochl='stereo':rematrix_maxval=0.000000dB:osr=48000[0] \
-map [0] -metadata:s:1 language=fra -codec:1 aac_mf -b:1 256k -f dash -seg_duration 5 \
-dash_segment_type mp4 -init_seg_name init-stream$RepresentationID$.m4s \
-media_seg_name chunk-stream$RepresentationID$-$Number%05d$.m4s -window_size 5 -delete_removed false \
-skip_to_segment 1 -manifest_name CB/manifest?X-Plex-Http-Pipeline=infinite -avoid_negative_ts disabled \
-map_metadata -1 -map_chapters -1 dash";

    #[test]
    fn writes_the_references_dash_command_line() {
        let mut asked = request(AudioAction::ConvertToAac);
        asked.protocol = Protocol::Dash;
        asked.audio = track(2, "eac3", "fr");
        let joined = arguments(&asked, 1, None)
            .join(" ")
            .replace("[0:2] aresample", "[0:2]_aresample");
        assert_eq!(joined, CAPTURED_DASH);
    }

    #[test]
    fn copied_aac_leaves_adts_behind_only_for_mp4_segments() {
        let filter = |protocol, audio| {
            let mut asked = request(audio);
            asked.protocol = protocol;
            asked.audio = track(2, "aac", "fr");
            let args = arguments(&asked, 1, None);
            args.windows(2)
                .any(|pair| pair[0] == "-bsf:1" && pair[1] == "aac_adtstoasc")
        };
        assert!(filter(Protocol::Dash, AudioAction::Copy));
        assert!(!filter(Protocol::Hls, AudioAction::Copy));
        assert!(!filter(Protocol::Dash, AudioAction::ConvertToAac));
    }

    /// From the reference's log: film 513702 limited to 2 Mbps, H.264 at 0 and E-AC-3 at 1.
    const CAPTURED_ENCODE: &str = "-codec:0 h264 -codec:1 eac3_mf -analyzeduration 20000000 \
-probesize 20000000 -i CB/source -start_at_zero -copyts -fps_mode cfr -y -nostats -loglevel quiet \
-loglevel_plex error -progressurl CB/progress \
-filter_complex [0:0]scale=w=720:h=388:force_divisible_by=4[0];[0]format=pix_fmts=yuv420p|nv12[1] \
-map [1] -metadata:s:0 language=eng -codec:0 libx264 -crf:0 20 -maxrate:0 1333k -bufsize:0 2666k \
-r:0 25 -preset:0 veryfast -x264opts:0 subme=2:me_range=4:rc_lookahead=20:me=hex \
-force_key_frames:0 expr:gte(t,n_forced*8) \
-filter_complex [0:1]_aresample=async=1:ochl='stereo':rematrix_maxval=0.000000dB:osr=48000[2] \
-map [2] -metadata:s:1 language=fra -codec:1 aac -b:1 167k -f dash -seg_duration 8 \
-dash_segment_type mp4 -init_seg_name init-stream$RepresentationID$.m4s \
-media_seg_name chunk-stream$RepresentationID$-$Number%05d$.m4s -window_size 5 -delete_removed false \
-skip_to_segment 1 -manifest_name CB/manifest?X-Plex-Http-Pipeline=infinite -avoid_negative_ts disabled \
-map_metadata -1 -map_chapters -1 dash";

    fn encode(width: u32, height: u32, video_kbps: u32, audio_kbps: u32, crf: u32) -> VideoEncode {
        VideoEncode {
            width,
            height,
            video_kbps,
            audio_kbps,
            crf,
            frame_rate: Some(25.0),
            key_frames: KeyFrames::Every(match width {
                0..=720 => 8,
                721..=1280 => 5,
                _ => 1,
            }),
            shared_headers: false,
        }
    }

    fn encoded(
        width: u32,
        height: u32,
        video_kbps: u32,
        audio_kbps: u32,
        crf: u32,
    ) -> StreamRequest {
        let mut asked = request(AudioAction::ConvertToAac);
        asked.protocol = Protocol::Dash;
        asked.audio = track(1, "eac3", "fr");
        asked.offset = Some(0.0);
        asked.ladder = fixed(StreamPlan {
            video: VideoAction::Encode(encode(width, height, video_kbps, audio_kbps, crf)),
            audio: AudioAction::ConvertToAac,
        });
        asked
    }

    /// The filter of the reference's command burning Soul's French subtitles at 100%.
    const CAPTURED_BURN_FILTER: &str = r"[0:5]scale=w=1920:h=1080:force_divisible_by=4[0];[0]format=pix_fmts=yuv420p|nv12[1];[1]inlineass=font_scale=1.000000:font_path=C\\\:\\\\Program\\\ Files\\\\Plex\\\\Plex\\\ Media\\\ Server\\\\Resources\\\\Fonts\\\\NotoSans-Medium.otf:fonts_dir=\\\\\\\\?\\\\C\\\:\\\\Program\\\ Files\\\\Plex\\\\Plex\\\ Media\\\ Server\\\\Resources\\\\Fonts:language=fr:overrides=ScaledBorderAndShadow\\\=yes\\\,FontName\\\=Noto\\\ Sans\\\ Medium\\\,Bold\\\=500\\\,PrimaryColour\\\=&H00FFFFFF\\\,OutlineColour\\\=&H00020713\\\,BackColour\\\=&HCC000000:outline=2.6:shadow=1.7:font_size=54[2]";

    fn burned(scale: f64) -> StreamRequest {
        let mut asked = encoded(1920, 1080, 20_000, 256, 16);
        asked.video = track(5, "h264", "en");
        asked.audio = track(0, "eac3", "fr");
        asked.offset = None;
        asked.subtitle = Some(SubtitleDelivery::Burn {
            track: track(3, "srt", "fr"),
            scale,
        });
        asked
    }

    #[cfg(windows)]
    #[test]
    fn burns_a_subtitle_with_the_references_filter() {
        let fonts = Path::new(r"\\?\C:\Program Files\Plex\Plex Media Server\Resources\Fonts");
        let args = arguments(&burned(1.0), 1, Some(fonts));
        let at = |flag: &str| args.iter().position(|arg| arg == flag).unwrap();
        assert_eq!(args[at("-map_inlineass") + 1], "0:3");
        // The burn's filter comes right after the subtitle's mapping, and the picture it
        // draws is what the output maps.
        assert_eq!(args[at("-map_inlineass") + 3], CAPTURED_BURN_FILTER);
        assert_eq!(args[at("-map_inlineass") + 5], "[2]");
        let joined = args.join(" ");
        assert!(joined.contains("osr=48000[3] -map [3]"), "{joined}");
        assert!(
            joined.ends_with("dash -map 0:3 -f null -codec ass nullfile"),
            "{joined}"
        );
    }

    #[test]
    fn lays_an_image_subtitle_over_the_picture_without_fonts() {
        let mut asked = burned(1.0);
        asked.subtitle = Some(SubtitleDelivery::Burn {
            track: track(3, "pgs", "fr"),
            scale: 1.0,
        });
        let args = arguments(&asked, 1, None);
        let at = |flag: &str| args.iter().position(|arg| arg == flag).unwrap();
        assert_eq!(
            args[at("-filter_complex") + 1],
            "[0:5]scale=w=1920:h=1080:force_divisible_by=4[0];[0]format=pix_fmts=yuv420p|nv12[1];\
             [0:3]scale=1920:1080[2];[1][2]overlay[3]"
        );
        assert_eq!(args[at("-filter_complex") + 3], "[3]");
        let joined = args.join(" ");
        assert!(joined.contains("osr=48000[4] -map [4]"), "{joined}");
        // The filter reads the track itself: no libass, no output decoding it alongside.
        assert!(!joined.contains("inlineass"), "{joined}");
        assert!(!joined.contains("nullfile"), "{joined}");
    }

    #[test]
    fn which_subtitles_can_be_delivered() {
        let burn = |codec| SubtitleDelivery::Burn {
            track: track(3, codec, "fr"),
            scale: 1.0,
        };
        let text = |codec| SubtitleDelivery::Text {
            track: track(3, codec, "fr"),
            format: SubtitleFormat::Ass,
        };
        assert!(burn("pgs").deliverable(false));
        assert!(burn("dvb_subtitle").deliverable(false));
        assert!(burn("srt").deliverable(true));
        assert!(!burn("srt").deliverable(false));
        assert!(!burn("dvb_teletext").deliverable(true));
        assert!(text("ass").deliverable(false));
        assert!(!text("pgs").deliverable(true));
    }

    #[test]
    fn a_subtitle_scales_with_the_clients_size() {
        let args = arguments(&burned(1.5), 1, Some(Path::new("fonts")));
        assert!(args
            .iter()
            .any(|arg| arg.contains("inlineass=font_scale=1.500000:")));
        // Without fonts there is nothing to draw with, and the command is an encode's.
        let plain = arguments(&burned(1.0), 1, None).join(" ");
        assert!(!plain.contains("inlineass"), "{plain}");
    }

    #[test]
    fn escapes_a_filter_value_at_both_levels() {
        assert_eq!(filter_value(r"C:\a b"), r"C\\\:\\\\a\\\ b");
        assert_eq!(filter_value("A=1,B"), r"A\\\=1\\\,B");
        assert_eq!(filter_value("&H00-x.otf"), "&H00-x.otf");
    }

    #[test]
    fn writes_the_references_encoding_command_line() {
        let joined = arguments(&encoded(720, 388, 1333, 167, 20), 1, None)
            .join(" ")
            .replace("[0:1] aresample", "[0:1]_aresample");
        assert_eq!(joined, CAPTURED_ENCODE);
    }

    #[test]
    fn an_encode_takes_the_preset_asked_for() {
        let mut asked = encoded(720, 388, 1333, 167, 20);
        asked.h264_preset = H264Preset::Slow;
        let joined = arguments(&asked, 1, None).join(" ");
        assert!(joined.contains(" -preset:0 slow -x264opts:0 "));
        assert!(!joined.contains("veryfast"));
    }

    #[test]
    fn every_preset_is_named_as_x264_names_it() {
        for preset in H264Preset::ALL {
            assert_eq!(H264Preset::named(preset.name()), Some(preset));
        }
        assert_eq!(H264Preset::named("placebo"), None);
        assert_eq!(H264Preset::default().name(), "veryfast");
    }

    /// From the reference's log, started at 600 seconds: an exact seek, and no `-fps_mode`.
    #[test]
    fn an_encode_seeks_as_the_reference_does() {
        let mut asked = encoded(1280, 692, 2566, 98, 24);
        asked.offset = Some(600.0);
        let joined = arguments(&asked, 121, None).join(" ");
        assert!(joined.starts_with("-codec:0 h264 -codec:1 eac3_mf -ss 600 -analyzeduration"));
        assert!(joined.contains("-copyts -y -nostats"));
        assert!(joined.contains("-force_key_frames:0 expr:gte(t,n_forced*5) "));
        assert!(joined.contains("-seg_duration 5 "));
        assert!(joined.contains("-skip_to_segment 121 "));
        assert!(joined.contains(
            "-x264opts:0 subme=1:me_range=4:rc_lookahead=10:me=hex:8x8dct=0:partitions=none "
        ));
    }

    #[test]
    fn a_change_of_quality_is_not_the_same_stream() {
        let copied = request(AudioAction::ConvertToAac);
        let mut resumed = copied.clone();
        resumed.offset = Some(90.0);
        assert!(copied.produces_the_same_as(&resumed));

        let lowered = encoded(720, 388, 1333, 167, 20);
        assert!(!copied.produces_the_same_as(&lowered));
    }

    #[test]
    fn an_encode_lands_on_its_own_chunk_grid() {
        let mut asked = encoded(720, 388, 1333, 167, 20);
        asked.offset = Some(603.0);
        assert_eq!(align_to_chunk(&mut asked), 75);
        assert_eq!(asked.offset, Some(600.0));
        assert_eq!(first_file(Protocol::Dash, 75), 76);
    }

    /// Measured on the transcoder itself: chunk 121 of a 5-second grid starts at 600 seconds.
    #[test]
    fn a_seek_names_its_time_and_its_chunk_together() {
        let mut asked = request(AudioAction::Copy);
        asked.protocol = Protocol::Dash;
        asked.offset = Some(600.0);
        let joined = arguments(&asked, 121, None).join(" ");
        assert!(joined.contains("-ss 600 -noaccurate_seek"));
        assert!(joined.contains("-skip_to_segment 121 "));
    }

    #[test]
    fn a_dash_start_lands_on_the_chunk_grid() {
        let mut asked = request(AudioAction::Copy);
        asked.protocol = Protocol::Dash;
        asked.offset = Some(603.0);
        assert_eq!(align_to_chunk(&mut asked), 120);
        assert_eq!(asked.offset, Some(600.0));
        assert_eq!(first_file(Protocol::Dash, 120), 121);

        asked.offset = None;
        assert_eq!(align_to_chunk(&mut asked), 0);
        assert_eq!(asked.offset, Some(0.0));
        assert_eq!(first_file(Protocol::Dash, 0), 1);
    }

    #[test]
    fn an_hls_start_lands_on_the_chunk_grid() {
        let mut asked = request(AudioAction::Copy);
        asked.offset = Some(603.0);
        assert_eq!(align_to_chunk(&mut asked), 60);
        assert_eq!(asked.offset, Some(600.0));
        assert_eq!(first_file(Protocol::Hls, 60), 60);

        asked.offset = None;
        assert_eq!(align_to_chunk(&mut asked), 0);
        assert_eq!(asked.offset, Some(0.0));
    }

    /// The reference's log shows an HLS seek at 600s on an 8-second grid as
    /// `-ss 600 … -segment_start_number 75` (75 = 600 / 8): `-ss N*seconds`, output numbered
    /// from the target segment N.
    #[test]
    fn an_hls_seek_numbers_its_segments_from_the_target() {
        let mut asked = request(AudioAction::Copy);
        asked.offset = Some(3000.0);
        let joined = arguments(&asked, 300, None).join(" ");
        assert!(joined.contains("-ss 3000 -noaccurate_seek"));
        assert!(joined.contains("-segment_start_number 300 "));
    }

    #[test]
    fn copied_audio_is_neither_decoded_nor_filtered() {
        let joined = arguments(&request(AudioAction::Copy), 1, None).join(" ");
        assert!(!joined.contains("-codec:1 dca"));
        assert!(!joined.contains("aresample"));
        assert!(joined.contains("-map 0:1 -metadata:s:1 language=fra -codec:1 copy"));
    }

    #[test]
    fn follows_the_tracks_where_the_file_puts_them() {
        let mut asked = request(AudioAction::Copy);
        asked.video.index = 5;
        asked.audio.index = 0;
        let joined = arguments(&asked, 1, None).join(" ");
        assert!(joined.starts_with("-codec:5 h264"));
        assert!(joined.contains("-map 0:5 "));
        assert!(joined.contains("-map 0:0 "));
    }

    #[test]
    fn decodes_the_video_with_its_own_codec() {
        for (codec, decoder) in [
            ("hevc", "hevc"),
            ("mpeg2video", "mpeg2video"),
            ("av1", "libdav1d"),
        ] {
            let mut asked = encoded(1920, 1080, 20_000, 256, 16);
            asked.video = track(0, codec, "en");
            let joined = arguments(&asked, 1, None).join(" ");
            assert!(
                joined.starts_with(&format!("-codec:0 {decoder} ")),
                "{joined}"
            );
            assert!(joined.contains("libx264"), "{joined}");
        }
    }

    #[test]
    fn leaves_the_transcoder_to_pick_a_decoder_it_has_no_name_for() {
        let mut asked = encoded(1920, 1080, 20_000, 256, 16);
        asked.video = track(0, "encv", "en");
        asked.audio = track(1, "pcm", "fr");
        let joined = arguments(&asked, 1, None).join(" ");
        assert!(!joined.contains("-codec:0 encv"), "{joined}");
        assert!(!joined.contains("-codec:1 pcm"), "{joined}");
        assert!(joined.contains("-codec:1 aac"), "{joined}");
    }

    #[test]
    fn seeks_before_opening_the_input() {
        let mut asked = request(AudioAction::Copy);
        asked.offset = Some(90.0);
        let joined = arguments(&asked, 1, None).join(" ");
        assert!(joined.contains("-ss 90 -noaccurate_seek -analyzeduration"));
    }

    /// A ladder the player chooses from: the copy, then an encode cut where the copy is.
    fn chosen(protocol: Protocol) -> StreamRequest {
        let mut asked = request(AudioAction::ConvertToAac);
        asked.protocol = protocol;
        asked.offset = Some(0.0);
        asked.ladder = Ladder {
            adaptation: Adaptation::Client,
            rungs: vec![
                StreamPlan {
                    video: VideoAction::Copy,
                    audio: AudioAction::ConvertToAac,
                },
                StreamPlan {
                    video: VideoAction::Encode(VideoEncode {
                        key_frames: KeyFrames::AsSource,
                        ..encode(480, 268, 300, 256, 23)
                    }),
                    audio: AudioAction::ConvertToAac,
                },
            ],
        };
        asked
    }

    fn run_of(request: &StreamRequest, rung: usize, offset: f64) -> String {
        let args = stream_arguments(
            request,
            &RunPlan {
                plan: &request.ladder.rungs[rung],
                offset: Some(offset),
                first_file: first_file(request.protocol, 7),
                callback: "CB",
            },
            None,
        );
        without_read_rate(args).join(" ")
    }

    #[test]
    fn rungs_a_player_chooses_from_are_cut_at_every_key_frame_of_the_source() {
        let asked = chosen(Protocol::Dash);
        let copy = run_of(&asked, 0, 35.0);
        let encode = run_of(&asked, 1, 35.0);
        for run in [&copy, &encode] {
            // Both start on the key frame before, where the copy has to.
            assert!(
                run.contains("-ss 35 -noaccurate_seek -analyzeduration"),
                "{run}"
            );
            assert!(
                run.contains("-f dash -seg_duration 5 -absolute_segments 1 "),
                "{run}"
            );
            // Not 1, which would make this build write an encode's timestamps from zero.
            assert!(run.contains("-skip_to_segment 8 "), "{run}");
            // One audio encoder for every rung, or the audio would shift between them.
            assert!(run.contains("-codec:1 aac -b:1 256k"), "{run}");
        }
        assert!(encode.contains(":scenecut=0:keyint=infinite -force_key_frames:0 source "));
        let hls = run_of(&chosen(Protocol::Hls), 1, 35.0);
        assert!(
            hls.contains("-segment_time 5 -segment_start_number 7 "),
            "{hls}"
        );
        assert!(hls.contains("-segment_absolute 1 "), "{hls}");
        // Asked of rungs the player chooses only: a build without the option refuses it.
        let fixed = arguments(&request(AudioAction::Copy), 0, None).join(" ");
        assert!(!fixed.contains("absolute"), "{fixed}");
    }

    #[test]
    fn rungs_the_server_moves_between_share_their_headers() {
        let mut asked = encoded(1920, 1080, 3419, 256, 23);
        asked.ladder.adaptation = Adaptation::Server;
        asked.ladder.rungs[0].video = VideoAction::Encode(VideoEncode {
            key_frames: KeyFrames::Every(3),
            shared_headers: true,
            ..encode(1920, 1080, 3419, 256, 23)
        });
        let joined = arguments(&asked, 41, None).join(" ");
        assert!(
            joined.contains(":partitions=none:stitchable=1 -level:0 4.2 -force_key_frames:0 expr:gte(t,n_forced*3) "),
            "{joined}"
        );
        assert!(joined.contains("-seg_duration 3 "), "{joined}");
    }

    #[test]
    fn reads_an_announcement() {
        // Real output of the transcoder.
        let parsed = parse_announcement(
            "media-00000.ts,0.000000,8.000000,7.914667,8.000000\n\
             #media-00001.ts,8.000000,8.000000,8.000000,8.000000",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, 0);
        assert!(parsed[0].1.finished);
        assert!((parsed[0].1.seconds - 8.0).abs() < f64::EPSILON);
        assert_eq!(parsed[1].0, 1);
        assert!((parsed[1].1.start - 8.0).abs() < f64::EPSILON);
        assert!(!parsed[1].1.finished);
    }

    #[test]
    fn ignores_what_is_not_a_segment_line() {
        assert!(parse_announcement("").is_empty());
        assert!(parse_announcement("garbage,1,2").is_empty());
        assert!(parse_announcement("other-00000.ts,0,1").is_empty());
    }

    /// Writes DASH segment files into a run's directory, as the process names them once whole.
    fn write_chunks(run: &Run, stream: u32, numbers: &[u32]) {
        for number in numbers {
            std::fs::write(run.directory.join(chunk_name(stream, number + 1)), b"").unwrap();
        }
    }

    #[test]
    fn a_segment_comes_from_the_newest_run_of_its_rendition_that_wrote_it() {
        let directory = scratch_directory();
        let from_start = run_in(directory.join("run-0"), 0, 0, 0);
        write_chunks(&from_start, 0, &[0, 1, 2, 3]);
        // Moved to rung 1 at segment 2, then back to rung 0 at segment 3.
        let mut lower = run_in(directory.join("run-1"), 1, 1, 2);
        lower.rendition = 1;
        write_chunks(&lower, 0, &[1, 2]);
        write_chunks(&lower, 1, &[1, 2]);
        let back = run_in(directory.join("run-2"), 2, 0, 3);
        write_chunks(&back, 0, &[2, 3]);
        let session = session_with(
            chosen(Protocol::Dash),
            directory.clone(),
            vec![from_start, lower, back],
        );
        let video = |rendition, number| {
            session.chosen_file(Some(DashTrack::Video(rendition)), rendition, number)
        };
        assert!(video(0, 3).unwrap().starts_with(directory.join("run-2")));
        // The end of segment 2 the last run opened with is not segment 2: it started at 3.
        assert!(video(0, 2).unwrap().starts_with(directory.join("run-0")));
        assert!(video(1, 2).unwrap().starts_with(directory.join("run-1")));
        assert_eq!(video(1, 1), None);
        // The audio of any rung.
        let audio = session.chosen_file(Some(DashTrack::Audio), 0, 2).unwrap();
        assert!(audio.ends_with(Path::new("run-1").join(chunk_name(1, 3))));
    }

    #[test]
    fn a_segment_without_a_key_frame_is_the_one_it_is_shown_in() {
        let directory = scratch_directory();
        // Key frames ten seconds apart: no file starts segments 1 and 3.
        let run = run_in(directory.join("run-0"), 0, 0, 0);
        write_chunks(&run, 0, &[0, 2]);
        let session = session_with(chosen(Protocol::Dash), directory, vec![run]);
        let video = |number| session.chosen_file(Some(DashTrack::Video(0)), 0, number);
        assert!(video(1).unwrap().ends_with(chunk_name(0, 1)));
        // Not until a later file proves that segment 3 has none of its own.
        assert_eq!(video(3), None);
    }

    #[test]
    fn a_run_started_without_a_key_frame_shows_its_start_in_the_file_before() {
        let directory = scratch_directory();
        // Started at 512, it opens at the key frame in 511 and has none until 513.
        let run = run_in(directory.join("run-0"), 0, 0, 512);
        write_chunks(&run, 0, &[511, 513]);
        let session = session_with(chosen(Protocol::Dash), directory, vec![run]);
        let video = |number| session.chosen_file(Some(DashTrack::Video(0)), 0, number);
        assert!(video(512).unwrap().ends_with(chunk_name(0, 512)));
        assert!(video(513).unwrap().ends_with(chunk_name(0, 514)));
    }

    #[test]
    fn an_hls_segment_is_there_once_announced_finished() {
        let directory = scratch_directory();
        let mut run = run_in(directory.join("run-0"), 0, 0, 0);
        for (number, finished) in [(0, true), (1, false)] {
            run.segments.insert(
                number,
                Announced {
                    start: f64::from(number) * 5.0,
                    seconds: 5.0,
                    finished,
                },
            );
        }
        let session = session_with(chosen(Protocol::Hls), directory, vec![run]);
        assert!(session
            .chosen_file(None, 0, 0)
            .unwrap()
            .ends_with(hls_name(0)));
        assert_eq!(session.chosen_file(None, 0, 1), None);
    }

    fn run_in(directory: PathBuf, generation: u64, rung: usize, start_chunk: u32) -> Run {
        std::fs::create_dir_all(&directory).unwrap();
        Run {
            generation,
            rung,
            rendition: 0,
            start_chunk,
            directory,
            segments: BTreeMap::new(),
            ended: false,
            completed: false,
        }
    }

    fn session_with(request: StreamRequest, directory: PathBuf, runs: Vec<Run>) -> Arc<Session> {
        std::fs::create_dir_all(&directory).unwrap();
        Arc::new(Session {
            callback: "callback".to_owned(),
            callback_base: String::new(),
            request,
            directory,
            progress: Mutex::new(Progress {
                runs,
                ..Progress::default()
            }),
            changed: Notify::new(),
            closed: AtomicBool::new(false),
            process: Mutex::new(None),
            restarting: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
            switching: Mutex::new(Switching {
                server: None,
                last_client_switch: None,
                served_rung: 0,
                last_video_chunk: None,
                sent: None,
            }),
            subtitle: None,
        })
    }

    fn session_writing_to(directory: PathBuf, ended: bool) -> Arc<Session> {
        let mut run = run_in(directory.clone(), 0, 0, 0);
        run.ended = ended;
        session_with(request(AudioAction::Copy), directory, vec![run])
    }

    fn scratch_directory() -> PathBuf {
        std::env::temp_dir().join(format!("klapz-test-{}", uuid::Uuid::new_v4().simple()))
    }

    /// Two runs of one rendition: the first from chunk 0, then one moved to another rung at
    /// chunk 40, while the first had already written chunk 45.
    fn moved_at_forty(adaptation: Adaptation) -> Arc<Session> {
        let directory = scratch_directory();
        let first = run_in(directory.join("run-0"), 0, 0, 0);
        let second = run_in(directory.join("run-1"), 1, 1, 40);
        std::fs::write(first.directory.join(chunk_name(0, 46)), b"top rung").unwrap();
        std::fs::write(first.directory.join(chunk_name(0, 10)), b"top rung").unwrap();
        let mut asked = encoded(1920, 1080, 20_000, 256, 16);
        asked.ladder.adaptation = adaptation;
        session_with(asked, directory, vec![first, second])
    }

    #[test]
    fn once_the_server_moves_rung_a_segment_is_answered_by_the_newest_run_only() {
        let session = moved_at_forty(Adaptation::Server);
        assert!(session.written_chunk(0, 45).is_none());
        assert!(session.written_chunk(0, 9).is_some());
        assert_eq!(session.rung_answering(45), 1);
        assert_eq!(session.rung_answering(9), 0);
        // A seek of a fixed quality loses nothing another run wrote.
        assert!(moved_at_forty(Adaptation::Fixed)
            .written_chunk(0, 45)
            .is_some());
    }

    #[test]
    fn soft_subtitles_leave_the_stream_alone() {
        let mut asked = request(AudioAction::ConvertToAac);
        asked.protocol = Protocol::Dash;
        asked.subtitle = Some(SubtitleDelivery::Text {
            track: track(3, "srt", "fr"),
            format: SubtitleFormat::WebVtt,
        });
        let joined = arguments(&asked, 1, None).join(" ");
        // Extracted apart, the subtitle is no output of the stream's process.
        assert!(!joined.contains("0:3"), "{joined}");
        assert!(joined.contains("-codec:0 copy"), "{joined}");
    }

    #[tokio::test]
    async fn a_session_without_soft_subtitles_has_none_to_extract() {
        let transcoder = Transcoder::new(crate::Config::default());
        register(&transcoder, session_writing_to(scratch_directory(), false));
        assert!(matches!(
            transcoder.subtitles("known").await,
            Err(TranscodeError::UnknownSegment)
        ));
    }

    fn register(transcoder: &Transcoder, session: Arc<Session>) {
        transcoder
            .sessions
            .lock()
            .unwrap()
            .insert("known".to_owned(), session);
    }

    #[test]
    fn only_a_session_still_idle_is_stopped_for_it() {
        let transcoder = Transcoder::default();
        let idle = session_writing_to(scratch_directory(), false);
        *idle.last_used.lock().unwrap() = Instant::now()
            .checked_sub(IDLE_LIFETIME + Duration::from_secs(1))
            .unwrap();
        register(&transcoder, idle);
        let busy = session_writing_to(scratch_directory(), false);
        transcoder
            .sessions
            .lock()
            .unwrap()
            .insert("busy".to_owned(), busy);

        assert_eq!(transcoder.idle_sessions(), ["known"]);
        assert!(!transcoder.stop_session_if_idle("busy"));
        // A request between the listing and the stop keeps it.
        assert!(transcoder.ping_session("known"));
        assert!(!transcoder.stop_session_if_idle("known"));
        *transcoder.sessions.lock().unwrap()["known"]
            .last_used
            .lock()
            .unwrap() = Instant::now()
            .checked_sub(IDLE_LIFETIME + Duration::from_secs(1))
            .unwrap();
        assert!(transcoder.stop_session_if_idle("known"));
        assert!(!transcoder.has_session("known"));
        assert!(transcoder.has_session("busy"));
    }

    #[test]
    fn only_a_settled_directory_no_session_owns_is_removed() {
        let root = scratch_directory();
        for name in ["owned", "orphan", "subtitle-aa"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let owned = std::iter::once("owned".to_owned()).collect();
        // Just created, as a session's is before it is registered.
        remove_unowned(&root, &owned, ORPHAN_AGE);
        assert!(root.join("orphan").exists());
        remove_unowned(&root, &owned, Duration::ZERO);
        assert!(root.join("owned").exists());
        assert!(!root.join("orphan").exists());
        // A file's subtitles belong to no session, and are pruning's.
        assert!(root.join("subtitle-aa").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn stopping_every_session_empties_the_table_and_their_directories() {
        let transcoder = Transcoder::default();
        let directory = scratch_directory();
        std::fs::create_dir_all(&directory).unwrap();
        register(&transcoder, session_writing_to(directory.clone(), false));
        transcoder.stop_all_sessions().await;
        assert!(!transcoder.has_session("known"));
        // The last reference gone, the directory goes with it.
        assert!(!directory.exists());
    }

    /// Sessions under their keys, each of a playback and asked for that many seconds in.
    fn playing(sessions: &[(&str, Option<&str>, u64)]) -> (Transcoder, Instant) {
        let transcoder = Transcoder::default();
        let origin = Instant::now();
        for &(id, playback, second) in sessions {
            let mut session = session_writing_to(scratch_directory(), false);
            let request = &mut Arc::get_mut(&mut session).unwrap().request;
            request.session = id.to_owned();
            request.playback = playback.map(str::to_owned);
            request.asked_at = origin + Duration::from_secs(second);
            transcoder
                .sessions
                .lock()
                .unwrap()
                .insert(id.to_owned(), session);
        }
        (transcoder, origin)
    }

    fn asking(id: &str, playback: Option<&str>, at: Instant) -> StreamRequest {
        StreamRequest {
            session: id.to_owned(),
            playback: playback.map(str::to_owned),
            asked_at: at,
            ..request(AudioAction::Copy)
        }
    }

    #[test]
    fn a_new_stream_replaces_the_one_before_it_under_the_same_playback() {
        let (transcoder, origin) = playing(&[
            ("old", Some("tab"), 0),
            ("new", Some("tab"), 1),
            ("elsewhere", Some("other"), 2),
            ("unnamed", None, 3),
        ]);
        let new = origin + Duration::from_secs(1);

        assert_eq!(
            transcoder
                .stop_streams_of(&asking("new", Some("tab"), new))
                .unwrap(),
            ["old"]
        );
        assert!(transcoder.session("old").is_none());
        for id in ["new", "elsewhere", "unnamed"] {
            assert!(transcoder.session(id).is_some(), "{id}");
        }
        assert!(transcoder
            .stop_streams_of(&asking("new", None, new))
            .unwrap()
            .is_empty());
    }

    /// A player starting twice in a row: its second start reached here first, and its first,
    /// arriving late, must not take it down.
    #[test]
    fn a_start_asked_for_before_a_running_one_of_its_playback_replaces_nothing() {
        let (transcoder, origin) = playing(&[("second", Some("tab"), 1)]);

        assert!(matches!(
            transcoder.stop_streams_of(&asking("first", Some("tab"), origin)),
            Err(TranscodeError::Superseded)
        ));
        assert!(transcoder.session("second").is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_for_no_session_is_refused_at_once() {
        let transcoder = Transcoder::default();
        let began = tokio::time::Instant::now();
        assert!(matches!(
            transcoder
                .dash_chunk("missing", DashTrack::Video(0), 0)
                .await,
            Err(TranscodeError::UnknownSession)
        ));
        assert!(matches!(
            transcoder.dash_header("missing", DashTrack::Audio).await,
            Err(TranscodeError::UnknownSession)
        ));
        assert!(matches!(
            transcoder.session_segment("missing", 0, 0).await,
            Err(TranscodeError::UnknownSession)
        ));
        assert_eq!(began.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_ends_as_soon_as_the_session_is_stopped() {
        let session = session_writing_to(scratch_directory(), false);
        let missing = session.directory.join(chunk_name(0, 1));
        let stopped = Arc::clone(&session);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            stopped.closed.store(true, Ordering::Release);
        });
        let began = tokio::time::Instant::now();
        assert!(session
            .wait_for(|| missing.exists().then_some(()))
            .await
            .is_none());
        assert!(began.elapsed() < HOLD_BEFORE_REFUSAL);
    }

    #[tokio::test(start_paused = true)]
    async fn a_segment_of_an_ended_process_is_held_before_being_refused() {
        let transcoder = Transcoder::default();
        register(&transcoder, session_writing_to(scratch_directory(), true));
        let began = tokio::time::Instant::now();
        let refused = transcoder.session_segment("known", 0, 3).await;
        assert!(matches!(refused, Err(TranscodeError::UnknownSegment)));
        assert!(began.elapsed() >= HOLD_BEFORE_REFUSAL);
        assert!(began.elapsed() < WAIT_FOR_OUTPUT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_segment_past_what_a_completed_process_wrote_is_empty_at_once() {
        let transcoder = Transcoder::default();
        let session = session_writing_to(scratch_directory(), true);
        session.with_progress(|progress| progress.runs[0].completed = true);
        register(&transcoder, session);
        let began = tokio::time::Instant::now();
        let files = transcoder.session_segment("known", 0, 3).await.unwrap();
        assert!(files.is_empty());
        assert_eq!(began.elapsed(), Duration::ZERO);
    }

    #[test]
    fn only_a_process_that_completed_tells_where_the_media_ends() {
        let directory = scratch_directory();
        let mut run = run_in(directory.join("run-0"), 0, 0, 0);
        write_chunks(&run, 0, &[0, 1, 2]);
        run.ended = true;
        let session = session_with(chosen(Protocol::Dash), directory, vec![run]);
        // Failed: what it did not write may still exist.
        assert!(!session.past_the_end(0, 3));
        session.with_progress(|progress| progress.runs[0].completed = true);
        assert!(session.past_the_end(0, 3));
        assert!(session.past_the_end(0, 4));
        assert!(!session.past_the_end(0, 2));
        assert!(!session.past_the_end(0, 0));
    }

    #[test]
    fn seeking_back_before_a_completed_run_is_not_past_the_end() {
        let directory = scratch_directory();
        // A seek near the end ran to completion from segment 10.
        let mut run = run_in(directory.join("run-0"), 0, 0, 10);
        write_chunks(&run, 0, &[10, 11]);
        run.ended = true;
        run.completed = true;
        let session = session_with(chosen(Protocol::Dash), directory, vec![run]);
        assert!(session.past_the_end(0, 12));
        assert!(!session.past_the_end(0, 0));
        assert!(!session.past_the_end(0, 9));
    }

    #[tokio::test(start_paused = true)]
    async fn a_running_process_gets_the_full_wait() {
        let session = session_writing_to(scratch_directory(), false);
        let missing = session.directory.join(chunk_name(0, 1));
        let began = tokio::time::Instant::now();
        assert!(session
            .wait_for(|| missing.exists().then_some(()))
            .await
            .is_none());
        assert!(began.elapsed() >= WAIT_FOR_OUTPUT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_file_arriving_during_the_hold_is_served() {
        let session = session_writing_to(scratch_directory(), true);
        let path = session.directory.join(chunk_name(0, 1));
        let written = path.clone();
        tokio::spawn(async move {
            tokio::time::sleep(HOLD_BEFORE_REFUSAL / 2).await;
            std::fs::write(written, b"chunk").unwrap();
        });
        let began = tokio::time::Instant::now();
        session
            .wait_for(|| path.exists().then_some(()))
            .await
            .unwrap();
        assert!(began.elapsed() < HOLD_BEFORE_REFUSAL);
    }

    #[test]
    fn a_rendition_beyond_one_exists_only_where_the_player_chooses() {
        let asked = chosen(Protocol::Dash);
        assert_eq!(asked.rendition_of(1), 1);
        let mut server = asked;
        server.ladder.adaptation = Adaptation::Server;
        assert_eq!(server.rendition_of(1), 0);
    }
}
