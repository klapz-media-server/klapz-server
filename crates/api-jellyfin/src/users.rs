//! Signing in and out, and who the caller is.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::ConnectInfo;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Extension;
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::ClientInfo;
use crate::error::JellyfinError;
use crate::gql::{self, Viewer};
use crate::query::Query;
use crate::response::{json, no_content};
use crate::time;
use crate::JellyfinState;

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/users/authenticatebyname", post(authenticate_by_name))
        .route("/users/me", get(me))
        .route("/users/public", get(public_users))
        .route("/users/{user}", get(user))
        .route("/sessions/logout", post(log_out))
        .route("/sessions/capabilities", post(capabilities))
        .route("/sessions/capabilities/full", post(capabilities))
        .route(
            "/displaypreferences/{id}",
            get(display_preferences).post(keep_display_preferences),
        )
}

/// Jellyfin's `UserDto`. Every user here gets the policy of a user with access to every
/// library it reaches, and none of an administrator's: the server is managed at `/admin`, and a
/// client shown an administrator would open dashboards over machinery this server lacks.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Jellyfin's wire shape, field for field"
)]
pub struct UserDto {
    name: String,
    server_id: String,
    id: String,
    has_password: bool,
    has_configured_password: bool,
    has_configured_easy_password: bool,
    enable_auto_login: bool,
    configuration: serde_json::Value,
    policy: serde_json::Value,
}

pub fn user_dto(viewer: &Viewer, server_id: &str) -> UserDto {
    UserDto {
        name: viewer.username.clone(),
        server_id: server_id.to_owned(),
        id: viewer.jellyfin_id(),
        has_password: viewer.has_password,
        has_configured_password: viewer.has_password,
        has_configured_easy_password: false,
        enable_auto_login: false,
        configuration: json!({
            "PlayDefaultAudioTrack": true,
            "SubtitleLanguagePreference": "",
            "DisplayMissingEpisodes": false,
            "GroupedFolders": [],
            "SubtitleMode": "Default",
            "DisplayCollectionsView": false,
            "EnableLocalPassword": false,
            "OrderedViews": [],
            "LatestItemsExcludes": [],
            "MyMediaExcludes": [],
            "HidePlayedInLatest": true,
            "RememberAudioSelections": true,
            "RememberSubtitleSelections": true,
            "EnableNextEpisodeAutoPlay": true,
        }),
        policy: json!({
            "IsAdministrator": false,
            "IsHidden": true,
            "EnableCollectionManagement": false,
            "EnableSubtitleManagement": false,
            "EnableLyricManagement": false,
            "IsDisabled": false,
            "BlockedTags": [],
            "AllowedTags": [],
            "EnableUserPreferenceAccess": true,
            "AccessSchedules": [],
            "BlockUnratedItems": [],
            "EnableRemoteControlOfOtherUsers": false,
            "EnableSharedDeviceControl": false,
            "EnableRemoteAccess": true,
            "EnableLiveTvManagement": false,
            "EnableLiveTvAccess": false,
            "EnableMediaPlayback": true,
            "EnableAudioPlaybackTranscoding": true,
            "EnableVideoPlaybackTranscoding": true,
            "EnablePlaybackRemuxing": true,
            "ForceRemoteSourceTranscoding": false,
            "EnableContentDeletion": false,
            "EnableContentDeletionFromFolders": [],
            "EnableContentDownloading": true,
            "EnableSyncTranscoding": false,
            "EnableMediaConversion": false,
            "EnabledDevices": [],
            "EnableAllDevices": true,
            "EnabledChannels": [],
            "EnableAllChannels": false,
            "EnabledFolders": [],
            "EnableAllFolders": true,
            "InvalidLoginAttemptCount": 0,
            "LoginAttemptsBeforeLockout": -1,
            "MaxActiveSessions": 0,
            "EnablePublicSharing": false,
            "BlockedMediaFolders": [],
            "BlockedChannels": [],
            "RemoteClientBitrateLimit": 0,
            "AuthenticationProviderId":
                "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider",
            "PasswordResetProviderId":
                "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider",
            "SyncPlayAccess": "None",
        }),
    }
}

/// The body of a sign-in. The reference reads `Pw`; `Password` is what older clients sent.
#[derive(Debug, Deserialize)]
struct Credentials {
    #[serde(rename = "Username", alias = "username")]
    username: String,
    #[serde(
        rename = "Pw",
        alias = "pw",
        alias = "Password",
        alias = "password",
        default
    )]
    password: String,
}

