//! `/:/timeline`: a player saying where it stands, every few seconds and at every change.
//!
//! Declared `POST` by the spec; Plex Web sends `GET`, and the reference answers `PUT` too.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::GlobalId;
use serde::Deserialize;
use serde_json::{json, Value as Json};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::playback::resolution_label;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

const ITEM_EXISTS: &str = r"
query PlexTimelineItem($id: ID!) { node(id: $id) { id } }
";
const REPORT: &str = r"
mutation PlexReportPlayback($input: ReportPlaybackInput!) { reportPlayback(input: $input) }
";
const REPORT_VIEWING: &str = r"
mutation PlexReportViewing($input: ReportViewingInput!) {
  reportViewing(input: $input) { outcome state { viewCount offsetMs } }
}
";

const SESSIONS: &str = r"
query PlexTimelineSessions($playback: String!) {
  playbackSessions(first: 500) {
    edges {
      node { key playbackId videoDecision audioDecision videoBitrate audioBitrate width }
    }
  }
  playbackTermination(playbackId: $playback) { reason }
}
";

/// What the reference's decision table calls an administrator's termination. Plex Web only checks
/// that a code is there, and shows the text.
const ADMIN_TERMINATION_CODE: u32 = 2006;
const ADMIN_TERMINATION_TEXT: &str = "Admin Terminated Playback.";

pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    router.route("/:/timeline", get(timeline).post(timeline).put(timeline))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimelineParams {
    state: Option<String>,
    time: Option<String>,
    duration: Option<String>,
    rating_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewingData {
    report_viewing: GqlViewingReport,
}

#[derive(Debug, Deserialize)]
struct GqlViewingReport {
    outcome: String,
    state: GqlViewingState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlViewingState {
    view_count: u32,
    offset_ms: Option<i64>,
}

/// Records the report on the item's watch state and says what became of it, in the
/// reference's words: `ignore`, `progress` or `complete`, then the item's count and offset,
/// `-1` standing for none. Failing to record is no reason to disturb a film: the answer is
/// then the reference's to a report it ignored.
///
/// A report too early to keep reads `progress` from a player that names its session and
/// `ignore` from one that does not, as measured; nothing is kept either way.
async fn record_viewing(
    state: &PlexState,
    item: i64,
    playback: String,
    named_session: bool,
    position: i64,
    duration: Option<i64>,
) -> Json {
    let fields = [
        (
            Name::new("itemId"),
            Value::String(GlobalId::encode(ITEM_TYPE, item)),
        ),
        (Name::new("playbackId"), Value::String(playback)),
        (Name::new("positionMs"), Value::Number(position.into())),
        (
            Name::new("durationMs"),
            duration.map_or(Value::Null, |duration| Value::Number(duration.into())),
        ),
    ];
    let mut variables = Variables::default();
    variables.insert(
        Name::new("input"),
        Value::Object(fields.into_iter().collect()),
    );
    match gql::execute::<ViewingData>(&state.schema, REPORT_VIEWING, variables).await {
        Ok(data) => {
            let report = data.report_viewing;
            json!({
                "playbackState": match report.outcome.as_str() {
                    "PROGRESS" => "progress",
                    "EARLY" if named_session => "progress",
                    "COMPLETED" => "complete",
                    _ => "ignore",
                },
                "skipCount": 0,
                "viewCount": report.state.view_count,
                "viewOffset": report.state.offset_ms.unwrap_or(-1),
            })
        }
        Err(error) => {
            tracing::warn!(%error, "could not record a watch report");
            json!({ "playbackState": "ignore" })
        }
    }
}

fn reported_state(state: Option<&str>) -> Result<&'static str, ApiError> {
    match state {
        Some("playing") => Ok("PLAYING"),
        Some("paused") => Ok("PAUSED"),
        Some("buffering") => Ok("BUFFERING"),
        Some("stopped") => Ok("STOPPED"),
        _ => Err(ApiError::BadRequest),
    }
}

#[derive(Debug, Deserialize)]
struct NodeData {
    node: Option<serde::de::IgnoredAny>,
}

async fn require_item(state: &PlexState, key: i64) -> Result<(), ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, key)),
    );
    let data: NodeData = gql::execute(&state.schema, ITEM_EXISTS, variables).await?;
    data.node.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Failing to record a report is no reason to disturb a film: the next one comes in seconds.
