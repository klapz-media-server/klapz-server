//! A stream in progress: one supervised transcoder process and the segments it has written.
//!
//! The command line is the reference's, taken from its log. The process reads the source and
//! reports its segments over HTTP on loopback, as it does on Plex, at URLs holding an
//! unguessable per-session id: this also keeps the client's token out of the process list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::plan::{
    audio_decoder, language_tag, AudioAction, StreamPlan, Track, VideoAction, VideoEncode,
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

/// The length of a segment of copied video, as the reference asks for it.
const COPIED_DASH_SEGMENT_SECONDS: u32 = 5;
const COPIED_HLS_SEGMENT_SECONDS: u32 = 10;

/// How the result reaches the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Hls,
    Dash,
}

/// How long a session's segments are. An encode decides by its picture size.
#[must_use]
pub const fn segment_seconds(plan: &StreamPlan, protocol: Protocol) -> u32 {
    match (plan.video, protocol) {
        (VideoAction::Encode(encode), _) => encode.segment_seconds(),
        (VideoAction::Copy, Protocol::Dash) => COPIED_DASH_SEGMENT_SECONDS,
        (VideoAction::Copy, Protocol::Hls) => COPIED_HLS_SEGMENT_SECONDS,
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
    pub plan: StreamPlan,
    /// Where to start, in seconds.
    pub offset: Option<f64>,
    pub protocol: Protocol,
    /// The media's full duration in milliseconds, for the predicted HLS playlist; 0 if
    /// unknown.
    pub total_ms: u64,
    /// A subtitle to draw into the picture. The plan must then encode the video.
    pub subtitle: Option<BurnedSubtitle>,
}

/// A subtitle drawn into the picture.
#[derive(Debug, Clone, PartialEq)]
pub struct BurnedSubtitle {
    pub track: Track,
    /// The client's size, 1.0 for its 100%.
    pub scale: f64,
}

impl BurnedSubtitle {
    /// Text the process draws with libass. Image subtitles it overlays instead, by a command
    /// not captured yet.
    fn is_text(&self) -> bool {
        matches!(
            self.track.codec.as_str(),
            "srt" | "subrip" | "ass" | "ssa" | "mov_text" | "webvtt" | "vtt" | "text"
        )
    }
}

impl StreamRequest {
    /// Whether this asks for what a running session already produces. A player may change quality
    /// under the same session id.
    fn produces_the_same_as(&self, other: &Self) -> bool {
        self.source == other.source
            && self.video == other.video
            && self.audio == other.audio
            && self.plan == other.plan
            && self.protocol == other.protocol
            && self.subtitle == other.subtitle
    }

    const fn segment_seconds(&self) -> u32 {
        segment_seconds(&self.plan, self.protocol)
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
    seconds: f64,
    finished: bool,
}

#[derive(Debug, Default)]
struct Progress {
    segments: BTreeMap<u32, Announced>,
    /// The manifest the process wrote for DASH, once it holds the tracks.
    manifest: Option<String>,
    ended: bool,
}

/// The process currently working for a session.
#[derive(Debug)]
struct Run {
    /// Tells runs apart: one that was replaced must not report the session as finished.
    generation: u64,
    /// The first DASH chunk it writes, numbered as its files are.
    first_chunk: u32,
    /// When this process started, for the transcode speed a status reports.
    started: Instant,
    stop: Arc<Notify>,
    task: Option<JoinHandle<()>>,
}

#[derive(Debug)]
pub(crate) struct Session {
    callback: String,
    url: String,
    request: StreamRequest,
    directory: PathBuf,
    progress: Mutex<Progress>,
    changed: Notify,
    closed: AtomicBool,
    run: Mutex<Option<Run>>,
    /// Held while one process replaces another: the video and the audio of one seek both
    /// arrive here, and only the first may restart anything.
    restarting: tokio::sync::Mutex<()>,
    last_used: Mutex<Instant>,
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

    /// Waits until `ready` holds or the process has had long enough.
    async fn wait_until(&self, ready: impl Fn(&Progress) -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT_FOR_OUTPUT;
        loop {
            // Taken before checking, so a change arriving in between is not missed.
            let change = self.changed.notified();
            if self.with_progress(|progress| ready(progress) || progress.ended) {
                return;
            }
            if tokio::time::timeout_at(deadline, change).await.is_err() {
                return;
            }
        }
    }

    /// Waits for part of the output to exist. Whether it did in time.
    async fn wait_for(&self, exists: impl Fn() -> bool) -> bool {
        let began = tokio::time::Instant::now();
        loop {
            if exists() {
                return true;
            }
            if self.closed.load(Ordering::Acquire) {
                return false;
            }
            let limit = if self.with_progress(|progress| progress.ended) {
                HOLD_BEFORE_REFUSAL
            } else {
                WAIT_FOR_OUTPUT
            };
            if began.elapsed() >= limit {
                return false;
            }
            tokio::time::sleep(FILE_POLL).await;
        }
    }

    /// Waits for one of the files the process is expected to write.
    async fn wait_for_file(&self, paths: &[PathBuf]) -> Result<(), TranscodeError> {
        if self
            .wait_for(|| paths.iter().any(|path| path.exists()))
            .await
        {
            Ok(())
        } else {
            Err(TranscodeError::UnknownSegment)
        }
    }

    fn with_run<T>(&self, read: impl FnOnce(&mut Option<Run>) -> T) -> T {
        read(&mut self.run.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn chunk_path(&self, stream: u32, number: u32) -> PathBuf {
        self.directory
            .join(format!("chunk-stream{stream}-{number:05}.m4s"))
    }

    /// Whether the running process will reach this chunk soon without being told to.
    fn will_reach(&self, stream: u32, number: u32) -> bool {
        if self.with_progress(|progress| progress.ended) {
            return false;
        }
        let Some(first) = self.with_run(|run| run.as_ref().map(|run| run.first_chunk)) else {
            return false;
        };
        if number < first {
            return false;
        }
        // A chunk being written carries a `.tmp` suffix until it is whole.
        let in_progress = self
            .directory
            .join(format!("chunk-stream{stream}-{number:05}.m4s.tmp"))
            .exists();
        let margin = SEEK_MARGIN_SECONDS.div_ceil(self.request.segment_seconds());
        number == first
            || in_progress
            || (number.saturating_sub(margin).max(first)..number)
                .any(|earlier| self.chunk_path(stream, earlier).exists())
    }

    /// Whether the running process will reach this HLS segment soon without being told to.
    /// HLS muxes one `.ts` stream, numbered sequentially from the run's first segment.
    fn hls_will_reach(&self, number: u32) -> bool {
        if self.with_progress(|progress| progress.ended) {
            return false;
        }
        let Some(first) = self.with_run(|run| run.as_ref().map(|run| run.first_chunk)) else {
            return false;
        };
        if number < first {
            return false;
        }
        let margin = SEEK_MARGIN_SECONDS.div_ceil(self.request.segment_seconds());
        let frontier = self
            .with_progress(|progress| {
                progress
                    .segments
                    .iter()
                    .filter(|(_, segment)| segment.finished)
                    .map(|(number, _)| *number)
                    .max()
            })
            .map_or(first, |latest| latest.max(first));
        number <= frontier.saturating_add(margin)
    }

    fn output(&self) -> Output {
        let total_ms = self.request.total_ms;
        let segment_seconds = self.request.segment_seconds();
        self.with_progress(|progress| Output {
            segments: progress
                .segments
                .iter()
                .filter(|(_, segment)| segment.finished)
                .map(|(number, segment)| Segment {
                    number: *number,
                    seconds: segment.seconds,
                })
                .collect(),
            ended: progress.ended,
            total_ms,
            segment_seconds,
        })
    }

    /// How many segment files the process has finished writing. HLS segments and DASH chunks are
    /// named differently; a chunk still being written carries a `.tmp` suffix and is not counted.
    fn produced_segments(&self) -> usize {
        let (prefix, suffix) = match self.request.protocol {
            Protocol::Hls => ("media-", ".ts"),
            Protocol::Dash => ("chunk-stream0-", ".m4s"),
        };
        std::fs::read_dir(&self.directory).map_or(0, |entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with(prefix) && name.ends_with(suffix)
                })
                .count()
        })
    }
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
                    seconds: end - start,
                    finished,
                },
            ))
        })
        .collect()
}