const SIGN_IN: &str = r"mutation($input: ExtensionSignInInput!) {
    signInThroughExtension(input: $input) {
        token
        user { id username hasPassword }
        userErrors { code }
    }
}";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignInData {
    sign_in_through_extension: SignInPayload,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignInPayload {
    token: Option<String>,
    user: Option<Viewer>,
}

/// The `SessionInfo` a sign-in answers with: the device that signed in, playing nothing.
fn session_info(
    viewer: &Viewer,
    client: &ClientInfo,
    server_id: &str,
    peer: Option<SocketAddr>,
) -> serde_json::Value {
    let device_id = client.device_id.clone().unwrap_or_default();
    let mut session = json!({
        "PlayState": {
            "CanSeek": false,
            "IsPaused": false,
            "IsMuted": false,
            "RepeatMode": "RepeatNone",
            "PlaybackOrder": "Default",
        },
        "AdditionalUsers": [],
        "Capabilities": {
            "PlayableMediaTypes": [],
            "SupportedCommands": [],
            "SupportsMediaControl": false,
            "SupportsPersistentIdentifier": true,
        },
        "PlayableMediaTypes": [],
        "Id": session_id(viewer, &device_id),
        "UserId": viewer.jellyfin_id(),
        "UserName": viewer.username,
        "Client": client.client.clone().unwrap_or_default(),
        "LastActivityDate": time::now(),
        "LastPlaybackCheckIn": time::NEVER,
        "DeviceName": client.device.clone().unwrap_or_default(),
        "DeviceId": device_id,
        "ApplicationVersion": client.version.clone().unwrap_or_default(),
        "IsActive": true,
        "SupportsMediaControl": false,
        "SupportsRemoteControl": false,
        "NowPlayingQueue": [],
        "HasCustomDeviceName": false,
        "ServerId": server_id,
        "SupportedCommands": [],
    });
    if let Some(peer) = peer {
        session["RemoteEndPoint"] = json!(peer.ip().to_string());
    }
    session
}

