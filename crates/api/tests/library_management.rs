//! Creating, changing and deleting libraries on the official API: owners choose directories
//! and delete, a library's admins rename, and nobody else sees the library at all. Owners
//! alone browse the server's directories to find roots.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use klapz_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use klapz_core::access::{LibraryAccess, LibraryRole};
use klapz_core::testing::{InMemoryLibrary, InMemoryMetadata, KnownDirectories, RecordedScans};
use klapz_core::{LibrarySection, Principal, SectionId, SectionKind, SectionRoot, UserId};
use serde_json::{json, Value};

fn section(id: i64, kind: SectionKind) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: format!("library {id}"),
        title_sort: Some(format!("sorted {id}")),
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
    let library = InMemoryLibrary::new(vec![
        section(27, SectionKind::Movie),
        section(28, SectionKind::Show),
    ])
    .with_roots(vec![SectionRoot {
        id: 3,
        section_id: SectionId::new(28),
        path: "/media/series".to_owned(),
        created_at: 0,
    }]);
    build_schema(ApiContext {
        capabilities: klapz_core::ServerCapabilities::new(
            klapz_core::ServerIdentity::new("0000000000000000000000000000000000000000", "0.1.0"),
            0,
        ),
        library: Arc::new(library),
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
        settings: std::sync::Arc::new(klapz_core::testing::InMemorySettings::default()),
        scans,
        matching: None,
    })
}

