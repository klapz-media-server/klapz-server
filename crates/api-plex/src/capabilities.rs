//! The response body for `GET /`.
//!
//! Field order and types follow a capture of the reference server. Two values deserve a note.
//!
//! `myPlexSubscription` reports **true**. Clients read it to decide whether to offer the
//! features Plex reserves for a subscription, and open-plex gates nothing — so the answer
//! that unlocks them is the correct one here.
//!
//! `myPlex` reports **false**, because the server genuinely is not linked to a plex.tv
//! account. Claiming otherwise would be a lie that breaks differently, later and less
//! legibly, than an absent link does.

use serde::Serialize;

use crate::general::{GqlFeatures, GqlServer};

/// The groups the reference server lists, with the counts it reports.
///
/// These enumerate *Plex's* endpoint families, so they are pure protocol and belong here
/// rather than in the official API.
const CAPABILITY_DIRECTORIES: &[(&str, u32)] = &[
    ("actions", 1),
    ("activities", 1),
    ("butler", 1),
    ("channels", 1),
    ("clients", 1),
    ("devices", 1),
    ("diagnostics", 1),
    ("downloadQueue", 1),
    ("hubs", 1),
    ("library", 3),
    ("livetv", 3),
    ("media", 3),
    ("neighborhood", 1),
    ("playQueues", 1),
    ("playlists", 1),
    ("resources", 1),
    ("search", 1),
    ("server", 1),
    ("servers", 1),
    ("service", 1),
    ("statistics", 1),
    ("system", 1),
    ("transcode", 1),
    ("updater", 1),
    ("user", 1),
];

/// The ladders the reference server advertises.
const TRANSCODER_BITRATES: &str = "64,96,208,320,720,1500,2000,3000,4000,8000,10000,12000,20000";
const TRANSCODER_QUALITIES: &str = "0,1,2,3,4,5,6,7,8,9,10,11,12";
const TRANSCODER_RESOLUTIONS: &str = "128,128,160,240,320,480,768,720,720,1080,1080,1080,1080";

/// One endpoint group the server advertises.
#[derive(Debug, Serialize)]
pub struct CapabilityDirectory {
    pub count: u32,
    pub key: &'static str,
    pub title: &'static str,
}

/// Response to `GET /`: what the server is and what it can do.
// The flag count mirrors the reference server's attributes; grouping them would change
// the serialized shape, which is the one thing that must not move.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesContainer {
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
    /// ISO 3166-1 alpha-3 country, used for age-rating conventions. Empty when unset.
    pub country_code: String,
    /// Which diagnostic bundles the server can produce.
    pub diagnostics: String,
    pub event_stream: bool,
    pub friendly_name: String,
    pub hub_search: bool,
    pub item_clusters: bool,
    pub livetv: u8,
    pub machine_identifier: String,
    pub media_providers: bool,
    pub multiuser: bool,
    pub music_analysis: u8,
    #[serde(rename = "myPlex")]
    pub my_plex: bool,
    /// Whether a plex.tv account is signed in. Always `none`: there is none to sign in.
    #[serde(rename = "myPlexSigninState")]
    pub my_plex_signin_state: &'static str,
    #[serde(rename = "myPlexSubscription")]
    pub my_plex_subscription: bool,
    pub offline_transcode: u8,
    /// The named capabilities Plex lists alongside its subscription GUIDs.
    ///
    /// A client checking by name finds every one. We publish no GUIDs: they identify an
    /// account's entitlements upstream and mean nothing here.
    pub owner_features: String,
    pub platform: String,
    pub platform_version: String,
    pub plugin_host: bool,
    pub push_notifications: bool,
    pub read_only_libraries: bool,
    /// Versions of Plex's bitrate-decision logic. Reported for clients that branch on them.
    #[serde(rename = "streamingBrainABRVersion")]
    pub streaming_brain_abr_version: u8,
    pub streaming_brain_version: u8,
    pub sync: bool,
    pub transcoder_active_video_sessions: u32,
    pub transcoder_audio: bool,
    pub transcoder_lyrics: bool,
    pub transcoder_photo: bool,
    pub transcoder_subtitles: bool,
    pub transcoder_video: bool,
    pub transcoder_video_bitrates: String,
    pub transcoder_video_qualities: String,
    pub transcoder_video_resolutions: String,
    pub updated_at: i64,
    pub updater: bool,
    pub version: String,
    pub voice_search: bool,
    #[serde(rename = "Directory")]
    pub directory: Vec<CapabilityDirectory>,
}