async fn report(state: &PlexState, playback_id: String, reported: &str, position: Option<i64>) {
    let fields = [
        (Name::new("playbackId"), Value::String(playback_id)),
        (Name::new("state"), Value::Enum(Name::new(reported))),
        (
            Name::new("positionMs"),
            position.map_or(Value::Null, |position| Value::Number(position.into())),
        ),
    ];
    let mut variables = Variables::default();
    variables.insert(
        Name::new("input"),
        Value::Object(fields.into_iter().collect()),
    );
    let recorded: Result<serde::de::IgnoredAny, ApiError> =
        gql::execute(&state.schema, REPORT, variables).await;
    if let Err(error) = recorded {
        tracing::warn!(%error, "could not record a playback report");
    }
}

/// The checks run in the order the reference applies them: a bad `state` or `time` is refused
/// even for an item that does not exist, and a `ratingKey` that is not a number is ignored
/// rather than refused.
///
/// `playbackState` says what became of the item's watch state, and Plex Web moves its own copy
/// of the position on `progress` and `complete`. A playback is the client's session, or the
/// client itself when it names none, as on the reference.
async fn timeline(
    _caller: Caller,
    State(state): State<PlexState>,
    client: ClientIdentity,
    format: ResponseFormat,
    Query(params): Query<TimelineParams>,
) -> Result<PlexResponse<Json>, ApiError> {
    let reported = reported_state(params.state.as_deref())?;
    let position = params
        .time
        .as_deref()
        .map(str::parse::<i64>)
        .transpose()
        .map_err(|_| ApiError::BadRequest)?;

    let duration = params
        .duration
        .as_deref()
        .and_then(|duration| duration.parse::<i64>().ok());

    let mut body = json!({ "size": 0, "playbackState": "ignore" });
    let item = params
        .rating_key
        .as_deref()
        .and_then(|key| key.parse().ok());
    if let Some(item) = item {
        require_item(&state, item).await?;
        let playback = client
            .session_identifier
            .clone()
            .or_else(|| client.identifier.as_ref().map(|id| id.as_str().to_owned()));
        if let (Some(playback), Some(position)) = (playback, position) {
            let named_session = client.session_identifier.is_some();
            let viewing =
                record_viewing(&state, item, playback, named_session, position, duration).await;
            if let (Some(body), Json::Object(viewing)) = (body.as_object_mut(), viewing) {
                body.extend(viewing);
            }
        }
        if let Some(playback_id) = client.session_identifier {
            // Read before the report, which ends the sessions once the player has stopped: the
            // reference tears the conversion down then, and so does the official API.
            let found = playback_state(&state, &playback_id).await;
            report(&state, playback_id.clone(), reported, position).await;
            match (&found.session, reported) {
                (_, "STOPPED") | (None, _) => {}
                (Some(session), _) => {
                    // The key GraphQL hands back is the client's; the transcoder stores it under
                    // the caller's name.
                    let status = state
                        .transcoder
                        .session_status(&gql::session_key(&session.key));
                    describe_session(&mut body, session, status.as_ref());
                }
            }
            if let (Some(termination), false) = (found.termination, reported == "STOPPED") {
                body["terminationCode"] = json!(ADMIN_TERMINATION_CODE);
                body["terminationText"] = json!(termination
                    .reason
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or_else(|| ADMIN_TERMINATION_TEXT.to_owned()));
            }
        }
    }

    Ok(PlexResponse::new(body, format))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionsData {
    playback_sessions: GqlSessions,
    playback_termination: Option<GqlTermination>,
}

#[derive(Debug, Deserialize)]
struct GqlTermination {
    reason: Option<String>,
}

/// Where a playback stands: its latest session playing, or an owner's termination of it.
#[derive(Debug, Default)]
struct PlaybackFound {
    session: Option<GqlSession>,
    termination: Option<GqlTermination>,
}

#[derive(Debug, Deserialize)]
struct GqlSessions {
    edges: Vec<GqlSessionEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlSessionEdge {
    node: GqlSession,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlSession {
    key: String,
    playback_id: Option<String>,
    video_decision: String,
    audio_decision: String,
    video_bitrate: Option<i32>,
    audio_bitrate: Option<i32>,
    width: Option<i32>,
}

/// Rows come oldest first, and the latest is the conversion the player is on. This only
/// decorates the answer: failing to read it costs a report nothing.
async fn playback_state(state: &PlexState, playback_id: &str) -> PlaybackFound {
    let mut variables = Variables::default();
    variables.insert(Name::new("playback"), Value::String(playback_id.to_owned()));
    let data: Result<SessionsData, ApiError> =
        gql::execute(&state.schema, SESSIONS, variables).await;
    let Ok(data) =
        data.map_err(|error| tracing::warn!(%error, "could not read the playback sessions"))
    else {
        return PlaybackFound::default();
    };
    PlaybackFound {
        session: data
            .playback_sessions
            .edges
            .into_iter()
            .rev()
            .map(|edge| edge.node)
            .find(|session| session.playback_id.as_deref() == Some(playback_id)),
        termination: data.playback_termination,
    }
}

/// What the player is being sent: the rung of the latest video segment it received when the
/// conversion is running, which an adaptive stream changes, or what the session recorded at
/// its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sent {
    video_bits: Option<i32>,
    video_copied: bool,
    audio_bits: Option<i32>,
    audio_copied: bool,
    width: u32,
}

impl Sent {
    fn of(
        session: &GqlSession,
        status: Option<&open_plex_transcoder::session::SessionStatus>,
    ) -> Self {
        let recorded = Self {
            video_bits: session.video_bitrate,
            video_copied: session.video_decision == "COPY",
            audio_bits: session.audio_bitrate,
            audio_copied: session.audio_decision == "COPY",
            width: session.width.map_or(0, i32::unsigned_abs),
        };
        let Some(status) = status else {
            return recorded;
        };
        let bits = |kbps: u32| i32::try_from(u64::from(kbps) * 1000).ok();
        let audio = match status.audio_kbps {
            Some(kbps) => (bits(kbps), false),
            None => (recorded.audio_bits, true),
        };
        match status.video_encode {
            Some(encode) => Self {
                video_bits: bits(encode.video_kbps),
                video_copied: false,
                audio_bits: audio.0,
                audio_copied: audio.1,
                width: encode.width,
            },
            // The copy is the source, whose measurements the session recorded when it began
            // on it; a stream that began on an encode has no copy among its rungs.
            None => recorded,
        }
    }

    /// A copied track weighs 2.1 times its bitrate and an encoded one 1.05, rounded up over
    /// their sum: every `bandwidth` measured on the reference fits this.
    fn bandwidth_kbps(self) -> i64 {
        let weighted = |bits: Option<i32>, copied: bool| {
            let kbps = i64::from(bits.unwrap_or(0)) / 1000;
            kbps * if copied { 210 } else { 105 }
        };
        let total = weighted(self.video_bits, self.video_copied)
            + weighted(self.audio_bits, self.audio_copied);
        (total + 99) / 100
    }
}

/// What Plex Web reads to show the current quality and buffered range. `TranscodeSession.key`
/// has to be the session the player started; the fields follow a capture of the reference.
fn describe_session(
    body: &mut Json,
    session: &GqlSession,
    status: Option<&open_plex_transcoder::session::SessionStatus>,
) {
    let sent = Sent::of(session, status);
    body["size"] = json!(2);
    body["Bandwidths"] = json!([{
        "Bandwidth": [{
            "time": 0,
            "bandwidth": sent.bandwidth_kbps(),
            "resolution": resolution_label(sent.width),
        }],
    }]);
    let key = format!("/transcode/sessions/{}", session.key);
    body["TranscodeSession"] = json!([transcode_session(&key, session, status)]);
}

/// The reference's `TranscodeSession` object, filled from the live conversion where it is known.
/// A gone conversion still answers with the shape a client expects, from what the session stored.
fn transcode_session(
    key: &str,
    session: &GqlSession,
    status: Option<&open_plex_transcoder::session::SessionStatus>,
) -> Json {
    let Some(status) = status else {
        return json!({
            "key": key,
            "throttled": false,
            "complete": false,
            "context": "streaming",
            "videoDecision": session.video_decision.to_ascii_lowercase(),
            "audioDecision": session.audio_decision.to_ascii_lowercase(),
        });
    };
    let mut transcode = json!({
        "key": key,
        "throttled": false,
        "complete": status.complete,
        "progress": status.progress,
        "size": -1,
        "speed": status.speed,
        "error": false,
        "duration": status.duration_ms,
        "remaining": status.remaining_seconds,
        "context": "streaming",
        "sourceVideoCodec": status.source_video_codec,
        "sourceAudioCodec": status.source_audio_codec,
        "videoDecision": status.video_decision,
        "audioDecision": status.audio_decision,
        "subtitleDecision": "ignore",
        "protocol": status.protocol,
        "container": status.container,
        "videoCodec": status.video_codec,
        "audioCodec": status.audio_codec,
        "transcodeHwRequested": false,
        "timeStamp": open_plex_core::now_epoch(),
        "maxOffsetAvailable": status.max_offset_seconds,
        "minOffsetAvailable": status.min_offset_seconds,
    });
    if let Some(channels) = status.audio_channels {
        transcode["audioChannels"] = json!(channels);
    }
    transcode
}

#[cfg(test)]
mod tests {
    use open_plex_transcoder::plan::{KeyFrames, VideoEncode};
    use open_plex_transcoder::session::SessionStatus;

    use super::*;

    /// Soul started on its copy, with E-AC-3 converted to AAC.
    fn started_on_the_copy() -> GqlSession {
        GqlSession {
            key: "k".to_owned(),
            playback_id: None,
            video_decision: "COPY".to_owned(),
            audio_decision: "TRANSCODE".to_owned(),
            video_bitrate: Some(7_726_000),
            audio_bitrate: Some(256_000),
            width: Some(1920),
        }
    }

    fn status(video_encode: Option<VideoEncode>) -> SessionStatus {
        SessionStatus {
            protocol: "dash",
            container: "mp4",
            source_video_codec: "h264".to_owned(),
            source_audio_codec: "eac3".to_owned(),
            video_decision: if video_encode.is_some() {
                "transcode"
            } else {
                "copy"
            },
            audio_decision: "transcode",
            video_codec: "h264".to_owned(),
            audio_codec: "aac".to_owned(),
            audio_channels: Some(2),
            video_encode,
            audio_kbps: Some(256),
            duration_ms: 6_031_568,
            max_offset_seconds: 0.0,
            min_offset_seconds: 0.0,
            progress: 0.0,
            speed: 0.0,
            remaining_seconds: 0,
            complete: false,
        }
    }

    #[test]
    fn the_bandwidth_reported_is_the_rung_being_sent() {
        let session = started_on_the_copy();
        // 7726 × 2.1 + 256 × 1.05, rounded up.
        assert_eq!(Sent::of(&session, None).bandwidth_kbps(), 16_494);
        assert_eq!(
            Sent::of(&session, Some(&status(None))).bandwidth_kbps(),
            16_494
        );
        let lower = VideoEncode {
            width: 720,
            height: 404,
            video_kbps: 1297,
            audio_kbps: 256,
            crf: 21,
            frame_rate: Some(23.976),
            key_frames: KeyFrames::AsSource,
            shared_headers: false,
        };
        let sent = Sent::of(&session, Some(&status(Some(lower))));
        // 1297 × 1.05 + 256 × 1.05, rounded up; and "SD" for 720 wide.
        assert_eq!(sent.bandwidth_kbps(), 1631);
        assert_eq!(resolution_label(sent.width), "SD");
    }
}
