//! Streams at addresses of their own: `/streams/{id}/…?token=…`.
//!
//! The engine behind `startStream`, over the transcoder, and the routes a player reads a
//! stream from. Those routes take no account token. Each stream has an id and a secret of its
//! own, handed out once by the mutation, and a wrong secret answers as if the stream did not
//! exist. Every URL in a manifest carries the secret, because a relative reference would drop
//! the query, and every one is an absolute path, so that it resolves the same whether or not
//! the player followed a redirect to the manifest.
//!
//! A stream lives as long as its playback session, and reading it is a sign of life for that
//! session: at most every `TOUCH_INTERVAL`, a read pushes the session's expiry forward, and a
//! read whose session has gone stops the stream there and then.
//!
//! A stream's renditions are numbered from 0, the best: one, unless the player chooses its
//! quality. `/streams/{id}/{rendition}/…` holds a rendition's HLS playlist and segments, or its
//! DASH video; `/streams/{id}/audio/…` the DASH audio every rendition shares;
//! `/streams/{id}/subtitles.{vtt,ass,srt,ttml}` the subtitle, whole.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::StreamExt as _;
use klapz_core::lifecycle::{Lifecycle, SessionLocks};
use klapz_core::playback::{PlaybackOutput, PlaybackRepository, StreamDecision};
use klapz_core::sidecar::{SidecarError, SidecarSource};
use klapz_core::stream::SubtitleFormat as PlayerSubtitleFormat;
use klapz_core::stream::{
    Delivery, FallbackRefusal, StartedStream, StreamEngine, StreamError, StreamOrder,
    StreamProtocol, StreamQuality, StreamTrack, SubtitleOutput,
};
use klapz_transcoder::adapt;
use klapz_transcoder::media::{read_sidecar, resolve_sidecar, resolve_within};
use klapz_transcoder::plan::{
    plan_ladder, Adaptation, AudioAction, CopyCodecs, Ladder, Limits, StreamPlan, StreamSource,
    Track, VideoAction,
};
use klapz_transcoder::session::{DashTrack, Output, Protocol, StreamRequest, SubtitleDelivery};
use klapz_transcoder::subtitle::SubtitleFormat;
use klapz_transcoder::{TranscodeError, Transcoder};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tokio::io::AsyncReadExt as _;
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

/// How often reading a stream counts as a sign of life for its session. A player reads a
/// segment every few seconds; one write to the sessions per segment would be for nothing.
const TOUCH_INTERVAL: Duration = Duration::from_secs(30);

/// Streams kept at once. Each start makes one, and a client calling in a loop must not grow
/// the table without end: past this the least recently read goes.
const MAX_STREAMS: usize = 1000;

/// The codecs an encode is announced with before the process has written anything, as the
/// reference announces them.
const ENCODED_VIDEO_CODECS: &str = "avc1.640028";

#[derive(Debug)]
pub struct Streams {
    transcoder: Arc<Transcoder>,
    entries: Mutex<HashMap<String, Entry>>,
}

#[derive(Debug)]
struct Entry {
    /// The transcoder session, for a conversion; the stored name of the playback otherwise.
    key: String,
    token: String,
    content: Content,
    read_at: Instant,
    /// When a read last counted as a sign of life for the session.
    touched_at: Instant,
}

#[derive(Debug, Clone)]
enum Content {
    File(PathBuf),
    Converted(Arc<Conversion>),
}

/// What a manifest announces of a conversion.
#[derive(Debug)]
struct Conversion {
    protocol: Protocol,
    ladder: Ladder,
    /// One per rendition, best first.
    pictures: Vec<Picture>,
    audio_bandwidth: i64,
    duration_ms: i64,
    frame_rate: Option<f64>,
    subtitle: Option<TextSubtitle>,
}

/// A subtitle beside a conversion, text or pictures as TTML.
#[derive(Debug, Clone)]
struct TextSubtitle {
    format: PlayerSubtitleFormat,
    /// As stored, `fr`: what a DASH manifest's `lang` takes.
    language: Option<String>,
}

impl TextSubtitle {
    fn of(output: &SubtitleOutput) -> Option<Self> {
        match output {
            SubtitleOutput::Text { track, format } => Some(Self {
                format: *format,
                language: track.language.clone(),
            }),
            SubtitleOutput::Burn { .. } => None,
        }
    }

    fn file_name(&self) -> String {
        format!("subtitles.{}", subtitle_format(self.format).extension())
    }

    const fn mime_type(&self) -> &'static str {
        subtitle_format(self.format).mime_type()
    }
}

impl Conversion {
    /// What the player reads as separate representations: every rung when it chooses, the
    /// first when the server moves between them under one.
    fn renditions(&self) -> &[Picture] {
        match self.ladder.adaptation {
            Adaptation::Client => &self.pictures,
            Adaptation::Fixed | Adaptation::Server => &self.pictures[..1],
        }
    }
}

/// The video as it reaches the player: the file's, unless it is being re-encoded.
#[derive(Debug, Clone, Copy)]
struct Picture {
    video_bandwidth: i64,
    width: i64,
    height: i64,
    copied: bool,
}

/// The namespace of the attribute that tells, on each DASH representation, whether its track is
/// the file's or re-encoded: the schema admits foreign attributes, which players ignore.
const DELIVERY_NAMESPACE: &str = "urn:klapz:stream:delivery";

/// How a track reaches the player, as the manifests tell it for a client to show.
const fn delivery(copied: bool) -> &'static str {
    if copied {
        "remux"
    } else {
        "transcode"
    }
}

impl Picture {
    fn of(source: &klapz_core::stream::StreamSource, plan: &StreamPlan) -> Self {
        match plan.video {
            VideoAction::Copy => Self {
                video_bandwidth: i64::from(source.bitrate.unwrap_or(0)),
                width: i64::from(source.width.unwrap_or(0)),
                height: i64::from(source.height.unwrap_or(0)),
                copied: true,
            },
            VideoAction::Encode(encode) => Self {
                video_bandwidth: i64::from(encode.video_kbps) * 1000,
                width: i64::from(encode.width),
                height: i64::from(encode.height),
                copied: false,
            },
        }
    }
}

fn track(track: &StreamTrack) -> Track {
    Track {
        index: track.position,
        codec: track.codec.clone(),
        language: track.language.clone(),
    }
}

fn subtitle_delivery(subtitle: &SubtitleOutput) -> SubtitleDelivery {
    match subtitle {
        SubtitleOutput::Burn {
            track: burned,
            scale,
        } => SubtitleDelivery::Burn {
            track: track(burned),
            scale: *scale,
        },
        SubtitleOutput::Text {
            track: text,
            format,
        } => SubtitleDelivery::Text {
            track: track(text),
            format: subtitle_format(*format),
        },
    }
}

/// Each subtitle track of the file and the format it would reach this player in, for the
/// transcoder to extract those it can before the player switches to one.
fn text_subtitles(order: &StreamOrder) -> Vec<(Track, SubtitleFormat)> {
    order
        .source
        .subtitles
        .iter()
        .filter_map(|subtitle| {
            PlayerSubtitleFormat::for_track(&subtitle.codec, &order.subtitle_formats)
                .map(|format| (track(subtitle), subtitle_format(format)))
        })
        .collect()
}

