//! What the server reports about itself.

/// Identity announced before any authentication happens.
#[derive(Debug, Clone)]
pub struct ServerIdentity {
    /// Stable per-install identifier. Clients key their server list on it, so it must
    /// survive restarts and never collide between installs.
    pub machine_identifier: String,
    /// Name shown to users.
    pub friendly_name: String,
    pub version: String,
    /// Version of the API surface we implement, independent of our own version.
    pub api_version: String,
    /// Operating system family, e.g. `Linux`.
    pub platform: String,
    /// Operating system version as the host reports it.
    ///
    /// Empty when unknown: reading it portably needs platform-specific calls, and inventing
    /// a value would be worse than admitting we do not have one.
    pub platform_version: String,
}

impl ServerIdentity {
    pub fn new(machine_identifier: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            machine_identifier: machine_identifier.into(),
            friendly_name: "klapz".to_owned(),
            version: version.into(),
            api_version: "1.2.3".to_owned(),
            platform: capitalise(std::env::consts::OS),
            platform_version: String::new(),
        }
    }

    #[must_use]
    pub fn named(mut self, friendly_name: impl Into<String>) -> Self {
        self.friendly_name = friendly_name.into();
        self
    }
}

/// What the transcoder can do.
///
/// Every field is on, including the ones Plex reserves for a subscription: hardware
/// transcoding and offline sync are among the features this project refuses to gate.
///
/// A flag per capability, because that is what a client reads one at a time. Grouping them
/// would not make any caller simpler.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct TranscoderCapabilities {
    pub video: bool,
    pub audio: bool,
    pub subtitles: bool,
    pub photo: bool,
    pub lyrics: bool,
    /// How many video transcodes are running right now.
    pub active_video_sessions: u32,
}

impl Default for TranscoderCapabilities {
    fn default() -> Self {
        Self {
            video: true,
            audio: true,
            subtitles: true,
            photo: true,
            lyrics: true,
            active_video_sessions: 0,
        }
    }
}

/// Optional behaviours a client may ask about before offering them.
///
/// There is no paid tier here, so there is nothing to report as unavailable. A client that
/// reads one of these to decide whether to show a feature gets the answer that shows it —
/// which is the whole point of the no-paywall rule, expressed in the one place a client can
/// actually observe it.
///
/// A flag per feature, because a client asks about them individually. Grouping them into
/// sub-structs would reshape the wire format for no caller's benefit.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct ServerFeatures {
    pub sync: bool,
    pub camera_upload: bool,
    pub sharing: bool,
    pub media_deletion: bool,
    pub tuners: bool,
    /// Plex's plug-in channels. Not a subscription feature: there is no plug-in framework
    /// here, and Plex Web offers a plug-in screen on this flag alone.
    pub channel_access: bool,
    pub hub_search: bool,
    pub voice_search: bool,
    pub event_stream: bool,
    pub multiuser: bool,
    pub collections: bool,
    /// Adaptive bitrate streaming, which Plex gates behind a subscription.
    pub adaptive_bitrate: bool,
    /// Skipping intros and credits, likewise gated upstream.
    pub skip_markers: bool,
    /// Loudness levelling, likewise.
    pub loudness: bool,
    /// Live TV and DVR, likewise.
    pub live_tv: bool,
    /// Hardware-accelerated transcoding, likewise.
    pub hardware_transcoding: bool,
}

impl Default for ServerFeatures {
    /// Everything on. Not a placeholder — see the type's documentation.
    fn default() -> Self {
        Self {
            sync: true,
            camera_upload: true,
            sharing: true,
            media_deletion: true,
            tuners: true,
            channel_access: false,
            hub_search: true,
            voice_search: true,
            event_stream: true,
            multiuser: true,
            collections: true,
            adaptive_bitrate: true,
            skip_markers: true,
            loudness: true,
            live_tv: true,
            hardware_transcoding: true,
        }
    }
}

/// Everything a client asks for before doing anything else.
///
/// This is the first request any client makes: it decides from the answer what the server is
/// capable of, and therefore what to offer. A server that cannot answer it is a server no
/// client will talk to.
#[derive(Debug, Clone)]
pub struct ServerCapabilities {
    pub identity: ServerIdentity,
    pub transcoder: TranscoderCapabilities,
    pub features: ServerFeatures,
    /// When the server last started or its configuration changed, in epoch seconds.
    pub updated_at: i64,
}

impl ServerCapabilities {
    pub fn new(identity: ServerIdentity, updated_at: i64) -> Self {
        Self {
            identity,
            transcoder: TranscoderCapabilities::default(),
            features: ServerFeatures::default(),
            updated_at,
        }
    }
}

/// Upper-cases the first character.
///
/// `std::env::consts::OS` is lowercase (`windows`, `linux`) while clients match on the
/// capitalised family name the reference server reports.
fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Seconds since the Unix epoch, saturating at 0 before it.
pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn every_feature_is_available() {
        // The no-paywall rule is only observable here, so it is asserted here. A future
        // change that gates one of these has to change this test deliberately.
        let features = ServerFeatures::default();
        for (name, on) in [
            ("sync", features.sync),
            ("camera_upload", features.camera_upload),
            ("sharing", features.sharing),
            ("media_deletion", features.media_deletion),
            ("tuners", features.tuners),
            ("hub_search", features.hub_search),
            ("voice_search", features.voice_search),
            ("event_stream", features.event_stream),
            ("multiuser", features.multiuser),
            ("collections", features.collections),
            ("adaptive_bitrate", features.adaptive_bitrate),
            ("skip_markers", features.skip_markers),
            ("loudness", features.loudness),
            ("live_tv", features.live_tv),
            ("hardware_transcoding", features.hardware_transcoding),
        ] {
            assert!(on, "{name} must not be gated");
        }
    }

    #[test]
    fn the_transcoder_advertises_every_kind() {
        let transcoder = TranscoderCapabilities::default();
        assert!(transcoder.video && transcoder.audio && transcoder.subtitles);
        assert!(transcoder.photo && transcoder.lyrics);
        assert_eq!(transcoder.active_video_sessions, 0);
    }

    #[test]
    fn capitalises_the_platform_family() {
        assert_eq!(capitalise("windows"), "Windows");
        assert_eq!(capitalise("linux"), "Linux");
        assert_eq!(capitalise(""), "");
    }

    #[test]
    fn identity_defaults_the_platform_to_the_host() {
        let identity = ServerIdentity::new("abc", "0.1.0");
        assert_eq!(identity.platform.to_lowercase(), std::env::consts::OS);
        assert_eq!(identity.api_version, "1.2.3");
        assert_eq!(identity.friendly_name, "klapz");
    }
}
