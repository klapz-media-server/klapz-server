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
//! for it. When the player chooses, each rung is a representation of its own, written as one
//! piece per group of pictures and served as the pieces starting within each segment's time:
//! the transcoder cuts its segments relative to where its run started, so two runs started at
//! different points would not otherwise agree on where a segment begins.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt as _;
use tokio::process::Command;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::adapt::{self, Delivery, ServerAdaptation};
use crate::fragment;
use crate::plan::{
    audio_decoder, language_tag, Adaptation, AudioAction, KeyFrames, Ladder, StreamPlan, Track,
    VideoAction, VideoEncode,
};
use crate::{TranscodeError, Transcoder};

/// Each session is a process and a directory of segments, and a client controls how many.
const MAX_SESSIONS: usize = 4;

/// A client that stopped asking has gone away without saying so.
const IDLE_LIFETIME: Duration = Duration::from_secs(300);

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

/// How far ahead of what it has written a process is left to catch up by itself. A request
/// beyond this is a seek: a moment for a remux, a few seconds for an encode.
const SEEK_MARGIN_SECONDS: u32 = 30;

/// A piece's start is read in its track's timescale: one starting on a segment's boundary
/// must not fall before it by a rounding.
const BOUNDARY_TOLERANCE: f64 = 0.000_5;

/// An audio piece is cut along with the video's, within a few audio frames of it.
const AUDIO_CUT_TOLERANCE: f64 = 1.0;

/// How much of a piece is read to find where it starts: its `moof` comes first.
const PIECE_HEAD_BYTES: u64 = 4096;

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
}

/// How a subtitle reaches the player.
#[derive(Debug, Clone, PartialEq)]
pub enum SubtitleDelivery {
    /// Drawn into the picture. `scale` is the client's size, 1.0 for its 100%.
    Burn { track: Track, scale: f64 },
    /// Converted to ASS beside the stream, in pieces of a second, for the player to draw:
    /// Plex Web's soft subtitles.
    Text { track: Track },
}

impl SubtitleDelivery {
    const fn track(&self) -> &Track {
        match self {
            Self::Burn { track, .. } | Self::Text { track } => track,
        }
    }

    /// Text the process can draw with libass or convert to ASS. Image subtitles it overlays
    /// instead, by a command not captured yet.
    fn is_text(&self) -> bool {
        matches!(
            self.track().codec.as_str(),
            "srt" | "subrip" | "ass" | "ssa" | "mov_text" | "webvtt" | "vtt" | "text"
        )
    }
}

/// A part of a session's text subtitles, in the order they are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitlePiece {
    /// The ASS script's header, styles included.
    Header,
    /// The events of one second of the stream, numbered from 0.
    Chunk(u32),
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

/// A session's text subtitles, piece by piece as the process finishes them.
#[derive(Debug)]
pub struct SubtitleReader {
    session: Arc<Session>,
    /// The run whose pieces these are: a seek restarts the process, which numbers its pieces
    /// from 0 again, and they would not follow what was already read.
    generation: Option<u64>,
    directory: PathBuf,
    next: SubtitlePiece,
}

