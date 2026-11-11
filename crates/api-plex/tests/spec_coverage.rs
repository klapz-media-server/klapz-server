//! Every operation the spec declares must resolve to a route.
//!
//! This is the cheapest high-value test in the project: it walks `docs/openapi.json` and
//! checks that nothing answers `404`. A mistyped or forgotten path is otherwise found by a
//! client months later, and the failure looks like "the app does not work" rather than "this
//! route is missing".
//!
//! `501` is a pass. A route we have deliberately not written yet is not a bug; a route the
//! spec declares and we never registered is. That distinction is the whole reason the stub
//! routes exist.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use open_plex_api::{build_schema, ApiContext};
use open_plex_api_plex::{router, PlexState};
use open_plex_core::library::{LibrarySection, SectionKind};
use open_plex_core::metadata::{ItemKind, MetadataItem};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{
    Accounts, Credential, Principal, RatingKey, Scope, SectionId, ServerCapabilities,
    ServerIdentity,
};
use serde_json::Value;
use tower::ServiceExt;

const TOKEN: &str = "coverage-token";

/// Whether the spec writes this path in a shape no router can express.
///
/// Two shapes qualify: a trailing `.*` standing for "any extension", and a segment packing
/// two parameters together, which cannot be split unambiguously. Both need handlers that
/// parse the segment themselves.
///
/// This is the same rule `tools/gen_stub_routes.py` applies, deliberately expressed as a
/// predicate rather than a list of paths — an earlier version listed them, and the list was
/// already missing one by the time this test first ran.
fn unroutable(path: &str) -> bool {
    path.contains('*')
        || path
            .split('/')
            .any(|segment| segment.matches('{').count() > 1)
}

/// Scanning, matching and media analysis are a separate binary on Plex and out of scope, so
/// the operations that only drive them are not expected to be routed.
const DEFERRED_FRAGMENTS: &[&str] = &[
    "/refresh",
    "/analyze",
    "/analyzeLoudness",
    "/match",
    "/unmatch",
    "/split",
    "/merge",
    "/addetect",
    "/voiceActivity",
    "/chapterThumbs",
    "/library/optimize",
    "/library/clean/bundles",
    "/library/caches",
    "/indexes",
    "/intros",
];

const DEFERRED_TAGS: &[&str] = &["Metadata Agents"];

/// A library and an item numbered 1, which is what [`substitute`] puts in every id slot.
///
/// Seeded rather than left empty because the handlers that really look an id up answer `404`
/// when it is absent — correctly, and indistinguishably from an unrouted path by status
/// alone. With the id present, a registered route cannot answer `404` for any reason, which
/// is what makes the assertion below mean what it says.
fn catalog() -> (InMemoryLibrary, InMemoryMetadata) {
    let section = LibrarySection {
        id: SectionId::new(1),
        kind: SectionKind::Movie,
        title: "Films".to_owned(),
        title_sort: None,
        language: "fr-FR".to_owned(),
        uuid: "00000000-0000-0000-0000-000000000001".to_owned(),
        created_at: 0,
        updated_at: 0,
        scanned_at: None,
        content_changed_at: 0,
        hidden: false,
    };
    let item = MetadataItem {
        id: RatingKey::new(1),
        section_id: SectionId::new(1),
        parent_id: None,
        kind: ItemKind::Movie,
        guid: "plex://movie/1".to_owned(),
        title: Some("A Film".to_owned()),
        title_sort: None,
        original_title: None,
        summary: None,
        tagline: None,
        studio: None,
        content_rating: None,
        content_rating_age: None,
        position: Some(1),
        year: Some(2000),
        released_on: None,
        duration_ms: None,
        rating: None,
        audience_rating: None,
        added_at: 0,
        updated_at: 0,
    };
    (
        InMemoryLibrary::new(vec![section]),
        InMemoryMetadata::new(vec![item]),
    )
}

fn app() -> axum::Router {
    let identity = ServerIdentity::new("0000000000000000000000000000000000000000", "0.1.0");
    let (library, metadata) = catalog();
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
        metadata: std::sync::Arc::new(metadata),
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
    let accounts = Accounts::new(vec![Credential::new(TOKEN, "owner", Scope::Admin)]);

    // A principal is attached directly: authentication lives in the `server` crate, and this
    // test is about routing, not about credentials.
    router(PlexState::new(identity, schema, accounts))
        .layer(axum::Extension(Principal::operator("owner")))
}

