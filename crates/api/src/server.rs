//! GraphQL type for the server itself.
//!
//! This answers the question every client asks first: what can this server do? Exposing it
//! through the official API rather than only through the Plex surface is what lets the
//! compatibility layer stay a translator with no data of its own.

use async_graphql::{Object, SimpleObject, ID};
use open_plex_core::identity::{ServerCapabilities, ServerFeatures, TranscoderCapabilities};

use crate::node::GlobalId;
use crate::DateTime;

pub const SERVER_TYPE: &str = "Server";

/// What the transcoder can do.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, SimpleObject)]
pub struct Transcoder {
    /// Whether video can be transcoded.
    pub video: bool,
    /// Whether audio can be transcoded.
    pub audio: bool,
    /// Whether subtitles can be burned in or converted.
    pub subtitles: bool,
    /// Whether photos can be resized.
    pub photo: bool,
    /// Whether lyrics can be converted.
    pub lyrics: bool,
    /// How many video transcodes are running right now.
    pub active_video_sessions: u32,
}

impl From<TranscoderCapabilities> for Transcoder {
    fn from(capabilities: TranscoderCapabilities) -> Self {
        Self {
            video: capabilities.video,
            audio: capabilities.audio,
            subtitles: capabilities.subtitles,
            photo: capabilities.photo,
            lyrics: capabilities.lyrics,
            active_video_sessions: capabilities.active_video_sessions,
        }
    }
}

/// Optional behaviours a client may ask about before offering them.
///
/// Every field is true. open-plex has no paid tier, so there is nothing to report as
/// unavailable — including the features Plex reserves for a subscription, such as offline
/// sync, skipping intros, loudness levelling, Live TV and hardware transcoding.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, SimpleObject)]
pub struct Features {
    /// Downloading items for offline playback.
    pub sync: bool,
    /// Uploading photos from a device.
    pub camera_upload: bool,
    /// Sharing libraries with other accounts.
    pub sharing: bool,
    /// Deleting media through the API.
    pub media_deletion: bool,
    /// Attaching TV tuners.
    pub tuners: bool,
    /// Accessing channels.
    pub channel_access: bool,
    /// Searching across libraries from one query.
    pub hub_search: bool,
    /// Searching by voice.
    pub voice_search: bool,
    /// Subscribing to live change notifications.
    pub event_stream: bool,
    /// Serving more than one account.
    pub multiuser: bool,
    /// Grouping items into collections.
    pub collections: bool,
    /// Adapting bitrate to the client's bandwidth mid-stream.
    pub adaptive_bitrate: bool,
    /// Skipping intros and credits.
    pub skip_markers: bool,
    /// Levelling loudness across tracks.
    pub loudness: bool,
    /// Live TV and recording.
    pub live_tv: bool,
    /// Using the GPU to transcode.
    pub hardware_transcoding: bool,
}

impl From<ServerFeatures> for Features {
    fn from(features: ServerFeatures) -> Self {
        Self {
            sync: features.sync,
            camera_upload: features.camera_upload,
            sharing: features.sharing,
            media_deletion: features.media_deletion,
            tuners: features.tuners,
            channel_access: features.channel_access,
            hub_search: features.hub_search,
            voice_search: features.voice_search,
            event_stream: features.event_stream,
            multiuser: features.multiuser,
            collections: features.collections,
            adaptive_bitrate: features.adaptive_bitrate,
            skip_markers: features.skip_markers,
            loudness: features.loudness,
            live_tv: features.live_tv,
            hardware_transcoding: features.hardware_transcoding,
        }
    }
}

/// This server.
#[derive(Debug, Clone)]
pub struct Server(pub ServerCapabilities);

#[allow(clippy::unused_async)]
#[Object]
impl Server {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch the server.
    pub async fn id(&self) -> ID {
        // One server per process, so the key is fixed; the tag is what makes the id unique
        // across types.
        ID(GlobalId::encode(SERVER_TYPE, 0))
    }

    /// Stable per-install identifier.
    ///
    /// Clients key their server list on it, so it survives restarts and never collides
    /// between installs.
    async fn machine_identifier(&self) -> &str {
        &self.0.identity.machine_identifier
    }

    /// Name shown to users.
    async fn friendly_name(&self) -> &str {
        &self.0.identity.friendly_name
    }

    /// This server's own version.
    async fn version(&self) -> &str {
        &self.0.identity.version
    }

    /// Version of the API surface implemented, independent of `version`.
    async fn api_version(&self) -> &str {
        &self.0.identity.api_version
    }

    /// Operating system family.
    async fn platform(&self) -> &str {
        &self.0.identity.platform
    }

    /// Operating system version as the host reports it. Empty when unknown.
    async fn platform_version(&self) -> &str {
        &self.0.identity.platform_version
    }

    /// What the transcoder can do.
    async fn transcoder(&self) -> Transcoder {
        self.0.transcoder.into()
    }

    /// Optional behaviours, all of them available.
    async fn features(&self) -> Features {
        self.0.features.into()
    }

    /// When the server last started or its configuration changed.
    async fn updated_at(&self) -> DateTime {
        self.0.updated_at.into()
    }
}