fn hls_output(callback: &str, seconds: u32, first_chunk: u32) -> Vec<String> {
    [
        "-segment_format",
        "mpegts",
        "-f",
        "ssegment",
        "-individual_header_trailer",
        "0",
        "-segment_time",
        &seconds.to_string(),
        "-segment_start_number",
        &first_chunk.to_string(),
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
/// says where they are in time: 494 chunks of 5 seconds for 2467 seconds.
fn dash_output(callback: &str, seconds: u32, first_chunk: u32) -> Vec<String> {
    [
        "-f",
        "dash",
        "-seg_duration",
        &seconds.to_string(),
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
        &first_chunk.to_string(),
        "-manifest_name",
        &format!("{callback}/manifest?X-Plex-Http-Pipeline=infinite"),
        "-avoid_negative_ts",
        "disabled",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "dash",
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn input_arguments(request: &StreamRequest, callback: &str) -> Vec<String> {
    let StreamRequest {
        video,
        audio,
        plan,
        offset,
        protocol,
        ..
    } = request;
    let encodes = plan.video != VideoAction::Copy;
    let mut args = strings(&[&format!("-codec:{}", video.index), "h264"]);
    if plan.audio == AudioAction::ConvertToAac {
        args.extend(strings(&[
            &format!("-codec:{}", audio.index),
            audio_decoder(&audio.codec),
        ]));
    }
    // The reference always states it for copied DASH, otherwise only when there is one.
    let start = match protocol {
        _ if encodes => offset.filter(|seconds| *seconds > 0.0),
        Protocol::Dash => Some(offset.unwrap_or(0.0)),
        Protocol::Hls => *offset,
    };
    if let Some(start) = start {
        args.extend(strings(&["-ss", &start.to_string()]));
    }
    if !encodes {
        args.push("-noaccurate_seek".to_owned());
    }
    args.extend(strings(&[
        "-analyzeduration",
        "20000000",
        "-probesize",
        "20000000",
        "-i",
        &format!("{callback}/source"),
        "-start_at_zero",
        "-copyts",
    ]));
    if !encodes || start.is_none() {
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
        &format!("{callback}/progress"),
    ]));
    args
}

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
    args.extend(strings(&[
        "-preset:0",
        "veryfast",
        "-x264opts:0",
        encode.x264_options(),
        "-force_key_frames:0",
        &format!("expr:gte(t,n_forced*{})", encode.segment_seconds()),
    ]));
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
fn burn_filter(subtitle: &BurnedSubtitle, fonts: &Path) -> String {
    // The reference names the directory in Windows' verbatim form and the font without it.
    let font = fonts.join("NotoSans-Medium.otf");
    let font = font.to_string_lossy();
    let font = font.trim_start_matches(r"\\?\");
    let language = subtitle
        .track
        .language
        .as_deref()
        .map_or_else(String::new, |language| format!(":language={language}"));
    format!(
        "[1]inlineass=font_scale={:.6}:font_path={}:fonts_dir={}{language}:overrides={}:\
outline=2.6:shadow=1.7:font_size=54[2]",
        subtitle.scale,
        filter_value(font),
        filter_value(&fonts.to_string_lossy()),
        filter_value(SUBTITLE_STYLE),
    )
}

fn video_arguments(
    video: &Track,
    action: VideoAction,
    burn: Option<(&BurnedSubtitle, &Path)>,
) -> Vec<String> {
    let mut args = match action {
        VideoAction::Copy => strings(&["-map", &format!("0:{}", video.index)]),
        VideoAction::Encode(encode) => {
            let scale = format!(
                "[0:{}]scale=w={}:h={}:force_divisible_by=4[0];[0]format=pix_fmts=yuv420p|nv12[1]",
                video.index, encode.width, encode.height
            );
            match burn {
                Some((subtitle, fonts)) => strings(&[
                    "-map_inlineass",
                    &format!("0:{}", subtitle.track.index),
                    "-filter_complex",
                    &format!("{scale};{}", burn_filter(subtitle, fonts)),
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

fn audio_arguments(audio: &Track, plan: &StreamPlan, burns: bool) -> Vec<String> {
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
            if encodes { "aac" } else { "aac_mf" },
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
    callback: &str,
    first_chunk: u32,
    fonts: Option<&Path>,
) -> Vec<String> {
    let seconds = request.segment_seconds();
    let burn = request.subtitle.as_ref().zip(fonts);
    let mut args = input_arguments(request, callback);
    args.extend(video_arguments(&request.video, request.plan.video, burn));
    args.extend(audio_arguments(
        &request.audio,
        &request.plan,
        burn.is_some(),
    ));
    args.extend(match request.protocol {
        Protocol::Hls => hls_output(callback, seconds, first_chunk),
        Protocol::Dash => dash_output(callback, seconds, first_chunk),
    });
    if let Some((subtitle, _)) = burn {
        // The filter draws only what this output decodes alongside: without it, nothing.
        args.extend(strings(&[
            "-map",
            &format!("0:{}", subtitle.track.index),
            "-f",
            "null",
            "-codec",
            "ass",
            "nullfile",
        ]));
    }
    args
}

fn sessions_root() -> PathBuf {
    std::env::temp_dir().join("open-plex-transcode")
}

/// Moves a DASH start onto the chunk grid and says which chunk that is. A chunk's number has to
/// say where it is in time, so the process must start on one.
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
    // DASH numbers its chunks from one, HLS its segments from zero.
    match request.protocol {
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
        match std::fs::remove_dir_all(sessions_root()) {
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
            if !subtitle.is_text() || self.config.fonts.is_none() {
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
        let directory = sessions_root().join(&callback);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(TranscodeError::Io)?;

        let first_chunk = align_to_chunk(&mut request);
        let url = format!(
            "{}/video/:/transcode/session/{callback}/{callback}",
            self.config.callback_base
        );
        let key = request.session.clone();
        let session = Arc::new(Session {
            callback,
            url,
            request: request.clone(),
            directory,
            progress: Mutex::default(),
            changed: Notify::new(),
            closed: AtomicBool::new(false),
            run: Mutex::new(None),
            restarting: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
        });
        // Registered before the process exists: its first act is to read its source back.
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), Arc::clone(&session));

        if let Err(error) = self.launch(&session, &request, first_chunk) {
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

    /// Starts a process for this session, which must have none running.
    fn launch(
        &self,
        session: &Arc<Session>,
        request: &StreamRequest,
        first_chunk: u32,
    ) -> Result<(), TranscodeError> {
        let mut command = Command::new(&self.config.binary);
        command
            .args(stream_arguments(
                request,
                &session.url,
                first_chunk,
                self.config.fonts.as_deref(),
            ))
            .current_dir(&session.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(codecs) = &self.config.external_libs {
            command.env("FFMPEG_EXTERNAL_LIBS", external_libs_value(codecs));
        }
        let mut child = command.spawn().map_err(TranscodeError::Spawn)?;

        let stop = Arc::new(Notify::new());
        let generation = session.with_run(|run| {
            let generation = run.as_ref().map_or(0, |previous| previous.generation + 1);
            *run = Some(Run {
                generation,
                first_chunk,
                started: Instant::now(),
                stop: Arc::clone(&stop),
                task: None,
            });
            generation
        });
        session.with_progress(|progress| progress.ended = false);

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
            let current = supervised
                .with_run(|run| run.as_ref().is_some_and(|run| run.generation == generation));
            if current {
                supervised.with_progress(|progress| progress.ended = true);
                supervised.changed.notify_waiters();
            }
        });
        session.with_run(|run| {
            if let Some(run) = run.as_mut().filter(|run| run.generation == generation) {
                run.task = Some(task);
            }
        });
        Ok(())
    }

    /// Replaces the session's process with one starting at this DASH chunk. What is already
    /// written stays, so going back over it costs nothing.
    async fn seek(
        &self,
        session: &Arc<Session>,
        stream: u32,
        number: u32,
    ) -> Result<(), TranscodeError> {
        let _exclusive = session.restarting.lock().await;
        // The other track of the same seek may have done this while we waited.
        if session.closed.load(Ordering::Acquire) || session.will_reach(stream, number) {
            return Ok(());
        }
        let offset = (number - 1) * session.request.segment_seconds();
        self.relaunch(session, number, offset).await
    }

    /// Replaces the session's process with one starting at this HLS segment, which the running
    /// process is too far behind to reach on its own.
    async fn seek_hls(&self, session: &Arc<Session>, number: u32) -> Result<(), TranscodeError> {
        let _exclusive = session.restarting.lock().await;
        if session.closed.load(Ordering::Acquire) || session.hls_will_reach(number) {
            return Ok(());
        }
        let offset = number * session.request.segment_seconds();
        self.relaunch(session, number, offset).await
    }

    /// Stops the running process and starts a new one numbering its output from `first_chunk`
    /// and seeking the source to `offset_seconds`. The caller holds `restarting` and has
    /// checked the segment is not coming on its own.
    async fn relaunch(
        &self,
        session: &Arc<Session>,
        first_chunk: u32,
        offset_seconds: u32,
    ) -> Result<(), TranscodeError> {
        let previous = session.with_run(|run| {
            run.as_mut()
                .map(|run| (Arc::clone(&run.stop), run.task.take()))
        });
        if let Some((stop, task)) = previous {
            stop.notify_one();
            if let Some(task) = task {
                if let Err(error) = task.await {
                    tracing::warn!(%error, "a transcoder's supervisor failed");
                }
            }
        }
        let mut request = session.request.clone();
        request.offset = Some(f64::from(offset_seconds));
        self.launch(session, &request, first_chunk)
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
            let stop = session.with_run(|run| run.as_ref().map(|run| Arc::clone(&run.stop)));
            if let Some(stop) = stop {
                stop.notify_one();
            }
        }
        session.is_some()
    }

    /// Whether the session exists; keeps it alive if so.
    pub fn ping_session(&self, id: &str) -> bool {
        self.session(id).is_some()
    }

    /// The segments finished so far, once there are enough to start on.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`].
    pub async fn session_output(&self, id: &str) -> Result<Output, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        session
            .wait_until(|progress| {
                progress
                    .segments
                    .values()
                    .filter(|segment| segment.finished)
                    .count()
                    >= SEGMENTS_BEFORE_PLAYLIST
            })
            .await;
        Ok(session.output())
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
        let seconds = request.segment_seconds();
        let (protocol, container) = match request.protocol {
            Protocol::Dash => ("dash", "mp4"),
            Protocol::Hls => ("hls", "mpegts"),
        };
        let video_decision = match request.plan.video {
            VideoAction::Copy => "copy",
            VideoAction::Encode(_) => "transcode",
        };
        let video_codec = match request.plan.video {
            VideoAction::Copy => request.video.codec.clone(),
            VideoAction::Encode(_) => "h264".to_owned(),
        };
        let (audio_decision, audio_codec, audio_channels) = match request.plan.audio {
            AudioAction::Copy => ("copy", request.audio.codec.clone(), None),
            AudioAction::ConvertToAac => ("transcode", "aac".to_owned(), Some(2)),
        };
        let ended = session.with_progress(|progress| progress.ended);
        let count = u32::try_from(session.produced_segments()).unwrap_or(u32::MAX);
        let produced = f64::from(count) * f64::from(seconds);
        let total_seconds = request.total_ms as f64 / 1000.0;
        let elapsed = session
            .with_run(|run| run.as_ref().map(|run| run.started.elapsed().as_secs_f64()))
            .unwrap_or(0.0);
        let speed = if elapsed > 0.0 {
            produced / elapsed
        } else {
            0.0
        };
        let progress = if total_seconds > 0.0 {
            (produced / total_seconds * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let remaining = if speed > 0.0 {
            ((total_seconds - produced) / speed).max(0.0) as i64
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
            duration_ms: request.total_ms,
            max_offset_seconds: produced,
            min_offset_seconds: 0.0,
            progress: round(progress),
            speed: round(speed),
            remaining_seconds: remaining,
            complete: ended,
        })
    }

    /// A finished segment's file, waiting for it if it is still being written.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`], or [`TranscodeError::UnknownSegment`] if it never
    /// appears.
    pub async fn session_segment(&self, id: &str, number: u32) -> Result<PathBuf, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        let present = || {
            session.with_progress(|progress| {
                progress.segments.get(&number).is_some_and(|s| s.finished)
            })
        };
        // Not written yet, and the running process is too far behind to reach it soon: restart
        // there, as a far forward seek needs. Without this the request only waits, then refuses a
        // segment the process was never going to produce. A process that has already ended is held
        // then refused rather than restarted, so a player does not drive an endless relaunch.
        let running = || !session.with_progress(|progress| progress.ended);
        if !present() && running() && !session.hls_will_reach(number) {
            self.seek_hls(&session, number).await?;
        }
        if session.wait_for(present).await {
            Ok(session.directory.join(format!("media-{number:05}.ts")))
        } else {
            Err(TranscodeError::UnknownSegment)
        }
    }

    /// The manifest the process wrote for a DASH session, waiting for it.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`], or [`TranscodeError::UnknownSegment`] if the process
    /// ended without writing one.
    pub async fn session_manifest(&self, id: &str) -> Result<String, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        session
            .wait_until(|progress| progress.manifest.is_some())
            .await;
        session
            .with_progress(|progress| progress.manifest.clone())
            .ok_or(TranscodeError::UnknownSegment)
    }

    /// A DASH stream's initialization segment, waiting for it.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`], or [`TranscodeError::UnknownSegment`] if it never
    /// appears.
    pub async fn dash_header(&self, id: &str, stream: u32) -> Result<PathBuf, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        // Unlike a chunk it has no temporary name: it is whole once the first chunk is begun.
        let first = session.with_run(|run| run.as_ref().map_or(1, |run| run.first_chunk));
        let finished = session.chunk_path(stream, first);
        let begun = finished.with_extension("m4s.tmp");
        session.wait_for_file(&[begun, finished]).await?;
        Ok(session.directory.join(format!("init-stream{stream}.m4s")))
    }

    /// A finished DASH chunk. `number` counts from zero, as the manifest given to the player
    /// does. One the running process will not get to soon is a seek, and restarts it there.
    ///
    /// # Errors
    ///
    /// As [`Self::dash_header`].
    pub async fn dash_chunk(
        &self,
        id: &str,
        stream: u32,
        number: u32,
    ) -> Result<PathBuf, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        // The process numbers its files from one, and renames each only once it is whole.
        let number = number + 1;
        let path = session.chunk_path(stream, number);
        if path.exists() {
            return Ok(path);
        }
        if !session.will_reach(stream, number) {
            self.seek(&session, stream, number).await?;
        }
        session.wait_for_file(std::slice::from_ref(&path)).await?;
        Ok(path)
    }

    /// The source of the session behind a callback URL, for the process to read.
    #[must_use]
    pub fn callback_source(&self, callback: &str) -> Option<PathBuf> {
        Some(self.by_callback(callback)?.request.source.clone())
    }

    /// Records the process's announcement of its segments. Whether the callback is known.
    #[must_use]
    pub fn announce_segments(&self, callback: &str, body: &str) -> bool {
        let Some(session) = self.by_callback(callback) else {
            return false;
        };
        session.with_progress(|progress| {
            progress.segments.extend(parse_announcement(body));
            if body.contains("<Representation") {
                progress.manifest = Some(body.to_owned());
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

    fn request(audio: AudioAction) -> StreamRequest {
        StreamRequest {
            session: "s".to_owned(),
            playback: None,
            source: PathBuf::from("x"),
            video: track(0, "h264", "en"),
            audio: track(1, "dca", "fr"),
            plan: StreamPlan {
                video: VideoAction::Copy,
                audio,
            },
            offset: None,
            protocol: Protocol::Hls,
            total_ms: 0,
            subtitle: None,
        }
    }

    #[test]
    fn writes_the_references_command_line() {
        let args = stream_arguments(&request(AudioAction::ConvertToAac), "CB", 0, None);
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
        let joined = stream_arguments(&asked, "CB", 1, None)
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
        asked.plan.video = VideoAction::Encode(VideoEncode {
            width,
            height,
            video_kbps,
            audio_kbps,
            crf,
            frame_rate: Some(25.0),
            adaptive: false,
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
        asked.subtitle = Some(BurnedSubtitle {
            track: track(3, "srt", "fr"),
            scale,
        });
        asked
    }

    #[cfg(windows)]
    #[test]
    fn burns_a_subtitle_with_the_references_filter() {
        let fonts = Path::new(r"\\?\C:\Program Files\Plex\Plex Media Server\Resources\Fonts");
        let args = stream_arguments(&burned(1.0), "CB", 1, Some(fonts));
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
        let args = stream_arguments(&burned(1.5), "CB", 1, Some(Path::new("fonts")));
        assert!(args
            .iter()
            .any(|arg| arg.contains("inlineass=font_scale=1.500000:")));
        // Without fonts there is nothing to draw with, and the command is an encode's.
        let plain = stream_arguments(&burned(1.0), "CB", 1, None).join(" ");
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
        let joined = stream_arguments(&encoded(720, 388, 1333, 167, 20), "CB", 1, None)
            .join(" ")
            .replace("[0:1] aresample", "[0:1]_aresample");
        assert_eq!(joined, CAPTURED_ENCODE);
    }

    /// From the reference's log, started at 600 seconds: an exact seek, and no `-fps_mode`.
    #[test]
    fn an_encode_seeks_as_the_reference_does() {
        let mut asked = encoded(1280, 692, 2566, 98, 24);
        asked.offset = Some(600.0);
        let joined = stream_arguments(&asked, "CB", 121, None).join(" ");
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
        assert_eq!(align_to_chunk(&mut asked), 76);
        assert_eq!(asked.offset, Some(600.0));
    }

    /// Measured on the transcoder itself: chunk 121 of a 5-second grid starts at 600 seconds.
    #[test]
    fn a_seek_names_its_time_and_its_chunk_together() {
        let mut asked = request(AudioAction::Copy);
        asked.protocol = Protocol::Dash;
        asked.offset = Some(600.0);
        let joined = stream_arguments(&asked, "CB", 121, None).join(" ");
        assert!(joined.contains("-ss 600 -noaccurate_seek"));
        assert!(joined.contains("-skip_to_segment 121 "));
    }

    #[test]
    fn a_dash_start_lands_on_the_chunk_grid() {
        let mut asked = request(AudioAction::Copy);
        asked.protocol = Protocol::Dash;
        asked.offset = Some(603.0);
        assert_eq!(align_to_chunk(&mut asked), 121);
        assert_eq!(asked.offset, Some(600.0));

        asked.offset = None;
        assert_eq!(align_to_chunk(&mut asked), 1);
        assert_eq!(asked.offset, Some(0.0));
    }

    #[test]
    fn an_hls_start_lands_on_the_chunk_grid() {
        let mut asked = request(AudioAction::Copy);
        asked.offset = Some(603.0);
        assert_eq!(align_to_chunk(&mut asked), 60);
        assert_eq!(asked.offset, Some(600.0));

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
        let joined = stream_arguments(&asked, "CB", 300, None).join(" ");
        assert!(joined.contains("-ss 3000 -noaccurate_seek"));
        assert!(joined.contains("-segment_start_number 300 "));
    }

    #[test]
    fn copied_audio_is_neither_decoded_nor_filtered() {
        let joined = stream_arguments(&request(AudioAction::Copy), "CB", 1, None).join(" ");
        assert!(!joined.contains("-codec:1 dca"));
        assert!(!joined.contains("aresample"));
        assert!(joined.contains("-map 0:1 -metadata:s:1 language=fra -codec:1 copy"));
    }

    #[test]
    fn follows_the_tracks_where_the_file_puts_them() {
        let mut asked = request(AudioAction::Copy);
        asked.video.index = 5;
        asked.audio.index = 0;
        let joined = stream_arguments(&asked, "CB", 1, None).join(" ");
        assert!(joined.starts_with("-codec:5 h264"));
        assert!(joined.contains("-map 0:5 "));
        assert!(joined.contains("-map 0:0 "));
    }

    #[test]
    fn seeks_before_opening_the_input() {
        let mut asked = request(AudioAction::Copy);
        asked.offset = Some(90.0);
        let joined = stream_arguments(&asked, "CB", 1, None).join(" ");
        assert!(joined.contains("-ss 90 -noaccurate_seek -analyzeduration"));
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
        assert!(!parsed[1].1.finished);
    }

    #[test]
    fn ignores_what_is_not_a_segment_line() {
        assert!(parse_announcement("").is_empty());
        assert!(parse_announcement("garbage,1,2").is_empty());
        assert!(parse_announcement("other-00000.ts,0,1").is_empty());
    }

    fn session_writing_to(directory: PathBuf, ended: bool) -> Arc<Session> {
        std::fs::create_dir_all(&directory).unwrap();
        Arc::new(Session {
            callback: "callback".to_owned(),
            url: String::new(),
            request: request(AudioAction::Copy),
            directory,
            progress: Mutex::new(Progress {
                ended,
                ..Progress::default()
            }),
            changed: Notify::new(),
            closed: AtomicBool::new(false),
            run: Mutex::new(None),
            restarting: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
        })
    }

    fn scratch_directory() -> PathBuf {
        std::env::temp_dir().join(format!("open-plex-test-{}", uuid::Uuid::new_v4().simple()))
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
        for refused in [
            transcoder.dash_chunk("missing", 0, 0).await,
            transcoder.dash_header("missing", 0).await,
            transcoder.session_segment("missing", 0).await,
        ] {
            assert!(matches!(refused, Err(TranscodeError::UnknownSession)));
        }
        assert_eq!(began.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_ends_as_soon_as_the_session_is_stopped() {
        let session = session_writing_to(scratch_directory(), false);
        let missing = session.chunk_path(0, 1);
        let stopped = Arc::clone(&session);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            stopped.closed.store(true, Ordering::Release);
        });
        let began = tokio::time::Instant::now();
        assert!(session.wait_for_file(&[missing]).await.is_err());
        assert!(began.elapsed() < HOLD_BEFORE_REFUSAL);
    }

    #[tokio::test(start_paused = true)]
    async fn a_segment_of_an_ended_process_is_held_before_being_refused() {
        let transcoder = Transcoder::default();
        register(&transcoder, session_writing_to(scratch_directory(), true));
        let began = tokio::time::Instant::now();
        let refused = transcoder.session_segment("known", 3).await;
        assert!(matches!(refused, Err(TranscodeError::UnknownSegment)));
        assert!(began.elapsed() >= HOLD_BEFORE_REFUSAL);
        assert!(began.elapsed() < WAIT_FOR_OUTPUT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_running_process_gets_the_full_wait() {
        let session = session_writing_to(scratch_directory(), false);
        let missing = session.chunk_path(0, 1);
        let began = tokio::time::Instant::now();
        assert!(session.wait_for_file(&[missing]).await.is_err());
        assert!(began.elapsed() >= WAIT_FOR_OUTPUT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_file_arriving_during_the_hold_is_served() {
        let session = session_writing_to(scratch_directory(), true);
        let path = session.chunk_path(0, 1);
        let written = path.clone();
        tokio::spawn(async move {
            tokio::time::sleep(HOLD_BEFORE_REFUSAL / 2).await;
            std::fs::write(written, b"chunk").unwrap();
        });
        let began = tokio::time::Instant::now();
        session.wait_for_file(&[path]).await.unwrap();
        assert!(began.elapsed() < HOLD_BEFORE_REFUSAL);
    }
}
