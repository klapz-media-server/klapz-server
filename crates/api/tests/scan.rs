//! Scanning a library on the official API: who may start and stop a scan, and what it asks
//! the scan control for.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use open_plex_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use open_plex_core::access::{LibraryAccess, LibraryRole};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata, RecordedScans};
use open_plex_core::{LibrarySection, Principal, SectionId, SectionKind, UserId};
use serde_json::{json, Value};

fn section(id: i64, kind: SectionKind) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: format!("library {id}"),
        title_sort: None,
        language: "fr-FR".to_owned(),
        uuid: format!("uuid-{id}"),
        created_at: 0,
        updated_at: 0,
        scanned_at: None,
        content_changed_at: 0,
        hidden: false,
    }
}

fn schema(scans: Arc<RecordedScans>) -> ApiSchema {
    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie),
            section(28, SectionKind::Show),
            section(29, SectionKind::Artist),
        ])),
        metadata: Arc::new(InMemoryMetadata::new(Vec::new())),
        playback: Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(open_plex_core::testing::InMemoryPlaylists::new(Vec::new())),
        collections: Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: Arc::default(),
        scans,
    })
}

/// A user who views library 27 and administers 28.
fn user() -> Principal {
    Principal {
        name: "user".to_owned(),
        role: open_plex_core::ServerRole::User,
        libraries: LibraryAccess::Only(HashMap::from([
            (SectionId::new(27), LibraryRole::Viewer),
            (SectionId::new(28), LibraryRole::Admin),
        ])),
        user: UserId::new(5),
        server_identifier: None,
    }
}

async fn run(schema: &ApiSchema, caller: Principal, query: &str, variables: Value) -> Value {
    let request = async_graphql::Request::new(query)
        .variables(async_graphql::Variables::from_json(variables))
        .data(caller);
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

const SCAN: &str = r"
mutation Scan($input: ScanLibraryInput!) {
  scanLibrary(input: $input) {
    clientMutationId
    library { title scanning }
    userErrors { field code }
  }
}
";

const CANCEL: &str = r"
mutation Cancel($input: CancelLibraryScanInput!) {
  cancelLibraryScan(input: $input) { cancelled userErrors { code } }
}
";

fn library(id: i64) -> String {
    GlobalId::encode("Library", id)
}

#[tokio::test]
async fn an_owner_starts_a_scan() {
    let scans = Arc::new(RecordedScans::default());
    let schema = schema(Arc::clone(&scans));
    let data = run(
        &schema,
        open_plex_core::testing::owner(),
        SCAN,
        json!({ "input": { "clientMutationId": "a", "libraryId": library(27), "force": true } }),
    )
    .await;
    assert_eq!(
        data["scanLibrary"],
        json!({
            "clientMutationId": "a",
            "library": { "title": "library 27", "scanning": false },
            "userErrors": [],
        })
    );
    let started = scans.started.lock().unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].section, SectionId::new(27));
    assert!(started[0].force);
    assert!(started[0].below.is_none());
}

#[tokio::test]
async fn a_library_admin_scans_it_and_a_viewer_does_not_see_it() {
    let scans = Arc::new(RecordedScans::default());
    let schema = schema(Arc::clone(&scans));
    let admin = run(
        &schema,
        user(),
        SCAN,
        json!({ "input": { "libraryId": library(28) } }),
    )
    .await;
    assert_eq!(admin["scanLibrary"]["userErrors"], json!([]));
    let viewer = run(
        &schema,
        user(),
        SCAN,
        json!({ "input": { "libraryId": library(27) } }),
    )
    .await;
    assert_eq!(
        viewer["scanLibrary"],
        json!({
            "clientMutationId": null,
            "library": null,
            "userErrors": [{ "field": ["input", "libraryId"], "code": "LIBRARY_NOT_FOUND" }],
        })
    );
    assert_eq!(scans.started.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn music_is_not_scanned() {
    let scans = Arc::new(RecordedScans::default());
    let schema = schema(Arc::clone(&scans));
    let data = run(
        &schema,
        open_plex_core::testing::owner(),
        SCAN,
        json!({ "input": { "libraryId": library(29) } }),
    )
    .await;
    assert_eq!(
        data["scanLibrary"]["userErrors"][0]["code"],
        json!("UNSUPPORTED_KIND")
    );
    assert!(scans.started.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancelling_with_no_scan_running_cancels_nothing() {
    let schema = schema(Arc::new(RecordedScans::default()));
    let data = run(
        &schema,
        open_plex_core::testing::owner(),
        CANCEL,
        json!({ "input": { "libraryId": library(27) } }),
    )
    .await;
    assert_eq!(
        data["cancelLibraryScan"],
        json!({ "cancelled": false, "userErrors": [] })
    );
}