impl SubtitleReader {
    /// The next piece once whole: when the one after it exists, or the process has ended.
    /// `None` at the end: the process ended, was stopped, or restarted for a seek.
    pub async fn next_piece(&mut self) -> Option<Vec<u8>> {
        let (path, following) = match self.next {
            SubtitlePiece::Header => (SUBTITLE_HEADER.to_owned(), subtitle_chunk_name(0)),
            SubtitlePiece::Chunk(number) => {
                (subtitle_chunk_name(number), subtitle_chunk_name(number + 1))
            }
        };
        let (path, following) = (self.directory.join(path), self.directory.join(following));
        loop {
            if self.session.closed.load(Ordering::Acquire)
                || self.session.current_generation() != self.generation
            {
                return None;
            }
            let ended = self.session.ended();
            if following.exists() || (ended && path.exists()) {
                let piece = tokio::fs::read(&path).await.ok()?;
                self.next = match self.next {
                    SubtitlePiece::Header => SubtitlePiece::Chunk(0),
                    SubtitlePiece::Chunk(number) => SubtitlePiece::Chunk(number + 1),
                };
                return Some(piece);
            }
            if ended {
                return None;
            }
            tokio::time::sleep(FILE_POLL).await;
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
    /// Where each DASH piece starts, by track, in the order of their files.
    pieces: [Vec<f64>; 2],
    timescales: [Option<u32>; 2],
    ended: bool,
}

impl Run {
    /// HLS pieces as `(start, file number)`, the finished ones.
    fn finished_pieces(&self) -> Vec<(f64, u32)> {
        self.segments
            .iter()
            .filter(|(_, piece)| piece.finished)
            .map(|(number, piece)| (piece.start, *number))
            .collect()
    }

    fn latest_finished(&self) -> Option<u32> {
        self.segments
            .iter()
            .rev()
            .find(|(_, segment)| segment.finished)
            .map(|(number, _)| *number)
    }
}

/// A run's pieces as far as they were read, to read the ones written since.
struct ReadPieces {
    generation: u64,
    directory: PathBuf,
    first: u32,
    known: [usize; 2],
    timescales: [Option<u32>; 2],
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
    /// The process's way back in, up to the run: `…/video/:/transcode/session`.
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
}

/// Removed with the last reference, which the process's supervising task holds until it exits.
impl Drop for Session {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.directory) {
            tracing::warn!(%error, "could not remove a transcode session's directory");
        }
    }
}

/// Which pieces of a run make up the segment `[from, to)`: those starting within it or, when the
/// source's key frames are further apart than a segment, the one it falls in. `None` until all
/// of them are written. The run must have started at or before the segment, so that none of
/// its pieces can be missing before the first one found.
fn chunk_pieces(starts: &[f64], ended: bool, from: f64, to: f64) -> Option<Range<usize>> {
    let begin = starts.partition_point(|start| *start < from - BOUNDARY_TOLERANCE);
    let end = starts.partition_point(|start| *start < to - BOUNDARY_TOLERANCE);
    let range = if begin < end {
        begin..end
    } else if begin > 0 && begin < starts.len() {
        begin - 1..begin
    } else {
        // Past what the run wrote: not reached yet, or past the end of the media.
        return None;
    };
    (range.end < starts.len() || ended).then_some(range)
}

