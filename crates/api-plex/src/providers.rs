//! `GET /media/providers`: what the server serves, as the web client discovers it.
//!
//! Plex Web asks for this right after the handshake and builds its navigation from it: the
//! sidebar lists the libraries of the `content` feature, and each feature's `key` tells the
//! client where that capability lives. The envelope repeats the capabilities of `GET /`,
//! read through the same query so the two cannot disagree.
//!
//! The features are the reference's seventeen, keys included. Several point at endpoints
//! that still answer `501`; announcing them anyway keeps the contract a real server offers,
//! and the client then asks for exactly what remains to be written.

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::auth::Caller;
use crate::capabilities::CapabilitiesContainer;
use crate::container::{kind_name, LibraryKind};
use crate::error::ApiError;
use crate::library::{libraries, GqlLibrary};
use crate::response::{PlexResponse, ResponseFormat};
use crate::{general, PlexState};

pub fn router() -> Router<PlexState> {
    Router::new().route("/media/providers", get(providers))
}

/// Response to `GET /media/providers`.
///
/// The capability attributes of `GET /`, minus the endpoint groups and a few flags the
/// reference sends only there, followed by the providers.
// The flag count mirrors the reference server's attributes; grouping them would change the
// serialized shape, which is the one thing that must not move.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersContainer {
    pub size: u32,
    pub allow_camera_upload: bool,
    pub allow_channel_access: bool,
    pub allow_media_deletion: bool,
    pub allow_sharing: bool,
    pub allow_sync: bool,
    pub allow_tuners: bool,
    pub api_version: String,
    pub background_processing: bool,
    pub certificate: bool,
    pub companion_proxy: bool,
    pub country_code: String,
    pub diagnostics: String,
    pub event_stream: bool,
    pub friendly_name: String,
    pub livetv: u8,
    pub machine_identifier: String,
    pub music_analysis: u8,
    #[serde(rename = "myPlex")]
    pub my_plex: bool,
    #[serde(rename = "myPlexMappingState", skip_serializing_if = "Option::is_none")]
    pub my_plex_mapping_state: Option<&'static str>,
    #[serde(rename = "myPlexSigninState")]
    pub my_plex_signin_state: &'static str,
    #[serde(rename = "myPlexSubscription")]
    pub my_plex_subscription: bool,
    #[serde(rename = "myPlexUsername", skip_serializing_if = "Option::is_none")]
    pub my_plex_username: Option<String>,
    pub offline_transcode: u8,
    pub owner_features: String,
    pub platform: String,
    pub platform_version: String,
    pub plugin_host: bool,
    pub push_notifications: bool,
    pub read_only_libraries: bool,
    #[serde(rename = "streamingBrainABRVersion")]
    pub streaming_brain_abr_version: u8,
    pub streaming_brain_version: u8,
    pub sync: bool,
    pub transcoder_active_video_sessions: u32,
    pub transcoder_audio: bool,
    pub transcoder_lyrics: bool,
    pub transcoder_subtitles: bool,
    pub transcoder_video: bool,
    pub transcoder_video_bitrates: String,
    pub transcoder_video_qualities: String,
    pub transcoder_video_resolutions: String,
    pub updated_at: i64,
    pub updater: bool,
    pub version: String,
    pub voice_search: bool,
    #[serde(rename = "MediaProvider")]
    pub media_provider: Vec<MediaProvider>,
}