const fn subtitle_format(format: PlayerSubtitleFormat) -> SubtitleFormat {
    match format {
        PlayerSubtitleFormat::Ass => SubtitleFormat::Ass,
        PlayerSubtitleFormat::WebVtt => SubtitleFormat::WebVtt,
        PlayerSubtitleFormat::Srt => SubtitleFormat::Srt,
        PlayerSubtitleFormat::Ttml => SubtitleFormat::Ttml,
    }
}

const fn protocol_of(protocol: StreamProtocol) -> Protocol {
    match protocol {
        StreamProtocol::Dash => Protocol::Dash,
        StreamProtocol::Hls => Protocol::Hls,
    }
}

const fn adaptation_of(quality: StreamQuality) -> Adaptation {
    match quality {
        StreamQuality::Fixed => Adaptation::Fixed,
        StreamQuality::ServerAdaptive => Adaptation::Server,
        StreamQuality::ClientAdaptive => Adaptation::Client,
    }
}

fn ladder_limits(order: &StreamOrder) -> Limits {
    Limits {
        max_video_kbps: order.limits.max_video_bitrate.map(|bits| bits / 1000),
        convert_audio: order.limits.convert_audio,
        adaptation: adaptation_of(order.limits.quality),
        burn_subtitle: matches!(order.subtitle, Some(SubtitleOutput::Burn { .. })),
        copy_codecs: order
            .limits
            .video_codecs
            .as_deref()
            .map_or(CopyCodecs::ANY, |names| {
                CopyCodecs::named(names.iter().map(String::as_str))
            }),
    }
}

fn stream_error(error: TranscodeError) -> StreamError {
    match error {
        TranscodeError::MissingFile => StreamError::MissingFile,
        TranscodeError::VideoNeedsTranscoding => StreamError::VideoNotConvertible,
        TranscodeError::SubtitleCannotBeBurned => StreamError::SubtitleNotDeliverable,
        TranscodeError::TooManySessions => StreamError::TooManyStreams,
        TranscodeError::Superseded => StreamError::Superseded,
        other => StreamError::Engine(Box::new(other)),
    }
}

fn narrow(value: i64) -> Option<i32> {
    i32::try_from(value).ok()
}

impl Streams {
    pub fn new(transcoder: Arc<Transcoder>) -> Self {
        Self {
            transcoder,
            entries: Mutex::default(),
        }
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Conversions whose process is gone, and the oldest beyond the cap. A file goes with its
    /// session.
    fn prune(&self, entries: &mut HashMap<String, Entry>) {
        entries.retain(|_, entry| !self.conversion_gone(entry));
        while entries.len() >= MAX_STREAMS {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.read_at)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
    }

    /// Records a stream and returns its id and secret.
    fn admit(&self, key: String, content: Content) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let token = klapz_core::access::new_token();
        let mut entries = self.entries();
        self.prune(&mut entries);
        let now = Instant::now();
        entries.insert(
            id.clone(),
            Entry {
                key,
                token: token.clone(),
                content,
                read_at: now,
                touched_at: now,
            },
        );
        (id, token)
    }

    fn conversion_gone(&self, entry: &Entry) -> bool {
        matches!(entry.content, Content::Converted(_)) && !self.transcoder.has_session(&entry.key)
    }

    /// The stream an address names, if its secret is the one handed out and, for a
    /// conversion, its process still runs: a stopped one is forgotten at once. Says whether
    /// this read is due to count as a sign of life for the session, too.
    fn open(&self, id: &str, token: &str) -> Option<Opened> {
        let mut entries = self.entries();
        let entry = entries.get(id)?;
        if !bool::from(entry.token.as_bytes().ct_eq(token.as_bytes())) {
            return None;
        }
        if self.conversion_gone(entry) {
            entries.remove(id);
            return None;
        }
        let entry = entries.get_mut(id)?;
        let now = Instant::now();
        entry.read_at = now;
        let touch_due = now.duration_since(entry.touched_at) >= TOUCH_INTERVAL;
        if touch_due {
            entry.touched_at = now;
        }
        Some(Opened {
            key: entry.key.clone(),
            content: entry.content.clone(),
            touch_due,
        })
    }

    /// Forgets every address of a key. Whether there was any.
    fn forget(&self, key: &str) -> bool {
        let mut entries = self.entries();
        let before = entries.len();
        entries.retain(|_, entry| entry.key != key);
        entries.len() < before
    }

    async fn convert(
        &self,
        order: StreamOrder,
        source: PathBuf,
        protocol: StreamProtocol,
    ) -> Result<StartedStream, StreamError> {
        let protocol = protocol_of(protocol);
        let text_subtitle = order.subtitle.as_ref().and_then(TextSubtitle::of);
        let measured = &order.source;
        let ladder = plan_ladder(
            &StreamSource {
                video_codec: &measured.video.codec,
                audio_codec: &measured.audio.codec,
                width: measured.width,
                height: measured.height,
                bits_per_second: measured.bitrate,
                frame_rate: measured.frame_rate,
            },
            protocol,
            ladder_limits(&order),
        )
        .map_err(stream_error)?;
        let top = *ladder.top();
        let duration_ms = measured.duration_ms.unwrap_or(0).max(0);
        #[allow(
            clippy::cast_precision_loss,
            reason = "a position in milliseconds, far below 2^52"
        )]
        let offset = (order.position_ms > 0).then(|| order.position_ms as f64 / 1000.0);
        let pictures = ladder
            .rungs
            .iter()
            .map(|rung| Picture::of(measured, rung))
            .collect();
        let replaced = self
            .transcoder
            .start_stream(StreamRequest {
                session: order.key.clone(),
                playback: order.playback.clone(),
                asked_at: order.asked_at,
                source,
                video: track(&measured.video),
                audio: track(&measured.audio),
                ladder: ladder.clone(),
                offset,
                protocol,
                total_ms: u64::try_from(duration_ms).unwrap_or(0),
                subtitle: order.subtitle.as_ref().map(subtitle_delivery),
                text_subtitles: text_subtitles(&order),
            })
            .await
            .map_err(stream_error)?;

        let picture = Picture::of(measured, &top);
        let audio_bandwidth = match top.audio {
            AudioAction::ConvertToAac => i64::from(top.audio_kbps()) * 1000,
            AudioAction::Copy => i64::from(measured.audio.bitrate.unwrap_or(0)),
        };
        let conversion = Conversion {
            protocol,
            ladder,
            pictures,
            audio_bandwidth,
            duration_ms,
            frame_rate: measured.frame_rate,
            subtitle: text_subtitle.clone(),
        };
        let (id, token) = self.admit(order.key, Content::Converted(Arc::new(conversion)));
        let manifest = match protocol {
            Protocol::Dash => "manifest.mpd",
            Protocol::Hls => "master.m3u8",
        };
        Ok(StartedStream {
            url: format!("/streams/{id}/{manifest}?token={token}"),
            subtitles_url: text_subtitle
                .as_ref()
                .map(|subtitle| format!("/streams/{id}/{}?token={token}", subtitle.file_name())),
            subtitle_format: text_subtitle.map(|subtitle| subtitle.format),
            video: match top.video {
                VideoAction::Copy => StreamDecision::Copy,
                VideoAction::Encode(_) => StreamDecision::Transcode,
            },
            audio: match top.audio {
                AudioAction::Copy => StreamDecision::Copy,
                AudioAction::ConvertToAac => StreamDecision::Transcode,
            },
            subtitle: order.subtitle.as_ref().map(SubtitleOutput::decision),
            output: PlaybackOutput {
                video_bitrate: narrow(picture.video_bandwidth),
                audio_bitrate: narrow(audio_bandwidth),
                width: narrow(picture.width),
                height: narrow(picture.height),
            },
            replaced,
        })
    }
}

