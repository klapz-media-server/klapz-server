//! The response body for `GET /`.
//!
//! Field order and types follow a capture of the reference server. Two values deserve a note.
//!
//! `myPlexSubscription` reports **true**. Clients read it to decide whether to offer the
//! features Plex reserves for a subscription, and klapz gates nothing — so the answer
//! that unlocks them is the correct one here.
//!
//! `myPlex` reports whether this caller's server is paired with a plex.tv account. Claiming a
//! link that does not exist would be a lie that breaks differently, later and less legibly,
//! than an absent link does.

use serde::Serialize;

use crate::general::GqlServer;

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
    #[serde(rename = "myPlexMappingState", skip_serializing_if = "Option::is_none")]
    pub my_plex_mapping_state: Option<&'static str>,
    /// `ok` is what Plex Web reads as claimed.
    #[serde(rename = "myPlexSigninState")]
    pub my_plex_signin_state: &'static str,
    #[serde(rename = "myPlexSubscription")]
    pub my_plex_subscription: bool,
    #[serde(rename = "myPlexUsername", skip_serializing_if = "Option::is_none")]
    pub my_plex_username: Option<String>,
    pub offline_transcode: u8,
    /// What the owner has unlocked, by name and by GUID: Plex Web tests both.
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
            machine_identifier: crate::gql::machine_identifier(&server.machine_identifier),
            media_providers: true,
            multiuser: features.multiuser,
            music_analysis: if features.loudness { 2 } else { 0 },
            my_plex: server.account.is_some(),
            my_plex_mapping_state: server.account.as_ref().map(|_| "mapped"),
            my_plex_signin_state: if server.account.is_some() {
                "ok"
            } else {
                "none"
            },
            my_plex_subscription: true,
            my_plex_username: server
                .account
                .as_ref()
                .map(|account| account.account_name.clone()),
            offline_transcode: u8::from(features.sync),
            owner_features: owner_features(),
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
            version: crate::general::PLEX_VERSION.to_owned(),
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

/// Every feature a Plex Pass server announces, as captured from one. Nothing here is behind a
/// paywall, so the list is whole, screens whose endpoints still answer `501` included.
fn owner_features() -> String {
    include_str!("owner_features.txt")
        .lines()
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::owner_features;

    #[test]
    fn every_feature_of_a_plex_pass_server_is_announced() {
        let list = owner_features();
        let features: Vec<&str> = list.split(',').collect();
        assert_eq!(features.len(), 209);
        assert!(features.iter().all(|feature| !feature.trim().is_empty()));
        for wanted in [
            "dvr",
            "session_kick",
            "76ddd91e-8321-4916-94b6-ded8e3727a64",
        ] {
            assert!(features.contains(&wanted), "{wanted}");
        }
    }
}