impl ProvidersContainer {
    /// Moves the shared attributes out of the `GET /` body, so a capability can only ever be
    /// computed in one place.
    fn new(capabilities: CapabilitiesContainer, provider: MediaProvider) -> Self {
        Self {
            size: 1,
            allow_camera_upload: capabilities.allow_camera_upload,
            allow_channel_access: capabilities.allow_channel_access,
            allow_media_deletion: capabilities.allow_media_deletion,
            allow_sharing: capabilities.allow_sharing,
            allow_sync: capabilities.allow_sync,
            allow_tuners: capabilities.allow_tuners,
            api_version: capabilities.api_version,
            background_processing: capabilities.background_processing,
            certificate: capabilities.certificate,
            companion_proxy: capabilities.companion_proxy,
            country_code: capabilities.country_code,
            diagnostics: capabilities.diagnostics,
            event_stream: capabilities.event_stream,
            friendly_name: capabilities.friendly_name,
            livetv: capabilities.livetv,
            machine_identifier: capabilities.machine_identifier,
            music_analysis: capabilities.music_analysis,
            my_plex: capabilities.my_plex,
            my_plex_mapping_state: capabilities.my_plex_mapping_state,
            my_plex_signin_state: capabilities.my_plex_signin_state,
            my_plex_subscription: capabilities.my_plex_subscription,
            my_plex_username: capabilities.my_plex_username,
            offline_transcode: capabilities.offline_transcode,
            owner_features: capabilities.owner_features,
            platform: capabilities.platform,
            platform_version: capabilities.platform_version,
            plugin_host: capabilities.plugin_host,
            push_notifications: capabilities.push_notifications,
            read_only_libraries: capabilities.read_only_libraries,
            streaming_brain_abr_version: capabilities.streaming_brain_abr_version,
            streaming_brain_version: capabilities.streaming_brain_version,
            sync: capabilities.sync,
            transcoder_active_video_sessions: capabilities.transcoder_active_video_sessions,
            transcoder_audio: capabilities.transcoder_audio,
            transcoder_lyrics: capabilities.transcoder_lyrics,
            transcoder_subtitles: capabilities.transcoder_subtitles,
            transcoder_video: capabilities.transcoder_video,
            transcoder_video_bitrates: capabilities.transcoder_video_bitrates,
            transcoder_video_qualities: capabilities.transcoder_video_qualities,
            transcoder_video_resolutions: capabilities.transcoder_video_resolutions,
            updated_at: capabilities.updated_at,
            updater: capabilities.updater,
            version: capabilities.version,
            voice_search: capabilities.voice_search,
            media_provider: vec![provider],
        }
    }
}

/// A source of content. A server has one, its own library; channels and online sources
/// would be others.
#[derive(Debug, Serialize)]
pub struct MediaProvider {
    pub identifier: &'static str,
    pub title: &'static str,
    /// The media kinds the server can serve, not the ones its libraries happen to hold.
    pub types: &'static str,
    pub protocols: &'static str,
    #[serde(rename = "Feature")]
    pub feature: Vec<Feature>,
}

/// One capability of a provider, and where it lives.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feature {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flavor: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrobble_key: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unscrobble_key: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<&'static str>,
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    pub directory: Vec<ContentDirectory>,
    #[serde(rename = "Action", skip_serializing_if = "Vec::is_empty")]
    pub action: Vec<FeatureAction>,
}

impl Feature {
    const fn at(kind: &'static str, key: &'static str) -> Self {
        Self {
            flavor: None,
            scrobble_key: None,
            unscrobble_key: None,
            key: Some(key),
            kind,
            directory: Vec::new(),
            action: Vec::new(),
        }
    }

    /// A capability with no endpoint of its own.
    const fn bare(kind: &'static str) -> Self {
        let mut feature = Self::at(kind, "");
        feature.key = None;
        feature
    }

    const fn flavored(mut self, flavor: &'static str) -> Self {
        self.flavor = Some(flavor);
        self
    }
}

/// An entry of the `content` feature: the home screen, a library, or the playlists.
///
/// One type for the three shapes because they share an element and an attribute order;
/// each fills the attributes it has.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDirectory {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanner: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// A string even though a library's is digits, as the reference sends it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_key: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_at: Option<i64>,
    #[serde(rename = "Pivot", skip_serializing_if = "Vec::is_empty")]
    pub pivot: Vec<Pivot>,
}

/// A view of a directory the client offers as a tab: recommendations, the full listing,
/// the collections.
#[derive(Debug, Serialize)]
pub struct Pivot {
    pub id: &'static str,
    pub key: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: &'static str,
    pub context: &'static str,
    pub symbol: &'static str,
}

#[derive(Debug, Serialize)]
pub struct FeatureAction {
    pub id: &'static str,
    pub key: &'static str,
}