#[async_trait::async_trait]
impl StreamEngine for Streams {
    async fn start(&self, order: StreamOrder) -> Result<StartedStream, StreamError> {
        let source = resolve_within(&order.source.path, &order.source.roots)
            .await
            .map_err(stream_error)?;
        match order.delivery {
            Delivery::Converted(protocol) => self.convert(order, source, protocol).await,
            Delivery::Original => {
                let output = PlaybackOutput {
                    video_bitrate: order.source.bitrate,
                    audio_bitrate: order.source.audio.bitrate,
                    width: order.source.width,
                    height: order.source.height,
                };
                let (id, token) = self.admit(order.key, Content::File(source));
                Ok(StartedStream {
                    url: format!("/streams/{id}/file?token={token}"),
                    subtitles_url: None,
                    subtitle_format: None,
                    video: StreamDecision::Copy,
                    audio: StreamDecision::Copy,
                    subtitle: None,
                    output,
                    replaced: Vec::new(),
                })
            }
        }
    }

    fn stop(&self, key: &str) -> bool {
        let forgotten = self.forget(key);
        self.transcoder.stop_session(key) || forgotten
    }

    fn touch(&self, key: &str) -> bool {
        let mut known = false;
        for entry in self.entries().values_mut() {
            if entry.key == key {
                entry.read_at = Instant::now();
                known = true;
            }
        }
        self.transcoder.ping_session(key) || known
    }

    /// Every conversion here decodes in software and encodes H.264, which is where a fallback
    /// leads: there is never a more conservative way to restart one.
    fn fall_back(&self, key: &str) -> Result<(), FallbackRefusal> {
        Err(if self.transcoder.has_session(key) {
            FallbackRefusal::Exhausted
        } else {
            FallbackRefusal::NoConversion
        })
    }

    fn idle(&self) -> Vec<String> {
        let mut keys = self.transcoder.idle_sessions();
        keys.extend(
            self.entries()
                .values()
                .filter(|entry| self.conversion_gone(entry))
                .map(|entry| entry.key.clone()),
        );
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn stop_if_idle(&self, key: &str) -> bool {
        if self.transcoder.stop_session_if_idle(key) {
            self.forget(key);
            return true;
        }
        let mut entries = self.entries();
        let before = entries.len();
        entries.retain(|_, entry| entry.key != key || !self.conversion_gone(entry));
        entries.len() < before
    }

    async fn stop_all(&self) {
        self.entries().clear();
        self.transcoder.stop_all_sessions().await;
    }

    async fn read_sidecar(&self, sidecar: &SidecarSource) -> Result<Vec<u8>, SidecarError> {
        let file = resolve_sidecar(&sidecar.path, &sidecar.media_path, &sidecar.roots)
            .await
            .map_err(sidecar_error)?;
        read_sidecar(&file).await.map_err(sidecar_error)
    }

    async fn remove_sidecar(&self, sidecar: &SidecarSource) -> Result<(), SidecarError> {
        let file = match resolve_sidecar(&sidecar.path, &sidecar.media_path, &sidecar.roots).await {
            Ok(file) => file,
            Err(TranscodeError::MissingFile) => return Ok(()),
            Err(error) => return Err(sidecar_error(error)),
        };
        match tokio::fs::remove_file(&file).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SidecarError::Io(error)),
        }
    }
}

fn sidecar_error(error: TranscodeError) -> SidecarError {
    match error {
        TranscodeError::SidecarTooLarge => SidecarError::TooLarge,
        TranscodeError::Io(error) => SidecarError::Io(error),
        // Not there, or not where it may be: either way there is no sidecar to open.
        _ => SidecarError::Missing,
    }
}

/// The sessions, their streams and the locks between them: what the stream routes reach, and
/// what ends sessions outside a request (the sweep, startup and shutdown).
#[derive(Debug, Clone)]
pub struct Sessions {
    pub streams: Arc<Streams>,
    pub playback: Arc<dyn PlaybackRepository>,
    pub locks: Arc<SessionLocks>,
}

impl Sessions {
    pub fn lifecycle(&self) -> Lifecycle<'_> {
        Lifecycle {
            sessions: self.playback.as_ref(),
            streams: self.streams.as_ref(),
            locks: &self.locks,
        }
    }

    /// The stream an address names, the read counting for its session when due. A stream whose
    /// session is gone is stopped, and answers as one that never existed.
    async fn open(&self, id: &str, secret: &Secret) -> Result<(String, Content), Refusal> {
        let token = secret.token.as_deref().ok_or(Refusal::NotFound)?;
        let opened = self.streams.open(id, token).ok_or(Refusal::NotFound)?;
        if opened.touch_due {
            match self
                .lifecycle()
                .read(&opened.key, klapz_core::now_epoch())
                .await
            {
                Ok(true) => {}
                Ok(false) => return Err(Refusal::NotFound),
                // Failing to record a sign of life is no reason to cut a film off.
                Err(error) => tracing::warn!(%error, "could not keep a playback session alive"),
            }
        }
        Ok((opened.key, opened.content))
    }

    async fn conversion(
        &self,
        id: &str,
        secret: &Secret,
    ) -> Result<(String, Arc<Conversion>), Refusal> {
        match self.open(id, secret).await? {
            (key, Content::Converted(conversion)) => Ok((key, conversion)),
            (_, Content::File(_)) => Err(Refusal::NotFound),
        }
    }
}

/// A stream an address opened.
struct Opened {
    key: String,
    content: Content,
    touch_due: bool,
}

pub fn router(routes: Sessions) -> Router {
    Router::new()
        .route("/streams/{id}/manifest.mpd", get(dash))
        .route("/streams/{id}/master.m3u8", get(master_playlist))
        .route("/streams/{id}/file", get(original))
        .route("/streams/{id}/{name}", get(subtitles))
        .route("/streams/{id}/{rendition}/{part}", get(rendition_part))
        .with_state(routes)
}

#[derive(Debug, Deserialize)]
struct Secret {
    token: Option<String>,
}

/// Why a stream's route answers without its content.
#[derive(Debug)]
enum Refusal {
    /// No such stream, or not this secret: the two answer alike.
    NotFound,
    Failed(TranscodeError),
}

impl From<TranscodeError> for Refusal {
    fn from(error: TranscodeError) -> Self {
        match error {
            TranscodeError::UnknownSession | TranscodeError::UnknownSegment => Self::NotFound,
            other => Self::Failed(other),
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND.into_response(),
            Self::Failed(error) => {
                tracing::warn!(%error, "a stream could not be served");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}

/// Where a stream's files are, and the query that opens them.
struct Address<'a> {
    base: String,
    query: &'a str,
}

impl<'a> Address<'a> {
    fn new(id: &str, query: &'a str) -> Self {
        Self {
            base: format!("/streams/{id}"),
            query,
        }
    }
}

/// `Range`, conditional requests and the content type come from `ServeFile`.
async fn serve(path: PathBuf, request: Request) -> Response {
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    }
}

/// A segment made of a rung's pieces, one after the other: whole segments only, so no `Range`.
async fn serve_pieces(paths: Vec<PathBuf>, content_type: &'static str) -> Response {
    let mut length = 0;
    for path in &paths {
        match tokio::fs::metadata(path).await {
            Ok(metadata) => length += metadata.len(),
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        }
    }
    let pieces = futures_util::stream::iter(paths).then(|path| async move {
        let mut file = tokio::fs::File::open(path).await?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).await?;
        Ok::<_, std::io::Error>(Bytes::from(bytes))
    });
    (
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (header::CONTENT_LENGTH, length.to_string()),
        ],
        Body::from_stream(pieces),
    )
        .into_response()
}

