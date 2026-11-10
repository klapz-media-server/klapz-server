//! Routes declared by the spec but not implemented yet.
//!
//! **Generated** — regenerate with `python tools/gen_stub_routes.py`.
//!
//! Each answers `501 Not Implemented`. That is what gives the spec-coverage test
//! something to assert: a route the spec declares and we never registered answers
//! `404`, which is a bug, while one we have deliberately not written yet answers
//! `501`, which is not. Without the distinction the test could only check that the
//! router exists.
//!
//! Replacing a stub means deleting its row from `STUBS` and adding the real handler
//! to the module that owns that domain.
//!
//! These are added onto the caller's router rather than merged in from a second one.
//! `merge` re-validates the incoming paths with Axum's default rules, which reject
//! the segments Plex writes as a literal colon — so the routes have to be declared on
//! the one router that has those checks turned off.

use axum::routing::MethodRouter;
use axum::Router;

use crate::error::ApiError;
use crate::PlexState;

/// Placeholder for routes that exist in the spec but have no handler yet.
async fn not_implemented() -> ApiError {
    ApiError::NotImplemented
}

/// Methods a stub answers, as a mask so each row of `STUBS` stays one line.
mod verb {
    pub const GET: u8 = 1 << 0;
    pub const POST: u8 = 1 << 1;
    pub const PUT: u8 = 1 << 2;
    pub const DELETE: u8 = 1 << 3;
}

/// Builds the method router for one mask.
///
/// A mask rather than a handler per method because every method answers the same way;
/// what varies is only which ones are declared at all. A method the spec does not
/// declare must stay undeclared, so it answers `405` instead of `501`.
fn stub(methods: u8) -> MethodRouter<PlexState> {
    let mut router = MethodRouter::new();
    if methods & verb::GET != 0 {
        router = router.get(not_implemented);
    }
    if methods & verb::POST != 0 {
        router = router.post(not_implemented);
    }
    if methods & verb::PUT != 0 {
        router = router.put(not_implemented);
    }
    if methods & verb::DELETE != 0 {
        router = router.delete(not_implemented);
    }
    router
}