fn scans() -> Arc<RecordedScans> {
    Arc::new(RecordedScans {
        directories: KnownDirectories {
            unreadable: vec!["/media/private".to_owned()],
            ..KnownDirectories::new(&[
                "/media",
                "/media/films",
                "/media/films/vf",
                "/media/series",
                "/media/anime",
                "/media/Bandes-annonces",
                "/media/private",
            ])
        },
        ..RecordedScans::default()
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

async fn execute(
    schema: &ApiSchema,
    caller: Principal,
    query: &str,
    variables: Value,
) -> async_graphql::Response {
    let request = async_graphql::Request::new(query)
        .variables(async_graphql::Variables::from_json(variables))
        .data(caller);
    schema.execute(request).await
}

async fn run(schema: &ApiSchema, caller: Principal, query: &str, variables: Value) -> Value {
    let response = execute(schema, caller, query, variables).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

const CREATE: &str = r"
mutation Create($input: CreateLibraryInput!) {
  createLibrary(input: $input) {
    clientMutationId
    library { title kind language roots { path } }
    userErrors { field code }
  }
}
";

const UPDATE: &str = r"
mutation Update($input: UpdateLibraryInput!) {
  updateLibrary(input: $input) {
    library { title titleSort language roots { id path } }
    userErrors { field code }
  }
}
";

const DELETE: &str = r"
mutation Delete($input: DeleteLibraryInput!) {
  deleteLibrary(input: $input) { clientMutationId deletedLibraryId userErrors { field code } }
}
";

fn library(id: i64) -> String {
    GlobalId::encode("Library", id)
}

fn films(roots: &[&str]) -> Value {
    json!({ "input": {
        "clientMutationId": "c",
        "title": "  Films  ",
        "kind": "MOVIE",
        "language": "fr-FR",
        "roots": roots,
    } })
}

#[tokio::test]
async fn an_owner_creates_a_library_which_is_then_scanned() {
    let scans = scans();
    let schema = schema(Arc::clone(&scans));
    let data = run(
        &schema,
        klapz_core::testing::owner(),
        CREATE,
        films(&["/media/films/", "/media/anime", "/media/films"]),
    )
    .await;
    assert_eq!(
        data["createLibrary"],
        json!({
            "clientMutationId": "c",
            "library": {
                "title": "Films",
                "kind": "MOVIE",
                "language": "fr-FR",
                "roots": [{ "path": "/media/films" }, { "path": "/media/anime" }],
            },
            "userErrors": [],
        })
    );
    let started = scans.started.lock().unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].section, SectionId::new(29));
    assert!(!started[0].force);
}

#[tokio::test]
async fn only_an_owner_creates_a_library() {
    let scans = scans();
    let schema = schema(Arc::clone(&scans));
    let response = execute(&schema, user(), CREATE, films(&["/media/films"])).await;
    assert_eq!(response.errors.len(), 1);
    assert!(scans.started.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_library_is_refused_what_it_cannot_hold_or_reach() {
    let schema = schema(scans());
    let refused = |input: Value| {
        let schema = &schema;
        async move {
            run(
                schema,
                klapz_core::testing::owner(),
                CREATE,
                json!({ "input": input }),
            )
            .await["createLibrary"]
                .clone()
        }
    };
    let valid = json!({ "title": "Films", "kind": "MOVIE", "language": "fr-FR", "roots": ["/media/films"] });
    let with = |field: &str, value: Value| {
        let mut input = valid.clone();
        input[field] = value;
        input
    };
    let cases = [
        (
            with("kind", json!("ARTIST")),
            json!(["input", "kind"]),
            "UNSUPPORTED_KIND",
        ),
        (
            with("title", json!("   ")),
            json!(["input", "title"]),
            "INVALID_TITLE",
        ),
        (
            with("language", json!("french")),
            json!(["input", "language"]),
            "INVALID_LANGUAGE",
        ),
        (
            with("roots", json!([])),
            json!(["input", "roots"]),
            "NO_ROOT",
        ),
        (
            with("roots", json!(["/media/anime", "/etc"])),
            json!(["input", "roots", "1"]),
            "ROOT_NOT_FOUND",
        ),
        (
            with("roots", json!(["/media/films", "/media/films/vf"])),
            json!(["input", "roots", "1"]),
            "OVERLAPPING_ROOTS",
        ),
    ];
    for (input, field, code) in cases {
        let payload = refused(input).await;
        assert_eq!(payload["library"], Value::Null, "{code}");
        assert_eq!(
            payload["userErrors"],
            json!([{ "field": field, "code": code }])
        );
    }
}

#[tokio::test]
async fn a_library_admin_renames_it_but_does_not_choose_its_directories() {
    let schema = schema(scans());
    let renamed = run(
        &schema,
        user(),
        UPDATE,
        json!({ "input": { "id": library(28), "title": "Séries", "language": "en-US" } }),
    )
    .await;
    assert_eq!(
        renamed["updateLibrary"],
        json!({
            "library": {
                "title": "Séries",
                "titleSort": null,
                "language": "en-US",
                "roots": [{ "id": GlobalId::encode("Root", 3), "path": "/media/series" }],
            },
            "userErrors": [],
        })
    );
    let roots = run(
        &schema,
        user(),
        UPDATE,
        json!({ "input": { "id": library(28), "roots": ["/media/anime"] } }),
    )
    .await;
    assert_eq!(
        roots["updateLibrary"]["userErrors"],
        json!([{ "field": ["input", "roots"], "code": "OWNER_ONLY" }])
    );
    for id in [library(27), library(99), GlobalId::encode("Item", 28)] {
        let hidden = run(
            &schema,
            user(),
            UPDATE,
            json!({ "input": { "id": id, "title": "Mine" } }),
        )
        .await;
        assert_eq!(
            hidden["updateLibrary"]["userErrors"],
            json!([{ "field": ["input", "id"], "code": "LIBRARY_NOT_FOUND" }])
        );
    }
}

#[tokio::test]
async fn an_owner_replaces_the_roots_and_the_library_is_scanned_again() {
    let scans = scans();
    let schema = schema(Arc::clone(&scans));
    let data = run(
        &schema,
        klapz_core::testing::owner(),
        UPDATE,
        json!({ "input": { "id": library(28), "roots": ["/media/series", "/media/anime"] } }),
    )
    .await;
    let roots = &data["updateLibrary"]["library"]["roots"];
    assert_eq!(roots[0]["id"], GlobalId::encode("Root", 3), "kept");
    assert_eq!(roots[1]["path"], "/media/anime");
    assert_eq!(scans.started.lock().unwrap().len(), 1);

    scans.running.lock().unwrap().push(SectionId::new(28));
    let scanning = run(
        &schema,
        klapz_core::testing::owner(),
        UPDATE,
        json!({ "input": { "id": library(28), "roots": ["/media/series"] } }),
    )
    .await;
    assert_eq!(
        scanning["updateLibrary"]["userErrors"],
        json!([{ "field": ["input", "roots"], "code": "SCANNING" }])
    );
}

#[tokio::test]
async fn an_owner_deletes_a_library_unless_it_is_being_scanned() {
    let scans = scans();
    let schema = schema(Arc::clone(&scans));
    let input = json!({ "input": { "clientMutationId": "d", "id": library(27) } });

    let response = execute(&schema, user(), DELETE, input.clone()).await;
    assert_eq!(response.errors.len(), 1, "a library's admins do not delete");

    scans.running.lock().unwrap().push(SectionId::new(27));
    let scanning = run(&schema, klapz_core::testing::owner(), DELETE, input.clone()).await;
    assert_eq!(
        scanning["deleteLibrary"]["userErrors"],
        json!([{ "field": ["input", "id"], "code": "SCANNING" }])
    );
    scans.running.lock().unwrap().clear();

    let deleted = run(&schema, klapz_core::testing::owner(), DELETE, input.clone()).await;
    assert_eq!(
        deleted["deleteLibrary"],
        json!({ "clientMutationId": "d", "deletedLibraryId": library(27), "userErrors": [] })
    );
    let gone = run(
        &schema,
        klapz_core::testing::owner(),
        "query($id: ID!) { node(id: $id) { id } }",
        json!({ "id": library(27) }),
    )
    .await;
    assert_eq!(gone["node"], Value::Null);
    let again = run(&schema, klapz_core::testing::owner(), DELETE, input).await;
    assert_eq!(
        again["deleteLibrary"]["userErrors"],
        json!([{ "field": ["input", "id"], "code": "LIBRARY_NOT_FOUND" }])
    );
}

const BROWSE: &str = r"
query Browse($path: String, $first: Int, $after: String) {
  serverDirectories(path: $path) {
    error
    listing {
      path
      parent
      truncated
      directories(first: $first, after: $after) {
        edges { cursor node { name path } }
        pageInfo { hasNextPage }
      }
    }
  }
}
";

fn names(answer: &Value) -> Vec<&str> {
    answer["serverDirectories"]["listing"]["directories"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn an_owner_browses_from_the_top_into_a_directory() {
    let schema = schema(scans());
    let owner = klapz_core::testing::owner;
    let top = run(&schema, owner(), BROWSE, json!({})).await;
    assert_eq!(
        top["serverDirectories"]["listing"]["path"],
        Value::Null,
        "{top}"
    );
    assert_eq!(names(&top), ["/"]);

    let media = run(&schema, owner(), BROWSE, json!({ "path": "/media/" })).await;
    let listing = &media["serverDirectories"]["listing"];
    assert_eq!(listing["path"], "/media");
    assert_eq!(listing["parent"], "/");
    assert_eq!(listing["truncated"], false);
    assert_eq!(
        names(&media),
        ["anime", "Bandes-annonces", "films", "private", "series"],
        "by name, ignoring case"
    );
    assert_eq!(
        listing["directories"]["edges"][2]["node"]["path"],
        "/media/films"
    );
}

#[tokio::test]
async fn a_directory_listing_pages_forward() {
    let schema = schema(scans());
    let first = run(
        &schema,
        klapz_core::testing::owner(),
        BROWSE,
        json!({ "path": "/media", "first": 2 }),
    )
    .await;
    assert_eq!(names(&first), ["anime", "Bandes-annonces"]);
    let directories = &first["serverDirectories"]["listing"]["directories"];
    assert_eq!(directories["pageInfo"]["hasNextPage"], true);
    let after = directories["edges"][1]["cursor"].clone();
    let rest = run(
        &schema,
        klapz_core::testing::owner(),
        BROWSE,
        json!({ "path": "/media", "after": after }),
    )
    .await;
    assert_eq!(names(&rest), ["films", "private", "series"]);
    let refused = execute(
        &schema,
        klapz_core::testing::owner(),
        BROWSE,
        json!({ "path": "/media", "first": 501 }),
    )
    .await;
    assert_eq!(refused.errors.len(), 1);
}

#[tokio::test]
async fn a_directory_not_listed_says_why() {
    let schema = schema(scans());
    for (path, code) in [
        ("/media/gone", "NOT_FOUND"),
        ("media/films", "NOT_A_DIRECTORY"),
        ("/media/private", "UNREADABLE"),
    ] {
        let answer = run(
            &schema,
            klapz_core::testing::owner(),
            BROWSE,
            json!({ "path": path }),
        )
        .await;
        assert_eq!(
            answer["serverDirectories"],
            json!({ "error": code, "listing": null }),
            "{path}"
        );
    }
}

#[tokio::test]
async fn only_an_owner_browses_the_server() {
    let schema = schema(scans());
    for path in [Value::Null, json!("/media")] {
        let response = execute(&schema, user(), BROWSE, json!({ "path": path })).await;
        assert_eq!(response.errors.len(), 1, "{path}");
        assert_eq!(response.data.into_json().unwrap(), Value::Null);
        let nobody = schema
            .execute(
                async_graphql::Request::new(BROWSE)
                    .variables(async_graphql::Variables::from_json(json!({ "path": path }))),
            )
            .await;
        assert_eq!(nobody.errors.len(), 1, "{path}");
    }
}