/// Fills path parameters with values of a plausible shape.
///
/// `{sectionId}` is not a URI, so something has to stand in. The values only need to make the
/// path well-formed — whether the id exists is irrelevant to whether the route is registered.
fn concretize(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;

    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map_or(rest.len(), |at| open + at);
        let name = &rest[open + 1..close];
        out.push_str(substitute(name));
        rest = &rest[(close + 1).min(rest.len())..];
    }
    out.push_str(rest);
    out
}

/// Enumerated segments need a member of their set; a number there would 404 on its own.
fn substitute(parameter: &str) -> &'static str {
    match parameter.to_lowercase().as_str() {
        "transcodetype" => "video",
        // Not a number on purpose: these routes are real, and would answer `404` for something
        // that does not exist, which this test could not tell from a missing route. An id it
        // cannot parse is a `400`.
        "partid" | "playqueueid" => "not-a-number",
        "element" => "thumb",
        "ext" => "jpg",
        "action" => "play",
        _ => "1",
    }
}

fn spec_operations() -> Vec<(Method, String)> {
    let spec: Value =
        serde_json::from_str(include_str!("../../../docs/openapi.json")).expect("the spec parses");
    let paths = spec["paths"].as_object().expect("paths object");

    let mut operations = Vec::new();
    for (path, item) in paths {
        if unroutable(path) {
            continue;
        }
        if DEFERRED_FRAGMENTS
            .iter()
            .any(|fragment| path.contains(fragment))
        {
            continue;
        }

        for (verb, operation) in item.as_object().expect("path item") {
            let method = match verb.as_str() {
                "get" => Method::GET,
                "post" => Method::POST,
                "put" => Method::PUT,
                "patch" => Method::PATCH,
                "delete" => Method::DELETE,
                "head" => Method::HEAD,
                _ => continue,
            };
            let tag = operation["tags"][0].as_str().unwrap_or_default();
            if DEFERRED_TAGS.contains(&tag) {
                continue;
            }
            operations.push((method, path.clone()));
        }
    }
    operations
}

async fn status(app: &axum::Router, method: &Method, uri: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method.clone())
                .uri(uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds")
        .status()
}

/// Whether a path that answered `404` has a route all the same.
///
/// Some routes answer `404` for any id, because nothing they could name can exist here: no
/// DVR, tuner, recording or live session. Seeding cannot help them, so they are told apart
/// from an unrouted path by a method the spec never uses: a registered path refuses it with
/// `405`, an unregistered one does not know it with `404`. The original method is then
/// registered too, since an undeclared one would have answered `405` itself.
async fn is_routed(app: &axum::Router, uri: &str) -> bool {
    status(app, &Method::PATCH, uri).await == StatusCode::METHOD_NOT_ALLOWED
}

#[tokio::test]
async fn every_spec_operation_resolves_to_a_route() {
    let app = app();
    let operations = spec_operations();
    assert!(
        operations.len() > 150,
        "only {} operations found — did the spec move?",
        operations.len()
    );

    // Collected rather than asserted in the loop: a test that stops at the first missing
    // route makes you re-run it once per fix.
    let mut unrouted = BTreeSet::new();

    for (method, path) in &operations {
        let uri = concretize(path);
        if status(&app, method, &uri).await == StatusCode::NOT_FOUND && !is_routed(&app, &uri).await
        {
            unrouted.insert(format!("{method} {path}  (tried {uri})"));
        }
    }

    assert!(
        unrouted.is_empty(),
        "{} of {} spec operations are not routed:\n{}",
        unrouted.len(),
        operations.len(),
        unrouted.into_iter().collect::<Vec<_>>().join("\n")
    );
}

#[tokio::test]
async fn an_unknown_path_still_answers_not_found() {
    // The counterpart to the test above: if everything answered 501, it would pass for the
    // wrong reason.
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/this/is/not/a/plex/endpoint")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unimplemented_route_answers_not_implemented() {
    // And the other half: a declared route that has no handler says so, rather than pretending
    // to be missing.
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/hubs")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
}