/// Each audio piece counted at the start of the video piece it was cut with.
fn audio_starts(audio: &[f64], video: &[f64]) -> Vec<f64> {
    audio
        .iter()
        .map(|&start| {
            let at = video.partition_point(|cut| *cut < start);
            [at.checked_sub(1), Some(at)]
                .into_iter()
                .flatten()
                .filter_map(|index| video.get(index))
                .copied()
                .filter(|cut| (cut - start).abs() <= AUDIO_CUT_TOLERANCE)
                .min_by(|a, b| (a - start).abs().total_cmp(&(b - start).abs()))
                .unwrap_or(start)
        })
        .collect()
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

    /// Reads where the DASH pieces written since the last look start.
    async fn refresh_pieces(&self) {
        let pending: Vec<ReadPieces> = self.with_progress(|progress| {
            progress
                .runs
                .iter()
                .map(|run| ReadPieces {
                    generation: run.generation,
                    directory: run.directory.clone(),
                    first: first_file(Protocol::Dash, run.start_chunk),
                    known: [run.pieces[0].len(), run.pieces[1].len()],
                    timescales: run.timescales,
                })
                .collect()
        });
        for ReadPieces {
            generation,
            directory,
            first,
            known,
            timescales,
        } in pending
        {
            for (index, stream) in [(0, 0), (1, 1)] {
                let Some(timescale) = (match timescales[index] {
                    Some(timescale) => Some(timescale),
                    None => tokio::fs::read(directory.join(init_name(stream)))
                        .await
                        .ok()
                        .and_then(|init| fragment::timescale(&init))
                        .filter(|timescale| *timescale > 0),
                }) else {
                    continue;
                };
                let mut found = Vec::new();
                loop {
                    let Some(file) = u32::try_from(known[index] + found.len())
                        .ok()
                        .and_then(|index| first.checked_add(index))
                    else {
                        break;
                    };
                    let path = directory.join(chunk_name(stream, file));
                    let Some(shown) = read_head(&path)
                        .await
                        .and_then(|head| fragment::first_presentation_time(&head))
                    else {
                        break;
                    };
                    #[allow(
                        clippy::cast_precision_loss,
                        reason = "a presentation time far below 2^52 ticks"
                    )]
                    found.push(shown as f64 / f64::from(timescale));
                }
                self.with_progress(|progress| {
                    if let Some(run) = progress
                        .runs
                        .iter_mut()
                        .find(|run| run.generation == generation)
                    {
                        run.timescales[index] = Some(timescale);
                        if run.pieces[index].len() == known[index] {
                            run.pieces[index].extend(found);
                        }
                    }
                });
            }
        }
    }

    /// The files of segment `number` of a rendition the player chose, `None` for audio, when
    /// some run has written all of them.
    fn assembled(
        &self,
        track: Option<DashTrack>,
        rendition: usize,
        number: u32,
    ) -> Option<Vec<PathBuf>> {
        let seconds = self.seconds();
        let (from, to) = (f64::from(number) * seconds, f64::from(number + 1) * seconds);
        self.with_progress(|progress| {
            progress
                .runs
                .iter()
                .rev()
                .filter(|run| run.start_chunk <= number)
                .find_map(|run| match track {
                    None if run.rendition == rendition => {
                        let pieces = run.finished_pieces();
                        let starts: Vec<f64> = pieces.iter().map(|(start, _)| *start).collect();
                        let range = chunk_pieces(&starts, run.ended, from, to)?;
                        Some(
                            pieces[range]
                                .iter()
                                .map(|(_, file)| run.directory.join(hls_name(*file)))
                                .collect(),
                        )
                    }
                    Some(DashTrack::Video(wanted)) if run.rendition == wanted => {
                        let range = chunk_pieces(&run.pieces[0], run.ended, from, to)?;
                        Some(piece_paths(run, 0, range))
                    }
                    Some(DashTrack::Audio) => {
                        let starts = audio_starts(&run.pieces[1], &run.pieces[0]);
                        let range = chunk_pieces(&starts, run.ended, from, to)?;
                        Some(piece_paths(run, 1, range))
                    }
                    _ => None,
                })
        })
    }

    /// Whether the running process, of this rendition, will write segment `number` soon.
    fn pieces_will_reach(&self, rendition: usize, number: u32) -> bool {
        let seconds = self.seconds();
        let from = f64::from(number) * seconds;
        let margin = f64::from(SEEK_MARGIN_SECONDS);
        self.with_progress(|progress| {
            progress.current().is_some_and(|run| {
                let frontier = match self.request.protocol {
                    Protocol::Dash => run.pieces[0].last().copied(),
                    Protocol::Hls => run.finished_pieces().last().map(|(start, _)| *start),
                }
                .unwrap_or(f64::from(run.start_chunk) * seconds);
                run.rendition == rendition
                    && !run.ended
                    && run.start_chunk <= number
                    && frontier >= from - margin
            })
        })
    }

    fn output(&self, rendition: usize) -> Output {
        let total_ms = self.request.total_ms;
        let segment_seconds = self.request.segment_seconds();
        let segments = if self.request.client_adapted() {
            // Every segment some run of the rendition completed, at its nominal length.
            let mut segments = Vec::new();
            let mut number = 0;
            while let Some(files) = self.assembled(None, rendition, number) {
                if files.is_empty() {
                    break;
                }
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

fn piece_paths(run: &Run, stream: u32, range: Range<usize>) -> Vec<PathBuf> {
    let first = first_file(Protocol::Dash, run.start_chunk);
    range
        .filter_map(|index| u32::try_from(index).ok())
        .map(|index| run.directory.join(chunk_name(stream, first + index)))
        .collect()
}

/// The start of a finished piece, or `None` while it does not exist under its final name.
async fn read_head(path: &Path) -> Option<Vec<u8>> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut head = Vec::new();
    file.take(PIECE_HEAD_BYTES)
        .read_to_end(&mut head)
        .await
        .ok()?;
    Some(head)
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

/// A process's outputs cut in pieces at every key frame, for a rung the player chooses: no
/// cut can then fall anywhere but on one.
const PIECE_SECONDS: &str = "0.001";

fn hls_output(callback: &str, seconds: u32, first_file: u32, pieces: bool) -> Vec<String> {
    let segment_time = if pieces {
        PIECE_SECONDS.to_owned()
    } else {
        seconds.to_string()
    };
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
        "-max_delay",
        "5000000",
        "-avoid_negative_ts",
        "disabled",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "media-%05d.ts",
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

/// This build keeps the chunks on a grid of `seconds` even when copying video, so their number
/// says where they are in time: 494 chunks of 5 seconds for 2467 seconds. The grid starts where
/// the run does, though. Cut in pieces, the audio follows the video's cuts.
fn dash_output(callback: &str, seconds: u32, first_file: u32, pieces: bool) -> Vec<String> {
    let mut args = strings(&["-f", "dash", "-seg_duration", &seconds.to_string()]);
    if pieces {
        args.extend(strings(&[
            "-adaptation_sets",
            &format!("id=0,seg_duration={PIECE_SECONDS},streams=0 id=1,streams=1"),
        ]));
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
    let mut args = strings(&[&format!("-codec:{}", video.index), "h264"]);
    if plan.audio == AudioAction::ConvertToAac {
        args.extend(strings(&[
            &format!("-codec:{}", audio.index),
            audio_decoder(&audio.codec),
        ]));
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

fn video_encoder_options(encode: &VideoEncode) -> Vec<String> {
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
    args.extend(strings(&["-preset:0", "veryfast", "-x264opts:0", &x264]));
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
    burn: Option<(&Track, f64, &Path)>,
) -> Vec<String> {
    let mut args = match action {
        VideoAction::Copy => strings(&["-map", &format!("0:{}", video.index)]),
        VideoAction::Encode(encode) => {
            let scale = format!(
                "[0:{}]scale=w={}:h={}:force_divisible_by=4[0];[0]format=pix_fmts=yuv420p|nv12[1]",
                video.index, encode.width, encode.height
            );
            match burn {
                Some((subtitle, size, fonts)) => strings(&[
                    "-map_inlineass",
                    &format!("0:{}", subtitle.index),
                    "-filter_complex",
                    &format!("{scale};{}", burn_filter(subtitle, size, fonts)),
                    "-map",
                    "[2]",
                ]),
                None => strings(&["-filter_complex", &scale, "-map", "[1]"]),
            }
        }
    };
    if let Some(tag) = video.language.as_deref().and_then(language_tag) {
        args.extend(strings(&["-metadata:s:0", &format!("language={tag}")]));
    }
    match action {
        VideoAction::Copy => args.extend(strings(&["-codec:0", "copy"])),
        VideoAction::Encode(encode) => args.extend(video_encoder_options(&encode)),
    }
    args
}

/// `native_encoder`: the reference converts audio beside copied video with Media Foundation's
/// encoder and beside an encode with ffmpeg's own; rungs a player moves between must all use
/// the same, and use ffmpeg's.
fn audio_arguments(
    audio: &Track,
    plan: &StreamPlan,
    burns: bool,
    native_encoder: bool,
) -> Vec<String> {
    let converts = plan.audio == AudioAction::ConvertToAac;
    let encodes = plan.video != VideoAction::Copy;
    let mut args = if converts {
        // Filter outputs are numbered across the command, after the video's: two to scale,
        // a third to draw a subtitle.
        let label = match (encodes, burns) {
            (false, _) => 0,
            (true, false) => 2,
            (true, true) => 3,
        };
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
    if let Some(tag) = audio.language.as_deref().and_then(language_tag) {
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
    let pieces = request.client_adapted();
    let burn = match (&request.subtitle, fonts) {
        (Some(SubtitleDelivery::Burn { track, scale }), Some(fonts)) => {
            Some((track, *scale, fonts))
        }
        _ => None,
    };
    let mut args = input_arguments(request, run);
    args.extend(video_arguments(&request.video, run.plan.video, burn));
    args.extend(audio_arguments(
        &request.audio,
        run.plan,
        burn.is_some(),
        pieces,
    ));
    args.extend(match request.protocol {
        Protocol::Hls => hls_output(run.callback, seconds, run.first_file, pieces),
        Protocol::Dash => dash_output(run.callback, seconds, run.first_file, pieces),
    });
    if let Some((subtitle, _, _)) = burn {
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
    if let Some(SubtitleDelivery::Text { track }) = &request.subtitle {
        args.extend(text_subtitle_output(track, run.callback));
    }
    args
}

/// The reference's output for soft subtitles: the track as ASS, its header apart, then a piece
/// per second, announced on a list of its own the server ignores.
fn text_subtitle_output(track: &Track, callback: &str) -> Vec<String> {
    let mut args = strings(&["-map", &format!("0:{}", track.index)]);
    if let Some(tag) = track.language.as_deref().and_then(language_tag) {
        args.extend(strings(&["-metadata:s:0", &format!("language={tag}")]));
    }
    args.extend(strings(&[
        "-codec:0",
        "ass",
        "-strict_ts:0",
        "0",
        "-f",
        "segment",
        "-segment_format",
        "ass",
        "-segment_time",
        "1",
        "-segment_header_filename",
        SUBTITLE_HEADER,
        "-segment_start_number",
        "0",
        "-segment_list",
        &format!("{callback}/manifest?stream=subtitles&X-Plex-Http-Pipeline=infinite"),
        "-segment_list_type",
        "csv",
        "-segment_list_size",
        "5",
        "-segment_list_separate_stream_times",
        "1",
        "-segment_format_options",
        "ignore_readorder=1",
        "-segment_list_unfinished",
        "1",
        "-fflags",
        "+flush_packets",
        "sub-chunk-%05d",
    ]));
    args
}

const SUBTITLE_HEADER: &str = "sub-header";

fn subtitle_chunk_name(number: u32) -> String {
    format!("sub-chunk-{number:05}")
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
/// HLS its segments from zero. A run cut in pieces numbers them from there too, though its
/// numbers no longer say where a piece is: this build writes an encode's timestamps from zero
/// when told its first DASH segment is the first, wherever it starts.
const fn first_file(protocol: Protocol, chunk: u32) -> u32 {
    match protocol {
        Protocol::Dash => chunk + 1,
        Protocol::Hls => chunk,
    }
}

/// The form Plex's build expects its codec directory in: backslashes, colons and spaces escaped
/// once, and on Windows the extended-length prefix.
fn external_libs_value(directory: &Path) -> String {
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
        tokio::select! {
            result = child.wait() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "lost track of a transcoder process");
                }
            }
            () = stop.notified() => {
                if let Err(error) = child.kill().await {
                    tracing::warn!(%error, "could not kill a transcoder process");
                }
            }
        }
        supervised.with_progress(|progress| {
            if let Some(run) = progress
                .runs
                .iter_mut()
                .find(|run| run.generation == generation)
            {
                run.ended = true;
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

    /// Removes what a previous run left on disk. A session cleans up after itself, unless the
    /// server is killed before it can.
    pub fn remove_leftover_sessions(&self) {
        match std::fs::remove_dir_all(&self.config.sessions_directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "could not remove old transcode sessions"),
        }
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
            let burns = matches!(subtitle, SubtitleDelivery::Burn { .. });
            if !subtitle.is_text() || (burns && self.config.fonts.is_none()) {
                return Err(TranscodeError::SubtitleCannotBeBurned);
            }
        }
        self.stop_idle_sessions();
        let replaced = self.stop_streams_of(request.playback.as_deref(), &request.session);
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
        let directory = self.config.sessions_directory.join(&callback);
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
        let session = Arc::new(Session {
            callback_base: format!("{}/video/:/transcode/session", self.config.callback_base),
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
        });
        // Registered before the process exists: its first act is to read its source back.
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), Arc::clone(&session));

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
    fn stop_streams_of(&self, playback: Option<&str>, keep: &str) -> Vec<String> {
        let Some(playback) = playback else {
            return Vec::new();
        };
        let replaced: Vec<String> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(id, session)| {
                id.as_str() != keep && session.request.playback.as_deref() == Some(playback)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &replaced {
            self.stop_session(id);
        }
        replaced
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
            "{}/{generation}/{}",
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
                pieces: [Vec::new(), Vec::new()],
                timescales: [None, None],
                ended: false,
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
        let idle: Vec<String> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, session)| session.idle_for() > IDLE_LIFETIME)
            .map(|(id, _)| id.clone())
            .collect();
        for id in idle {
            self.stop_session(&id);
        }
    }

    /// Whether the session existed.
    pub fn stop_session(&self, id: &str) -> bool {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        if let Some(session) = &session {
            session.closed.store(true, Ordering::Release);
            let stop = session
                .with_process(|process| process.as_ref().map(|process| Arc::clone(&process.stop)));
            if let Some(stop) = stop {
                stop.notify_one();
            }
        }
        session.is_some()
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
                    Protocol::Dash if request.client_adapted() => {
                        run.pieces[0].last().copied().unwrap_or(0.0)
                    }
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

    /// A finished HLS segment's files, waiting for them if they are still being written: one,
    /// or the pieces of a rung the player chose.
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
            return self.chosen_chunk(&session, None, rendition, number).await;
        }
        if rendition != 0 {
            return Err(TranscodeError::UnknownSegment);
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
        let path = session
            .wait_for(|| session.hls_written(number))
            .await
            .ok_or(TranscodeError::UnknownSegment)?;
        session.served(session.rung_answering(number), number);
        Ok(vec![path])
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
                        let finished = run.directory.join(chunk_name(stream, first));
                        let begun = run
                            .directory
                            .join(format!("{}.tmp", chunk_name(stream, first)));
                        finished.exists() || begun.exists()
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

    /// Reads a session's text subtitles from their header on, waiting for the session to start:
    /// Plex Web asks for its subtitles right after the decision, before it starts the stream,
    /// and the reference holds that request until it does.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`] if none starts in time, or
    /// [`TranscodeError::UnknownSegment`] for a session that writes no text subtitles.
    pub async fn subtitles(&self, id: &str) -> Result<SubtitleReader, TranscodeError> {
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
        if !matches!(
            session.request.subtitle,
            Some(SubtitleDelivery::Text { .. })
        ) {
            return Err(TranscodeError::UnknownSegment);
        }
        let (generation, directory) = session.with_progress(|progress| {
            progress
                .current()
                .map_or((None, session.directory.clone()), |run| {
                    (Some(run.generation), run.directory.clone())
                })
        });
        Ok(SubtitleReader {
            session,
            generation,
            directory,
            next: SubtitlePiece::Header,
        })
    }

    /// A finished DASH chunk's files: one, or the pieces of a rung the player chose. `number`
    /// counts from zero, as the manifest given to the player does. One the running process will
    /// not get to soon is a seek, and restarts it there.
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
                .await;
        }
        if matches!(track, DashTrack::Video(rendition) if rendition != 0) {
            return Err(TranscodeError::UnknownSegment);
        }
        let stream = track.stream();
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
        let path = session
            .wait_for(|| session.written_chunk(stream, number))
            .await
            .ok_or(TranscodeError::UnknownSegment)?;
        if stream == 0 {
            session.served(session.rung_answering(number), number);
        }
        Ok(vec![path])
    }

    /// A segment of a stream whose player chooses the rung, `track` `None` for HLS. Served from
    /// whatever run wrote it; otherwise the running process is left to reach it, moved there
    /// if it is too far, or replaced by one at the rung asked for, once the rules allow.
    async fn chosen_chunk(
        &self,
        session: &Arc<Session>,
        track: Option<DashTrack>,
        rendition: usize,
        number: u32,
    ) -> Result<Vec<PathBuf>, TranscodeError> {
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
            if session.request.protocol == Protocol::Dash {
                session.refresh_pieces().await;
            }
            if let Some(files) = session.assembled(track, rendition, number) {
                if video {
                    session.served(rendition, number);
                }
                return Ok(files);
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
                if !exhausted && !session.pieces_will_reach(rendition, number) {
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
            index,
            codec: codec.to_owned(),
            language: Some(language.to_owned()),
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
        }
    }

    /// The command of a run of the request's top rung, from where it asks to start.
    fn arguments(request: &StreamRequest, first_file: u32, fonts: Option<&Path>) -> Vec<String> {
        stream_arguments(
            request,
            &RunPlan {
                plan: request.ladder.top(),
                offset: request.offset,
                first_file,
                callback: "CB",
            },
            fonts,
        )
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
        stream_arguments(
            request,
            &RunPlan {
                plan: &request.ladder.rungs[rung],
                offset: Some(offset),
                first_file: first_file(request.protocol, 7),
                callback: "CB",
            },
            None,
        )
        .join(" ")
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
                run.contains("-adaptation_sets id=0,seg_duration=0.001,streams=0 id=1,streams=1 "),
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
            hls.contains("-segment_time 0.001 -segment_start_number 7 "),
            "{hls}"
        );
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

    /// Where the transcoder started the pieces of a test episode, run from 27 seconds: a key
    /// frame every 3 seconds.
    const PIECES: [f64; 6] = [27.0, 30.0, 33.0, 36.0, 39.0, 42.0];

    #[test]
    fn a_segment_is_the_pieces_starting_within_it() {
        // [30, 35): the pieces at 30 and 33, complete since the one at 36 exists.
        assert_eq!(chunk_pieces(&PIECES, false, 30.0, 35.0), Some(1..3));
        // [35, 40): 36 and 39, whose end only the piece at 42 proves.
        assert_eq!(chunk_pieces(&PIECES, false, 35.0, 40.0), Some(3..5));
        assert_eq!(chunk_pieces(&PIECES, false, 40.0, 45.0), None);
        assert_eq!(chunk_pieces(&PIECES, true, 40.0, 45.0), Some(5..6));
        // A rounding below a boundary does not move a piece to the segment before.
        assert_eq!(
            chunk_pieces(&[29.9999, 35.0], false, 30.0, 35.0),
            Some(0..1)
        );
    }

    #[test]
    fn a_segment_before_every_piece_is_not_there() {
        assert_eq!(chunk_pieces(&[40.0, 45.0], true, 25.0, 30.0), None);
        assert_eq!(chunk_pieces(&[], true, 0.0, 5.0), None);
    }

    #[test]
    fn a_segment_comes_from_a_run_started_at_or_before_it() {
        let directory = scratch_directory();
        let mut from_start = run_in(directory.join("run-0"), 0, 0, 0);
        from_start.pieces[0] = vec![0.08, 3.9, 7.8, 11.8];
        // Started at segment 2 on the key frame before it, so holding a piece of segment 1.
        let mut later = run_in(directory.join("run-1"), 1, 0, 2);
        later.pieces[0] = vec![7.8, 11.8, 15.7];
        let session = session_with(chosen(Protocol::Dash), directory, vec![from_start, later]);
        let video = Some(DashTrack::Video(0));
        // The first piece is shown a little after the start, and still opens segment 0.
        let first = session.assembled(video, 0, 0).unwrap();
        assert_eq!(first.len(), 2);
        assert!(first[0].ends_with(Path::new("run-0").join(chunk_name(0, 1))));
        // Segment 1 only from the run that covers all of it.
        let second = session.assembled(video, 0, 1).unwrap();
        assert!(second[0].starts_with(session.directory.join("run-0")));
        let third = session.assembled(video, 0, 2).unwrap();
        assert!(third[0].ends_with(Path::new("run-1").join(chunk_name(0, 4))));
    }

    #[test]
    fn a_segment_between_two_far_key_frames_is_the_piece_it_falls_in() {
        // Ten seconds between key frames, five-second segments.
        let sparse = [0.0, 10.0, 20.0];
        assert_eq!(chunk_pieces(&sparse, false, 5.0, 10.0), Some(0..1));
        assert_eq!(chunk_pieces(&sparse, false, 10.0, 15.0), Some(1..2));
        assert_eq!(chunk_pieces(&sparse, false, 15.0, 20.0), Some(1..2));
        assert_eq!(chunk_pieces(&sparse, true, 25.0, 30.0), None);
    }

    #[test]
    fn audio_pieces_count_from_the_video_cut_they_follow() {
        // Measured: audio cut a few hundredths of a second before each video key frame, and
        // a run's first audio piece starting where its seek landed rather than on a cut.
        let audio = [29.979, 32.966, 35.974];
        assert_eq!(audio_starts(&audio, &PIECES), [30.0, 33.0, 36.0]);
        assert_eq!(audio_starts(&[50.0], &PIECES), [50.0]);
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
            pieces: [Vec::new(), Vec::new()],
            timescales: [None, None],
            ended: false,
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
        })
    }

    fn session_writing_to(directory: PathBuf, ended: bool) -> Arc<Session> {
        let mut run = run_in(directory.clone(), 0, 0, 0);
        run.ended = ended;
        session_with(request(AudioAction::Copy), directory, vec![run])
    }

    fn scratch_directory() -> PathBuf {
        std::env::temp_dir().join(format!("open-plex-test-{}", uuid::Uuid::new_v4().simple()))
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

    /// The subtitle output of the reference's command for Soul's French subtitles, `auto`.
    const CAPTURED_TEXT_SUBTITLES: &str = "dash -map 0:3 -metadata:s:0 language=fra -codec:0 ass -strict_ts:0 0 -f segment -segment_format ass -segment_time 1 -segment_header_filename sub-header -segment_start_number 0 -segment_list CB/manifest?stream=subtitles&X-Plex-Http-Pipeline=infinite -segment_list_type csv -segment_list_size 5 -segment_list_separate_stream_times 1 -segment_format_options ignore_readorder=1 -segment_list_unfinished 1 -fflags +flush_packets sub-chunk-%05d";

    #[test]
    fn writes_soft_subtitles_as_the_reference_does() {
        let mut asked = request(AudioAction::ConvertToAac);
        asked.protocol = Protocol::Dash;
        asked.subtitle = Some(SubtitleDelivery::Text {
            track: track(3, "srt", "fr"),
        });
        let joined = arguments(&asked, 1, None).join(" ");
        assert!(joined.ends_with(CAPTURED_TEXT_SUBTITLES), "{joined}");
        // Copied video stays copied: a soft subtitle needs no encode.
        assert!(joined.contains("-codec:0 copy"), "{joined}");
    }

    async fn reader_over(directory: PathBuf, ended: bool) -> SubtitleReader {
        let mut session = session_writing_to(directory, ended);
        Arc::get_mut(&mut session).unwrap().request.subtitle = Some(SubtitleDelivery::Text {
            track: track(2, "srt", "fr"),
        });
        let transcoder = Transcoder::new(crate::Config::default());
        register(&transcoder, session);
        transcoder.subtitles("known").await.unwrap()
    }

    #[tokio::test]
    async fn reads_subtitle_pieces_in_order_once_whole() {
        let directory = scratch_directory();
        let mut reader = reader_over(directory.clone(), false).await;
        std::fs::write(directory.join("sub-header"), "[Script Info]").unwrap();
        std::fs::write(directory.join("sub-chunk-00000"), "Dialogue: 0").unwrap();
        std::fs::write(directory.join("sub-chunk-00001"), "Dialogue: 1").unwrap();
        assert_eq!(reader.next_piece().await.unwrap(), b"[Script Info]");
        assert_eq!(reader.next_piece().await.unwrap(), b"Dialogue: 0");
        // The last piece may still be written: it waits for the next one, or the end.
        let pending = tokio::time::timeout(Duration::from_millis(300), reader.next_piece()).await;
        assert!(pending.is_err());
    }

    #[tokio::test]
    async fn an_ended_process_gives_its_last_piece_then_the_end() {
        let directory = scratch_directory();
        let mut reader = reader_over(directory.clone(), true).await;
        std::fs::write(directory.join("sub-header"), "[Script Info]").unwrap();
        std::fs::write(directory.join("sub-chunk-00000"), "Dialogue: 0").unwrap();
        assert_eq!(reader.next_piece().await.unwrap(), b"[Script Info]");
        assert_eq!(reader.next_piece().await.unwrap(), b"Dialogue: 0");
        assert_eq!(reader.next_piece().await, None);
    }

    fn register(transcoder: &Transcoder, session: Arc<Session>) {
        transcoder
            .sessions
            .lock()
            .unwrap()
            .insert("known".to_owned(), session);
    }

    #[test]
    fn a_new_stream_replaces_the_one_before_it_under_the_same_playback() {
        let transcoder = Transcoder::default();
        let running = |playback: Option<&str>| {
            let mut session = session_writing_to(scratch_directory(), false);
            Arc::get_mut(&mut session).unwrap().request.playback = playback.map(str::to_owned);
            session
        };
        for (id, playback) in [
            ("old", Some("tab")),
            ("new", Some("tab")),
            ("elsewhere", Some("other")),
            ("unnamed", None),
        ] {
            transcoder
                .sessions
                .lock()
                .unwrap()
                .insert(id.to_owned(), running(playback));
        }

        assert_eq!(transcoder.stop_streams_of(Some("tab"), "new"), ["old"]);
        assert!(transcoder.session("old").is_none());
        for id in ["new", "elsewhere", "unnamed"] {
            assert!(transcoder.session(id).is_some(), "{id}");
        }
        assert!(transcoder.stop_streams_of(None, "new").is_empty());
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
