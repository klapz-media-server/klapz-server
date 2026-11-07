//! Handshake and server-level endpoints.

use async_graphql::Variables;
use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::capabilities::CapabilitiesContainer;
use crate::container::IdentityContainer;
use crate::error::ApiError;
use crate::gql;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/", get(capabilities))
        .route("/identity", get(identity))
}

/// Everything `GET /` needs from the official API.
const SERVER_QUERY: &str = r"
query PlexCapabilities {
  server {
    machineIdentifier
    friendlyName
    version
    apiVersion
    platform
    platformVersion
    updatedAt
    transcoder {
      video
      audio
      subtitles
      photo
      lyrics
      activeVideoSessions
    }
    features {
      sync
      cameraUpload
      sharing
      mediaDeletion
      tuners
      channelAccess
      hubSearch
      voiceSearch
      eventStream
      multiuser
      collections
      adaptiveBitrate
      skipMarkers
      loudness
      liveTv
      hardwareTranscoding
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct ServerData {
    server: GqlServer,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlServer {
    pub machine_identifier: String,
    pub friendly_name: String,
    pub version: String,
    pub api_version: String,
    pub platform: String,
    pub platform_version: String,
    pub updated_at: i64,
    pub transcoder: GqlTranscoder,
    pub features: GqlFeatures,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlTranscoder {
    pub video: bool,
    pub audio: bool,
    pub subtitles: bool,
    pub photo: bool,
    pub lyrics: bool,
    pub active_video_sessions: u32,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlFeatures {
    pub sync: bool,
    pub camera_upload: bool,
    pub sharing: bool,
    pub media_deletion: bool,
    pub tuners: bool,
    pub channel_access: bool,
    pub hub_search: bool,
    pub voice_search: bool,
    pub event_stream: bool,
    pub multiuser: bool,
    pub collections: bool,
    pub adaptive_bitrate: bool,
    pub skip_markers: bool,
    pub loudness: bool,
    pub live_tv: bool,
    pub hardware_transcoding: bool,
}

/// `GET /` — the capabilities handshake.
///
/// The first request any client makes: it reads the answer to decide what the server can do,
/// and therefore what to offer. Public, like `/identity`, because a client asks before it
/// holds a token.
async fn capabilities(
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<CapabilitiesContainer>, ApiError> {
    let data: ServerData = gql::execute(&state.schema, SERVER_QUERY, Variables::default()).await?;
    Ok(PlexResponse::new(
        CapabilitiesContainer::from_graphql(&data.server),
        format,
    ))
}

/// `GET /identity` — unauthenticated by design.
///
/// Clients call this before holding any token, to decide whether a host is a Plex server at
/// all. Requiring a token here would make the server undiscoverable.
async fn identity(
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> PlexResponse<IdentityContainer> {
    let body = IdentityContainer {
        size: 0,
        api_version: state.identity.api_version.clone(),
        claimed: true,
        machine_identifier: state.identity.machine_identifier.clone(),
        version: state.identity.version.clone(),
    };
    PlexResponse::new(body, format)
}
