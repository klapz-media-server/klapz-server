//! A remux in progress: one supervised transcoder process and the segments it has written.
//!
//! The command line is the reference's, taken from its log. The process reads the source and
//! reports its segments over HTTP on loopback, as it does on Plex, at URLs holding an
//! unguessable per-session id: this also keeps the client's token out of the process list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::Notify;

use crate::plan::{audio_decoder, language_tag, AudioAction, Track, AUDIO_BITRATE_KBPS};
use crate::{TranscodeError, Transcoder};

/// Each session is a process and a directory of segments, and a client controls how many.
const MAX_SESSIONS: usize = 4;

/// A client that stopped asking has gone away without saying so.
const IDLE_LIFETIME: Duration = Duration::from_secs(300);

/// How long a request for a playlist or a segment waits for the process to reach it.
const WAIT_FOR_OUTPUT: Duration = Duration::from_secs(20);

/// Enough for a player to start without running dry.
const SEGMENTS_BEFORE_PLAYLIST: usize = 3;

#[derive(Debug, Clone)]
pub struct RemuxRequest {
    /// The client's session identifier, which later requests name it by.
    pub session: String,
    /// The file, already checked to lie within a library.
    pub source: PathBuf,
    pub video: Track,
    pub audio: Track,
    pub audio_action: AudioAction,
    /// Where to start, in seconds.
    pub offset: Option<f64>,
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
}

#[derive(Debug, Clone, Copy)]
struct Announced {
    seconds: f64,
    finished: bool,
}

#[derive(Debug, Default)]
struct Progress {
    segments: BTreeMap<u32, Announced>,
    ended: bool,
}

#[derive(Debug)]
pub(crate) struct Session {
    callback: String,
    source: PathBuf,
    directory: PathBuf,
    progress: Mutex<Progress>,
    changed: Notify,
    stop: Notify,
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