/// Reports how long the body took to send, from its first byte to its last, once it is whole:
/// the wait for the process to write the segment came before and is not in it.
fn measured(response: Response, report: impl FnOnce(adapt::Delivery) + Send + 'static) -> Response {
    // An empty segment, past the end of the media, says nothing about the link.
    let empty = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .is_some_and(|length| length == "0");
    if response.status() != StatusCode::OK || empty {
        return response;
    }
    let (parts, body) = response.into_parts();
    // With a length declared, the connection stops reading the body at its last byte and never
    // asks for the end of the stream.
    let length = parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok());
    let state = (
        body.into_data_stream(),
        0_u64,
        None::<Instant>,
        Some(report),
    );
    let stream = futures_util::stream::unfold(
        state,
        move |(mut data, bytes, began, mut report)| async move {
            let began = began.unwrap_or_else(Instant::now);
            match data.next().await {
                Some(Ok(chunk)) => {
                    let bytes = bytes + chunk.len() as u64;
                    if length.is_some_and(|length| bytes >= length) {
                        if let Some(report) = report.take() {
                            report(adapt::Delivery {
                                bytes,
                                sending: began.elapsed(),
                            });
                        }
                    }
                    Some((Ok(chunk), (data, bytes, Some(began), report)))
                }
                // A send that failed says nothing about how fast the player reads.
                Some(Err(error)) => Some((Err(error), (data, bytes, Some(began), None))),
                None => {
                    if let Some(report) = report {
                        report(adapt::Delivery {
                            bytes,
                            sending: began.elapsed(),
                        });
                    }
                    None
                }
            }
        },
    );
    Response::from_parts(parts, Body::from_stream(stream))
}

/// `GET /streams/{id}/file`: the file as it is.
async fn original(
    State(routes): State<Sessions>,
    Path(id): Path<String>,
    Query(secret): Query<Secret>,
    request: Request,
) -> Result<Response, Refusal> {
    match routes.open(&id, &secret).await? {
        (_, Content::File(path)) => Ok(serve(path, request).await),
        (_, Content::Converted(_)) => Err(Refusal::NotFound),
    }
}

/// `GET /streams/{id}/manifest.mpd`: the reference's DASH manifest, whole and static, with a
/// video representation per rendition.
///
/// A copy's codec strings are known only to the process, and read from the manifest it writes.
/// An encode's are fixed: the reference announces them at once, and so does this.
async fn dash(
    State(routes): State<Sessions>,
    Path(id): Path<String>,
    Query(secret): Query<Secret>,
) -> Result<Response, Refusal> {
    let (key, conversion) = routes.conversion(&id, &secret).await?;
    if conversion.protocol != Protocol::Dash {
        return Err(Refusal::NotFound);
    }
    let written = if conversion.ladder.copies() {
        routes.streams.transcoder.session_manifest(&key).await?
    } else {
        String::new()
    };
    let token = secret.token.as_deref().unwrap_or_default();
    let query = format!("?token={token}");
    let body = dash_manifest(&DashManifest {
        address: &Address::new(&id, &query),
        duration_ms: conversion.duration_ms,
        segment_seconds: conversion.ladder.segment_seconds(Protocol::Dash),
        pictures: conversion.renditions(),
        audio_bandwidth: conversion.audio_bandwidth,
        audio_copied: conversion.ladder.top().audio == AudioAction::Copy,
        written: &written,
        subtitle: conversion.subtitle.as_ref(),
    });
    Ok(([(header::CONTENT_TYPE, "application/dash+xml")], body).into_response())
}

fn playlist(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        body,
    )
        .into_response()
}

/// `GET /streams/{id}/master.m3u8`: a variant per rendition, one unless the player chooses.
async fn master_playlist(
    State(routes): State<Sessions>,
    Path(id): Path<String>,
    Query(secret): Query<Secret>,
) -> Result<Response, Refusal> {
    let (_, conversion) = routes.conversion(&id, &secret).await?;
    if conversion.protocol != Protocol::Hls {
        return Err(Refusal::NotFound);
    }
    let token = secret.token.as_deref().unwrap_or_default();
    let query = format!("?token={token}");
    Ok(playlist(master_variants(
        &conversion,
        &Address::new(&id, &query),
    )))
}

fn master_variants(conversion: &Conversion, address: &Address<'_>) -> String {
    let Address { base, query } = address;
    let audio_bits = match conversion.ladder.top().audio {
        AudioAction::Copy => 0,
        AudioAction::ConvertToAac => conversion.audio_bandwidth,
    };
    let frame_rate = conversion
        .frame_rate
        .map_or_else(String::new, |rate| format!(",FRAME-RATE={rate:.6}"));
    let mut text = "#EXTM3U\n".to_owned();
    if conversion.ladder.adaptation == Adaptation::Client {
        // Every rung's segments start on the same key frames: a player may switch at any one.
        text.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
    }
    for (rendition, picture) in conversion.renditions().iter().enumerate() {
        let resolution = if picture.width > 0 && picture.height > 0 {
            format!(",RESOLUTION={}x{}", picture.width, picture.height)
        } else {
            String::new()
        };
        // Writing to a `String` cannot fail. A client ignores an attribute it does not know.
        let _ = write!(
            text,
            "#EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH={}{resolution}{frame_rate},\
             X-KLAPZ-DELIVERY={}\n\
             {base}/{rendition}/index.m3u8{query}\n",
            picture.video_bandwidth + audio_bits,
            delivery(picture.copied).to_ascii_uppercase(),
        );
    }
    text
}

/// What a rendition's address names: `index.m3u8`, `00012.ts`, `header` or `12.m4s`.
fn numbered(part: &str, extension: &str) -> Option<u32> {
    part.strip_suffix(extension)?.parse().ok()
}

