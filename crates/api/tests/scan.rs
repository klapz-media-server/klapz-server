//! Scanning a library on the official API: who may start and stop a scan, and what it asks
//! the scan control for.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use klapz_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use klapz_core::access::{LibraryAccess, LibraryRole};
use klapz_core::testing::{InMemoryLibrary, InMemoryMetadata, RecordedScans};
use klapz_core::{LibrarySection, Principal, SectionId, SectionKind, UserId};
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
        capabilities: klapz_core::ServerCapabilities::new(
            klapz_core::ServerIdentity::new("0000000000000000000000000000000000000000", "0.1.0"),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie),
            section(28, SectionKind::Show),
            section(29, SectionKind::Artist),
        ])),
        metadata: Arc::new(InMemoryMetadata::new(Vec::new())),
        playback: Arc::new(klapz_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(klapz_core::testing::InMemoryPlaylists::new(Vec::new())),
        collections: Arc::new(klapz_core::testing::InMemoryCollections::default()),
        access: Arc::new(klapz_core::testing::InMemoryAccess::default()),
        directory: Arc::new(klapz_core::testing::StubDirectory::default()),
        watch: Arc::new(klapz_core::testing::InMemoryWatch::default()),
        streams: Arc::new(klapz_core::testing::StubStreams::default()),
        session_locks: Arc::default(),
        scans,
        matching: None,
    })
}

/// A user who views library 27 and administers 28.
fn user() -> Principal {
    Principal {
        name: "user".to_owned(),
        role: klapz_core::ServerRole::User,
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
        klapz_core::testing::owner(),
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
        klapz_core::testing::owner(),
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
        klapz_core::testing::owner(),
        CANCEL,
        json!({ "input": { "libraryId": library(27) } }),
    )
    .await;
    assert_eq!(
        data["cancelLibraryScan"],
        json!({ "cancelled": false, "userErrors": [] })
    );
}

#[tokio::test]
async fn a_librarys_readers_see_how_far_its_files_are_read_and_its_admins_why_not() {
    let scans = Arc::new(RecordedScans::default());
    for id in [27, 28] {
        scans.analyses.lock().unwrap().insert(
            SectionId::new(id),
            klapz_core::scan::LibraryAnalysis {
                progress: klapz_core::scan::AnalysisProgress {
                    files: 300,
                    read: 120,
                    complete: 40,
                },
                reading: false,
                failure: Some("cannot open /films/a.mkv: gone".to_owned()),
            },
        );
    }
    let schema = schema(Arc::clone(&scans));
    let query = "query($id: ID!) { node(id: $id) { ... on Library { analysis {
        fileCount readFileCount completeFileCount reading failure
    } } } }";
    let admin = run(&schema, user(), query, json!({ "id": library(28) })).await;
    assert_eq!(
        admin["node"]["analysis"],
        json!({
            "fileCount": 300,
            "readFileCount": 120,
            "completeFileCount": 40,
            "reading": false,
            "failure": "cannot open /films/a.mkv: gone",
        })
    );
    let viewer = run(&schema, user(), query, json!({ "id": library(27) })).await;
    assert_eq!(viewer["node"]["analysis"]["readFileCount"], 120);
    assert_eq!(viewer["node"]["analysis"]["failure"], Value::Null);
    let music = run(
        &schema,
        klapz_core::testing::owner(),
        query,
        json!({ "id": library(29) }),
    )
    .await;
    assert_eq!(music["node"]["analysis"], Value::Null);
}

#[tokio::test]
async fn a_library_s_admins_read_why_its_last_scan_stopped_and_its_viewers_do_not() {
    let scans = Arc::new(RecordedScans::default());
    for id in [27, 28] {
        scans.failures.lock().unwrap().insert(
            SectionId::new(id),
            "the media analyzer cannot run: no ffprobe".to_owned(),
        );
    }
    let schema = schema(Arc::clone(&scans));
    let query = "query($id: ID!) { node(id: $id) { ... on Library { scanFailure } } }";
    let admin = run(&schema, user(), query, json!({ "id": library(28) })).await;
    assert_eq!(
        admin["node"]["scanFailure"],
        "the media analyzer cannot run: no ffprobe"
    );
    let viewer = run(&schema, user(), query, json!({ "id": library(27) })).await;
    assert_eq!(viewer["node"]["scanFailure"], Value::Null);
}
