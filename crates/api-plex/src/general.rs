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

/// The Plex Media Server version this server announces to Plex clients.
///
/// Not our own version: clients gate on the server's, and Plex Web refuses to run against
/// anything older than 1.18.3 — `0.1.0` reads as a very old Plex. This is the version of the
/// reference server every capture in this repository was taken from, so a client that
/// enables a feature by version expects exactly the behavior we copied. The official API
/// keeps publishing klapz's own version.
pub const PLEX_VERSION: &str = "1.43.5.11029-6bf8af3f0";

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
  viewerAccount(extension: PLEX) {
    accountName
  }
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerData {
    server: GqlServer,
    viewer_account: Option<GqlAccount>,
}

/// The plex.tv account the caller's server is paired with.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlAccount {
    pub account_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlServer {
    pub machine_identifier: String,
    pub friendly_name: String,
    pub api_version: String,
    pub platform: String,
    pub platform_version: String,
    #[serde(deserialize_with = "crate::gql::date_time::seconds")]
    pub updated_at: i64,
    pub transcoder: GqlTranscoder,
    pub features: GqlFeatures,
    #[serde(skip)]
    pub account: Option<GqlAccount>,
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
/// and therefore what to offer.
async fn capabilities(
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<CapabilitiesContainer>, ApiError> {
    let server = server(&state).await?;
    Ok(PlexResponse::new(
        CapabilitiesContainer::from_graphql(&server),
        format,
    ))
}

/// What the server is and can do, as the official API reports it.
///
/// Shared by `GET /` and `/media/providers`, which publish the same capabilities under two
/// envelopes: reading them once keeps the two from ever disagreeing.
///
/// # Errors
///
/// Fails when the query fails, which is a defect in the schema rather than client input.
pub(crate) async fn server(state: &PlexState) -> Result<GqlServer, ApiError> {
    let data: ServerData = gql::execute(&state.schema, SERVER_QUERY, Variables::default()).await?;
    Ok(GqlServer {
        account: data.viewer_account,
        ..data.server
    })
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
        machine_identifier: crate::gql::machine_identifier(&state.identity.machine_identifier),
        version: PLEX_VERSION.to_owned(),
    };
    PlexResponse::new(body, format)
}