/// `GET /media/providers`.
async fn providers(
    _caller: Caller,
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<ProvidersContainer>, ApiError> {
    let server = general::server(&state).await?;
    let libraries = libraries(&state).await?;
    let body = ProvidersContainer::new(
        CapabilitiesContainer::from_graphql(&server),
        library_provider(&libraries),
    );
    Ok(PlexResponse::new(body, format))
}

/// The server's own library provider, with its features in the reference's order.
fn library_provider(libraries: &[GqlLibrary]) -> MediaProvider {
    let mut content = Feature::at("content", "/library/sections");
    content.directory = content_directories(libraries);

    let mut actions = Feature::at("actions", "/actions");
    actions.action = vec![FeatureAction {
        id: "removeFromContinueWatching",
        key: "/actions/removeFromContinueWatching",
    }];

    let mut timeline = Feature::at("timeline", "/:/timeline");
    timeline.scrobble_key = Some("/:/scrobble");
    timeline.unscrobble_key = Some("/:/unscrobble");

    MediaProvider {
        identifier: crate::items::LIBRARY_IDENTIFIER,
        title: "Library",
        types: "video,audio,photo",
        protocols: "stream,download",
        feature: vec![
            content,
            Feature::at("search", "/hubs/search"),
            Feature::at("match", "/library/matches"),
            Feature::at("metadata", "/library/metadata"),
            Feature::at("rate", "/:/rate"),
            Feature::at("imagetranscoder", "/photo/:/transcode"),
            Feature::at("promoted", "/hubs/promoted"),
            Feature::at("continuewatching", "/hubs/continueWatching"),
            actions,
            Feature::at("playlist", "/playlists").flavored("universal"),
            Feature::at("playqueue", "/playQueues").flavored("universal"),
            Feature::at("collection", "/library/collections"),
            timeline,
            Feature::bare("manage"),
            Feature::bare("queryParser"),
            Feature::bare("subscribe").flavored("download"),
            Feature::at("universalsearch", "/library/search"),
        ],
    }
}

/// The home screen, every library, then the playlists — the sidebar, in order.
fn content_directories(libraries: &[GqlLibrary]) -> Vec<ContentDirectory> {
    let home = ContentDirectory {
        hub_key: Some("/hubs".to_owned()),
        title: "Home".to_owned(),
        ..ContentDirectory::default()
    };
    let playlists = ContentDirectory {
        id: Some("playlists".to_owned()),
        key: Some("/playlists".to_owned()),
        kind: Some("playlist"),
        title: "Playlists".to_owned(),
        pivot: vec![Pivot {
            id: "playlists.video",
            key: "/playlists?playlistType=video".to_owned(),
            kind: "list",
            title: "Video",
            context: "content.playlists.video",
            symbol: "playlist",
        }],
        ..ContentDirectory::default()
    };

    std::iter::once(home)
        .chain(libraries.iter().map(library_directory))
        .chain(std::iter::once(playlists))
        .collect()
}

fn library_directory(library: &GqlLibrary) -> ContentDirectory {
    let id = crate::container::decode_key(&library.id);
    ContentDirectory {
        agent: Some(crate::container::agent_for(library.kind)),
        language: Some(library.language.clone()),
        refreshing: Some(false),
        scanner: Some(crate::container::scanner_for(library.kind)),
        uuid: Some(library.uuid.clone()),
        id: Some(id.to_string()),
        key: Some(format!("/library/sections/{id}")),
        hub_key: Some(format!("/hubs/sections/{id}")),
        kind: Some(kind_name(library.kind)),
        title: library.title.clone(),
        updated_at: Some(library.updated_at),
        scanned_at: library.scanned_at,
        pivot: library_pivots(id, library.kind),
    }
}

/// The tabs a library opens on. The listing tab asks for the library's top-level type, by
/// Plex's number for it.
fn library_pivots(id: i64, kind: LibraryKind) -> Vec<Pivot> {
    let top_level_type = match kind {
        LibraryKind::Movie => 1,
        LibraryKind::Show => 2,
        LibraryKind::Artist => 8,
        LibraryKind::Photo => 13,
    };
    vec![
        Pivot {
            id: "recommended",
            key: format!("/hubs/sections/{id}"),
            kind: "hub",
            title: "Recommended",
            context: "content.discover",
            symbol: "star",
        },
        Pivot {
            id: "library",
            key: format!("/library/sections/{id}/all?type={top_level_type}"),
            kind: "list",
            title: "Library",
            context: "content.library",
            symbol: "library",
        },
        Pivot {
            id: "collections",
            key: format!("/library/sections/{id}/collections"),
            kind: "list",
            title: "Collections",
            context: "content.collections",
            symbol: "stack",
        },
    ]
}
