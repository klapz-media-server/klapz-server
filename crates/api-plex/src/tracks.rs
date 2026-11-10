//! `PUT /library/parts/{partId}`: the audio track or the subtitle a viewer plays a file with.
//!
//! Plex Web sends it when the player's menu changes either. Measured on the reference:
//! `subtitleStreamID` or `audioStreamID` names one of the part's tracks of that kind, and
//! `subtitleStreamID=0` turns subtitles off. A track of another kind or of another file, or
//! neither parameter, is a `400`. The answer is an empty `200`. The choice is the caller's
//! own, and later lookups mark it `selected`.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::put;
use axum::Router;
use open_plex_api::media::{FILE_TYPE, STREAM_TYPE};
use open_plex_api::GlobalId;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/library/parts/{partId}", put(choose))
}

const CHOOSE_TRACKS: &str = r"
mutation PlexChooseTracks($file: ID!, $audio: ID, $subtitle: ID, $off: Boolean) {
  chooseTracks(file: $file, audioTrack: $audio, subtitleTrack: $subtitle, subtitlesOff: $off) {
    id
  }
}
";

#[derive(Debug, serde::Deserialize)]
struct ChooseParams {
    #[serde(rename = "audioStreamID")]
    audio_stream_id: Option<String>,
    #[serde(rename = "subtitleStreamID")]
    subtitle_stream_id: Option<String>,
}

async fn choose(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(part): Path<i64>,
    Query(params): Query<ChooseParams>,
) -> Result<impl IntoResponse, ApiError> {
    let stream =
        |value: &str| -> Result<i64, ApiError> { value.parse().map_err(|_| ApiError::BadRequest) };
    let mut variables = Variables::default();
    variables.insert(
        Name::new("file"),
        Value::String(GlobalId::encode(FILE_TYPE, part)),
    );
    if let Some(audio) = params.audio_stream_id.as_deref() {
        variables.insert(
            Name::new("audio"),
            Value::String(GlobalId::encode(STREAM_TYPE, stream(audio)?)),
        );
    }
    match params
        .subtitle_stream_id
        .as_deref()
        .map(stream)
        .transpose()?
    {
        Some(0) => {
            variables.insert(Name::new("off"), Value::Boolean(true));
        }
        Some(subtitle) => {
            variables.insert(
                Name::new("subtitle"),
                Value::String(GlobalId::encode(STREAM_TYPE, subtitle)),
            );
        }
        None => {}
    }
    // The mutation's refusals are the client's: a track not of the file, of the wrong kind,
    // or nothing named. Logged, so that a storage failure answered the same way is seen.
    let _: serde::de::IgnoredAny = gql::execute(&state.schema, CHOOSE_TRACKS, variables)
        .await
        .map_err(|error| {
            tracing::info!(%error, part, "refused a choice of tracks");
            ApiError::BadRequest
        })?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}