/// The 182 stub operations, across 147 paths.
const STUBS: &[(&str, u8)] = &[
    ("/:/eventsource/notifications", verb::GET),
    ("/:/prefs/get", verb::GET),
    ("/:/rate", verb::PUT),
    ("/:/websocket/notifications", verb::GET),
    ("/activities/{activityId}", verb::DELETE),
    ("/butler", verb::GET | verb::POST | verb::DELETE),
    ("/butler/{task}", verb::POST | verb::DELETE),
    ("/downloadQueue", verb::POST),
    ("/downloadQueue/{queueId}", verb::GET),
    ("/downloadQueue/{queueId}/add", verb::POST),
    ("/downloadQueue/{queueId}/item/{itemId}/decision", verb::GET),
    ("/downloadQueue/{queueId}/item/{itemId}/media", verb::GET),
    ("/downloadQueue/{queueId}/items", verb::GET),
    (
        "/downloadQueue/{queueId}/items/{itemId}",
        verb::GET | verb::DELETE,
    ),
    (
        "/downloadQueue/{queueId}/items/{itemId}/restart",
        verb::POST,
    ),
    ("/hubs", verb::GET),
    ("/hubs/items", verb::GET),
    ("/hubs/metadata/{metadataId}", verb::GET),
    ("/hubs/metadata/{metadataId}/postplay", verb::GET),
    ("/hubs/metadata/{metadataId}/related", verb::GET),
    ("/hubs/popularInLibraries", verb::GET),
    ("/hubs/promoted", verb::GET),
    ("/hubs/search", verb::GET),
    ("/hubs/search/voice", verb::GET),
    (
        "/hubs/sections/{sectionId}/manage",
        verb::GET | verb::POST | verb::DELETE,
    ),
    ("/hubs/sections/{sectionId}/manage/move", verb::PUT),
    (
        "/hubs/sections/{sectionId}/manage/{identifier}",
        verb::PUT | verb::DELETE,
    ),
    ("/library/all", verb::GET),
    ("/library/collections", verb::POST),
    (
        "/library/collections/{collectionId}/composite/{updatedAt}",
        verb::GET,
    ),
    (
        "/library/collections/{collectionId}/items",
        verb::GET | verb::PUT,
    ),
    (
        "/library/collections/{collectionId}/items/{itemId}",
        verb::PUT,
    ),
    (
        "/library/collections/{collectionId}/items/{itemId}/move",
        verb::PUT,
    ),
    ("/library/file", verb::POST),
    (
        "/library/media/{mediaId}/chapterImages/{chapter}",
        verb::GET,
    ),
    (
        "/library/metadata/augmentations/{augmentationId}",
        verb::GET,
    ),
    ("/library/metadata/{ids}/credits", verb::PUT),
    ("/library/metadata/{ids}/extras", verb::GET | verb::POST),
    ("/library/metadata/{ids}/file", verb::GET),
    ("/library/metadata/{ids}/index", verb::PUT),
    ("/library/metadata/{ids}/intro", verb::PUT),
    ("/library/metadata/{ids}/marker", verb::POST),
    (
        "/library/metadata/{ids}/marker/{marker}",
        verb::PUT | verb::DELETE,
    ),
    ("/library/metadata/{ids}/media/{mediaItem}", verb::DELETE),
    ("/library/metadata/{ids}/nearest", verb::GET),
    ("/library/metadata/{ids}/prefs", verb::PUT),
    ("/library/metadata/{ids}/subtitles", verb::GET),
    ("/library/metadata/{ids}/tree", verb::GET),
    ("/library/metadata/{ids}/users/top", verb::GET),
    (
        "/library/metadata/{ids}/{element}",
        verb::POST | verb::PUT | verb::DELETE,
    ),
    ("/library/metadata/{ids}/{element}/{timestamp}", verb::GET),
    ("/library/people/{personId}", verb::GET),
    ("/library/people/{personId}/media", verb::GET),
    ("/library/randomArtwork", verb::GET),
    ("/library/sections/prefs", verb::GET),
    (
        "/library/sections/{sectionId}",
        verb::GET | verb::PUT | verb::DELETE,
    ),
    ("/library/sections/{sectionId}/albums", verb::GET),
    ("/library/sections/{sectionId}/allLeaves", verb::GET),
    ("/library/sections/{sectionId}/arts", verb::GET),
    ("/library/sections/{sectionId}/audioCodec", verb::GET),
    ("/library/sections/{sectionId}/audioLayout", verb::GET),
    ("/library/sections/{sectionId}/autocomplete", verb::GET),
    ("/library/sections/{sectionId}/categories", verb::GET),
    ("/library/sections/{sectionId}/cluster", verb::GET),
    (
        "/library/sections/{sectionId}/collection/{collectionId}",
        verb::DELETE,
    ),
    ("/library/sections/{sectionId}/collections", verb::GET),
    ("/library/sections/{sectionId}/common", verb::GET),
    (
        "/library/sections/{sectionId}/composite/{updatedAt}",
        verb::GET,
    ),
    ("/library/sections/{sectionId}/computePath", verb::GET),
    ("/library/sections/{sectionId}/emptyTrash", verb::PUT),
    ("/library/sections/{sectionId}/filters", verb::GET),
    ("/library/sections/{sectionId}/firstCharacters", verb::GET),
    ("/library/sections/{sectionId}/location", verb::GET),
    ("/library/sections/{sectionId}/moment", verb::GET),
    ("/library/sections/{sectionId}/nearest", verb::GET),
    ("/library/sections/{sectionId}/sorts", verb::GET),
    ("/library/sections/{sectionId}/subtitleCodec", verb::GET),
    ("/library/sections/{sectionId}/videoCodec", verb::GET),
    ("/library/streams/{streamId}/levels", verb::GET),
    ("/library/streams/{streamId}/loudness", verb::GET),
    ("/library/tags", verb::GET),
    ("/livetv/dvrs", verb::GET | verb::POST),
    ("/livetv/dvrs/{dvrId}", verb::GET | verb::DELETE),
    ("/livetv/dvrs/{dvrId}/channels/{channel}/tune", verb::POST),
    (
        "/livetv/dvrs/{dvrId}/devices/{deviceId}",
        verb::PUT | verb::DELETE,
    ),
    ("/livetv/dvrs/{dvrId}/lineups", verb::PUT | verb::DELETE),
    ("/livetv/dvrs/{dvrId}/prefs", verb::PUT),
    (
        "/livetv/dvrs/{dvrId}/reloadGuide",
        verb::POST | verb::DELETE,
    ),
    ("/livetv/epg/channelmap", verb::GET),
    ("/livetv/epg/channels", verb::GET),
    ("/livetv/epg/countries", verb::GET),
    ("/livetv/epg/countries/{country}/{epgId}/lineups", verb::GET),
    ("/livetv/epg/countries/{country}/{epgId}/regions", verb::GET),
    (
        "/livetv/epg/countries/{country}/{epgId}/regions/{region}/lineups",
        verb::GET,
    ),
    ("/livetv/epg/languages", verb::GET),
    ("/livetv/epg/lineup", verb::GET),
    ("/livetv/epg/lineupchannels", verb::GET),
    ("/livetv/sessions", verb::GET),
    ("/livetv/sessions/{sessionId}", verb::GET),
    (
        "/livetv/sessions/{sessionId}/{consumerId}/index.m3u8",
        verb::GET,
    ),
    (
        "/livetv/sessions/{sessionId}/{consumerId}/{segmentId}",
        verb::GET,
    ),
    ("/log", verb::POST | verb::PUT),
    ("/log/networked", verb::POST),
    ("/media/grabbers", verb::GET),
    ("/media/grabbers/devices", verb::GET | verb::POST),
    ("/media/grabbers/devices/discover", verb::POST),
    (
        "/media/grabbers/devices/{deviceId}",
        verb::GET | verb::PUT | verb::DELETE,
    ),
    ("/media/grabbers/devices/{deviceId}/channelmap", verb::PUT),
    ("/media/grabbers/devices/{deviceId}/channels", verb::GET),
    ("/media/grabbers/devices/{deviceId}/prefs", verb::PUT),
    (
        "/media/grabbers/devices/{deviceId}/scan",
        verb::POST | verb::DELETE,
    ),
    (
        "/media/grabbers/devices/{deviceId}/thumb/{version}",
        verb::GET,
    ),
    ("/media/grabbers/operations/{operationId}", verb::DELETE),
    ("/media/providers/{provider}", verb::DELETE),
    ("/media/subscriptions", verb::GET | verb::POST),
    ("/media/subscriptions/process", verb::POST),
    ("/media/subscriptions/scheduled", verb::GET),
    ("/media/subscriptions/template", verb::GET),
    (
        "/media/subscriptions/{subscriptionId}",
        verb::GET | verb::PUT | verb::DELETE,
    ),
    ("/media/subscriptions/{subscriptionId}/move", verb::PUT),
    ("/playQueues/{playQueueId}/items", verb::DELETE),
    (
        "/playQueues/{playQueueId}/items/{playQueueItemId}",
        verb::DELETE,
    ),
    (
        "/playQueues/{playQueueId}/items/{playQueueItemId}/move",
        verb::PUT,
    ),
    ("/playQueues/{playQueueId}/reset", verb::PUT),
    ("/playQueues/{playQueueId}/shuffle", verb::PUT),
    ("/playQueues/{playQueueId}/unshuffle", verb::PUT),
    ("/playlists", verb::GET | verb::POST),
    ("/playlists/upload", verb::POST),
    (
        "/playlists/{playlistId}",
        verb::GET | verb::PUT | verb::DELETE,
    ),
    ("/playlists/{playlistId}/generators", verb::GET),
    (
        "/playlists/{playlistId}/items",
        verb::GET | verb::PUT | verb::DELETE,
    ),
    (
        "/playlists/{playlistId}/items/{generatorId}",
        verb::GET | verb::PUT | verb::DELETE,
    ),
    (
        "/playlists/{playlistId}/items/{generatorId}/items",
        verb::GET,
    ),
    (
        "/playlists/{playlistId}/items/{generatorId}/{metadataId}/{action}",
        verb::PUT,
    ),
    (
        "/playlists/{playlistId}/items/{playlistItemId}/move",
        verb::PUT,
    ),
    ("/security/resources", verb::GET),
    ("/security/token", verb::POST),
    ("/services/ultrablur/colors", verb::GET),
    ("/services/ultrablur/image", verb::GET),
    ("/status/sessions/background", verb::GET),
    ("/status/sessions/history/all", verb::GET),
    (
        "/status/sessions/history/{historyId}",
        verb::GET | verb::DELETE,
    ),
    ("/status/sessions/terminate", verb::POST),
    ("/updater/apply", verb::PUT),
    ("/{transcodeType}/:/transcode/universal/decision", verb::GET),
    (
        "/{transcodeType}/:/transcode/universal/fallback",
        verb::POST,
    ),
    (
        "/{transcodeType}/:/transcode/universal/subtitles",
        verb::GET,
    ),
];

