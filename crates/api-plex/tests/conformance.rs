//! Endpoint-level conformance.
//!
//! These drive the real router with real requests and assert what a client actually reads:
//! the status, the negotiated content type, and the exact encoding of each field. The
//! expectations come from captures against a reference server (1.43.5), recorded in
//! `docs/wire-format.md`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use open_plex_api::{build_schema, ApiContext};
use open_plex_api_plex::{router, PlexState};
use open_plex_core::metadata::{
    ExternalId, ItemKind, ItemTag, MediaFile, MediaStream, MediaVersion, MetadataItem, Person,
    Rating, RatingKind, StreamKind, Tag, TagKind,
};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{
    Accounts, Credential, LibrarySection, MediaId, PartId, PersonId, Principal, RatingKey, Scope,
    SectionId, SectionKind, SectionRoot, ServerCapabilities, ServerIdentity, StreamId, TagId,
};
use serde_json::Value;
use tower::ServiceExt;

/// The token every authenticated request in this file presents.
const TOKEN: &str = "test-owner-token";

fn section(id: i64, kind: SectionKind, title: &str, hidden: bool) -> LibrarySection {
    section_with_uuid(id, kind, title, hidden, &format!("uuid-{id}"))
}

fn section_with_uuid(
    id: i64,
    kind: SectionKind,
    title: &str,
    hidden: bool,
    uuid: &str,
) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: title.to_owned(),
        title_sort: None,
        language: "fr-FR".to_owned(),
        uuid: uuid.to_owned(),
        created_at: 1_477_835_115,
        updated_at: 1_792_916_300,
        scanned_at: Some(1_792_916_245),
        content_changed_at: 8_457_612_157_739_110_435,
        hidden,
    }
}

fn root(id: i64, section: i64, path: &str) -> SectionRoot {
    SectionRoot {
        id,
        section_id: SectionId::new(section),
        path: path.to_owned(),
        created_at: 1_477_835_115,
    }
}

/// Builds the adapter over a schema backed by the given catalog, with a caller attached.
///
/// Two things this setup makes explicit. The adapter is handed only a GraphQL schema, so
/// every assertion below exercises the official API too. And authentication happens in the
/// `server` crate's middleware, so these tests inject the principal it would attach — the
/// token itself is tested where it is validated.
fn app_from(library: InMemoryLibrary) -> axum::Router {
    authenticated(unauthenticated(library))
}

fn unauthenticated(library: InMemoryLibrary) -> axum::Router {
    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let accounts = Accounts::new(vec![Credential::new(TOKEN, "owner", Scope::Admin)]);
    // One identity for both surfaces, as the binary wires it: `/identity` reads the
    // adapter's copy while `/` reads the schema's, and a client that sees two different
    // machine identifiers from one server treats it as two servers.
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
        metadata: std::sync::Arc::new(InMemoryMetadata::default()),
    });
    router(PlexState::new(identity, schema, accounts))
}

fn authenticated(router: axum::Router) -> axum::Router {
    router.layer(axum::Extension(Principal {
        name: "owner".to_owned(),
        scope: Scope::Admin,
    }))
}

/// Builds an app over an arbitrary catalog, for tests about ordering and flags.
fn app_with(sections: Vec<LibrarySection>) -> axum::Router {
    app_from(InMemoryLibrary::new(sections))
}

fn app() -> axum::Router {
    let movies = section_with_uuid(
        4,
        SectionKind::Movie,
        "VF - Films",
        false,
        "2cd48489-54c0-41e9-ae62-f3dcb0f3c249",
    );
    app_from(InMemoryLibrary::new(vec![movies]).with_roots(vec![
        root(17, 4, "/mnt/acd/Plex/Movies/Multi"),
        root(18, 4, "/mnt/acd/Plex/Movies/VF"),
    ]))
}

async fn get(uri: &str, accept: Option<&str>) -> (StatusCode, String, String) {
    request_with(uri, accept, Some(TOKEN)).await
}