/// `GET /streams/{id}/{rendition}/…`: an HLS rendition's playlist and segments, a DASH
/// rendition's video, and the DASH audio under `audio`.
async fn rendition_part(
    State(routes): State<Sessions>,
    Path((id, rendition, part)): Path<(String, String, String)>,
    Query(secret): Query<Secret>,
    request: Request,
) -> Result<Response, Refusal> {
    let (key, conversion) = routes.conversion(&id, &secret).await?;
    let server_adapted = conversion.ladder.adaptation == Adaptation::Server;
    let report = {
        let transcoder = Arc::clone(&routes.streams.transcoder);
        let key = key.clone();
        move |delivery| transcoder.record_delivery(&key, delivery)
    };
    match conversion.protocol {
        Protocol::Hls => {
            let rendition: usize = rendition.parse().map_err(|_| Refusal::NotFound)?;
            if part == "index.m3u8" {
                let output = routes
                    .streams
                    .transcoder
                    .session_output(&key, rendition)
                    .await?;
                let query = format!("?token={}", secret.token.as_deref().unwrap_or_default());
                return Ok(playlist(hls_playlist(
                    &output,
                    &Address::new(&id, &query),
                    rendition,
                )));
            }
            let number = numbered(&part, ".ts").ok_or(Refusal::NotFound)?;
            let files = routes
                .streams
                .transcoder
                .session_segment(&key, rendition, number)
                .await?;
            let response = served(files, "video/mp2t", request).await;
            Ok(if server_adapted {
                measured(response, report)
            } else {
                response
            })
        }
        Protocol::Dash => {
            let track = if rendition == "audio" {
                DashTrack::Audio
            } else {
                DashTrack::Video(rendition.parse().map_err(|_| Refusal::NotFound)?)
            };
            if part == "header" {
                let path = routes.streams.transcoder.dash_header(&key, track).await?;
                return Ok(serve(path, request).await);
            }
            let number = numbered(&part, ".m4s").ok_or(Refusal::NotFound)?;
            let files = routes
                .streams
                .transcoder
                .dash_chunk(&key, track, number)
                .await?;
            let response = served(files, "video/mp4", request).await;
            Ok(match track {
                DashTrack::Video(_) if server_adapted => measured(response, report),
                _ => response,
            })
        }
    }
}

/// No file is a segment past the end of the media, answered empty: the manifest, laid from the
/// stated duration, can list one more than the process found, and a `404` there stops a player.
async fn served(mut files: Vec<PathBuf>, content_type: &'static str, request: Request) -> Response {
    match files.len() {
        1 => serve(files.remove(0), request).await,
        _ => serve_pieces(files, content_type).await,
    }
}

/// `GET /streams/{id}/subtitles.{vtt,ass,srt,ttml}`: the stream's subtitle, whole, under the
/// extension of the format it was started with. Answered at once, the body following when the
/// extraction begun with the stream is done: over a network share it can take longer than the
/// server lets a response wait for its head. A failed extraction ends the body empty.
async fn subtitles(
    State(routes): State<Sessions>,
    Path((id, name)): Path<(String, String)>,
    Query(secret): Query<Secret>,
) -> Result<Response, Refusal> {
    let (key, conversion) = routes.conversion(&id, &secret).await?;
    let Some(subtitle) = conversion
        .subtitle
        .as_ref()
        .filter(|subtitle| subtitle.file_name() == name)
    else {
        return Err(Refusal::NotFound);
    };
    let transcoder = Arc::clone(&routes.streams.transcoder);
    let body = futures_util::stream::once(async move {
        let bytes = match transcoder.subtitles(&key).await {
            Ok((bytes, _)) => Bytes::from(bytes),
            Err(error) => {
                tracing::warn!(%error, "a stream's subtitle could not be served");
                Bytes::new()
            }
        };
        Ok::<_, std::convert::Infallible>(bytes)
    });
    Ok((
        [(
            header::CONTENT_TYPE,
            format!("{}; charset=utf-8", subtitle.mime_type()),
        )],
        Body::from_stream(body),
    )
        .into_response())
}

struct DashManifest<'a> {
    address: &'a Address<'a>,
    duration_ms: i64,
    segment_seconds: u32,
    /// The video representations, best first.
    pictures: &'a [Picture],
    audio_bandwidth: i64,
    audio_copied: bool,
    /// The manifest the process wrote, which alone states a copy's codecs.
    written: &'a str,
    subtitle: Option<&'a TextSubtitle>,
}

/// The value of `name` on the `Representation` element with this id.
fn representation_attribute<'a>(written: &'a str, id: u32, name: &str) -> Option<&'a str> {
    let opening = format!("<Representation id=\"{id}\"");
    let element = &written[written.find(&opening)?..];
    let element = &element[..element.find('>')?];
    let key = format!("{name}=\"");
    let value = &element[element.find(&key)? + key.len()..];
    Some(&value[..value.find('"')?])
}