/// A session is one user on one device: its id, 32 digits, follows from both.
fn session_id(viewer: &Viewer, device_id: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(format!("{}\0{device_id}", viewer.id))
        .iter()
        .take(16)
        .fold(String::with_capacity(32), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

async fn authenticate_by_name(
    State(state): State<JellyfinState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, JellyfinError> {
    if !crate::auth::has_client_header(&headers) {
        return Err(JellyfinError::BadRequest(
            "a sign-in names its client in the Authorization header",
        ));
    }
    let credentials: Credentials = serde_json::from_slice(&body)
        .map_err(|_| JellyfinError::BadRequest("a sign-in carries credentials"))?;
    let client = ClientInfo::of(&headers);
    let variables = json!({ "input": {
        "extension": "JELLYFIN",
        "login": credentials.username,
        "password": credentials.password,
        "deviceId": client.device_id.clone().unwrap_or_default(),
        "deviceName": client.device.clone().unwrap_or_default(),
        "client": client.client.clone().unwrap_or_default(),
        "clientVersion": client.version.clone().unwrap_or_default(),
    }});
    let request = async_graphql::Request::new(SIGN_IN)
        .variables(async_graphql::Variables::from_json(variables))
        .data(std::sync::Arc::clone(&state.sessions));
    let data: SignInData = gql::run(&state.schema, request).await?;
    // A wrong password, an account waiting for an owner and a user with no Jellyfin
    // connection all look alike to the client, as a wrong password does on the reference.
    let (Some(token), Some(viewer)) = (
        data.sign_in_through_extension.token,
        data.sign_in_through_extension.user,
    ) else {
        return Err(JellyfinError::Unauthorized);
    };
    Ok(json(&json!({
        "User": user_dto(&viewer, &state.server.id),
        "SessionInfo": session_info(
            &viewer,
            &client,
            &state.server.id,
            peer.map(|Extension(ConnectInfo(peer))| peer),
        ),
        "AccessToken": token,
        "ServerId": state.server.id,
    })))
}

async fn me(State(state): State<JellyfinState>) -> Result<Response, JellyfinError> {
    let viewer = gql::viewer(&state.schema).await?;
    Ok(json(&user_dto(&viewer, &state.server.id)))
}

async fn user(
    State(state): State<JellyfinState>,
    Path(user): Path<String>,
) -> Result<Response, JellyfinError> {
    let viewer = gql::viewer(&state.schema).await?;
    viewer.require_self(Some(&user))?;
    Ok(json(&user_dto(&viewer, &state.server.id)))
}

/// The users a sign-in page may offer to pick from. None: a name is typed, as on the
/// reference, whose users are hidden by default.
async fn public_users() -> Response {
    json(&[(); 0])
}

const SIGN_OUT: &str = r"mutation($token: String!) {
    signOutOfExtension(extension: JELLYFIN, token: $token)
}";

async fn log_out(
    State(state): State<JellyfinState>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let token =
        crate::auth::token(&headers, query.as_deref()).ok_or(JellyfinError::Unauthorized)?;
    let _: serde_json::Value =
        gql::query(&state.schema, SIGN_OUT, json!({ "token": token })).await?;
    Ok(no_content())
}

/// What a client can be told to do remotely. No remote control here: noted and dropped, as
/// nothing reads it.
async fn capabilities(State(state): State<JellyfinState>) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?;
    Ok(no_content())
}

/// A view's layout as a client keeps it: the reference's defaults, as nothing is stored here.
async fn display_preferences(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?;
    let query = Query::parse(query.as_deref());
    Ok(json(&json!({
        "Id": id,
        "SortBy": "SortName",
        "RememberIndexing": false,
        "PrimaryImageHeight": 250,
        "PrimaryImageWidth": 250,
        "CustomPrefs": {},
        "ScrollDirection": "Horizontal",
        "ShowBackdrop": true,
        "RememberSorting": false,
        "SortOrder": "Ascending",
        "ShowSidebar": false,
        "Client": query.get("client").unwrap_or("emby"),
    })))
}

async fn keep_display_preferences(
    State(state): State<JellyfinState>,
) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?;
    Ok(no_content())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn alice() -> Viewer {
        Viewer {
            id: open_plex_api::GlobalId::encode("User", 3),
            username: "alice".to_owned(),
            has_password: true,
        }
    }

    #[test]
    fn a_user_is_shaped_as_jellyfin_writes_it_and_never_an_administrator() {
        let dto = serde_json::to_value(user_dto(&alice(), "server")).unwrap();
        assert_eq!(dto["Name"], "alice");
        assert_eq!(dto["ServerId"], "server");
        assert_eq!(dto["Id"], crate::id::of("User", 3).unwrap());
        assert_eq!(dto["HasPassword"], true);
        // An owner too: the server is managed at /admin.
        assert_eq!(dto["Policy"]["IsAdministrator"], false);
        assert_eq!(dto["Policy"]["EnableAllFolders"], true);
        assert!(dto.get("LastLoginDate").is_none());
    }

    #[test]
    fn a_sign_in_reads_the_password_under_either_name() {
        for body in [
            r#"{"Username":"alice","Pw":"x"}"#,
            r#"{"Username":"alice","Password":"x"}"#,
        ] {
            let credentials: Credentials = serde_json::from_str(body).unwrap();
            assert_eq!(
                (credentials.username.as_str(), credentials.password.as_str()),
                ("alice", "x")
            );
        }
        let empty: Credentials = serde_json::from_str(r#"{"Username":"alice"}"#).unwrap();
        assert_eq!(empty.password, "");
    }

    #[test]
    fn a_session_is_one_user_on_one_device() {
        let alice = alice();
        let mut bob = alice.clone();
        bob.id = open_plex_api::GlobalId::encode("User", 4);
        assert_eq!(session_id(&alice, "d1"), session_id(&alice, "d1"));
        assert_ne!(session_id(&alice, "d1"), session_id(&alice, "d2"));
        assert_ne!(session_id(&alice, "d1"), session_id(&bob, "d1"));
        assert_eq!(session_id(&alice, "d1").len(), 32);
    }

    #[test]
    fn the_session_names_the_device_that_signed_in() {
        let client = ClientInfo {
            client: Some("Jellyfin Web".to_owned()),
            device: Some("Firefox".to_owned()),
            device_id: Some("d1".to_owned()),
            version: Some("10.11.0".to_owned()),
            token: None,
        };
        let session = session_info(&alice(), &client, "server", "10.0.0.2:5000".parse().ok());
        assert_eq!(session["Client"], "Jellyfin Web");
        assert_eq!(session["DeviceName"], "Firefox");
        assert_eq!(session["DeviceId"], "d1");
        assert_eq!(session["ApplicationVersion"], "10.11.0");
        assert_eq!(session["UserName"], "alice");
        assert_eq!(session["ServerId"], "server");
        assert_eq!(session["LastPlaybackCheckIn"], time::NEVER);
        assert_eq!(session["RemoteEndPoint"], "10.0.0.2");
    }
}