/// Adds 182 stub operations across 147 paths.
pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    STUBS.iter().fold(router, |router, (path, methods)| {
        router.route(path, stub(*methods))
    })
}

// Not routable as written, and needing real handlers regardless:
//   GET /library/streams/{streamId}.{ext} — two parameters in segment `{streamId}.{ext}`
//   PUT /library/streams/{streamId}.{ext} — two parameters in segment `{streamId}.{ext}`
//   DELETE /library/streams/{streamId}.{ext} — two parameters in segment `{streamId}.{ext}`
//   GET /tv.plex.providers.epg.{identifier}:{deviceId}/grid — two parameters in segment `tv.plex.providers.epg.{identifier}:{deviceId}`
//   GET /tv.plex.providers.epg.{identifier}:{deviceId}/lineups/dvr/channels — two parameters in segment `tv.plex.providers.epg.{identifier}:{deviceId}`
//   GET /tv.plex.providers.epg.{identifier}:{deviceId}/watchnow — two parameters in segment `tv.plex.providers.epg.{identifier}:{deviceId}`
//   GET /tv.plex.providers.epg.{identifier}:{deviceId}/watchnow/all — two parameters in segment `tv.plex.providers.epg.{identifier}:{deviceId}`
//   GET /{transcodeType}/:/transcode/universal/start.* — wildcard in path