impl CapabilitiesContainer {
    pub fn from_graphql(server: &GqlServer) -> Self {
        let features = &server.features;
        let transcoder = &server.transcoder;

        Self {
            size: u32::try_from(CAPABILITY_DIRECTORIES.len()).unwrap_or(u32::MAX),
            allow_camera_upload: features.camera_upload,
            allow_channel_access: features.channel_access,
            allow_media_deletion: features.media_deletion,
            allow_sharing: features.sharing,
            allow_sync: features.sync,
            allow_tuners: features.tuners,
            api_version: server.api_version.clone(),
            background_processing: true,
            certificate: false,
            companion_proxy: false,
            country_code: String::new(),
            diagnostics: "logs,databases".to_owned(),
            event_stream: features.event_stream,
            friendly_name: server.friendly_name.clone(),
            hub_search: features.hub_search,
            item_clusters: true,
            // Plex reports a capability level rather than a flag here; 7 is what a server
            // with tuners and recording advertises.
            livetv: if features.live_tv { 7 } else { 0 },
            machine_identifier: server.machine_identifier.clone(),
            media_providers: true,
            multiuser: features.multiuser,
            music_analysis: if features.loudness { 2 } else { 0 },
            my_plex: false,
            my_plex_signin_state: "none",
            my_plex_subscription: true,
            offline_transcode: u8::from(features.sync),
            owner_features: named_features(features),
            platform: server.platform.clone(),
            platform_version: server.platform_version.clone(),
            plugin_host: false,
            push_notifications: false,
            read_only_libraries: false,
            streaming_brain_abr_version: 3,
            streaming_brain_version: 2,
            sync: features.sync,
            transcoder_active_video_sessions: transcoder.active_video_sessions,
            transcoder_audio: transcoder.audio,
            transcoder_lyrics: transcoder.lyrics,
            transcoder_photo: transcoder.photo,
            transcoder_subtitles: transcoder.subtitles,
            transcoder_video: transcoder.video,
            transcoder_video_bitrates: TRANSCODER_BITRATES.to_owned(),
            transcoder_video_qualities: TRANSCODER_QUALITIES.to_owned(),
            transcoder_video_resolutions: TRANSCODER_RESOLUTIONS.to_owned(),
            updated_at: server.updated_at,
            updater: true,
            version: server.version.clone(),
            voice_search: features.voice_search,
            directory: CAPABILITY_DIRECTORIES
                .iter()
                .map(|(key, count)| CapabilityDirectory {
                    count: *count,
                    key,
                    title: key,
                })
                .collect(),
        }
    }
}

/// The comma-separated capability names Plex publishes in `ownerFeatures`.
fn named_features(features: &GqlFeatures) -> String {
    [
        ("adaptive_bitrate", features.adaptive_bitrate),
        ("camera_upload", features.camera_upload),
        ("collections", features.collections),
        ("download_certificates", features.sync),
        ("federated-auth", features.multiuser),
        ("home", features.multiuser),
        ("livetv", features.live_tv),
        ("loudness", features.loudness),
        ("radio", features.live_tv),
        ("server-manager", true),
        ("shared-radio", features.sharing),
        ("sync", features.sync),
        ("tuner-sharing", features.tuners),
        ("type-first", true),
    ]
    .into_iter()
    .filter_map(|(name, enabled)| enabled.then_some(name))
    .collect::<Vec<_>>()
    .join(",")
}