async fn request_with(
    uri: &str,
    accept: Option<&str>,
    token: Option<&str>,
) -> (StatusCode, String, String) {
    let mut request = Request::builder().uri(uri);
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    if let Some(token) = token {
        request = request.header("X-Plex-Token", token);
    }
    let response = app()
        .oneshot(request.body(Body::empty()).expect("request builds"))
        .await
        .expect("router responds");

    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

#[tokio::test]
async fn identity_is_reachable_without_a_token() {
    // Requiring a token here would make the server undiscoverable.
    let (status, content_type, body) = request_with("/identity", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/xml;charset=utf-8");
    assert!(body.contains("machineIdentifier=\"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\""));
}

#[tokio::test]
async fn defaults_to_xml_and_switches_on_accept() {
    let (_, xml_type, _) = get("/library/sections", None).await;
    let (_, json_type, _) = get("/library/sections", Some("application/json")).await;
    // What the bundled Plex Web App sends.
    let (_, web_type, _) = get(
        "/library/sections",
        Some("application/json, text/plain, */*"),
    )
    .await;
    // `*/*` yields XML on a real server, counter-intuitively.
    let (_, any_type, _) = get("/library/sections", Some("*/*")).await;

    assert_eq!(xml_type, "text/xml;charset=utf-8");
    assert_eq!(json_type, "application/json");
    assert_eq!(web_type, "application/json");
    assert_eq!(any_type, "text/xml;charset=utf-8");
}

#[tokio::test]
async fn xml_sections_match_the_reference_layout() {
    let (status, _, body) = get("/library/sections", None).await;
    assert_eq!(status, StatusCode::OK);

    assert!(body.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MediaContainer "));
    // Booleans are digits in XML; `refreshing` and `hidden` both read "0" here even though
    // they differ in JSON.
    assert!(body.contains("allowSync=\"1\""), "{body}");
    assert!(body.contains("refreshing=\"0\""), "{body}");
    assert!(body.contains("hidden=\"0\""), "{body}");
    // Locations are leaf elements, self-closed with a space before the slash.
    assert!(
        body.contains("<Location id=\"17\" path=\"/mnt/acd/Plex/Movies/Multi\" />\n"),
        "{body}"
    );
}

#[tokio::test]
async fn json_sections_nest_under_media_container_with_captured_types() {
    let (status, _, body) = get("/library/sections", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);

    let parsed: Value = serde_json::from_str(&body).expect("valid JSON");
    let container = &parsed["MediaContainer"];
    assert!(
        container.is_object(),
        "JSON nests under a MediaContainer key"
    );

    assert_eq!(container["size"], Value::from(1));
    assert_eq!(container["allowSync"], Value::Bool(false));

    let directory = &container["Directory"][0];
    // Plex is not self-consistent: these are booleans in JSON...
    assert_eq!(directory["refreshing"], Value::Bool(false));
    assert_eq!(directory["filters"], Value::Bool(true));
    // ...while `hidden` is a number, and `key` is a string despite holding digits.
    assert_eq!(directory["hidden"], Value::from(0));
    assert_eq!(directory["key"], Value::from("4"));
    assert_eq!(directory["updatedAt"], Value::from(1_792_916_300_i64));
    assert!(
        directory["Location"].is_array(),
        "Location is an array even when nested"
    );
}

#[tokio::test]
async fn both_formats_describe_the_same_container() {
    let (_, _, xml) = get("/library/sections", None).await;
    let (_, _, json) = get("/library/sections", Some("application/json")).await;
    let parsed: Value = serde_json::from_str(&json).expect("valid JSON");

    let sections = parsed["MediaContainer"]["Directory"]
        .as_array()
        .expect("Directory array")
        .len();
    assert_eq!(sections, xml.matches("<Directory ").count());
}

#[tokio::test]
async fn unknown_routes_answer_with_html_not_a_container() {
    // A real server returns a minimal HTML document and ignores content negotiation.
    let (status, _, body) = get("/nope/nothing", Some("application/json")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.contains("MediaContainer"));
}

#[tokio::test]
async fn refuses_a_request_with_no_authenticated_caller() {
    // The `Caller` extractor's contract: if the HTTP layer attached no principal, the route
    // must refuse rather than serve. Validating the token itself is the middleware's job and
    // is tested in the `server` crate.
    let response = unauthenticated(InMemoryLibrary::default())
        .oneshot(
            Request::builder()
                .uri("/library/sections")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    // A real server answers errors with HTML and ignores content negotiation.
    assert_eq!(
        String::from_utf8_lossy(&body),
        "<html><head><title>Unauthorized</title></head><body><h1>401 Unauthorized</h1></body></html>"
    );
}

#[tokio::test]
async fn serves_the_spec_declared_alias_identically() {
    let (canonical_status, _, canonical) = get("/library/sections", None).await;
    let (alias_status, _, alias) = get("/library/sections/all", None).await;
    assert_eq!(canonical_status, alias_status);
    assert_eq!(canonical, alias);
}

#[tokio::test]
async fn lists_sections_by_kind_then_title_not_by_id() {
    // The reference library's order: movies before shows, alphabetical within each.
    let app = app_with(vec![
        section(20, SectionKind::Show, "VO - TV Shows UHD", false),
        section(4, SectionKind::Movie, "VF - Films", false),
        section(5, SectionKind::Show, "VF - Series", false),
        section(17, SectionKind::Movie, "VF - Films UHD", false),
    ]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/library/sections")
                .header("X-Plex-Token", TOKEN)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = String::from_utf8_lossy(&body);

    let keys: Vec<&str> = body
        .match_indices("key=\"")
        .map(|(index, _)| {
            let rest = &body[index + 5..];
            &rest[..rest.find('"').expect("closing quote")]
        })
        .collect();
    assert_eq!(keys, vec!["4", "17", "5", "20"]);
}

#[tokio::test]
async fn encodes_hidden_as_a_digit_in_xml_and_a_number_in_json() {
    let hidden_section = vec![section(9, SectionKind::Movie, "VF - Films 3D", true)];

    let xml_response = app_with(hidden_section.clone())
        .oneshot(
            Request::builder()
                .uri("/library/sections")
                .header("X-Plex-Token", TOKEN)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let xml = xml_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(String::from_utf8_lossy(&xml).contains("hidden=\"1\""));

    let json_response = app_with(hidden_section)
        .oneshot(
            Request::builder()
                .uri("/library/sections")
                .header("X-Plex-Token", TOKEN)
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let json = json_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let parsed: Value = serde_json::from_slice(&json).expect("valid JSON");
    // A number, not a boolean, unlike `refreshing` in the same object.
    assert_eq!(
        parsed["MediaContainer"]["Directory"][0]["hidden"],
        Value::from(1)
    );
}

#[tokio::test]
async fn capabilities_are_reachable_without_a_token() {
    // Same reason as `/identity`: a client that cannot read this before authenticating has
    // no way to learn the server exists.
    let (status, content_type, body) = request_with("/", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/xml;charset=utf-8");
    assert!(body.contains("machineIdentifier=\"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\""));
    // The endpoint groups are what a client walks to find the rest of the surface.
    assert!(
        body.contains("<Directory count=\"3\" key=\"library\" title=\"library\" />"),
        "{body}"
    );
}

#[tokio::test]
async fn capabilities_advertise_every_gated_feature_as_available() {
    // The no-paywall rule, asserted where a client can actually observe it. Each of these
    // is a subscription feature upstream, and each decides whether a client offers a menu
    // entry at all.
    let (status, _, body) = get("/", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);

    let body: Value = serde_json::from_str(&body).expect("json body");
    let container = &body["MediaContainer"];
    for flag in [
        "allowCameraUpload",
        "allowChannelAccess",
        "allowMediaDeletion",
        "allowSharing",
        "allowSync",
        "allowTuners",
        "eventStream",
        "hubSearch",
        "multiuser",
        "myPlexSubscription",
        "sync",
        "voiceSearch",
    ] {
        assert_eq!(
            container[flag],
            Value::Bool(true),
            "{flag} must not be gated"
        );
    }
    // Reported as levels rather than flags by the reference server.
    assert_eq!(container["livetv"], 7);
    assert_eq!(container["musicAnalysis"], 2);
    assert_eq!(container["offlineTranscode"], 1);

    // Not linked to plex.tv, and saying so plainly beats a lie that breaks later.
    assert_eq!(container["myPlex"], Value::Bool(false));
    assert_eq!(container["myPlexSigninState"], "none");

    // Checked by name upstream, so every name has to be present.
    let owner_features = container["ownerFeatures"].as_str().expect("ownerFeatures");
    for name in ["sync", "livetv", "loudness", "camera_upload", "collections"] {
        assert!(
            owner_features.split(',').any(|feature| feature == name),
            "{name} missing from {owner_features}"
        );
    }
}

#[tokio::test]
async fn capabilities_encode_booleans_per_format() {
    // Same container, two encodings: the dual-format rule applied to the widest response
    // in the surface.
    let (_, _, xml) = get("/", None).await;
    let (_, _, json) = get("/", Some("application/json")).await;

    assert!(xml.contains("allowSync=\"1\""), "{xml}");
    assert!(xml.contains("myPlex=\"0\""), "{xml}");

    let container = &serde_json::from_str::<Value>(&json).expect("json body")["MediaContainer"];
    assert_eq!(container["allowSync"], Value::Bool(true));
    assert_eq!(container["size"], 25);
    assert_eq!(
        container["Directory"].as_array().map(Vec::len),
        Some(25),
        "size must match the number of directories"
    );
}

// ---------------------------------------------------------------------------------------
// Library listings and item lookups.
//
// Expectations come from a field-by-field comparison against the reference server over real
// items (`tools/compare_items.py`), not from invented fixtures: the derived rules — a media
// duration overriding the metadata one, ratings to one decimal, bitrates in kilobits, the
// aspect-ratio ladder — are only visible that way.
// ---------------------------------------------------------------------------------------

/// A catalog with one film library and one series library, each with contents.
fn catalog() -> (InMemoryLibrary, InMemoryMetadata) {
    let films = section_with_uuid(
        4,
        SectionKind::Movie,
        "VF - Films",
        false,
        "2cd48489-54c0-41e9-ae62-f3dcb0f3c249",
    );
    let series = section_with_uuid(
        5,
        SectionKind::Show,
        "VF - Series",
        false,
        "25858bde-d5e7-45f2-89f1-57005968c107",
    );

    let mut avatar = item(100, 4, ItemKind::Movie, None, "Avatar");
    avatar.title_sort = Some("Avatar".to_owned());
    avatar.studio = Some("Lightstorm".to_owned());
    avatar.year = Some(2009);
    avatar.released_on = Some("2009-12-16".to_owned());
    avatar.duration_ms = Some(9_900_000);
    avatar.audience_rating = Some(7.924);
    avatar.content_rating = Some("fr/12".to_owned());
    avatar.content_rating_age = Some(12);

    let mut barbarella = item(101, 4, ItemKind::Movie, None, "Barbarella");
    barbarella.title_sort = Some("Barbarella".to_owned());

    let mut show = item(200, 5, ItemKind::Show, None, "Severance");
    show.studio = Some("Fifth Season".to_owned());
    show.year = Some(2022);
    show.content_rating = Some("TV-MA".to_owned());
    let mut season = item(201, 5, ItemKind::Season, Some(200), "Season 1");
    season.year = Some(2022);
    let episode = item(202, 5, ItemKind::Episode, Some(201), "Good News About Hell");
    let mut half_loop = item(205, 5, ItemKind::Episode, Some(201), "Half Loop");
    half_loop.position = Some(2);
    // A second season as the source stores nearly all of them: untitled, undated.
    let mut untitled = item(203, 5, ItemKind::Season, Some(200), "");
    untitled.title = None;
    untitled.summary = None;
    untitled.position = Some(2);
    let hello = item(204, 5, ItemKind::Episode, Some(203), "The We We Are");

    let version = MediaVersion {
        id: MediaId::new(900),
        item_id: RatingKey::new(100),
        width: Some(1920),
        height: Some(808),
        aspect_ratio: Some(2.376_237_630_844_116),
        duration_ms: Some(9_890_048),
        bitrate: Some(11_285_835),
        container: Some("mkv".to_owned()),
        video_codec: Some("h264".to_owned()),
        audio_codec: Some("dca".to_owned()),
        audio_channels: Some(6),
        frame_rate: Some(23.976_024_627_685_547),
        interlaced: false,
        optimized_for_streaming: false,
    };
    let file = MediaFile {
        id: PartId::new(910),
        version_id: MediaId::new(900),
        position: 0,
        path: "/mnt/movies/Avatar.mkv".to_owned(),
        size_bytes: Some(8_534_980_661),
        duration_ms: Some(9_890_048),
    };
    let stream = MediaStream {
        id: StreamId::new(920),
        file_id: PartId::new(910),
        kind: StreamKind::Audio,
        position: 1,
        codec: Some("dca".to_owned()),
        language: Some("fra".to_owned()),
        channels: Some(6),
        bitrate: Some(1_536_000),
        is_default: true,
        is_forced: false,
        external_path: None,
    };

    let (external_ids, ratings) = references();
    (
        InMemoryLibrary::new(vec![films, series]),
        InMemoryMetadata::new(vec![
            avatar, barbarella, show, season, episode, half_loop, untitled, hello,
        ])
        .with_media(vec![version], vec![file], vec![stream])
        .with_tags(tags())
        .with_references(external_ids, ratings),
    )
}

/// One attached tag: the item, then the tag's id, kind and name, the provider key and the
/// portrait path of a person, and the character an actor plays.
type Attached = (
    i64,
    i64,
    TagKind,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// The Transporter's tags as the reference server publishes them, attached to the film, and
/// an episode's director. Four of the twenty-six in the cast: enough to see a listing cut.
const TAGS: &[Attached] = &[
    (100, 38, TagKind::Genre, "Action", None, None, None),
    (100, 198, TagKind::Genre, "Crime", None, None, None),
    (100, 39, TagKind::Genre, "Thriller", None, None, None),
    (
        100,
        189_878,
        TagKind::Country,
        "United States of America",
        None,
        None,
        None,
    ),
    (100, 10_097, TagKind::Country, "France", None, None, None),
    (
        100,
        623_027,
        TagKind::Director,
        "Corey Yuen",
        Some("5d77682d103a2d001f565d0f"),
        Some("8/people/8a2234c508d280ed4c32b2b77b500f3d"),
        None,
    ),
    (
        100,
        473_529,
        TagKind::Writer,
        "Luc Besson",
        Some("5d776824103a2d001f563ac4"),
        Some("4/people/40d0cdeec61238c85a109bdc72667fbd"),
        None,
    ),
    (
        100,
        473_530,
        TagKind::Writer,
        "Robert Mark Kamen",
        Some("5d776824103a2d001f563b3b"),
        Some("d/people/dce9a97d3f59278bbb39ea025e32c41f"),
        None,
    ),
    (
        100,
        472_942,
        TagKind::Actor,
        "Jason Statham",
        Some("5d776826e6d55c002040af58"),
        Some("c/people/c540bdd39873ee8e8612b0af06fc0d4c"),
        Some("Frank Martin"),
    ),
    (
        100,
        603_622,
        TagKind::Actor,
        "Shu Qi",
        Some("5d77682b961905001eb92495"),
        Some("0/people/08390c853f95fd5804434217ba9725ce"),
        Some("Lai"),
    ),
    (
        100,
        480_672,
        TagKind::Actor,
        "François Berléand",
        Some("5d77682a5af944001f1f7834"),
        Some("f/people/ffd18db93fb63a9808e4758a6ae1e4fb"),
        Some("Tarconi"),
    ),
    (
        100,
        783_495,
        TagKind::Actor,
        "Matt Schulze",
        Some("5d77682d4de0ee001fcc9d0d"),
        Some("1/people/1ccf176b4e93793e48d51085b9372894"),
        Some("Wall Street"),
    ),
    (
        100,
        475_888,
        TagKind::Producer,
        "Luc Besson",
        Some("5d776824103a2d001f563ac4"),
        Some("4/people/40d0cdeec61238c85a109bdc72667fbd"),
        None,
    ),
    (
        100,
        720_202,
        TagKind::Producer,
        "Steven Chasman",
        Some("5d77682b961905001eb9251c"),
        Some("3/people/34ec649e4f0fd848499d4a2b29be78f2"),
        None,
    ),
    (
        202,
        705_517,
        TagKind::Director,
        "David Silverman",
        Some("5d7768267e9a3c0020c6ab00"),
        None,
        None,
    ),
];

fn tags() -> Vec<ItemTag> {
    TAGS.iter()
        .map(|&(item, id, kind, name, key, thumb, role)| ItemTag {
            item_id: RatingKey::new(item),
            tag: Tag {
                id: TagId::new(id),
                kind,
                name: name.to_owned(),
                // The adapter never shows a person's id, so the tag's stands in for it.
                person: key.map(|key| Person {
                    id: PersonId::new(id),
                    name: name.to_owned(),
                    provider_key: Some(key.to_owned()),
                    thumb_url: thumb
                        .map(|path| format!("https://metadata-static.plex.tv/{path}.jpg")),
                }),
            },
            role: role.map(ToOwned::to_owned),
        })
        .collect()
}

/// The Transporter's external identifiers and ratings, as captured.
fn references() -> (Vec<ExternalId>, Vec<Rating>) {
    let film = RatingKey::new(100);
    let id = |source: &str, value: &str| ExternalId {
        item_id: film,
        source: source.to_owned(),
        value: value.to_owned(),
    };
    let rating = |source: &str, kind: RatingKind, value: f64, image: &str| Rating {
        item_id: film,
        source: source.to_owned(),
        kind,
        value,
        image: image.to_owned(),
    };
    (
        vec![
            id("imdb", "tt0293662"),
            id("tmdb", "4108"),
            id("tvdb", "1601"),
        ],
        vec![
            rating("imdb", RatingKind::Audience, 6.8, "imdb://image.rating"),
            rating(
                "rottentomatoes",
                RatingKind::Critic,
                5.3,
                "rottentomatoes://image.rating.rotten",
            ),
            rating(
                "rottentomatoes",
                RatingKind::Audience,
                7.3,
                "rottentomatoes://image.rating.upright",
            ),
            rating(
                "themoviedb",
                RatingKind::Audience,
                6.7,
                "themoviedb://image.rating",
            ),
        ],
    )
}

fn item(id: i64, section: i64, kind: ItemKind, parent: Option<i64>, title: &str) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: parent.map(RatingKey::new),
        kind,
        guid: format!("plex://item/{id}"),
        title: Some(title.to_owned()),
        title_sort: None,
        original_title: None,
        summary: Some(format!("Summary of {title}")),
        tagline: None,
        studio: None,
        content_rating: None,
        content_rating_age: None,
        position: Some(1),
        year: None,
        released_on: None,
        duration_ms: None,
        rating: None,
        audience_rating: None,
        added_at: 1_500_000_000,
        updated_at: 1_600_000_000,
    }
}

fn items_app() -> axum::Router {
    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let accounts = Accounts::new(vec![Credential::new(TOKEN, "owner", Scope::Admin)]);
    let (library, metadata) = catalog();
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
        metadata: std::sync::Arc::new(metadata),
    });
    authenticated(router(PlexState::new(identity, schema, accounts)))
}

async fn get_from(app: axum::Router, uri: &str, accept: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().uri(uri).header("X-Plex-Token", TOKEN);
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    let response = app
        .oneshot(request.body(Body::empty()).expect("request builds"))
        .await
        .expect("router responds");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn listing(uri: &str) -> Value {
    let (status, body) = get_from(items_app(), uri, Some("application/json")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str(&body).expect("json body")
}

#[tokio::test]
async fn a_listing_describes_the_library_once_on_the_container() {
    let body =
        listing("/library/sections/4/all?X-Plex-Container-Start=0&X-Plex-Container-Size=10").await;
    let container = &body["MediaContainer"];

    assert_eq!(container["librarySectionID"], 4);
    assert_eq!(container["librarySectionTitle"], "VF - Films");
    assert_eq!(
        container["librarySectionUUID"],
        "2cd48489-54c0-41e9-ae62-f3dcb0f3c249"
    );
    assert_eq!(container["identifier"], "com.plexapp.plugins.library");
    // Per media kind, not per library.
    assert_eq!(container["art"], "/:/resources/movie-fanart.jpg");
    assert_eq!(container["viewGroup"], "movie");
    assert_eq!(container["title2"], "All VF - Films");
    assert_eq!(container["size"], 2);
    assert_eq!(container["totalSize"], 2);

    // The library is stated once on the container, so the items must not repeat it — a
    // client reading `librarySectionID` off an item in a listing finds nothing.
    let first = &container["Metadata"][0];
    assert!(first.get("librarySectionID").is_none(), "{first}");
    assert!(first.get("librarySectionTitle").is_none(), "{first}");
}

#[tokio::test]
async fn a_listing_covers_the_libraries_top_level_only() {
    // The series library holds a show, a season and an episode. A reference server lists one
    // item here: the show. Flattening all three is the bug this asserts against.
    let body = listing("/library/sections/5/all").await;
    let container = &body["MediaContainer"];

    assert_eq!(container["totalSize"], 1);
    assert_eq!(container["viewGroup"], "show");
    assert_eq!(container["title2"], "All Shows");

    let items = container["Metadata"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "show");
    // A series is browsed into, so the key a client follows lists the children.
    assert_eq!(items[0]["key"], "/library/metadata/200/children");
    assert_eq!(items[0]["childCount"], 2);
    assert_eq!(items[0]["leafCount"], 3);
    // Playback state is not implemented; reporting anything else would mark episodes watched.
    assert_eq!(items[0]["viewedLeafCount"], 0);
}

#[tokio::test]
async fn an_explicit_type_reaches_past_the_top_level() {
    // `type=4` asks for episodes, which all have parents. Honoring the default depth here
    // would answer with nothing.
    let body = listing("/library/sections/5/all?type=4").await;
    let items = body["MediaContainer"]["Metadata"]
        .as_array()
        .expect("items");
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|item| item["type"] == "episode"));
    assert_eq!(items[0]["ratingKey"], "202");
}

#[tokio::test]
async fn a_listing_names_each_element_after_its_kind_in_xml() {
    // JSON puts every item under `Metadata`; XML names the element after what the item is.
    let (_, films) = get_from(items_app(), "/library/sections/4/all", None).await;
    let (_, series) = get_from(items_app(), "/library/sections/5/all", None).await;

    assert!(films.contains("<Video ratingKey=\"100\""), "{films}");
    assert!(series.contains("<Directory ratingKey=\"200\""), "{series}");
    assert!(!films.contains("<Metadata"), "{films}");
}

#[tokio::test]
async fn paging_is_reported_in_the_body_and_the_headers() {
    let app = items_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/library/sections/4/all?X-Plex-Container-Start=1&X-Plex-Container-Size=1")
                .header("X-Plex-Token", TOKEN)
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    // Note which header is absent: a response carries Start and Total-Size, never Size.
    let headers = response.headers().clone();
    assert_eq!(headers["X-Plex-Container-Start"], "1");
    assert_eq!(headers["X-Plex-Container-Total-Size"], "2");
    assert!(headers.get("X-Plex-Container-Size").is_none());

    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: Value = serde_json::from_slice(&body).expect("json body");
    let container = &body["MediaContainer"];
    assert_eq!(container["offset"], 1);
    assert_eq!(container["totalSize"], 2);
    // The offset skipped the first film, so the second is the only one left.
    assert_eq!(container["size"], 1);
    assert_eq!(container["Metadata"][0]["ratingKey"], "101");
}

#[tokio::test]
async fn a_descending_sort_reverses_the_listing() {
    let ascending = listing("/library/sections/4/all?sort=titleSort").await;
    let descending = listing("/library/sections/4/all?sort=titleSort:desc").await;

    assert_eq!(
        ascending["MediaContainer"]["Metadata"][0]["title"],
        "Avatar"
    );
    assert_eq!(
        descending["MediaContainer"]["Metadata"][0]["title"],
        "Barbarella"
    );
}

#[tokio::test]
async fn media_attributes_are_the_published_forms_not_the_measurements() {
    let body = listing("/library/sections/4/all").await;
    let media = &body["MediaContainer"]["Metadata"][0]["Media"][0];

    // Each of these is a conversion the official API does not do: it publishes the
    // measurement, and the wire format wants the display form.
    assert_eq!(media["bitrate"], 11_286); // stored 11,285,835 bits/s
    assert_eq!(media["aspectRatio"], 2.35); // stored 2.3762…, snapped to a cinema ratio
    assert_eq!(media["videoFrameRate"], "24p"); // stored 23.976
    assert_eq!(media["videoResolution"], "1080"); // from the width, not the height
    assert_eq!(media["width"], 1920);
    assert_eq!(media["height"], 808);

    // A part repeats the version's container and publishes the path, as Plex does. No
    // streaming `key` yet: playback has no handler, and a key into a route that answers 501
    // would make the item look playable.
    let part = &media["Part"][0];
    assert_eq!(part["container"], "mkv");
    assert_eq!(part["size"], 8_534_980_661_i64);
    assert_eq!(part["file"], "/mnt/movies/Avatar.mkv");
    assert!(part.get("key").is_none(), "{part}");

    // Streams belong to a lookup, not a listing.
    assert!(part.get("Stream").is_none(), "{part}");
}

#[tokio::test]
async fn an_items_runtime_comes_from_its_media() {
    // The metadata row says 9,900,000 ms and the file 9,890,048. A reference server reports
    // the file's: a client seeking by percentage needs the one it is playing.
    let body = listing("/library/sections/4/all").await;
    let first = &body["MediaContainer"]["Metadata"][0];
    assert_eq!(first["duration"], 9_890_048);
    assert_eq!(first["Media"][0]["duration"], 9_890_048);
}

#[tokio::test]
async fn a_sort_title_identical_to_the_title_is_omitted() {
    // The importer stores one for every row; a reference server sends it only where it adds
    // something.
    let body = listing("/library/sections/4/all").await;
    let first = &body["MediaContainer"]["Metadata"][0];
    assert_eq!(first["title"], "Avatar");
    assert!(first.get("titleSort").is_none(), "{first}");
}

#[tokio::test]
async fn ratings_are_published_to_one_decimal() {
    let body = listing("/library/sections/4/all").await;
    assert_eq!(body["MediaContainer"]["Metadata"][0]["audienceRating"], 7.9);
}

#[tokio::test]
async fn a_lookup_repeats_the_library_on_each_item_and_carries_streams() {
    let body = listing("/library/metadata/100").await;
    let container = &body["MediaContainer"];

    // No window over a collection, so no view attributes and no totals.
    assert_eq!(container["size"], 1);
    assert!(container.get("totalSize").is_none(), "{container}");
    assert!(container.get("viewGroup").is_none(), "{container}");

    let item = &container["Metadata"][0];
    // Unlike a listing, a lookup states the library on the item too.
    assert_eq!(item["librarySectionID"], 4);
    assert_eq!(item["librarySectionTitle"], "VF - Films");
    assert_eq!(item["librarySectionKey"], "/library/sections/4");
    assert_eq!(item["key"], "/library/metadata/100");
    assert_eq!(item["guid"], "plex://item/100");
    assert_eq!(item["contentRatingAge"], 12);

    let stream = &item["Media"][0]["Part"][0]["Stream"][0];
    assert_eq!(stream["streamType"], 2);
    assert_eq!(stream["bitrate"], 1536);
    assert_eq!(stream["language"], "fra");
    assert_eq!(stream["default"], true);
    // Reported only when true, as a reference server does.
    assert!(stream.get("forced").is_none(), "{stream}");
}

#[tokio::test]
async fn a_lookup_takes_several_identifiers_at_once() {
    // How a client refreshes a screenful of items it already holds keys for.
    let body = listing("/library/metadata/100,101").await;
    let items = body["MediaContainer"]["Metadata"]
        .as_array()
        .expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(body["MediaContainer"]["size"], 2);
}

#[tokio::test]
async fn an_episode_links_to_its_season_and_its_series() {
    let body = listing("/library/metadata/202").await;
    let episode = &body["MediaContainer"]["Metadata"][0];

    assert_eq!(episode["type"], "episode");
    assert_eq!(episode["parentRatingKey"], "201");
    assert_eq!(episode["grandparentRatingKey"], "200");
    assert_eq!(episode["grandparentTitle"], "Severance");
}

#[tokio::test]
async fn opening_a_series_lists_its_seasons_under_a_header() {
    let body = listing("/library/metadata/200/children").await;
    let container = &body["MediaContainer"];

    // The container describes the series being opened: a client renders the header from it.
    assert_eq!(container["key"], "200");
    assert_eq!(container["parentTitle"], "Severance");
    assert_eq!(container["parentYear"], 2022);
    assert_eq!(container["parentIndex"], 1);
    assert_eq!(container["summary"], "Summary of Severance");
    assert_eq!(container["title1"], "VF - Series");
    assert_eq!(container["title2"], "Severance");
    assert_eq!(container["viewGroup"], "season");
    assert_eq!(container["viewMode"], 65_593);
    assert_eq!(container["nocache"], true);
    assert_eq!(container["librarySectionID"], 5);
    assert_eq!(container["size"], 2);
    assert_eq!(container["totalSize"], 2);
    assert!(container.get("grandparentTitle").is_none(), "{container}");

    let seasons = container["Metadata"].as_array().expect("seasons");
    assert_eq!(seasons[0]["ratingKey"], "201");
    assert_eq!(seasons[0]["type"], "season");
    assert_eq!(seasons[0]["key"], "/library/metadata/201/children");
    assert_eq!(seasons[0]["parentRatingKey"], "200");
    assert_eq!(seasons[0]["parentGuid"], "plex://item/200");
    assert_eq!(seasons[0]["parentStudio"], "Fifth Season");
    assert_eq!(seasons[0]["parentTitle"], "Severance");
    assert_eq!(seasons[0]["parentIndex"], 1);
    assert_eq!(seasons[0]["leafCount"], 2);
    // A dated season reports its own year and not its series'.
    assert_eq!(seasons[0]["year"], 2022);
    assert!(seasons[0].get("parentYear").is_none(), "{}", seasons[0]);
    // A listing: the library is on the container, not repeated on each season.
    assert!(
        seasons[0].get("librarySectionID").is_none(),
        "{}",
        seasons[0]
    );
}

#[tokio::test]
async fn an_untitled_season_is_named_at_read_time() {
    let body = listing("/library/metadata/200/children").await;
    let season = &body["MediaContainer"]["Metadata"][1];

    // The source stores no title for 9,732 of 9,736 seasons; the reference names them
    // `Season N` on the way out, and reports an empty synopsis rather than none.
    assert_eq!(season["ratingKey"], "203");
    assert_eq!(season["title"], "Season 2");
    assert_eq!(season["summary"], "");
    // Undated, so it borrows the series' year — and reports it as such.
    assert!(season.get("year").is_none(), "{season}");
    assert_eq!(season["parentYear"], 2022);
}

#[tokio::test]
async fn opening_a_season_lists_its_episodes_below_the_series() {
    let body = listing("/library/metadata/203/children").await;
    let container = &body["MediaContainer"];

    // Below a season the header names both levels above: the season as the parent and the
    // series as the grandparent, with the series' rating key as a number.
    assert_eq!(container["key"], "203");
    assert_eq!(container["grandparentRatingKey"], 200);
    assert_eq!(container["grandparentTitle"], "Severance");
    assert_eq!(container["grandparentStudio"], "Fifth Season");
    assert_eq!(container["grandparentContentRating"], "TV-MA");
    // The stored title, which is empty, next to the display form.
    assert_eq!(container["parentTitle"], "");
    assert_eq!(container["title1"], "Severance");
    assert_eq!(container["title2"], "Season 2");
    assert_eq!(container["parentIndex"], 2);
    assert_eq!(container["viewGroup"], "episode");
    assert_eq!(container["viewMode"], 65_592);
    assert!(container.get("summary").is_none(), "{container}");

    let episode = &container["Metadata"][0];
    assert_eq!(episode["ratingKey"], "204");
    assert_eq!(episode["parentTitle"], "Season 2");
    assert_eq!(episode["parentGuid"], "plex://item/203");
    assert_eq!(episode["grandparentGuid"], "plex://item/200");
    assert_eq!(episode["grandparentTitle"], "Severance");
}

#[tokio::test]
async fn a_series_grandchildren_are_its_episodes_season_by_season() {
    let body = listing("/library/metadata/200/grandchildren").await;
    let container = &body["MediaContainer"];

    // A flat list: nothing about the series, only the library's view style.
    assert_eq!(container["art"], "/:/resources/show-fanart.jpg");
    assert_eq!(container["viewGroup"], "show");
    assert_eq!(container["title1"], "VF - Series");
    assert_eq!(container["nocache"], true);
    assert_eq!(container["totalSize"], 3);
    assert!(container.get("key").is_none(), "{container}");
    assert!(container.get("librarySectionID").is_none(), "{container}");

    let keys: Vec<&str> = container["Metadata"]
        .as_array()
        .expect("episodes")
        .iter()
        .filter_map(|episode| episode["ratingKey"].as_str())
        .collect();
    // Season 1's two episodes in number order, then season 2's — not by id.
    assert_eq!(keys, vec!["202", "205", "204"]);

    // Below a season the same list is the season's own episodes.
    let body = listing("/library/metadata/201/grandchildren").await;
    assert_eq!(body["MediaContainer"]["totalSize"], 2);
}

#[tokio::test]
async fn children_page_by_absolute_offset() {
    let body = listing(
        "/library/metadata/200/grandchildren?X-Plex-Container-Start=1&X-Plex-Container-Size=1",
    )
    .await;
    let container = &body["MediaContainer"];

    assert_eq!(container["offset"], 1);
    assert_eq!(container["size"], 1);
    assert_eq!(container["totalSize"], 3);
    assert_eq!(container["Metadata"][0]["ratingKey"], "205");
}

#[tokio::test]
async fn children_of_a_leaf_are_a_bad_request_and_its_grandchildren_are_empty() {
    // A reference server distinguishes the two: `/children` on a film is a 400, while
    // `/grandchildren` on one is an empty container that still names the library.
    let (status, _) = get_from(items_app(), "/library/metadata/100/children", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get_from(items_app(), "/library/metadata/202/children", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let body = listing("/library/metadata/100/grandchildren").await;
    let container = &body["MediaContainer"];
    assert_eq!(container["size"], 0);
    assert_eq!(container["viewGroup"], "movie");
    assert_eq!(container["art"], "/:/resources/movie-fanart.jpg");
    assert!(container.get("nocache").is_none(), "{container}");
    assert!(container.get("Metadata").is_none(), "{container}");

    let (status, _) = get_from(items_app(), "/library/metadata/999999/children", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn children_name_each_element_after_its_kind_in_xml() {
    let (_, seasons) = get_from(items_app(), "/library/metadata/200/children", None).await;
    let (_, episodes) = get_from(items_app(), "/library/metadata/201/children", None).await;

    assert!(
        seasons.contains("<Directory ratingKey=\"201\""),
        "{seasons}"
    );
    assert!(seasons.contains(" nocache=\"1\""), "{seasons}");
    assert!(episodes.contains("<Video ratingKey=\"202\""), "{episodes}");
    assert!(
        episodes.contains(" grandparentRatingKey=\"200\""),
        "{episodes}"
    );
}

#[tokio::test]
async fn an_unknown_library_or_item_is_not_found() {
    // Not an empty listing: a client told "zero items" caches an empty library.
    let (status, _) = get_from(items_app(), "/library/sections/999/all", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = get_from(items_app(), "/library/metadata/999999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = get_from(items_app(), "/library/metadata/not-a-number", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn search_matches_a_prefix_of_any_word() {
    // The reference's `title=` is a word prefix, not a column prefix: `hell` finds "Good
    // News About Hell" through its fourth word, which a prefix on the title would miss.
    for term in ["hell", "news", "good"] {
        let body = listing(&format!("/library/sections/5/all?type=4&title={term}")).await;
        let items = body["MediaContainer"]["Metadata"]
            .as_array()
            .unwrap_or_else(|| panic!("items for {term}"));
        assert_eq!(items.len(), 1, "{term}");
        assert_eq!(items[0]["title"], "Good News About Hell");
    }

    // Still a word boundary: a substring inside a word does not match.
    let body = listing("/library/sections/5/all?type=4&title=ell").await;
    assert!(body["MediaContainer"].get("Metadata").is_none(), "{body}");
}

#[tokio::test]
async fn an_unsupported_filter_is_refused_rather_than_ignored() {
    // Honoring the window and dropping the filter would answer with the whole library and
    // let the client present it as the filtered result.
    let (status, _) = get_from(items_app(), "/library/sections/4/all?collection=4058", None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);

    // A parameter that is not a filter at all stays tolerated, as a real server tolerates it.
    let (status, _) = get_from(
        items_app(),
        "/library/sections/4/all?includeMeta=1&nope=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn filters_narrow_the_total_not_just_the_page() {
    // `totalSize` has to describe the filtered listing, or a client paging through it walks
    // off the end.
    let all = listing("/library/sections/4/all").await;
    let filtered = listing("/library/sections/4/all?year=2009").await;

    assert_eq!(all["MediaContainer"]["totalSize"], 2);
    assert_eq!(filtered["MediaContainer"]["totalSize"], 1);
    assert_eq!(filtered["MediaContainer"]["Metadata"][0]["title"], "Avatar");
}

#[tokio::test]
async fn the_jump_bar_buckets_by_first_letter() {
    let body = listing("/library/sections/4/firstCharacter?type=1").await;
    let container = &body["MediaContainer"];

    // A "secondary" view: buckets to jump into, not items, so no paging attributes.
    assert_eq!(container["content"], "secondary");
    assert_eq!(container["viewGroup"], "secondary");
    assert_eq!(container["title2"], "By First Letter");
    assert!(container.get("totalSize").is_none(), "{container}");

    let buckets = container["Directory"].as_array().expect("buckets");
    assert_eq!(buckets.len(), 2);
    // The count attribute is `size` here, unlike every other Directory in the surface.
    assert_eq!(buckets[0]["title"], "A");
    assert_eq!(buckets[0]["key"], "A");
    assert_eq!(buckets[0]["size"], 1);
    assert_eq!(buckets[1]["title"], "B");
}

#[tokio::test]
async fn the_jump_bar_buckets_seasons_by_their_stored_title() {
    // One season is titled "Season 1", so it buckets under S — the same path the film
    // library takes, exercised on a kind whose titles are generated rather than matched.
    // The untitled one has no character to bucket by.
    let body = listing("/library/sections/5/firstCharacter?type=3").await;
    let buckets = body["MediaContainer"]["Directory"]
        .as_array()
        .expect("buckets");
    let titles: Vec<&str> = buckets.iter().filter_map(|b| b["title"].as_str()).collect();
    assert!(titles.contains(&"S"), "{titles:?}");
}

#[tokio::test]
async fn a_listing_carries_the_first_tag_names_only() {
    // Captured on The Transporter: three genres, four of the cast credited here, and the
    // listing keeps two genres and three actors, by name alone, and no producer.
    let body = listing("/library/sections/4/all").await;
    let film = &body["MediaContainer"]["Metadata"][0];

    let names = |element: &str| -> Vec<Value> {
        film[element]
            .as_array()
            .map(|tags| tags.iter().map(|tag| tag["tag"].clone()).collect())
            .unwrap_or_default()
    };
    assert_eq!(names("Genre"), ["Action", "Crime"]);
    assert_eq!(names("Country"), ["United States of America", "France"]);
    assert_eq!(names("Director"), ["Corey Yuen"]);
    assert_eq!(names("Writer"), ["Luc Besson", "Robert Mark Kamen"]);
    assert_eq!(
        names("Role"),
        ["Jason Statham", "Shu Qi", "François Berléand"]
    );
    assert!(film.get("Producer").is_none(), "{film}");

    let first_role = film["Role"][0].as_object().expect("a role");
    assert_eq!(first_role.len(), 1, "{first_role:?}");
}

#[tokio::test]
async fn a_lookup_carries_every_tag_with_its_identity() {
    let body = listing("/library/metadata/100").await;
    let film = &body["MediaContainer"]["Metadata"][0];

    assert_eq!(film["Genre"].as_array().map(Vec::len), Some(3));
    assert_eq!(film["Role"].as_array().map(Vec::len), Some(4));
    assert_eq!(
        film["Genre"][0],
        serde_json::json!({"id": 38, "filter": "genre=38", "tag": "Action"})
    );
    // Luc Besson the producer is not Luc Besson the writer: another tag, another id.
    assert_eq!(film["Producer"][0]["filter"], "producer=475888");
    assert_eq!(film["Writer"][0]["filter"], "writer=473529");
    // The one element whose filter is not named after it.
    assert_eq!(film["Role"][0]["filter"], "actor=472942");
    assert_eq!(film["Role"][0]["role"], "Frank Martin");
    // The listing form never carries one.
    let listed = listing("/library/sections/4/all").await;
    assert!(listed["MediaContainer"]["Metadata"][0]["Role"][0]
        .get("role")
        .is_none());
}

#[tokio::test]
async fn a_lookup_writes_tags_as_the_reference_does_in_xml() {
    // Byte for byte against the capture of The Transporter on the reference server.
    let (_, body) = get_from(items_app(), "/library/metadata/100", None).await;
    for line in [
        r#"<Genre id="38" filter="genre=38" tag="Action" />"#,
        r#"<Country id="189878" filter="country=189878" tag="United States of America" />"#,
        r#"<Role id="472942" filter="actor=472942" tag="Jason Statham" tagKey="5d776826e6d55c002040af58" role="Frank Martin" thumb="https://metadata-static.plex.tv/c/people/c540bdd39873ee8e8612b0af06fc0d4c.jpg" />"#,
        r#"<Producer id="720202" filter="producer=720202" tag="Steven Chasman" tagKey="5d77682b961905001eb9251c" thumb="https://metadata-static.plex.tv/3/people/34ec649e4f0fd848499d4a2b29be78f2.jpg" />"#,
    ] {
        assert!(body.contains(line), "missing {line} in {body}");
    }

    // Element order after the media, as captured.
    let order: Vec<usize> = [
        "<Media ",
        "<Genre ",
        "<Country ",
        "<Director ",
        "<Writer ",
        "<Role ",
        "<Producer ",
    ]
    .iter()
    .map(|element| body.find(element).expect(element))
    .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{body}");
}

#[tokio::test]
async fn an_episode_listed_under_its_season_carries_its_crew() {
    let body = listing("/library/metadata/201/children").await;
    let episode = &body["MediaContainer"]["Metadata"][0];
    assert_eq!(episode["Director"][0]["tag"], "David Silverman");
    assert!(episode["Director"][0].get("id").is_none(), "{episode}");
    assert!(episode.get("Producer").is_none(), "{episode}");
}

#[tokio::test]
async fn filters_by_tag_id_or_name() {
    let total = |body: Value| body["MediaContainer"]["totalSize"].clone();

    assert_eq!(total(listing("/library/sections/4/all?genre=38").await), 1);
    // Measured on the reference: a name works where an id does, ASCII case aside.
    assert_eq!(
        total(listing("/library/sections/4/all?genre=action").await),
        1
    );
    assert_eq!(
        total(listing("/library/sections/4/all?actor=472942").await),
        1
    );
    // A comma is an `OR`, a repeated parameter an `AND`.
    assert_eq!(
        total(listing("/library/sections/4/all?genre=999,38").await),
        1
    );
    assert_eq!(
        total(listing("/library/sections/4/all?genre=38&genre=999").await),
        0
    );
    // A writer's id does not select under `producer`, even for the same person.
    assert_eq!(
        total(listing("/library/sections/4/all?producer=473529").await),
        0
    );
}

#[tokio::test]
async fn a_lookup_carries_external_ids_and_ratings_and_a_listing_does_not() {
    let body = listing("/library/metadata/100").await;
    let film = &body["MediaContainer"]["Metadata"][0];
    assert_eq!(
        film["Guid"],
        serde_json::json!([
            {"id": "imdb://tt0293662"},
            {"id": "tmdb://4108"},
            {"id": "tvdb://1601"}
        ])
    );
    assert_eq!(
        film["Rating"][1],
        serde_json::json!({
            "image": "rottentomatoes://image.rating.rotten",
            "value": 5.3,
            "type": "critic"
        })
    );

    let listed = listing("/library/sections/4/all").await;
    let first = &listed["MediaContainer"]["Metadata"][0];
    assert!(first.get("Guid").is_none(), "{first}");
    assert!(first.get("Rating").is_none(), "{first}");

    // In XML, between the countries and the crew, as captured.
    let (_, xml) = get_from(items_app(), "/library/metadata/100", None).await;
    assert!(xml.contains(r#"<Guid id="imdb://tt0293662" />"#), "{xml}");
    assert!(
        xml.contains(r#"<Rating image="imdb://image.rating" value="6.8" type="audience" />"#),
        "{xml}"
    );
    let order: Vec<usize> = ["<Country ", "<Guid ", "<Rating ", "<Director "]
        .iter()
        .map(|element| xml.find(element).expect(element))
        .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{xml}");
}

#[tokio::test]
async fn media_providers_lists_the_libraries_and_features_the_reference_does() {
    let (status, _, body) = get("/media/providers", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: Value = serde_json::from_str(&body).expect("json body");
    let container = &json["MediaContainer"];
    assert_eq!(container["size"], 1);
    // The same capabilities as `GET /`, read through the same query.
    assert_eq!(container["myPlexSubscription"], true);
    assert!(container.get("Directory").is_none(), "{container}");

    let provider = &container["MediaProvider"][0];
    assert_eq!(provider["identifier"], "com.plexapp.plugins.library");
    assert_eq!(provider["protocols"], "stream,download");
    // The reference's seventeen features, in its order.
    let features: Vec<&str> = provider["Feature"]
        .as_array()
        .expect("features")
        .iter()
        .map(|feature| feature["type"].as_str().expect("a type"))
        .collect();
    assert_eq!(
        features,
        [
            "content",
            "search",
            "match",
            "metadata",
            "rate",
            "imagetranscoder",
            "promoted",
            "continuewatching",
            "actions",
            "playlist",
            "playqueue",
            "collection",
            "timeline",
            "manage",
            "queryParser",
            "subscribe",
            "universalsearch",
        ]
    );
    assert_eq!(
        provider["Feature"][12],
        serde_json::json!({
            "scrobbleKey": "/:/scrobble",
            "unscrobbleKey": "/:/unscrobble",
            "key": "/:/timeline",
            "type": "timeline"
        })
    );

    // Home, the library, then the playlists; the library exactly as captured on section 4.
    let directories = provider["Feature"][0]["Directory"]
        .as_array()
        .expect("directories");
    assert_eq!(directories.len(), 3);
    assert_eq!(
        directories[0],
        serde_json::json!({"hubKey": "/hubs", "title": "Home"})
    );
    let mut films = directories[1].clone();
    let pivots = films
        .as_object_mut()
        .expect("an object")
        .remove("Pivot")
        .expect("pivots");
    assert_eq!(
        films,
        serde_json::json!({
            "agent": "tv.plex.agents.movie",
            "language": "fr-FR",
            "refreshing": false,
            "scanner": "Plex Movie",
            "uuid": "2cd48489-54c0-41e9-ae62-f3dcb0f3c249",
            "id": "4",
            "key": "/library/sections/4",
            "hubKey": "/hubs/sections/4",
            "type": "movie",
            "title": "VF - Films",
            "updatedAt": 1_792_916_300,
            "scannedAt": 1_792_916_245
        })
    );
    assert_eq!(
        pivots[1],
        serde_json::json!({
            "id": "library",
            "key": "/library/sections/4/all?type=1",
            "type": "list",
            "title": "Library",
            "context": "content.library",
            "symbol": "library"
        })
    );
    assert_eq!(directories[2]["id"], "playlists");
}

#[tokio::test]
async fn media_providers_writes_nested_elements_in_xml() {
    let (status, _, body) = get("/media/providers", None).await;
    assert_eq!(status, StatusCode::OK);
    // Opening tags only: the reference writes an empty `Feature` or `Directory` here as
    // `<Feature …></Feature>` where we self-close it — the same document to any XML parser,
    // and the reference itself self-closes an empty `Directory` everywhere else.
    for fragment in [
        "<MediaProvider identifier=\"com.plexapp.plugins.library\" title=\"Library\" types=\"video,audio,photo\" protocols=\"stream,download\">",
        "<Feature key=\"/library/sections\" type=\"content\">",
        "<Directory hubKey=\"/hubs\" title=\"Home\"",
        "<Pivot id=\"recommended\" key=\"/hubs/sections/4\" type=\"hub\" title=\"Recommended\" context=\"content.discover\" symbol=\"star\" />",
        "<Feature flavor=\"universal\" key=\"/playlists\" type=\"playlist\"",
        "<Action id=\"removeFromContinueWatching\" key=\"/actions/removeFromContinueWatching\" />",
        "<Feature type=\"manage\"",
    ] {
        assert!(body.contains(fragment), "missing {fragment} in {body}");
    }
}

#[tokio::test]
async fn media_providers_refuses_a_request_with_no_authenticated_caller() {
    // Unlike `GET /`, which a client calls before it holds a token.
    let response = unauthenticated(InMemoryLibrary::default())
        .oneshot(
            Request::builder()
                .uri("/media/providers")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn sessions_answer_the_empty_container_the_reference_sends_when_idle() {
    // Captured on the reference with nothing playing, in both formats.
    let (status, content_type, body) = get("/status/sessions", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    assert_eq!(body, r#"{"MediaContainer":{"size":0}}"#);

    let (_, _, xml) = get("/status/sessions", None).await;
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MediaContainer size=\"0\">\n</MediaContainer>\n"
    );
}

#[tokio::test]
async fn prefs_publish_the_settings_plex_web_reads() {
    let (status, _, body) = get("/:/prefs", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: Value = serde_json::from_str(&body).expect("json body");
    let settings = json["MediaContainer"]["Setting"]
        .as_array()
        .expect("settings");
    assert_eq!(json["MediaContainer"]["size"], 10);

    let setting = |id: &str| {
        settings
            .iter()
            .find(|setting| setting["id"] == id)
            .unwrap_or_else(|| panic!("{id}"))
    };
    // Without it a client stops on Plex's licence agreement before anything else.
    assert_eq!(setting("AcceptedEULA")["value"], true);
    assert_eq!(setting("PublishServerOnPlexOnlineKey")["value"], false);
    // Typed per setting, as the reference types them.
    assert_eq!(setting("ManualPortMappingPort")["value"], 32_400);
    assert_eq!(setting("ButlerUpdateChannel")["enumValues"], "0:Public");
    assert_eq!(
        setting("allowMediaDeletion"),
        &serde_json::json!({
            "id": "allowMediaDeletion",
            "label": "Allow media deletion",
            "summary": "The owner of the server will be allowed to delete media files from disk.",
            "type": "bool",
            "default": true,
            "value": true,
            "hidden": false,
            "advanced": true,
            "group": "library"
        })
    );
}

#[tokio::test]
async fn prefs_write_settings_as_the_reference_does_in_xml() {
    let (_, _, xml) = get("/:/prefs", None).await;
    // Byte for byte against the capture, apostrophe escaping included.
    assert!(
        xml.contains(
            r#"<Setting id="ButlerUpdateChannel" label="Server update Channel" summary="" type="text" default="16" value="16" hidden="0" advanced="0" group="general" enumValues="0:Public" />"#
        ),
        "{xml}"
    );
    assert!(xml.contains("your computer&#39;s name"), "{xml}");
}

#[tokio::test]
async fn changing_prefs_is_declared_but_not_implemented() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/:/prefs?FriendlyName=x")
                .header("X-Plex-Token", TOKEN)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn the_updater_reports_an_install_that_cannot_update_itself() {
    let (status, _, body) = get("/updater/status", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        r#"{"MediaContainer":{"size":0,"autoUpdateVersion":1,"canInstall":false,"status":0}}"#
    );
    let (_, _, xml) = get("/updater/status", None).await;
    assert!(
        xml.contains(
            r#"<MediaContainer size="0" autoUpdateVersion="1" canInstall="0" status="0">"#
        ),
        "{xml}"
    );
}

#[tokio::test]
async fn an_update_check_is_acknowledged_with_an_empty_ok() {
    // As the reference answers `PUT /updater/check?download=0`: 200, no body, `text/html`.
    let response = app()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/updater/check?download=0")
                .header("X-Plex-Token", TOKEN)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(body.is_empty());
}

#[tokio::test]
async fn announces_the_plex_version_the_captures_come_from() {
    // Plex Web refuses a server older than 1.18.3, and our own `0.1.0` reads as one. Every
    // surface a client reads the version from announces the reference's.
    for uri in ["/identity", "/", "/media/providers"] {
        let (status, _, body) = get(uri, Some("application/json")).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let json: Value = serde_json::from_str(&body).expect("json body");
        assert_eq!(
            json["MediaContainer"]["version"], "1.43.5.11029-6bf8af3f0",
            "{uri}"
        );
    }
}

#[tokio::test]
async fn a_film_library_recommends_its_recent_films_under_the_reference_shelves() {
    let body = listing(
        "/hubs/sections/4?count=1&includeLibraryPlaylists=1&excludeFields=summary&X-Plex-Language=fr",
    )
    .await;
    let container = &body["MediaContainer"];
    assert_eq!(container["librarySectionID"], 4);
    assert_eq!(
        container["librarySectionUUID"],
        "2cd48489-54c0-41e9-ae62-f3dcb0f3c249"
    );

    let hubs = container["Hub"].as_array().expect("hubs");
    let identifiers: Vec<&str> = hubs
        .iter()
        .map(|hub| hub["hubIdentifier"].as_str().expect("an identifier"))
        .collect();
    assert_eq!(
        identifiers,
        [
            "movie.inprogress.4",
            "movie.recentlyadded.4",
            "recent.library.playlists.4"
        ]
    );
    assert_eq!(container["size"], 3);

    // Nothing is in progress: an empty shelf, as the reference shows one, with no hubKey.
    assert_eq!(hubs[0]["size"], 0);
    assert_eq!(hubs[0]["title"], "Continuer à regarder");
    assert!(hubs[0].get("hubKey").is_none(), "{}", hubs[0]);

    let recent = &hubs[1];
    assert_eq!(recent["title"], "Récemment ajouté dans VF - Films");
    assert_eq!(recent["key"], "/library/sections/4/all?sort=addedAt:desc");
    assert_eq!(recent["size"], 1);
    assert_eq!(recent["more"], true);
    assert_eq!(recent["promoted"], true);
    let film = &recent["Metadata"][0];
    assert_eq!(
        recent["hubKey"],
        format!(
            "/library/metadata/{}",
            film["ratingKey"].as_str().expect("a key")
        )
    );
    // Shelf items state their library, like a lookup, and drop the synopsis on request.
    assert_eq!(film["librarySectionID"], 4);
    assert!(film.get("summary").is_none(), "{film}");
}

#[tokio::test]
async fn hub_titles_default_to_english() {
    let body = listing("/hubs/sections/4").await;
    let hubs = body["MediaContainer"]["Hub"].as_array().expect("hubs");
    assert_eq!(hubs[0]["title"], "Continue Watching");
    assert_eq!(hubs[1]["title"], "Recently Added in VF - Films");
    // Not asked for, so not there.
    assert_eq!(hubs.len(), 2);
    assert!(hubs[1]["Metadata"][0].get("summary").is_some());
}

#[tokio::test]
async fn a_series_library_groups_its_new_episodes_on_the_recently_added_shelf() {
    let body = listing("/hubs/sections/5").await;
    let hubs = body["MediaContainer"]["Hub"].as_array().expect("hubs");
    let identifiers: Vec<&str> = hubs
        .iter()
        .map(|hub| hub["hubIdentifier"].as_str().expect("an identifier"))
        .collect();
    assert_eq!(identifiers, ["tv.inprogress.5", "tv.recentlyadded.5"]);
    assert_eq!(hubs[0]["type"], "episode");

    let recent = &hubs[1];
    assert_eq!(recent["type"], "mixed");
    assert_eq!(recent["context"], "hub.tv.recentlyadded");
    assert_eq!(recent["key"], "/hubs/home/recentlyAdded?type=2&sectionID=5");
    // Severance's three episodes arrived together across two seasons: one entry, the series.
    assert_eq!(recent["size"], 1);
    assert_eq!(recent["Metadata"][0]["type"], "show");
    assert_eq!(recent["Metadata"][0]["ratingKey"], "200");
    assert_eq!(recent["hubKey"], "/library/metadata/200");
}

#[tokio::test]
async fn hubs_of_an_unknown_library_are_not_found() {
    let (status, _) = get_from(items_app(), "/hubs/sections/999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn activities_answer_the_empty_container_the_reference_sends_when_idle() {
    // Captured on the reference with no scan or refresh running, in both formats.
    let (status, _, body) = get("/activities", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"MediaContainer":{"size":0}}"#);

    let (_, content_type, xml) = get("/activities", None).await;
    assert_eq!(content_type, "text/xml;charset=utf-8");
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MediaContainer size=\"0\">\n</MediaContainer>\n"
    );
}

#[tokio::test]
async fn seasons_are_named_in_the_clients_language() {
    // Measured on the reference: `Saison 1` for `fr`, `Staffel 1` for `de`, `Season 1` with
    // no language. A stored title is never translated.
    let french = listing("/library/metadata/200/children?X-Plex-Language=fr").await;
    let titles: Vec<&str> = french["MediaContainer"]["Metadata"]
        .as_array()
        .expect("seasons")
        .iter()
        .map(|season| season["title"].as_str().expect("a title"))
        .collect();
    assert_eq!(titles, ["Season 1", "Saison 2"]);

    let german = listing("/library/metadata/204?X-Plex-Language=de").await;
    assert_eq!(
        german["MediaContainer"]["Metadata"][0]["parentTitle"],
        "Staffel 2"
    );
}

#[tokio::test]
async fn a_grouped_entry_is_dated_by_its_batch() {
    let body = listing("/hubs/sections/5").await;
    let hub = &body["MediaContainer"]["Hub"][1];
    // Severance itself, added with its episodes in the fixture: the batch's time.
    assert_eq!(hub["Metadata"][0]["addedAt"], 1_500_000_000);
}

#[tokio::test]
async fn an_episode_without_a_rating_of_its_own_carries_its_series() {
    // Good News About Hell stores none; Severance is TV-MA.
    let body = listing("/library/metadata/202").await;
    assert_eq!(
        body["MediaContainer"]["Metadata"][0]["contentRating"],
        "TV-MA"
    );
}

#[tokio::test]
async fn a_photo_needs_a_source_and_a_size() {
    let (status, _, _) = get("/photo/:/transcode?width=60&height=60", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = get("/photo/:/transcode?url=https%3A%2F%2Fplex.tv%2Fa", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_photo_is_only_fetched_from_plex_hosts() {
    for source in [
        "http%3A%2F%2F127.0.0.1%3A32400%2Fidentity",
        "https%3A%2F%2Fexample.com%2Fa.png",
        "https%3A%2F%2Fplex.tv%40example.com%2Fa.png",
    ] {
        let uri = format!("/photo/:/transcode?width=60&height=60&url={source}");
        let (status, _, _) = get(&uri, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{source}");
    }
}

#[tokio::test]
async fn library_artwork_is_refused_rather_than_reported_missing() {
    let uri = "/photo/:/transcode?width=60&height=60&url=%2Flibrary%2Fmetadata%2F1%2Fthumb%2F2";
    let (status, _, _) = get(uri, None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn a_photo_requires_a_caller() {
    let uri = "/photo/:/transcode?width=60&height=60&url=https%3A%2F%2Fplex.tv%2Fa";
    let response = unauthenticated(InMemoryLibrary::new(vec![]))
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_search_field_needs_a_term() {
    let (status, _, _) = get("/library/search?limit=30", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_search_for_something_absent_is_an_empty_container() {
    let (status, _, body) = get(
        "/library/search?query=zzzz&limit=30&searchTypes=movies,tv,people",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"MediaContainer":{"size":0}}"#);
}

#[tokio::test]
async fn a_search_of_nothing_we_hold_asks_nothing_of_the_library() {
    let (status, _, body) = get(
        "/library/search?query=dune&searchTypes=music",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"MediaContainer":{"size":0}}"#);
}

#[tokio::test]
async fn the_shelves_of_an_unknown_item_are_a_404() {
    let (status, _, _) = get("/library/metadata/999999/related", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_listing_describes_its_filters_only_when_asked() {
    let (_, _, plain) = get("/library/sections/4/all", Some("application/json")).await;
    assert!(!plain.contains("\"Meta\""));

    let (status, _, body) = get(
        "/library/sections/4/all?includeMeta=1&sort=addedAt:desc",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body).expect("json");
    let film = &body["MediaContainer"]["Meta"]["Type"][0];
    assert_eq!(film["type"], "movie");
    assert_eq!(film["active"], true);
    assert_eq!(film["Filter"][0]["key"], "/library/sections/4/genre");
    assert_eq!(film["Sort"][1]["key"], "addedAt");
    assert_eq!(film["Sort"][1]["activeDirection"], "desc");
}

#[tokio::test]
async fn the_meta_block_is_nested_elements_in_xml() {
    let (_, _, body) = get("/library/sections/4/all?includeMeta=1", None).await;
    assert!(body.contains("<Meta>"), "{body}");
    assert!(
        body.contains("<Type key=\"/library/sections/4/all?type=1\""),
        "{body}"
    );
    assert!(body.contains("<Filter filter=\"genre\""), "{body}");
}

#[tokio::test]
async fn a_library_publishes_the_settings_a_client_reads() {
    let (status, _, body) = get("/library/sections/4/prefs", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body).expect("json");
    let settings = &body["MediaContainer"]["Setting"];
    assert_eq!(body["MediaContainer"]["size"], 2);
    assert_eq!(settings[0]["id"], "hidden");
    assert_eq!(settings[0]["value"], "0");
    assert_eq!(settings[1]["id"], "includeAdultContent");
    assert_eq!(settings[1]["value"], "false");
}

#[tokio::test]
async fn an_unknown_library_has_no_settings() {
    let (status, _, _) = get("/library/sections/999/prefs", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