    fn output(&self) -> Output {
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

fn segmenter_arguments(callback: &str) -> Vec<String> {
    [
        "-segment_format",
        "mpegts",
        "-f",
        "ssegment",
        "-individual_header_trailer",
        "0",
        "-segment_time",
        "10",
        "-segment_start_number",
        "0",
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

fn remux_arguments(request: &RemuxRequest, callback: &str) -> Vec<String> {
    let RemuxRequest {
        video,
        audio,
        audio_action,
        offset,
        ..
    } = request;
    let mut args: Vec<String> = Vec::new();
    let mut push = |values: &[&str]| args.extend(values.iter().map(ToString::to_string));
    let converts = *audio_action == AudioAction::ConvertToAac;

    push(&[&format!("-codec:{}", video.index), "h264"]);
    if converts {
        push(&[
            &format!("-codec:{}", audio.index),
            audio_decoder(&audio.codec),
        ]);
    }
    push(&["-noaccurate_seek"]);
    if let Some(offset) = offset {
        push(&["-ss", &format!("{offset:.3}")]);
    }
    push(&[
        "-analyzeduration",
        "20000000",
        "-probesize",
        "20000000",
        "-i",
        &format!("{callback}/source"),
        "-start_at_zero",
        "-copyts",
        "-fps_mode",
        "cfr",
        "-y",
        "-nostats",
        "-loglevel",
        "quiet",
        "-loglevel_plex",
        "error",
        "-progressurl",
        &format!("{callback}/progress"),
        "-map",
        &format!("0:{}", video.index),
    ]);
    if let Some(tag) = video.language.as_deref().and_then(language_tag) {
        push(&["-metadata:s:0", &format!("language={tag}")]);
    }
    push(&["-codec:0", "copy"]);

    if converts {
        push(&[
            "-filter_complex",
            &format!(
                "[0:{}] aresample=async=1:ochl='stereo':rematrix_maxval=0.000000dB:osr=48000[0]",
                audio.index
            ),
            "-map",
            "[0]",
        ]);
    } else {
        push(&["-map", &format!("0:{}", audio.index)]);
    }
    if let Some(tag) = audio.language.as_deref().and_then(language_tag) {
        push(&["-metadata:s:1", &format!("language={tag}")]);
    }
    if converts {
        push(&[
            "-codec:1",
            "aac_mf",
            "-b:1",
            &format!("{AUDIO_BITRATE_KBPS}k"),
        ]);
    } else {
        push(&["-codec:1", "copy"]);
    }

    args.extend(segmenter_arguments(callback));
    args
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

    /// Starts a session, or does nothing if the client's is already running: a player asks
    /// for the same start more than once.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::TooManySessions`] at the limit, or if the process cannot be started.
    pub async fn start_remux(&self, request: RemuxRequest) -> Result<(), TranscodeError> {
        self.stop_idle_sessions();
        if self.session(&request.session).is_some() {
            return Ok(());
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

        let callback_id = uuid::Uuid::new_v4().simple().to_string();
        let directory = std::env::temp_dir()
            .join("open-plex-transcode")
            .join(&callback_id);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(TranscodeError::Io)?;

        let url = format!(
            "{}/video/:/transcode/session/{}/{callback_id}",
            self.config.callback_base, callback_id
        );
        let mut command = Command::new(&self.config.binary);
        command
            .args(remux_arguments(&request, &url))
            .current_dir(&directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(codecs) = &self.config.external_libs {
            command.env("FFMPEG_EXTERNAL_LIBS", external_libs_value(codecs));
        }

        let session = Arc::new(Session {
            callback: callback_id,
            source: request.source,
            directory,
            progress: Mutex::default(),
            changed: Notify::new(),
            stop: Notify::new(),
            last_used: Mutex::new(Instant::now()),
        });
        // Registered before the process exists: its first act is to read its source back.
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request.session.clone(), Arc::clone(&session));

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.sessions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&request.session);
                return Err(TranscodeError::Spawn(error));
            }
        };

        tokio::spawn(async move {
            tokio::select! {
                result = child.wait() => {
                    if let Err(error) = result {
                        tracing::warn!(%error, "lost track of a transcoder process");
                    }
                }
                () = session.stop.notified() => {
                    if let Err(error) = child.kill().await {
                        tracing::warn!(%error, "could not kill a transcoder process");
                    }
                }
            }
            session.with_progress(|progress| progress.ended = true);
            session.changed.notify_waiters();
        });
        Ok(())
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
            session.stop.notify_one();
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

    /// A finished segment's file, waiting for it if it is still being written.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::UnknownSession`], or [`TranscodeError::UnknownSegment`] if it never
    /// appears.
    pub async fn session_segment(&self, id: &str, number: u32) -> Result<PathBuf, TranscodeError> {
        let session = self.session(id).ok_or(TranscodeError::UnknownSession)?;
        let finished =
            |progress: &Progress| progress.segments.get(&number).is_some_and(|s| s.finished);
        session.wait_until(finished).await;
        if session.with_progress(|progress| finished(progress)) {
            Ok(session.directory.join(format!("media-{number:05}.ts")))
        } else {
            Err(TranscodeError::UnknownSegment)
        }
    }

    /// The source of the session behind a callback URL, for the process to read.
    #[must_use]
    pub fn callback_source(&self, callback: &str) -> Option<PathBuf> {
        Some(self.by_callback(callback)?.source.clone())
    }

    /// Records the process's announcement of its segments. Whether the callback is known.
    #[must_use]
    pub fn announce_segments(&self, callback: &str, body: &str) -> bool {
        let Some(session) = self.by_callback(callback) else {
            return false;
        };
        session.with_progress(|progress| progress.segments.extend(parse_announcement(body)));
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

    fn request(audio_action: AudioAction) -> RemuxRequest {
        RemuxRequest {
            session: "s".to_owned(),
            source: PathBuf::from("x"),
            video: track(0, "h264", "en"),
            audio: track(1, "dca", "fr"),
            audio_action,
            offset: None,
        }
    }

    #[test]
    fn writes_the_references_command_line() {
        let args = remux_arguments(&request(AudioAction::ConvertToAac), "CB");
        // The space inside the filter is what makes it one argument here and several there.
        let joined = args.join(" ").replace("[0:1] aresample", "[0:1]_aresample");
        assert_eq!(joined, CAPTURED);
    }

    #[test]
    fn copied_audio_is_neither_decoded_nor_filtered() {
        let joined = remux_arguments(&request(AudioAction::Copy), "CB").join(" ");
        assert!(!joined.contains("-codec:1 dca"));
        assert!(!joined.contains("aresample"));
        assert!(joined.contains("-map 0:1 -metadata:s:1 language=fra -codec:1 copy"));
    }

    #[test]
    fn follows_the_tracks_where_the_file_puts_them() {
        let mut asked = request(AudioAction::Copy);
        asked.video.index = 5;
        asked.audio.index = 0;
        let joined = remux_arguments(&asked, "CB").join(" ");
        assert!(joined.starts_with("-codec:5 h264"));
        assert!(joined.contains("-map 0:5 "));
        assert!(joined.contains("-map 0:0 "));
    }

    #[test]
    fn seeks_before_opening_the_input() {
        let mut asked = request(AudioAction::Copy);
        asked.offset = Some(90.0);
        let joined = remux_arguments(&asked, "CB").join(" ");
        assert!(joined.contains("-noaccurate_seek -ss 90.000 -analyzeduration"));
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
}
