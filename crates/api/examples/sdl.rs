//! Prints the schema as SDL.
//!
//! Client codegen and schema-diffing in CI both need the SDL as a file, and generating it
//! from the code keeps it from drifting the way a hand-maintained copy would.
//!
//! Usage: `cargo run -p open-plex-api --example sdl > schema.graphql`

// Build-time tooling: a failure here must stop the build loudly rather than emit a
// half-written schema file that something downstream would then trust.
#![allow(clippy::expect_used)]

use std::sync::Arc;

use open_plex_api::{build_schema, ApiContext};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};

fn main() {
    // The schema's shape does not depend on its data, so an empty repository is enough.
    let schema = build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::default()),
        metadata: Arc::new(InMemoryMetadata::default()),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
    });
    // `@specifiedBy` is left out by default, and `DateTime` names the specification it follows.
    let options = async_graphql::SDLExportOptions::new().include_specified_by();
    print!("{}", schema.sdl_with_options(options));
}