fn dash_manifest(manifest: &DashManifest<'_>) -> String {
    let Address { base, query } = manifest.address;
    let seconds = manifest.duration_ms / 1000;
    let duration = format!(
        "PT{}H{}M{}S",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    let template = format!(
        "<SegmentTemplate timescale=\"1\" duration=\"{segment}\" \
         initialization=\"{base}/$RepresentationID$/header{query}\" \
         media=\"{base}/$RepresentationID$/$Number$.m4s{query}\" startNumber=\"0\">\n\
         \t\t\t</SegmentTemplate>",
        segment = manifest.segment_seconds
    );
    let copied_codecs =
        representation_attribute(manifest.written, 0, "codecs").unwrap_or(ENCODED_VIDEO_CODECS);
    let audio_codecs =
        representation_attribute(manifest.written, 1, "codecs").unwrap_or("mp4a.40.2");
    let sampling =
        representation_attribute(manifest.written, 1, "audioSamplingRate").unwrap_or("48000");
    let channels = manifest
        .written
        .find("audio_channel_configuration")
        .map(|at| &manifest.written[at..])
        .and_then(|rest| rest.split("value=\"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("2");
    let mut videos = String::new();
    for (rendition, picture) in manifest.pictures.iter().enumerate() {
        let codecs = if picture.copied {
            copied_codecs
        } else {
            ENCODED_VIDEO_CODECS
        };
        // Writing to a `String` cannot fail.
        let _ = write!(
            videos,
            "\t\t\t<Representation id=\"{rendition}\" mimeType=\"video/mp4\" codecs=\"{codecs}\" \
             bandwidth=\"{}\" width=\"{}\" height=\"{}\" klapz:delivery=\"{}\">\n\
             \t\t\t</Representation>\n",
            picture.video_bandwidth,
            picture.width,
            picture.height,
            delivery(picture.copied),
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <MPD xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n\
         \txmlns=\"urn:mpeg:dash:schema:mpd:2011\"\n\
         \txmlns:xlink=\"http://www.w3.org/1999/xlink\"\n\
         \txmlns:klapz=\"{DELIVERY_NAMESPACE}\"\n\
         \txsi:schemaLocation=\"urn:mpeg:dash:schema:mpd:2011 http://standards.iso.org/ittf/PubliclyAvailableStandards/MPEG-DASH_schema_files/DASH-MPD.xsd\"\n\
         \tprofiles=\"urn:mpeg:dash:profile:isoff-live:2011\"\n\
         \ttype=\"static\"\n\
         \tmediaPresentationDuration=\"{duration}\"\n\
         \tmaxSegmentDuration=\"PT{longest}S\"\n\
         \tminBufferTime=\"PT10S\">\n\
         \t<Period start=\"PT0S\" id=\"0\" duration=\"{duration}\">\n\
         \t\t<AdaptationSet segmentAlignment=\"true\">\n\
         \t\t\t{template}\n\
         {videos}\
         \t\t</AdaptationSet>\n\
         \t\t<AdaptationSet segmentAlignment=\"true\">\n\
         \t\t\t{template}\n\
         \t\t\t<Representation id=\"audio\" mimeType=\"audio/mp4\" codecs=\"{audio_codecs}\" \
         bandwidth=\"{audio}\" audioSamplingRate=\"{sampling}\" klapz:delivery=\"{audio_delivery}\">\n\
         \t\t\t\t<AudioChannelConfiguration \
         schemeIdUri=\"urn:mpeg:dash:23003:3:audio_channel_configuration:2011\" \
         value=\"{channels}\"/>\n\
         \t\t\t</Representation>\n\
         \t\t</AdaptationSet>\n\
         {subtitles}\
         \t</Period>\n\
         </MPD>\n",
        longest = manifest.segment_seconds * 2,
        audio = manifest.audio_bandwidth,
        audio_delivery = delivery(manifest.audio_copied),
        subtitles = manifest.subtitle.map_or_else(String::new, |subtitle| {
            subtitle_adaptation(subtitle, manifest.address)
        }),
    )
}

/// The subtitle's adaptation set: the whole file as the one segment of its representation,
/// which players read as a text track when it has no segment template.
fn subtitle_adaptation(subtitle: &TextSubtitle, address: &Address<'_>) -> String {
    let Address { base, query } = address;
    // Written into the manifest as it is stored only when it is a tag of letters, digits and
    // hyphens.
    let lang = subtitle
        .language
        .as_deref()
        .filter(|tag| {
            !tag.is_empty()
                && tag
                    .chars()
                    .all(|letter| letter.is_ascii_alphanumeric() || letter == '-')
        })
        .map_or_else(String::new, |tag| format!(" lang=\"{tag}\""));
    format!(
        "\t\t<AdaptationSet contentType=\"text\" mimeType=\"{mime}\"{lang}>\n\
         \t\t\t<Role schemeIdUri=\"urn:mpeg:dash:role:2011\" value=\"subtitle\"/>\n\
         \t\t\t<Representation id=\"subtitles\" bandwidth=\"0\">\n\
         \t\t\t\t<BaseURL>{base}/{file}{query}</BaseURL>\n\
         \t\t\t</Representation>\n\
         \t\t</AdaptationSet>\n",
        mime = subtitle.mime_type(),
        file = subtitle.file_name(),
    )
}

/// The reference builds the whole playlist up front from the media's duration: a fake VOD list
/// that already ends in `#EXT-X-ENDLIST` while the process is still writing, so a player learns
/// the real length at once instead of watching it grow. A remux cuts its segments on the file's
/// own key frames, close to these nominal boundaries. With no duration we fall back to listing
/// the segments that exist.
fn hls_playlist(output: &Output, address: &Address<'_>, rendition: usize) -> String {
    let Address { base, query } = address;
    let base = format!("{base}/{rendition}");
    let seconds = output.segment_seconds.max(1);
    let segment_ms = u64::from(seconds) * 1000;
    if output.total_ms == 0 {
        return growing_playlist(output, &base, query);
    }
    let full = output.total_ms / segment_ms;
    let remainder = output.total_ms % segment_ms;
    let mut text = format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:{seconds}\n#EXT-X-ALLOW-CACHE:NO\n#EXT-X-MEDIA-SEQUENCE:0\n"
    );
    for number in 0..full {
        // Writing to a `String` cannot fail.
        let _ = write!(
            text,
            "#EXTINF:{seconds}, nodesc\n{base}/{number:05}.ts{query}\n"
        );
    }
    if remainder > 0 {
        // The reference prints the remainder rounded to a whole second, never below one.
        let last = ((remainder + 500) / 1000).max(1);
        let _ = write!(text, "#EXTINF:{last}, nodesc\n{base}/{full:05}.ts{query}\n");
    }
    text.push_str("#EXT-X-ENDLIST\n");
    text
}

/// The playlist as segments arrive, used only when the media's duration is unknown.
fn growing_playlist(output: &Output, base: &str, query: &str) -> String {
    let longest = output
        .segments
        .iter()
        .map(|segment| segment.seconds)
        .fold(10.0_f64, f64::max);
    let mut text = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-ALLOW-CACHE:NO\n\
         #EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-MEDIA-SEQUENCE:0\n",
        longest.ceil()
    );
    for segment in &output.segments {
        // Writing to a `String` cannot fail.
        let _ = write!(
            text,
            "#EXTINF:{:.6}, nodesc\n{base}/{:05}.ts{query}\n",
            segment.seconds, segment.number
        );
    }
    if output.ended {
        text.push_str("#EXT-X-ENDLIST\n");
    }
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::body::to_bytes;
    use klapz_core::playback::NewPlaybackSession;
    use klapz_core::testing::InMemoryPlayback;
    use klapz_transcoder::session::Segment;

    use super::*;

    const ADDRESS: &str = "?token=t0";

    fn address() -> Address<'static> {
        Address::new("s1", ADDRESS)
    }

    /// As the process wrote it for a real file.
    const WRITTEN: &str = r#"<Representation id="0" mimeType="video/mp4" codecs="avc1.64001f" bandwidth="3174670" width="1280">
<Representation id="1" mimeType="audio/mp4" codecs="mp4a.40.2" bandwidth="256000" audioSamplingRate="48000">
<AudioChannelConfiguration schemeIdUri="urn:mpeg:dash:23003:3:audio_channel_configuration:2011" value="2" />"#;

    #[test]
    fn reads_the_codecs_the_process_found() {
        assert_eq!(
            representation_attribute(WRITTEN, 0, "codecs"),
            Some("avc1.64001f")
        );
        assert_eq!(
            representation_attribute(WRITTEN, 1, "audioSamplingRate"),
            Some("48000")
        );
        assert_eq!(representation_attribute(WRITTEN, 2, "codecs"), None);
    }

    const COPIED: Picture = Picture {
        video_bandwidth: 4_273_000,
        width: 1920,
        height: 1080,
        copied: true,
    };

    #[test]
    fn writes_the_references_manifest_with_the_secret_on_every_url() {
        let text = dash_manifest(&DashManifest {
            address: &address(),
            duration_ms: 7_200_000,
            segment_seconds: 5,
            pictures: &[COPIED],
            audio_bandwidth: 256_000,
            audio_copied: false,
            written: WRITTEN,
            subtitle: None,
        });
        assert!(text.contains("type=\"static\""));
        assert!(text.contains("mediaPresentationDuration=\"PT2H0M0S\""));
        assert!(text.contains("maxSegmentDuration=\"PT10S\""));
        assert!(text.contains(
            r#"duration="5" initialization="/streams/s1/$RepresentationID$/header?token=t0""#
        ));
        assert!(text.contains(
            r#"media="/streams/s1/$RepresentationID$/$Number$.m4s?token=t0" startNumber="0""#
        ));
        assert!(text.contains(
            r#"id="0" mimeType="video/mp4" codecs="avc1.64001f" bandwidth="4273000" width="1920" height="1080""#
        ));
        assert!(text.contains(r#"<Representation id="audio" mimeType="audio/mp4""#));
        assert!(text.contains(r#"value="2"/>"#));
    }

    #[test]
    fn a_player_choosing_its_quality_gets_a_representation_per_rung() {
        let encoded = Picture {
            video_bandwidth: 3_419_000,
            width: 1280,
            height: 720,
            copied: false,
        };
        let text = dash_manifest(&DashManifest {
            address: &address(),
            duration_ms: 7_200_000,
            segment_seconds: 5,
            pictures: &[COPIED, encoded],
            audio_bandwidth: 256_000,
            audio_copied: true,
            written: WRITTEN,
            subtitle: None,
        });
        assert_eq!(text.matches("mimeType=\"video/mp4\"").count(), 2);
        assert!(text.contains(r#"xmlns:klapz="urn:klapz:stream:delivery""#));
        assert!(text.contains(r#"height="1080" klapz:delivery="remux">"#));
        assert!(text.contains(r#"height="720" klapz:delivery="transcode">"#));
        assert!(text.contains(r#"audioSamplingRate="48000" klapz:delivery="remux">"#));
        // The copy's codecs are the process's; an encode's are known before it writes.
        assert!(text.contains(r#"id="0" mimeType="video/mp4" codecs="avc1.64001f""#));
        assert!(text.contains(
            r#"id="1" mimeType="video/mp4" codecs="avc1.640028" bandwidth="3419000" width="1280" height="720""#
        ));
        // In the same adaptation set, one template for both.
        assert_eq!(text.matches("<AdaptationSet").count(), 2);
    }

    #[test]
    fn a_text_subtitle_is_an_adaptation_set_of_one_whole_file() {
        let subtitle = TextSubtitle {
            format: PlayerSubtitleFormat::WebVtt,
            language: Some("fr".to_owned()),
        };
        let text = dash_manifest(&DashManifest {
            address: &address(),
            duration_ms: 7_200_000,
            segment_seconds: 5,
            pictures: &[COPIED],
            audio_bandwidth: 256_000,
            audio_copied: true,
            written: WRITTEN,
            subtitle: Some(&subtitle),
        });
        assert_eq!(text.matches("<AdaptationSet").count(), 3);
        assert!(
            text.contains(r#"<AdaptationSet contentType="text" mimeType="text/vtt" lang="fr">"#)
        );
        assert!(text.contains("<BaseURL>/streams/s1/subtitles.vtt?token=t0</BaseURL>"));
        // Within the period, after the audio.
        let set = text.find("contentType=\"text\"").unwrap();
        assert!(set > text.find("id=\"audio\"").unwrap());
        assert!(set < text.find("</Period>").unwrap());
        // A language that is no tag is left out of the manifest.
        let odd = TextSubtitle {
            format: PlayerSubtitleFormat::Ass,
            language: Some("fr\" onload=\"x".to_owned()),
        };
        let text = subtitle_adaptation(&odd, &address());
        assert!(text.contains(r#"mimeType="text/x-ssa">"#), "{text}");
        assert!(!text.contains("lang="));
        let pictures = TextSubtitle {
            format: PlayerSubtitleFormat::Ttml,
            language: Some("fr".to_owned()),
        };
        let text = subtitle_adaptation(&pictures, &address());
        assert!(text.contains(r#"mimeType="application/ttml+xml" lang="fr">"#));
        assert!(text.contains("<BaseURL>/streams/s1/subtitles.ttml?token=t0</BaseURL>"));
    }

    fn conversion_of(adaptation: Adaptation) -> Conversion {
        let encoded = Picture {
            video_bandwidth: 1_297_000,
            width: 720,
            height: 404,
            copied: false,
        };
        Conversion {
            protocol: Protocol::Hls,
            ladder: Ladder {
                adaptation,
                rungs: vec![
                    StreamPlan {
                        video: VideoAction::Copy,
                        audio: AudioAction::ConvertToAac,
                    };
                    2
                ],
            },
            pictures: vec![COPIED, encoded],
            audio_bandwidth: 256_000,
            duration_ms: 0,
            frame_rate: Some(25.0),
            subtitle: None,
        }
    }

    #[test]
    fn a_master_playlist_lists_a_variant_per_rung_the_player_chooses_from() {
        let text = master_variants(&conversion_of(Adaptation::Client), &address());
        assert_eq!(
            text,
            "#EXTM3U\n#EXT-X-INDEPENDENT-SEGMENTS\n\
             #EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH=4529000,RESOLUTION=1920x1080,FRAME-RATE=25.000000,X-KLAPZ-DELIVERY=REMUX\n\
             /streams/s1/0/index.m3u8?token=t0\n\
             #EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH=1553000,RESOLUTION=720x404,FRAME-RATE=25.000000,X-KLAPZ-DELIVERY=TRANSCODE\n\
             /streams/s1/1/index.m3u8?token=t0\n"
        );
        // Moved between by the server, the rungs are one variant.
        let one = master_variants(&conversion_of(Adaptation::Server), &address());
        assert_eq!(one.matches("#EXT-X-STREAM-INF").count(), 1);
        assert!(!one.contains("INDEPENDENT"));
    }

    #[test]
    fn the_playlist_predicts_every_segment_from_the_duration() {
        // Soul: 6_031_568 ms at 10s segments, as captured from the reference.
        let playlist = hls_playlist(
            &Output {
                segments: Vec::new(),
                ended: false,
                total_ms: 6_031_568,
                segment_seconds: 10,
            },
            &address(),
            0,
        );
        assert!(playlist.starts_with(
            "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-ALLOW-CACHE:NO\n#EXT-X-MEDIA-SEQUENCE:0\n"
        ));
        assert!(!playlist.contains("EXT-X-VERSION"));
        assert!(!playlist.contains("PLAYLIST-TYPE"));
        assert!(playlist.contains("#EXTINF:10, nodesc\n/streams/s1/0/00000.ts?token=t0\n"));
        assert!(playlist.contains("#EXTINF:10, nodesc\n/streams/s1/0/00602.ts?token=t0\n"));
        assert!(playlist.contains("#EXTINF:2, nodesc\n/streams/s1/0/00603.ts?token=t0\n"));
        assert!(!playlist.contains("00604.ts"));
        assert!(playlist.ends_with("#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn a_whole_number_of_segments_leaves_no_remainder() {
        let playlist = hls_playlist(
            &Output {
                segments: Vec::new(),
                ended: false,
                total_ms: 30_000,
                segment_seconds: 10,
            },
            &address(),
            2,
        );
        assert!(playlist.contains("/streams/s1/2/00002.ts?token=t0\n"));
        assert!(!playlist.contains("00003.ts"));
        assert!(playlist.ends_with("#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn without_a_duration_the_playlist_lists_what_exists() {
        let output = Output {
            segments: vec![
                Segment {
                    number: 0,
                    seconds: 9.0,
                },
                Segment {
                    number: 1,
                    seconds: 12.5,
                },
            ],
            ended: false,
            total_ms: 0,
            segment_seconds: 10,
        };
        let running = hls_playlist(&output, &address(), 0);
        assert!(running.contains("#EXT-X-TARGETDURATION:13\n"));
        assert!(running.contains("#EXTINF:9.000000, nodesc\n/streams/s1/0/00000.ts?token=t0\n"));
        assert!(!running.contains("ENDLIST"));
        assert!(hls_playlist(
            &Output {
                ended: true,
                ..output
            },
            &address(),
            0
        )
        .ends_with("#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn reads_what_a_renditions_address_names() {
        assert_eq!(numbered("00012.ts", ".ts"), Some(12));
        assert_eq!(numbered("12.m4s", ".m4s"), Some(12));
        assert_eq!(numbered("12.m4s", ".ts"), None);
        assert_eq!(numbered("-1.m4s", ".m4s"), None);
        assert_eq!(numbered("header", ".m4s"), None);
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("klapz-streams-test-{name}-{}", std::process::id()));
            std::fs::create_dir_all(path.join("movies")).unwrap();
            std::fs::write(path.join("movies").join("film.mkv"), b"the film").unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn order(scratch: &Scratch) -> StreamOrder {
        let track = |codec: &str| StreamTrack {
            position: 0,
            codec: codec.to_owned(),
            language: None,
            bitrate: Some(256_000),
        };
        StreamOrder {
            key: "u1~k".to_owned(),
            playback: None,
            source: klapz_core::stream::StreamSource {
                path: scratch
                    .0
                    .join("movies")
                    .join("film.mkv")
                    .display()
                    .to_string(),
                roots: vec![scratch.0.join("movies").display().to_string()],
                width: Some(1920),
                height: Some(1080),
                bitrate: Some(8_000_000),
                frame_rate: None,
                duration_ms: Some(1000),
                video: track("h264"),
                audio: track("aac"),
                subtitles: Vec::new(),
            },
            delivery: Delivery::Original,
            limits: klapz_core::stream::StreamLimits::default(),
            subtitle: None,
            subtitle_formats: Vec::new(),
            position_ms: 0,
            asked_at: Instant::now(),
        }
    }

    async fn fetch(app: Router, uri: &str) -> (StatusCode, Vec<u8>) {
        let response = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        (
            status,
            to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap()
                .to_vec(),
        )
    }

    fn routes(streams: &Arc<Streams>, sessions: &Arc<InMemoryPlayback>) -> Sessions {
        Sessions {
            streams: Arc::clone(streams),
            playback: Arc::clone(sessions) as Arc<dyn PlaybackRepository>,
            locks: Arc::default(),
        }
    }

    fn recorded(key: &str) -> NewPlaybackSession {
        NewPlaybackSession {
            key: key.to_owned(),
            user: klapz_core::UserId::new(1),
            playback_id: None,
            item: klapz_core::RatingKey::new(1),
            part: klapz_core::PartId::new(1),
            client: None,
            product: None,
            video: StreamDecision::Copy,
            audio: StreamDecision::Copy,
            position_ms: 0,
            output: PlaybackOutput::default(),
        }
    }

    /// As if the stream's last sign of life were older than the interval.
    fn age_touches(streams: &Streams) {
        for entry in streams.entries().values_mut() {
            entry.touched_at = Instant::now().checked_sub(TOUCH_INTERVAL).unwrap();
        }
    }

    #[tokio::test]
    async fn a_segment_past_the_end_of_the_media_is_answered_empty() {
        let response = served(Vec::new(), "video/mp4", Request::new(Body::empty())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "0");
        let reported = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&reported);
        let response = measured(response, move |_| flag.store(true, Ordering::Release));
        assert!(to_bytes(response.into_body(), 16).await.unwrap().is_empty());
        assert!(!reported.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn a_file_played_as_it_is_opens_with_its_secret_only() {
        let scratch = Scratch::new("original");
        let streams = Arc::new(Streams::new(Arc::new(Transcoder::default())));
        let started = streams.start(order(&scratch)).await.unwrap();
        assert!(started.url.starts_with("/streams/"));
        assert_eq!(started.video, StreamDecision::Copy);

        let app = router(routes(&streams, &Arc::default()));
        let (status, body) = fetch(app.clone(), &started.url).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"the film");

        let (path, _) = started.url.split_once('?').unwrap();
        assert_eq!(fetch(app.clone(), path).await.0, StatusCode::NOT_FOUND);
        let wrong = format!("{path}?token=0000");
        assert_eq!(fetch(app.clone(), &wrong).await.0, StatusCode::NOT_FOUND);

        assert!(streams.stop("u1~k"));
        assert_eq!(fetch(app, &started.url).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reading_a_stream_keeps_its_session_alive_at_most_every_interval() {
        let scratch = Scratch::new("keep-alive");
        let streams = Arc::new(Streams::new(Arc::new(Transcoder::default())));
        let sessions = Arc::new(InMemoryPlayback::default());
        let started = streams.start(order(&scratch)).await.unwrap();
        let first = sessions.start(recorded("u1~k"), 0).await.unwrap();
        let app = router(routes(&streams, &sessions));

        // Within the interval, a read writes nothing.
        assert_eq!(fetch(app.clone(), &started.url).await.0, StatusCode::OK);
        let now = klapz_core::now_epoch();
        assert!(sessions.session(first.id, now).await.unwrap().is_none());

        // The session expired meanwhile: a due read finds it gone and stops the stream.
        age_touches(&streams);
        assert_eq!(
            fetch(app.clone(), &started.url).await.0,
            StatusCode::NOT_FOUND
        );
        assert!(!streams.touch("u1~k"));

        let started = streams.start(order(&scratch)).await.unwrap();
        let live = sessions.start(recorded("u1~k"), now).await.unwrap();
        age_touches(&streams);
        assert_eq!(fetch(app, &started.url).await.0, StatusCode::OK);
        let touched = sessions.session(live.id, now).await.unwrap().unwrap();
        assert!(touched.updated_at >= now);
    }

    #[tokio::test]
    async fn a_terminated_session_takes_its_stream_down_at_the_next_due_read() {
        let scratch = Scratch::new("terminated");
        let streams = Arc::new(Streams::new(Arc::new(Transcoder::default())));
        let sessions = Arc::new(InMemoryPlayback::default());
        let started = streams.start(order(&scratch)).await.unwrap();
        let now = klapz_core::now_epoch();
        sessions.start(recorded("u1~k"), now).await.unwrap();
        // Marked terminated without the stream being reached.
        sessions.terminate("u1~k", None, now).await.unwrap();
        age_touches(&streams);
        let app = router(routes(&streams, &sessions));
        assert_eq!(fetch(app, &started.url).await.0, StatusCode::NOT_FOUND);
        assert!(streams.entries().is_empty());
    }

    #[tokio::test]
    async fn a_file_is_never_idle_by_itself() {
        let scratch = Scratch::new("never-idle");
        let streams = Streams::new(Arc::new(Transcoder::default()));
        streams.start(order(&scratch)).await.unwrap();
        assert!(streams.idle().is_empty());
        assert!(!streams.stop_if_idle("u1~k"));
        assert!(streams.touch("u1~k"));
        streams.stop_all().await;
        assert!(!streams.touch("u1~k"));
    }

    #[tokio::test]
    async fn a_file_outside_its_library_is_not_streamed() {
        let scratch = Scratch::new("outside");
        let streams = Streams::new(Arc::new(Transcoder::default()));
        let mut order = order(&scratch);
        order.source.roots = vec![scratch.0.join("elsewhere").display().to_string()];
        assert!(matches!(
            streams.start(order).await,
            Err(StreamError::Engine(_))
        ));
    }

    #[tokio::test]
    async fn a_segment_made_of_pieces_is_sent_whole_in_order() {
        let scratch = Scratch::new("pieces");
        let first = scratch.0.join("a.m4s");
        let second = scratch.0.join("b.m4s");
        std::fs::write(&first, b"first ").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let response = serve_pieces(vec![first, second], "video/mp4").await;
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "12");
        let body = to_bytes(response.into_body(), 1 << 10).await.unwrap();
        assert_eq!(&body[..], b"first second");
    }

    #[tokio::test]
    async fn a_measured_send_reports_its_bytes_once_whole() {
        let reported = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&reported);
        let response = measured(
            Response::new(Body::from(vec![0_u8; 1000])),
            move |delivery| *sink.lock().unwrap() = Some(delivery.bytes),
        );
        assert_eq!(*reported.lock().unwrap(), None);
        to_bytes(response.into_body(), 1 << 20).await.unwrap();
        assert_eq!(*reported.lock().unwrap(), Some(1000));
    }
}
