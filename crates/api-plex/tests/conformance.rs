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
use open_plex_core::access::{LibraryAccess, LibraryRole};
use open_plex_core::metadata::{
    AudioAnalysis, ExternalId, FileLayout, ItemKind, ItemTag, MediaFile, MediaStream, MediaVersion,
    MetadataItem, Person, Rating, RatingKind, StreamKind, Tag, TagKind, TrackTraits,
};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{
    LibrarySection, MediaId, PartId, PersonId, Principal, RatingKey, SectionId, SectionKind,
    SectionRoot, ServerCapabilities, ServerIdentity, StreamId, TagId, UserId,
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
    // One identity for both surfaces, as the binary wires it: `/identity` reads the
    // adapter's copy while `/` reads the schema's, and a client that sees two different
    // machine identifiers from one server treats it as two servers.
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
        metadata: std::sync::Arc::new(InMemoryMetadata::default()),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    router(PlexState::new(identity, schema))
}

fn authenticated(router: axum::Router) -> axum::Router {
    router.layer(axum::Extension(open_plex_core::testing::owner()))
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
/// Avatar's DTS track.
fn avatar_audio() -> MediaStream {
    MediaStream {
        id: StreamId::new(920),
        file_id: PartId::new(910),
        kind: StreamKind::Audio,
        position: 1,
        codec: Some("dca".to_owned()),
        // The source stores BCP 47 tags.
        language: Some("fr".to_owned()),
        channels: Some(6),
        bitrate: Some(1_536_000),
        is_default: true,
        is_forced: false,
        external_path: None,
        title: None,
        stream_identifier: None,
        profile: Some("dts".to_owned()),
        bit_depth: None,
        traits: TrackTraits::default(),
        video: None,
        audio: Some(AudioAnalysis {
            sampling_rate: Some(48_000),
            channel_layout: Some("5.1(side)".to_owned()),
            loudness: None,
        }),
        subtitle: None,
        required_bandwidths: Vec::new(),
        stated: open_plex_core::metadata::StatedTrack::default(),
    }
}

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
    avatar.released_on = open_plex_core::edit::CalendarDate::parse("2009-12-16")
        .map(open_plex_core::edit::CalendarDate::midnight);
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
        layout: FileLayout::default(),
        stream_url: None,
        content_hash: None,
        open_subtitles_hash: None,
        stated: open_plex_core::metadata::StatedFile::default(),
    };

    let (external_ids, ratings) = references();
    (
        InMemoryLibrary::new(vec![films, series]),
        InMemoryMetadata::new(vec![
            avatar, barbarella, show, season, episode, half_loop, untitled, hello,
        ])
        .with_media(vec![version], vec![file], vec![avatar_audio()])
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
        refreshed_at: None,
        chapter_source: None,
    }
}

fn items_app() -> axum::Router {
    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let (library, metadata) = catalog();
    let watch = open_plex_core::testing::InMemoryWatch::over(metadata.items());
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
        watch: std::sync::Arc::new(watch),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    authenticated(router(PlexState::new(identity, schema)))
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
async fn a_series_librarys_leaves_are_its_episodes_under_a_bare_container() {
    let body = listing("/library/sections/5/allLeaves").await;
    let container = &body["MediaContainer"];
    let episodes = container["Metadata"].as_array().expect("episodes");
    assert_eq!(episodes.len(), 3);
    assert!(episodes.iter().all(|item| item["type"] == "episode"));
    // As captured on the reference's test library 28: `/all`'s container without the
    // library's identity or the view's title, and `allowSync` off.
    let mut attributes = container.as_object().expect("container").clone();
    attributes.remove("Metadata");
    assert_eq!(
        Value::Object(attributes),
        serde_json::json!({
            "size": 3,
            "totalSize": 3,
            "offset": 0,
            "allowSync": false,
            "art": "/:/resources/show-fanart.jpg",
            "identifier": "com.plexapp.plugins.library",
            "mediaTagPrefix": "/system/bundle/media/flags/",
            "mediaTagVersion": 0,
            "nocache": true,
            "thumb": "/:/resources/show.png",
            "title1": "VF - Series",
            "viewGroup": "show",
            "viewMode": 65_592
        })
    );
    // Items as `/all?type=4` lists them, the library stated on the container alone.
    let all = listing("/library/sections/5/all?type=4").await;
    assert_eq!(container["Metadata"], all["MediaContainer"]["Metadata"]);
}

#[tokio::test]
async fn a_film_librarys_leaves_are_its_films() {
    let body = listing("/library/sections/4/allLeaves").await;
    let container = &body["MediaContainer"];
    assert_eq!(container["viewGroup"], "movie");
    assert!(container.get("nocache").is_none(), "{container}");
    assert!(container.get("librarySectionID").is_none(), "{container}");
    let all = listing("/library/sections/4/all").await;
    assert_eq!(container["Metadata"], all["MediaContainer"]["Metadata"]);
}

#[tokio::test]
async fn a_type_narrows_a_librarys_leaves_rather_than_replacing_them() {
    // The reference answers `type=2` on a series library's leaves, and `type=4` on a film
    // library's, with an empty container rather than the series or the films.
    for uri in [
        "/library/sections/5/allLeaves?type=2",
        "/library/sections/5/allLeaves?type=3",
        "/library/sections/4/allLeaves?type=4",
        "/library/sections/4/allLeaves?type=13",
        "/library/sections/5/allLeaves?type=99",
    ] {
        let body = listing(uri).await;
        assert_eq!(body["MediaContainer"]["totalSize"], 0, "{uri}");
        assert!(body["MediaContainer"].get("Metadata").is_none(), "{uri}");
    }
    let episodes = listing("/library/sections/5/allLeaves?type=4").await;
    assert_eq!(episodes["MediaContainer"]["totalSize"], 3);
}

#[tokio::test]
async fn a_librarys_leaves_window_and_refuse_what_all_refuses() {
    let body =
        listing("/library/sections/5/allLeaves?X-Plex-Container-Start=1&X-Plex-Container-Size=1")
            .await;
    let container = &body["MediaContainer"];
    assert_eq!(container["size"], 1);
    assert_eq!(container["offset"], 1);
    assert_eq!(container["totalSize"], 3);
    // `includeMeta` adds nothing here, on the reference too.
    let meta = listing("/library/sections/5/allLeaves?includeMeta=1").await;
    assert!(meta["MediaContainer"].get("Meta").is_none());

    let (status, _) = get_from(items_app(), "/library/sections/5/allLeaves?sort=year", None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let (status, _) = get_from(items_app(), "/library/sections/99/allLeaves", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_librarys_leaves_are_named_after_their_kind_in_xml() {
    let (status, xml) = get_from(items_app(), "/library/sections/5/allLeaves", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        xml.contains("<MediaContainer size=\"3\" totalSize=\"3\" offset=\"0\" allowSync=\"0\""),
        "{xml}"
    );
    assert!(xml.contains("<Video ratingKey=\"202\""), "{xml}");
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

    // A part repeats the version's container, publishes the path as Plex does, and says where
    // to fetch the file.
    let part = &media["Part"][0];
    assert_eq!(part["container"], "mkv");
    assert_eq!(part["size"], 8_534_980_661_i64);
    assert_eq!(part["file"], "/mnt/movies/Avatar.mkv");
    let key = part["key"].as_str().expect("a key");
    assert!(
        key.starts_with("/library/parts/") && key.ends_with("/file.mkv"),
        "{key}"
    );

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
    // Named in the client's language, in itself when it sends none, beside the stored tag.
    assert_eq!(stream["language"], "Français");
    assert_eq!(stream["languageTag"], "fr");
    assert_eq!(stream["languageCode"], "fra");
    assert_eq!(stream["samplingRate"], 48_000);
    assert_eq!(stream["displayTitle"], "Français (DTS 5.1)");
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
    // The `All episodes` entry counts in `size` and not in `totalSize`, which counts seasons.
    assert_eq!(container["size"], 3);
    assert_eq!(container["totalSize"], 2);
    assert!(container.get("grandparentTitle").is_none(), "{container}");
    assert_eq!(
        container["Directory"],
        serde_json::json!([{
            "leafCount": 3,
            "viewedLeafCount": 0,
            "key": "/library/metadata/200/allLeaves",
            "title": "All episodes"
        }])
    );

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
async fn a_later_page_of_seasons_has_no_all_episodes_entry() {
    let body =
        listing("/library/metadata/200/children?X-Plex-Container-Start=1&X-Plex-Container-Size=5")
            .await;
    let container = &body["MediaContainer"];
    assert!(container.get("Directory").is_none(), "{container}");
    assert_eq!(container["size"], 1);
}

#[tokio::test]
async fn all_leaves_of_a_series_are_its_episodes_with_its_studio() {
    let body = listing("/library/metadata/200/allLeaves").await;
    let container = &body["MediaContainer"];
    assert_eq!(container["key"], "200");
    assert_eq!(container["parentTitle"], "Severance");
    assert_eq!(container["parentYear"], 2022);
    assert_eq!(container["title1"], "VF - Series");
    assert_eq!(container["title2"], "Severance");
    assert_eq!(container["viewGroup"], "episode");
    assert_eq!(container["mixedParents"], true);
    assert!(container.get("grandparentTitle").is_none(), "{container}");
    let episodes = container["Metadata"].as_array().expect("episodes");
    assert_eq!(container["size"], episodes.len());
    assert!(episodes
        .iter()
        .all(|e| e["type"] == "episode" && e["studio"] == "Fifth Season"));
}

#[tokio::test]
async fn all_leaves_of_anything_but_a_series_is_an_empty_container_naming_it() {
    let season = listing("/library/metadata/203/allLeaves").await;
    let season = &season["MediaContainer"];
    assert_eq!(season["size"], 0);
    assert!(season.get("Metadata").is_none(), "{season}");
    assert_eq!(season["grandparentTitle"], "Severance");
    assert_eq!(season["title1"], "Severance");
    assert_eq!(season["title2"], "Season 2");
    assert_eq!(season["parentTitle"], "");

    let episode = listing("/library/metadata/204/allLeaves").await;
    let episode = &episode["MediaContainer"];
    assert_eq!(episode["size"], 0);
    assert_eq!(episode["grandparentTitle"], "");
    assert_eq!(episode["title1"], "Season 2");

    let film = listing("/library/metadata/100/allLeaves").await;
    let film = &film["MediaContainer"];
    assert_eq!(film["size"], 0);
    assert_eq!(film["viewGroup"], "movie");
    assert!(film.get("grandparentTitle").is_none(), "{film}");
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
    let (status, _) = get_from(items_app(), "/library/sections/4/all?label=4058", None).await;
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

/// A router over the two-library catalog, as one user sees it: films (section 4) as `role`,
/// and no permission on the series (section 5).
fn app_as_user(user: i64, role: LibraryRole) -> axum::Router {
    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
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
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    router(PlexState::new(identity, schema)).layer(axum::Extension(user_principal(user, role)))
}

fn user_principal(user: i64, role: LibraryRole) -> Principal {
    Principal {
        name: format!("user{user}"),
        role: open_plex_core::ServerRole::User,
        libraries: LibraryAccess::Only(std::iter::once((SectionId::new(4), role)).collect()),
        user: UserId::new(user),
        server_identifier: Some(format!("server-of-user-{user}")),
    }
}

/// A router as the user seen by [`user_principal`] sees it, with its server paired.
async fn app_as_paired_user() -> axum::Router {
    use open_plex_core::access::{AccessRepository, Extension, RemoteAccount};

    let access = open_plex_core::testing::InMemoryAccess::default();
    let user = access
        .add_user("alice", open_plex_core::ServerRole::User)
        .unwrap()
        .id;
    access
        .connect_extension(user, Extension::Plex, "server-of-user-1", "digest", 0)
        .await
        .unwrap();
    let account = RemoteAccount {
        id: "7".to_owned(),
        name: "alice".to_owned(),
        token: "token".to_owned(),
    };
    access
        .link_account(user, Extension::Plex, &account, "https://a.example.org", 0)
        .await
        .unwrap();

    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let (library, metadata) = catalog();
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
        metadata: std::sync::Arc::new(metadata),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(access),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    router(PlexState::new(identity, schema))
        .layer(axum::Extension(user_principal(1, LibraryRole::Viewer)))
}

#[tokio::test]
async fn user_state_has_the_shape_of_plex_discover() {
    let (status, body) = user_get("/library/metadata/100/userState").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"MediaContainer":{"identifier":"com.plexapp.plugins.library","size":1,"UserState":[{"ratingKey":"100","type":"movie","viewCount":0,"viewOffset":0}]}}"#
    );

    let (_, xml) = get_from(
        app_as_user(1, LibraryRole::Viewer),
        "/library/metadata/100/userState",
        None,
    )
    .await;
    assert!(
        xml.contains(r#"<UserState ratingKey="100" type="movie" viewCount="0" viewOffset="0" />"#)
            || xml.contains(
                r#"<UserState ratingKey="100" type="movie" viewCount="0" viewOffset="0"/>"#
            ),
        "{xml}"
    );
}

#[tokio::test]
async fn user_state_is_a_404_for_what_a_caller_cannot_see() {
    for uri in [
        "/library/metadata/200/userState",
        "/library/metadata/999999/userState",
        "/library/metadata/5d776826e6d55c002040af58/userState",
    ] {
        assert_eq!(user_get(uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn a_paired_server_reports_its_account_as_the_reference_does() {
    for uri in ["/", "/media/providers"] {
        let (status, body) =
            get_from(app_as_paired_user().await, uri, Some("application/json")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let json: Value = serde_json::from_str(&body).unwrap();
        let container = json["MediaContainer"].as_object().unwrap();
        let attributes: Vec<&str> = container
            .keys()
            .filter(|key| key.starts_with("myPlex"))
            .map(String::as_str)
            .collect();
        // The order captured on the reference.
        assert_eq!(
            attributes,
            [
                "myPlex",
                "myPlexMappingState",
                "myPlexSigninState",
                "myPlexSubscription",
                "myPlexUsername"
            ],
            "{uri}"
        );
        assert_eq!(container["myPlex"], true, "{uri}");
        assert_eq!(container["myPlexMappingState"], "mapped", "{uri}");
        assert_eq!(container["myPlexSigninState"], "ok", "{uri}");
        assert_eq!(container["myPlexUsername"], "alice", "{uri}");
    }
}

#[tokio::test]
async fn an_unpaired_server_names_no_account() {
    let (_, body) = user_get("/media/providers").await;
    let json: Value = serde_json::from_str(&body).unwrap();
    let container = &json["MediaContainer"];
    assert_eq!(container["myPlex"], false);
    assert_eq!(container["myPlexSigninState"], "none");
    assert!(container.get("myPlexUsername").is_none());
    assert!(container.get("myPlexMappingState").is_none());
}

async fn user_get(uri: &str) -> (StatusCode, String) {
    get_from(
        app_as_user(1, LibraryRole::Viewer),
        uri,
        Some("application/json"),
    )
    .await
}

#[tokio::test]
async fn a_user_lists_only_the_libraries_it_was_given() {
    let (status, body) = user_get("/library/sections").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    let keys: Vec<&str> = json["MediaContainer"]["Directory"]
        .as_array()
        .unwrap()
        .iter()
        .map(|library| library["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["4"]);
}

#[tokio::test]
async fn a_user_reads_what_it_was_given() {
    assert_eq!(user_get("/library/metadata/100").await.0, StatusCode::OK);
    assert_eq!(
        user_get("/library/sections/4/all?X-Plex-Container-Start=0&X-Plex-Container-Size=10")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        user_get("/library/sections/4/allLeaves").await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn what_a_user_was_not_given_does_not_exist() {
    for uri in [
        "/library/metadata/200",
        "/library/metadata/200/children",
        "/library/metadata/200/grandchildren",
        "/library/sections/5/all?X-Plex-Container-Start=0&X-Plex-Container-Size=10",
        "/library/sections/5/allLeaves",
        "/library/sections/5/prefs",
        "/hubs/sections/5",
    ] {
        let (status, body) = user_get(uri).await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
            "{uri} answered {status}: {body}"
        );
        assert!(!body.contains("Grimm"), "{uri} leaked: {body}");
    }
}

#[tokio::test]
async fn a_batch_lookup_leaves_out_what_a_user_was_not_given() {
    let (status, body) = user_get("/library/metadata/100,200").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    let keys: Vec<&str> = json["MediaContainer"]["Metadata"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["ratingKey"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["100"]);
}

#[tokio::test]
async fn each_user_has_its_own_server_identifier() {
    let identifier = |body: String| -> String {
        let json: Value = serde_json::from_str(&body).unwrap();
        json["MediaContainer"]["machineIdentifier"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    for uri in ["/identity", "/"] {
        let first = get_from(
            app_as_user(1, LibraryRole::Viewer),
            uri,
            Some("application/json"),
        )
        .await;
        let second = get_from(
            app_as_user(2, LibraryRole::Viewer),
            uri,
            Some("application/json"),
        )
        .await;
        assert_eq!(identifier(first.1), "server-of-user-1", "{uri}");
        assert_eq!(identifier(second.1), "server-of-user-2", "{uri}");
    }
}

#[tokio::test]
async fn an_owner_without_an_extension_keeps_the_servers_identifier() {
    let (_, _, body) = get("/identity", Some("application/json")).await;
    assert!(body.contains("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"));
}

#[tokio::test]
async fn a_stranger_sees_the_server_published_at_the_address_it_used() {
    // What the transport attaches to a request naming no account, at a published address.
    let app = unauthenticated(InMemoryLibrary::new(Vec::new())).layer(axum::Extension(
        open_plex_core::PublishedServer("server-published-here".to_owned()),
    ));
    let (status, body) = get_from(app, "/identity", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("server-published-here"), "{body}");
}

#[tokio::test]
async fn only_a_librarys_admin_gets_past_changing_it() {
    let change = |user_role: LibraryRole, section: i64| async move {
        app_as_user(1, user_role)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/library/sections/{section}/prefs"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    };
    assert_eq!(
        change(LibraryRole::Admin, 4).await,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(change(LibraryRole::Viewer, 4).await, StatusCode::NOT_FOUND);
    assert_eq!(change(LibraryRole::Admin, 5).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn server_wide_views_stay_with_owners() {
    assert_eq!(user_get("/status/sessions").await.0, StatusCode::NOT_FOUND);
    assert_eq!(user_get("/updater/status").await.0, StatusCode::NOT_FOUND);
}

async fn timeline(
    app: axum::Router,
    method: &str,
    query: &str,
    accept: &str,
) -> (StatusCode, String) {
    let request = Request::builder()
        .method(method)
        .uri(format!("/:/timeline?{query}"))
        .header("X-Plex-Token", TOKEN)
        .header(header::ACCEPT, accept)
        .body(Body::empty())
        .expect("request builds");
    let response = app.oneshot(request).await.expect("router responds");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

const IGNORED_TIMELINE: &str = r#"{"MediaContainer":{"size":0,"playbackState":"ignore"}}"#;

#[tokio::test]
async fn a_timeline_answers_as_the_reference_on_every_verb_it_accepts() {
    // Captured on the reference for a film, at position 0.
    for method in ["GET", "POST", "PUT"] {
        let (status, body) = timeline(
            items_app(),
            method,
            "ratingKey=100&state=playing&time=0",
            "application/json",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{method}");
        assert_eq!(body, IGNORED_TIMELINE, "{method}");
    }

    let (_, xml) = timeline(
        items_app(),
        "GET",
        "ratingKey=100&state=paused&time=0",
        "*/*",
    )
    .await;
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MediaContainer size=\"0\" playbackState=\"ignore\">\n</MediaContainer>\n"
    );
}

#[tokio::test]
async fn a_timeline_refuses_and_ignores_what_the_reference_does() {
    // Each line was captured on the reference.
    for (query, expected) in [
        ("", StatusCode::BAD_REQUEST),
        ("time=1000", StatusCode::BAD_REQUEST),
        ("ratingKey=999&time=0", StatusCode::BAD_REQUEST),
        (
            "ratingKey=999&state=nonsense&time=0",
            StatusCode::BAD_REQUEST,
        ),
        ("state=playing&time=abc", StatusCode::BAD_REQUEST),
        (
            "ratingKey=999&state=playing&time=abc",
            StatusCode::BAD_REQUEST,
        ),
        ("ratingKey=999&state=playing&time=0", StatusCode::NOT_FOUND),
        ("ratingKey=999&state=playing", StatusCode::NOT_FOUND),
        ("state=playing", StatusCode::OK),
        ("state=playing&time=1000", StatusCode::OK),
        ("ratingKey=abc&state=playing&time=0", StatusCode::OK),
        ("ratingKey=100&state=stopped", StatusCode::OK),
    ] {
        let (status, body) = timeline(items_app(), "GET", query, "application/json").await;
        assert_eq!(status, expected, "{query}");
        if expected == StatusCode::OK {
            assert_eq!(body, IGNORED_TIMELINE, "{query}");
        }
    }
}

#[tokio::test]
async fn a_timeline_updates_the_session_of_its_playback_then_closes_it() {
    use open_plex_core::now_epoch;
    use open_plex_core::page::Window;
    use open_plex_core::playback::{
        NewPlaybackSession, PlaybackOutput, PlaybackRepository, PlaybackState, SessionStatus,
        StreamDecision,
    };
    use open_plex_core::testing::InMemoryPlayback;

    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let (library, metadata) = catalog();
    let playback = std::sync::Arc::new(InMemoryPlayback::default());
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
        metadata: std::sync::Arc::new(metadata),
        playback: playback.clone(),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    let app = authenticated(router(PlexState::new(identity, schema)));

    // Plex Web's two identifiers differ: `session` names the conversion, this one the playback.
    playback
        .start(
            NewPlaybackSession {
                key: open_plex_core::testing::owner().session_key("transcode-session"),
                user: open_plex_core::testing::owner().user,
                playback_id: Some(open_plex_core::testing::owner().session_key("tab")),
                item: RatingKey::new(100),
                part: PartId::new(1),
                client: None,
                product: None,
                video: StreamDecision::Copy,
                audio: StreamDecision::Copy,
                position_ms: 0,
                output: PlaybackOutput {
                    video_bitrate: Some(3_419_000),
                    audio_bitrate: Some(119_000),
                    width: Some(1280),
                    height: Some(720),
                },
            },
            now_epoch(),
        )
        .await
        .expect("start");
    let sessions = || async {
        playback
            .sessions(
                None,
                SessionStatus::Playing,
                now_epoch(),
                Window::new(None, None, Some(10), None).expect("window"),
            )
            .await
            .expect("page")
            .items
    };
    let report = |query: &'static str| timeline(app.clone(), "GET", query, "application/json");

    report("ratingKey=100&state=paused&time=2000&X-Plex-Session-Identifier=someone-else").await;
    assert_eq!(sessions().await[0].state, PlaybackState::Playing);

    report("ratingKey=100&state=paused&time=2000&X-Plex-Session-Identifier=tab").await;
    let (_, answer) = timeline(
        app.clone(),
        "GET",
        "ratingKey=100&state=paused&time=2000&X-Plex-Session-Identifier=tab",
        "application/json",
    )
    .await;
    let answer: Value = serde_json::from_str(&answer).expect("json body");
    let container = &answer["MediaContainer"];
    assert_eq!(container["size"], 2);
    // Both tracks are copied here, and a copied track weighs 2.1 times its bitrate.
    assert_eq!(
        container["Bandwidths"][0]["Bandwidth"][0],
        serde_json::json!({ "time": 0, "bandwidth": 7430, "resolution": "720p" })
    );
    assert_eq!(
        container["TranscodeSession"][0]["key"],
        "/transcode/sessions/transcode-session"
    );

    let current = sessions().await;
    assert_eq!(current[0].state, PlaybackState::Paused);
    assert_eq!(current[0].position_ms, 2000);

    report("ratingKey=100&state=stopped&time=2000&X-Plex-Session-Identifier=tab").await;
    assert!(sessions().await.is_empty());
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
async fn server_settings_are_refused_so_plex_web_offers_no_settings_screen() {
    let (status, _, _) = get("/:/prefs", Some("application/json")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
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
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn capabilities_offer_no_plugin_channels() {
    let (_, _, body) = get("/", Some("application/json")).await;
    let body: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(body["MediaContainer"]["allowChannelAccess"], false);
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
    assert_eq!(
        drawn_apart(hubs),
        [
            "movie.inprogress.4",
            "movie.recentlyreleased.4",
            "movie.recentlyadded.4",
            "movie.genre.4",
            "movie.by.actor.or.director.4",
            "movie.topunwatched.4",
            "movie.recentlyviewed.4",
            "recent.library.playlists.4"
        ]
    );
    assert_eq!(container["size"], 8);

    // A drawn genre names itself in the identifier, the title and the list.
    let genre = &hubs[3];
    let id = genre["hubIdentifier"]
        .as_str()
        .and_then(|identifier| identifier.rsplit('.').next())
        .expect("a genre id");
    assert_eq!(
        genre["key"],
        format!("/library/sections/4/all?unwatched=1&genre={id}&audienceRating%3E=7.0")
    );
    assert!(
        genre["title"]
            .as_str()
            .is_some_and(|title| title.starts_with("Meilleurs films de ")),
        "{genre}"
    );
    assert!(genre.get("random").is_none());
    assert_eq!(hubs[4]["random"], true);

    // Nothing is in progress: an empty shelf, as the reference shows one, with no hubKey.
    assert_eq!(hubs[0]["size"], 0);
    assert_eq!(hubs[0]["title"], "Continuer à regarder");
    assert!(hubs[0].get("hubKey").is_none(), "{}", hubs[0]);

    // A listing shelf links to its list, the `>` encoded so the link can be followed here.
    let released = &hubs[1];
    assert_eq!(released["title"], "Films sortis récemment");
    assert_eq!(
        released["key"],
        "/library/sections/4/all?sort=originallyAvailableAt:desc&originallyAvailableAt%3E=-1y"
    );
    assert_eq!(released["context"], "hub.movie.recentlyreleased");
    assert!(released.get("random").is_none());
    let unwatched = &hubs[5];
    assert_eq!(unwatched["title"], "Meilleurs films non vus");
    assert_eq!(unwatched["random"], true);
    // Nothing was watched: an empty shelf rather than an absent one, as on the reference.
    let viewed = &hubs[6];
    assert_eq!(viewed["title"], "Films regardés récemment");
    assert_eq!(viewed["size"], 0);
    assert!(viewed.get("hubKey").is_none(), "{viewed}");

    let recent = &hubs[2];
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
    assert_eq!(hubs[1]["title"], "Recently Released Movies");
    assert_eq!(hubs[2]["title"], "Recently Added in VF - Films");
    // Playlists were not asked for, so not there.
    assert_eq!(hubs.len(), 7);
    assert!(hubs[2]["Metadata"][0].get("summary").is_some());
}

#[tokio::test]
async fn a_series_library_groups_its_new_episodes_on_the_recently_added_shelf() {
    let body = listing("/hubs/sections/5").await;
    let hubs = body["MediaContainer"]["Hub"].as_array().expect("hubs");
    let identifiers: Vec<&str> = hubs
        .iter()
        .map(|hub| hub["hubIdentifier"].as_str().expect("an identifier"))
        .collect();
    assert_eq!(
        identifiers,
        [
            "tv.inprogress.5",
            "tv.recentlyaired.5",
            "tv.recentlyadded.5",
            "tv.startwatching.5",
            "tv.rediscover.5",
            "tv.toprated.5",
            "tv.recentlyviewed.5"
        ]
    );
    assert_eq!(hubs[0]["type"], "episode");
    assert_eq!(hubs[1]["type"], "episode");
    assert_eq!(hubs[3]["type"], "show");
    // Nothing was begun, so the series is there to start.
    assert_eq!(hubs[3]["Metadata"][0]["type"], "show");

    let recent = &hubs[2];
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
    // Sixth in the reference's list, after the title, release date, ratings and duration.
    assert_eq!(film["Sort"][5]["key"], "addedAt");
    assert_eq!(film["Sort"][5]["activeDirection"], "desc");
}

#[tokio::test]
async fn a_sort_the_listing_cannot_honor_is_refused() {
    // Rather than answered in title order, which a client would show as what it asked for.
    for sort in ["unviewedLeafCount:desc", "year", "rating:desc,titleSort"] {
        let (status, _, _) = get(&format!("/library/sections/4/all?sort={sort}"), None).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{sort}");
    }
    for sort in [
        "rating:desc",
        "originallyAvailableAt",
        "lastViewedAt:desc",
        "random",
        "mediaHeight:desc",
        "duration",
    ] {
        let (status, _, _) = get(&format!("/library/sections/4/all?sort={sort}"), None).await;
        assert_eq!(status, StatusCode::OK, "{sort}");
    }
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

#[tokio::test]
async fn a_filter_lists_the_genres_a_library_carries() {
    let body = listing("/library/sections/4/genre").await;
    let container = &body["MediaContainer"];

    assert_eq!(container["title2"], "By Genre");
    assert_eq!(container["content"], "secondary");
    assert_eq!(container["viewGroup"], "secondary");
    assert!(container.get("nocache").is_none(), "{container}");
    let genres = container["Directory"].as_array().expect("genres");
    let titles: Vec<&str> = genres.iter().filter_map(|g| g["title"].as_str()).collect();
    assert_eq!(titles, ["Action", "Crime", "Thriller"]);
    assert_eq!(genres[0]["key"], "38");
    assert_eq!(genres[0]["fastKey"], "/library/sections/4/all?genre=38");
    assert_eq!(genres[0]["type"], "genre");
    assert_eq!(container["totalSize"], 3);
}

#[tokio::test]
async fn the_starring_list_stops_at_the_third_billed_actor() {
    // Four of the cast are attached; the reference lists an actor only when billed first,
    // second or third somewhere.
    let body = listing("/library/sections/4/actor").await;
    let container = &body["MediaContainer"];

    assert_eq!(container["title2"], "By Starring Actor");
    // The one list the reference sends without `content`.
    assert!(container.get("content").is_none(), "{container}");
    let names: Vec<&str> = container["Directory"]
        .as_array()
        .expect("actors")
        .iter()
        .filter_map(|actor| actor["title"].as_str())
        .collect();
    assert_eq!(names, ["François Berléand", "Jason Statham", "Shu Qi"]);
    let statham = &container["Directory"][1];
    assert_eq!(statham["key"], "472942");
    assert_eq!(
        statham["thumb"],
        "https://metadata-static.plex.tv/c/people/c540bdd39873ee8e8612b0af06fc0d4c.jpg"
    );
}

#[tokio::test]
async fn years_and_decades_list_newest_first() {
    let years = listing("/library/sections/4/year").await;
    assert_eq!(years["MediaContainer"]["Directory"][0]["key"], "2009");
    assert_eq!(
        years["MediaContainer"]["Directory"][0]["fastKey"],
        "/library/sections/4/all?year=2009"
    );

    let decades = listing("/library/sections/4/decade").await;
    let decade = &decades["MediaContainer"]["Directory"][0];
    assert_eq!(decade["key"], "2000");
    assert_eq!(decade["title"], "2000s");
    assert_eq!(decades["MediaContainer"]["title2"], "By Decade");
}

#[tokio::test]
async fn studios_and_ratings_are_encoded_as_the_reference_encodes_them() {
    let ratings = listing("/library/sections/4/contentRating").await;
    let rating = &ratings["MediaContainer"]["Directory"][0];
    assert_eq!(rating["key"], "fr%2F12");
    assert_eq!(
        rating["fastKey"],
        "/library/sections/4/all?contentRating=fr%2F12"
    );
    assert_eq!(rating["title"], "fr/12");

    let studios = listing("/library/sections/4/studio").await;
    let studio = &studios["MediaContainer"]["Directory"][0];
    assert_eq!(studio["key"], "Lightstorm");
    assert_eq!(
        studio["fastKey"],
        "/library/sections/4/all?studio=Lightstorm"
    );
}

#[tokio::test]
async fn a_series_library_lists_its_shows_choices_uncached() {
    let body = listing("/library/sections/5/studio?type=2").await;
    let container = &body["MediaContainer"];
    assert_eq!(container["nocache"], true);
    assert_eq!(container["Directory"][0]["title"], "Fifth Season");
}

#[tokio::test]
async fn an_unknown_list_or_library_is_absent() {
    let (status, _) = get_from(items_app(), "/library/sections/4/nonsense", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A library that does not exist is a 404, not an empty list.
    let (status, _) = get_from(items_app(), "/library/sections/99/genre", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_resolution_list_names_the_classes_present_without_a_listing_url() {
    // Avatar's only version is 1920×808: 1080p by its width.
    let body = listing("/library/sections/4/resolution").await;
    let container = &body["MediaContainer"];
    assert_eq!(container["title2"], "By Resolution");
    assert_eq!(
        container["Directory"],
        serde_json::json!([{"key": "1080", "title": "1080p"}])
    );
}

#[tokio::test]
async fn the_resolution_filter_selects_by_class() {
    let in_class = listing("/library/sections/4/all?resolution=1080").await;
    assert_eq!(in_class["MediaContainer"]["totalSize"], 1);
    assert_eq!(
        in_class["MediaContainer"]["Metadata"][0]["Media"][0]["videoResolution"],
        "1080"
    );
    // 808 lines is not the 1,072 to 1,440 the `2k` filter takes, nor 720p.
    for class in ["2k", "720", "sd"] {
        let body = listing(&format!("/library/sections/4/all?resolution={class}")).await;
        assert_eq!(body["MediaContainer"]["totalSize"], 0, "{class}");
    }
}

#[tokio::test]
async fn a_value_list_windows_like_a_listing() {
    let body =
        listing("/library/sections/4/genre?X-Plex-Container-Start=1&X-Plex-Container-Size=1").await;
    let container = &body["MediaContainer"];
    assert_eq!(container["size"], 1);
    assert_eq!(container["offset"], 1);
    assert_eq!(container["totalSize"], 3);
    assert_eq!(container["Directory"][0]["title"], "Crime");
}

/// One app whose state lives across requests: watch state is what a test changes.
fn watching_app() -> axum::Router {
    items_app()
}

async fn json_from(app: &axum::Router, uri: &str) -> Value {
    let (status, body) = get_from(app.clone(), uri, Some("application/json")).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    serde_json::from_str(&body).expect("json body")
}

/// A timeline report from a player naming its playback, as Plex Web does.
async fn timeline_report(app: &axum::Router, item: i64, position: i64, duration: i64) -> Value {
    let uri = format!(
        "/:/timeline?ratingKey={item}&key=/library/metadata/{item}&state=playing&time={position}&duration={duration}"
    );
    let request = Request::builder()
        .uri(uri)
        .header("X-Plex-Token", TOKEN)
        .header(header::ACCEPT, "application/json")
        .header("X-Plex-Client-Identifier", "conformance-player")
        .header("X-Plex-Session-Identifier", "conformance-playback")
        .body(Body::empty())
        .expect("request builds");
    let response = app.clone().oneshot(request).await.expect("router responds");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&body).expect("json body")
}

fn watched_fields(item: &Value) -> (Value, Value, bool) {
    (
        item["viewCount"].clone(),
        item["viewOffset"].clone(),
        item.get("lastViewedAt").is_some(),
    )
}

#[tokio::test]
async fn a_scrobble_counts_a_viewing_and_an_unscrobble_forgets_it() {
    let app = watching_app();
    let film = || async {
        json_from(&app, "/library/metadata/100").await["MediaContainer"]["Metadata"][0].clone()
    };
    assert_eq!(
        watched_fields(&film().await),
        (Value::Null, Value::Null, false)
    );

    let (status, body) = get_from(
        app.clone(),
        "/:/scrobble?identifier=com.plexapp.plugins.library&key=100",
        None,
    )
    .await;
    // An empty acknowledgement, as the reference sends.
    assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
    get_from(
        app.clone(),
        "/:/scrobble?identifier=com.plexapp.plugins.library&key=100",
        None,
    )
    .await;
    let watched = film().await;
    assert_eq!(watched["viewCount"], 2);
    assert!(watched.get("lastViewedAt").is_some(), "{watched}");
    // Placed where the reference places them: after the ratings, before the year.
    let keys: Vec<&str> = watched
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let position = |name: &str| keys.iter().position(|key| *key == name).unwrap();
    assert!(position("audienceRating") < position("viewCount"));
    assert!(position("lastViewedAt") < position("year"));

    get_from(
        app.clone(),
        "/:/unscrobble?identifier=com.plexapp.plugins.library&key=100",
        None,
    )
    .await;
    assert_eq!(
        watched_fields(&film().await),
        (Value::Null, Value::Null, false)
    );
}

#[tokio::test]
async fn a_mark_is_refused_as_the_reference_refuses_it() {
    let app = watching_app();
    let status = |uri: &'static str| {
        let app = app.clone();
        async move { get_from(app, uri, None).await.0 }
    };
    assert_eq!(status("/:/scrobble?key=100").await, StatusCode::BAD_REQUEST);
    assert_eq!(
        status("/:/scrobble?identifier=com.plexapp.plugins.library").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status("/:/scrobble?identifier=tv.plex.provider.discover&key=100").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status("/:/unscrobble?identifier=com.plexapp.plugins.library&key=999").await,
        StatusCode::NOT_FOUND
    );
    // `/:/progress` acknowledges an item it does not have.
    assert_eq!(
        status("/:/progress?identifier=com.plexapp.plugins.library&key=999&time=120000").await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn progress_keeps_a_position_without_asking_whether_the_film_is_over() {
    let app = watching_app();
    // 95% of Avatar's 9,890,048 ms: a viewing by the timeline, a mere position here.
    get_from(
        app.clone(),
        "/:/progress?identifier=com.plexapp.plugins.library&key=100&time=9395545&state=stopped",
        None,
    )
    .await;
    let film = json_from(&app, "/library/metadata/100").await;
    assert_eq!(
        film["MediaContainer"]["Metadata"][0]["viewOffset"],
        9_395_545
    );
    assert!(film["MediaContainer"]["Metadata"][0]
        .get("viewCount")
        .is_none());
}

#[tokio::test]
async fn the_timeline_answers_what_became_of_the_watch_state() {
    let app = watching_app();
    let duration = 9_890_048;
    // Under a minute nothing is kept; a player naming its session is told the playback goes
    // on, as the reference tells it.
    let early = timeline_report(&app, 100, 30_000, duration).await;
    assert_eq!(
        early["MediaContainer"],
        serde_json::json!({"size": 0, "playbackState": "progress", "skipCount": 0, "viewCount": 0, "viewOffset": -1})
    );
    let midway = timeline_report(&app, 100, 120_000, duration).await;
    assert_eq!(midway["MediaContainer"]["playbackState"], "progress");
    assert_eq!(midway["MediaContainer"]["viewOffset"], 120_000);
    let done = timeline_report(&app, 100, duration * 95 / 100, duration).await;
    assert_eq!(done["MediaContainer"]["playbackState"], "complete");
    assert_eq!(done["MediaContainer"]["viewCount"], 1);
    assert_eq!(done["MediaContainer"]["viewOffset"], -1);

    let film = json_from(&app, "/library/metadata/100").await;
    assert_eq!(film["MediaContainer"]["Metadata"][0]["viewCount"], 1);
    assert!(film["MediaContainer"]["Metadata"][0]
        .get("viewOffset")
        .is_none());
}

#[tokio::test]
async fn a_series_counts_the_episodes_watched_below_it() {
    let app = watching_app();
    get_from(
        app.clone(),
        "/:/scrobble?identifier=com.plexapp.plugins.library&key=202",
        None,
    )
    .await;

    let series = json_from(&app, "/library/sections/5/all").await;
    let show = &series["MediaContainer"]["Metadata"][0];
    assert_eq!(show["viewedLeafCount"], 1);
    // The episode counted on its series too, as on the reference.
    assert_eq!(show["viewCount"], 1);

    let seasons = json_from(&app, "/library/metadata/200/children").await;
    let container = &seasons["MediaContainer"];
    assert_eq!(container["Directory"][0]["viewedLeafCount"], 1);
    let first = container["Metadata"]
        .as_array()
        .unwrap()
        .iter()
        .find(|season| season["ratingKey"] == "201")
        .unwrap();
    assert_eq!(first["viewedLeafCount"], 1);

    // Marking the whole series watched marks its episodes, not the series again.
    get_from(
        app.clone(),
        "/:/scrobble?identifier=com.plexapp.plugins.library&key=200",
        None,
    )
    .await;
    let series = json_from(&app, "/library/sections/5/all").await;
    let show = &series["MediaContainer"]["Metadata"][0];
    assert_eq!(show["viewedLeafCount"], show["leafCount"]);
    assert_eq!(show["viewCount"], 1);
}

#[tokio::test]
async fn continue_watching_lists_films_started_and_next_episodes_newest_first() {
    let app = watching_app();
    get_from(
        app.clone(),
        "/:/scrobble?identifier=com.plexapp.plugins.library&key=202",
        None,
    )
    .await;
    // Started a second later, so listed first.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    get_from(
        app.clone(),
        "/:/progress?identifier=com.plexapp.plugins.library&key=100&time=600000",
        None,
    )
    .await;

    let body = json_from(&app, "/hubs/continueWatching").await;
    let hub = &body["MediaContainer"]["Hub"][0];
    assert_eq!(hub["hubIdentifier"], "continueWatching");
    assert_eq!(hub["context"], "hub.continueWatching");
    assert_eq!(hub["type"], "mixed");
    assert_eq!(hub["style"], "hero");
    assert_eq!(hub["hubKey"], "/library/metadata/100,205");
    let items = hub["Metadata"].as_array().unwrap();
    assert_eq!(items[0]["ratingKey"], "100");
    assert_eq!(items[0]["viewOffset"], 600_000);
    assert_eq!(items[0]["includedAt"], items[0]["lastViewedAt"]);
    // The episode after the one watched, dated by its series though never played itself.
    assert_eq!(items[1]["ratingKey"], "205");
    assert!(items[1].get("lastViewedAt").is_some(), "{}", items[1]);
    // Lookup-shaped: the library is stated on each item.
    assert_eq!(items[1]["librarySectionID"], 5);

    let started = json_from(&app, "/hubs/home/continueWatching").await;
    let keys = |body: &Value| -> Vec<String> {
        body["MediaContainer"]["Metadata"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["ratingKey"].as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(keys(&started), ["100"]);
    assert!(started["MediaContainer"]["Metadata"][0]
        .get("includedAt")
        .is_none());
    assert_eq!(keys(&json_from(&app, "/hubs/home/onDeck").await), ["205"]);
    assert_eq!(
        keys(&json_from(&app, "/hubs/sections/5/continueWatching/items").await),
        ["205"]
    );
}

#[tokio::test]
async fn a_seasons_last_episode_leads_to_the_next_season_on_the_librarys_shelf() {
    let app = watching_app();
    get_from(
        app.clone(),
        "/:/scrobble?identifier=com.plexapp.plugins.library&key=205",
        None,
    )
    .await;
    let hubs = json_from(&app, "/hubs/sections/5").await;
    let shelf = &hubs["MediaContainer"]["Hub"][0];
    assert_eq!(shelf["hubIdentifier"], "tv.inprogress.5");
    assert_eq!(shelf["size"], 1);
    assert_eq!(shelf["hubKey"], "/library/metadata/204");
    assert_eq!(shelf["Metadata"][0]["ratingKey"], "204");
}

#[tokio::test]
async fn continue_watching_is_empty_when_nothing_was_played() {
    let app = watching_app();
    let body = json_from(&app, "/hubs/continueWatching").await;
    let hub = &body["MediaContainer"]["Hub"][0];
    assert_eq!(hub["size"], 0);
    assert!(hub.get("hubKey").is_none(), "{hub}");
}

#[tokio::test]
async fn a_series_librarys_recent_list_is_its_shelf_unwindowed() {
    let body = listing("/hubs/home/recentlyAdded?type=2&sectionID=5").await;
    let container = &body["MediaContainer"];
    // The same single entry as the shelf: Severance's three new episodes, as the series.
    assert_eq!(container["size"], 1);
    assert_eq!(container["Metadata"][0]["ratingKey"], "200");
    // Not asked for a window and holding the whole list, so it says nothing of one.
    assert!(container.get("totalSize").is_none(), "{container}");
    assert!(container.get("offset").is_none(), "{container}");

    let windowed = listing(
        "/hubs/home/recentlyAdded?type=2&sectionID=5&X-Plex-Container-Start=0&X-Plex-Container-Size=5",
    )
    .await;
    assert_eq!(windowed["MediaContainer"]["totalSize"], 1);
    assert_eq!(windowed["MediaContainer"]["offset"], 0);
}

#[tokio::test]
async fn a_recent_list_of_no_type_or_the_wrong_kind_is_empty() {
    // Measured: an empty container, never an error.
    for query in [
        "sectionID=5",
        "type=4&sectionID=5",
        "type=2&sectionID=4",
        "type=1&sectionID=5",
        "type=2&sectionID=999",
    ] {
        let body = listing(&format!("/hubs/home/recentlyAdded?{query}")).await;
        assert_eq!(body["MediaContainer"]["size"], 0, "{query}");
        assert!(body["MediaContainer"].get("Metadata").is_none(), "{query}");
    }
}

#[tokio::test]
async fn a_film_recent_list_pages_over_the_whole_library() {
    let body = listing(
        "/hubs/home/recentlyAdded?type=1&sectionID=4&X-Plex-Container-Start=1&X-Plex-Container-Size=1",
    )
    .await;
    let container = &body["MediaContainer"];
    let all = listing("/library/sections/4/all").await;
    assert_eq!(container["totalSize"], all["MediaContainer"]["totalSize"]);
    assert_eq!(container["offset"], 1);
    assert_eq!(container["size"], 1);
    assert_eq!(container["Metadata"][0]["type"], "movie");
}

/// A library's shelf identifiers, without the id of the genre or person a shelf was drawn
/// for: the draw changes with the shuffle's period.
fn drawn_apart(hubs: &[Value]) -> Vec<String> {
    hubs.iter()
        .map(|hub| {
            let identifier = hub["hubIdentifier"].as_str().expect("an identifier");
            if identifier.starts_with("movie.genre.")
                || identifier.starts_with("movie.by.actor.or.director.")
            {
                identifier
                    .rsplit_once('.')
                    .map_or(identifier, |(head, _)| head)
                    .to_owned()
            } else {
                identifier.to_owned()
            }
        })
        .collect()
}

async fn put_status(app: &axum::Router, uri: &str) -> (StatusCode, Option<String>) {
    let request = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("X-Plex-Token", TOKEN)
        .body(Body::empty())
        .expect("request builds");
    let response = app.clone().oneshot(request).await.expect("router responds");
    let kind = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    (response.status(), kind)
}

#[tokio::test]
async fn choosing_a_track_is_an_empty_200_and_a_wrong_one_a_400() {
    let app = watching_app();
    // As measured: an empty `text/html` 200 for a track of the part and of the kind named,
    // and for `subtitleStreamID=0`, which turns subtitles off.
    for uri in [
        "/library/parts/910?audioStreamID=920",
        "/library/parts/910?subtitleStreamID=0&allParts=1",
    ] {
        assert_eq!(
            put_status(&app, uri).await,
            (StatusCode::OK, Some("text/html".to_owned())),
            "{uri}"
        );
    }
    // Nothing named, an audio track named as a subtitle, a track of no file, a part unknown.
    for uri in [
        "/library/parts/910",
        "/library/parts/910?subtitleStreamID=920",
        "/library/parts/910?audioStreamID=999",
        "/library/parts/999?audioStreamID=920",
        "/library/parts/910?audioStreamID=x",
    ] {
        assert_eq!(
            put_status(&app, uri).await.0,
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn a_lookup_marks_the_audio_the_caller_plays() {
    let body = json_from(&watching_app(), "/library/metadata/100").await;
    let stream = &body["MediaContainer"]["Metadata"][0]["Media"][0]["Part"][0]["Stream"][0];
    assert_eq!(stream["id"], 920);
    assert_eq!(stream["selected"], true);
}

/// The reference's test libraries: a film named after its file in 27, a series in 28. The
/// series carries the eight locks item 429849 of the reference carries, whatever a series holds
/// of them.
fn editing_app(principal: Principal) -> axum::Router {
    use open_plex_core::edit::ItemField;
    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let file_name = "Soul.2020.MULTi.TRUEFRENCH.1080p.WEB.H264-EXTREME_wWw.Extreme-Down.Video";
    let film = MetadataItem {
        title_sort: Some(file_name.to_owned()),
        summary: None,
        ..item(577_986, 27, ItemKind::Movie, None, file_name)
    };
    let show = MetadataItem {
        summary: None,
        ..item(577_987, 28, ItemKind::Show, None, "Bull")
    };
    let locks = [
        ItemField::Title,
        ItemField::TitleSort,
        ItemField::OriginalTitle,
        ItemField::Studio,
        ItemField::Poster,
        ItemField::ReleasedOn,
        ItemField::Year,
        ItemField::Writers,
    ]
    .map(|field| (RatingKey::new(577_987), field));
    let season = item(577_988, 28, ItemKind::Season, Some(577_987), "Season 1");
    let metadata =
        InMemoryMetadata::new(vec![film, show, season]).with_locked_fields(locks.to_vec());
    let library = InMemoryLibrary::new(vec![
        section(27, SectionKind::Movie, "open-plex tests", false),
        section(28, SectionKind::Show, "open-plex tests series", false),
    ]);
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
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    router(PlexState::new(identity, schema)).layer(axum::Extension(principal))
}

async fn put_on(app: &axum::Router, uri: &str) -> (StatusCode, String, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

async fn film_after(app: &axum::Router) -> Value {
    let (status, body) = get_from(
        app.clone(),
        "/library/metadata/577986",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    body["MediaContainer"]["Metadata"][0].clone()
}

#[tokio::test]
async fn a_lookup_lists_its_locked_fields_last_as_the_reference_does() {
    let app = editing_app(open_plex_core::testing::owner());
    let (status, xml) = get_from(app.clone(), "/library/metadata/577987", None).await;
    assert_eq!(status, StatusCode::OK);
    // As the reference lists item 429849's, but for `writer`: a series holds no writer.
    let captured = "<Field locked=\"1\" name=\"title\" />\
<Field locked=\"1\" name=\"titleSort\" />\
<Field locked=\"1\" name=\"originalTitle\" />\
<Field locked=\"1\" name=\"studio\" />\
<Field locked=\"1\" name=\"thumb\" />\
<Field locked=\"1\" name=\"originallyAvailableAt\" />\
<Field locked=\"1\" name=\"year\" />\
<Field locked=\"1\" name=\"writer\" />";
    let fields: String = xml
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("<Field"))
        .collect();
    assert_eq!(fields, captured);
    assert!(
        xml.contains("<Field locked=\"1\" name=\"writer\" />\n</Directory>"),
        "{xml}"
    );

    let (_, json) = get_from(
        app.clone(),
        "/library/metadata/577987",
        Some("application/json"),
    )
    .await;
    let json: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        json["MediaContainer"]["Metadata"][0]["Field"][0],
        serde_json::json!({ "locked": true, "name": "title" })
    );

    let (_, listing) = get_from(app, "/library/sections/28/all", None).await;
    assert!(!listing.contains("<Field"), "{listing}");
}

#[tokio::test]
async fn an_edit_answers_an_empty_200_and_locks_only_what_it_says() {
    let app = editing_app(open_plex_core::testing::owner());
    let (status, content_type, body) = put_on(
        &app,
        "/library/sections/27/all?type=1&id=577986&includeExternalMedia=1\
         &title.value=The%20Soul&summary.value=Probe%20summary&summary.locked=1",
    )
    .await;
    assert_eq!(
        (status, content_type.as_str(), body.as_str()),
        (StatusCode::OK, "text/html", "")
    );
    let film = film_after(&app).await;
    assert_eq!(film["title"], "The Soul");
    assert_eq!(film["titleSort"], "Soul");
    assert_eq!(film["summary"], "Probe summary");
    assert_eq!(
        film["Field"],
        serde_json::json!([{ "locked": true, "name": "summary" }])
    );

    // A film's year follows its date: locking the date locks both, the year alone nothing.
    put_on(
        &app,
        "/library/sections/27/all?type=1&id=577986&year.locked=1",
    )
    .await;
    assert_eq!(film_after(&app).await["Field"].as_array().unwrap().len(), 1);
    put_on(
        &app,
        "/library/sections/27/all?type=1&id=577986&originallyAvailableAt.value=2019-05-01\
         &originallyAvailableAt.locked=1&summary.locked=0",
    )
    .await;
    let film = film_after(&app).await;
    assert_eq!(film["year"], 2019);
    assert_eq!(film["originallyAvailableAt"], "2019-05-01");
    assert_eq!(
        film["Field"],
        serde_json::json!([
            { "locked": true, "name": "originallyAvailableAt" },
            { "locked": true, "name": "year" },
        ])
    );
}

#[tokio::test]
async fn an_edit_ignores_items_of_another_type_or_library_as_the_reference_does() {
    let app = editing_app(open_plex_core::testing::owner());
    for uri in [
        "/library/sections/27/all?type=2&id=577986&summary.value=wrong%20type",
        "/library/sections/28/all?type=1&id=577986&summary.value=wrong%20section",
        "/library/sections/27/all?type=1&id=abc&summary.value=no%20item",
    ] {
        assert_eq!(put_on(&app, uri).await.0, StatusCode::OK, "{uri}");
    }
    assert_eq!(film_after(&app).await.get("summary"), None);
}

#[tokio::test]
async fn an_edit_refuses_what_it_cannot_honor_and_what_makes_no_sense() {
    let app = editing_app(open_plex_core::testing::owner());
    for (uri, expected) in [
        (
            "/library/sections/27/all?id=577986&summary.value=x",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/all?type=1&id=577986&bogus.locked=1",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/all?type=1&id=577986&summary.locked=2",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/all?type=1&id=577986&year.value=abc",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/all?type=1&id=577986&originallyAvailableAt.value=garbage",
            StatusCode::BAD_REQUEST,
        ),
        // The reference would edit every film of the library.
        (
            "/library/sections/27/all?type=1&summary.value=x",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/library/sections/27/all?type=1&id=577986&thumb.value=x",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/library/sections/27/all?type=1&id=577986&genre%5B0%5D.tag.tag=Drama",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/library/sections/28/all?type=3&id=577988&title.value=x",
            StatusCode::NOT_IMPLEMENTED,
        ),
    ] {
        assert_eq!(put_on(&app, uri).await.0, expected, "{uri}");
    }
    assert_eq!(film_after(&app).await.get("summary"), None);
}

#[tokio::test]
async fn only_a_librarys_admin_edits_it() {
    let viewer = Principal {
        libraries: LibraryAccess::Only(
            [(SectionId::new(27), LibraryRole::Viewer)]
                .into_iter()
                .collect(),
        ),
        ..user_principal(1, LibraryRole::Viewer)
    };
    let uri = "/library/sections/27/all?type=1&id=577986&summary.value=x";
    assert_eq!(
        put_on(&editing_app(viewer), uri).await.0,
        StatusCode::NOT_FOUND
    );
    let admin = Principal {
        libraries: LibraryAccess::Only(
            [(SectionId::new(27), LibraryRole::Admin)]
                .into_iter()
                .collect(),
        ),
        ..user_principal(1, LibraryRole::Admin)
    };
    let app = editing_app(admin);
    assert_eq!(put_on(&app, uri).await.0, StatusCode::OK);
    assert_eq!(film_after(&app).await["summary"], "x");
}

async fn show_after(app: &axum::Router) -> Value {
    let (status, body) = get_from(
        app.clone(),
        "/library/metadata/577987",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    body["MediaContainer"]["Metadata"][0].clone()
}

#[tokio::test]
async fn an_item_edit_takes_the_specs_bare_fields_and_every_item_named() {
    let app = editing_app(open_plex_core::testing::owner());
    // As on the reference: `type` ignored, the bare form of the spec, `.value` winning over it,
    // and a lock that is neither `0` nor `1` let pass.
    let (status, content_type, body) = put_on(
        &app,
        "/library/metadata/577986,577987?type=2&title=Bare&title.value=The%20Soul&tagline=A%20tagline&summary.locked=1&studio.locked=yes&X-Plex-Product=Plex%20Web",
    )
    .await;
    assert_eq!(
        (status, content_type.as_str(), body.as_str()),
        (StatusCode::OK, "text/html", "")
    );
    let film = film_after(&app).await;
    assert_eq!(film["title"], "The Soul");
    assert_eq!(film["titleSort"], "Soul");
    assert_eq!(film["tagline"], "A tagline");
    assert_eq!(
        film["Field"],
        serde_json::json!([{ "locked": true, "name": "summary" }])
    );
    // The series, in another library and of another type, edited alike: its title is locked,
    // and a value is written whatever the lock, as the official API writes it.
    let show = show_after(&app).await;
    assert_eq!(show["title"], "The Soul");
    assert_eq!(show["tagline"], "A tagline");
}

#[tokio::test]
async fn an_item_edit_answers_as_the_reference_does_when_it_edits_nothing() {
    let app = editing_app(open_plex_core::testing::owner());
    for (uri, expected) in [
        // Measured on the reference: no parameter, and a field name it does not know.
        ("/library/metadata/577986", StatusCode::BAD_REQUEST),
        (
            "/library/metadata/577986?nonsense=x",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/metadata/577986?nonsense.value=x",
            StatusCode::BAD_REQUEST,
        ),
        // No item named, or nothing a number; one known among unknown ones is edited.
        ("/library/metadata/999999?title=x", StatusCode::NOT_FOUND),
        ("/library/metadata/abc?title=x", StatusCode::NOT_FOUND),
        (
            "/library/metadata/577986,999999?summary=Known",
            StatusCode::OK,
        ),
        // `type` alone changes nothing, and answers as an edit.
        ("/library/metadata/577986?type=1", StatusCode::OK),
        // What we cannot do: a season, artwork, tags.
        (
            "/library/metadata/577988?title=x",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/library/metadata/577986?thumb=x",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/library/metadata/577986?genre%5B0%5D.tag.tag=Drama",
            StatusCode::NOT_IMPLEMENTED,
        ),
        ("/library/metadata/577986?year=abc", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(put_on(&app, uri).await.0, expected, "{uri}");
    }
    assert_eq!(film_after(&app).await["summary"], "Known");
}

#[tokio::test]
async fn only_a_librarys_admin_edits_its_items() {
    let viewer = Principal {
        libraries: LibraryAccess::Only(
            [(SectionId::new(27), LibraryRole::Viewer)]
                .into_iter()
                .collect(),
        ),
        ..user_principal(1, LibraryRole::Viewer)
    };
    let uri = "/library/metadata/577986?summary=x";
    assert_eq!(
        put_on(&editing_app(viewer), uri).await.0,
        StatusCode::NOT_FOUND
    );
    let admin = Principal {
        libraries: LibraryAccess::Only(
            [(SectionId::new(27), LibraryRole::Admin)]
                .into_iter()
                .collect(),
        ),
        ..user_principal(1, LibraryRole::Admin)
    };
    let app = editing_app(admin);
    // The series is in a library this user was not given: as if it did not exist.
    assert_eq!(
        put_on(&app, "/library/metadata/577986,577987?summary=x")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(film_after(&app).await["summary"], "x");
}

/// A request with any method, answered with its status, content type, paging headers and body.
async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    accept: Option<&str>,
) -> (StatusCode, String, Vec<(String, String)>, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Plex-Token", TOKEN);
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    let response = app
        .clone()
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
    let paging = response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str().starts_with("x-plex-container"))
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        content_type,
        paging,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

const EMPTY_XML: &str =
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MediaContainer size=\"0\">\n</MediaContainer>\n";

#[tokio::test]
async fn live_tv_lists_answer_the_empty_container_the_reference_sends_without_a_dvr() {
    // Captured on the reference, which has no DVR: both formats, byte for byte.
    let app = app();
    for uri in ["/livetv/dvrs", "/livetv/sessions"] {
        let (status, content_type, paging, xml) = call(&app, "GET", uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(content_type, "text/xml;charset=utf-8", "{uri}");
        assert!(paging.is_empty(), "{uri}");
        assert_eq!(xml, EMPTY_XML, "{uri}");
        let (_, content_type, _, json) = call(&app, "GET", uri, Some("application/json")).await;
        assert_eq!(content_type, "application/json", "{uri}");
        assert_eq!(json, r#"{"MediaContainer":{"size":0}}"#, "{uri}");
    }

    // Empty here where the reference lists what it has: the grabbers it ships, a tuner left
    // over from an old setup, plex.tv's guide countries and languages, and recordings it
    // refuses to list without a subscription. The empty form is the one above.
    for uri in [
        "/media/grabbers",
        "/media/grabbers/devices",
        "/media/subscriptions",
        "/media/subscriptions/scheduled",
        "/livetv/epg/countries",
        "/livetv/epg/languages",
        "/media/subscriptions/template?guid=plex://movie/5d776825880197001ec967c8",
    ] {
        let (status, _, _, xml) = call(&app, "GET", uri, None).await;
        assert_eq!((status, xml.as_str()), (StatusCode::OK, EMPTY_XML), "{uri}");
    }
    let (status, _, _, xml) = call(&app, "POST", "/media/grabbers/devices/discover", None).await;
    assert_eq!((status, xml.as_str()), (StatusCode::OK, EMPTY_XML));
}

#[tokio::test]
async fn an_empty_live_tv_list_reports_the_window_it_was_asked_for() {
    let app = app();
    let uri = "/livetv/dvrs?X-Plex-Container-Start=0&X-Plex-Container-Size=1";
    let expected_paging = vec![
        ("x-plex-container-start".to_owned(), "0".to_owned()),
        ("x-plex-container-total-size".to_owned(), "0".to_owned()),
    ];
    let (_, _, mut paging, xml) = call(&app, "GET", uri, None).await;
    paging.sort();
    assert_eq!(paging, expected_paging);
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MediaContainer size=\"0\" totalSize=\"0\" offset=\"0\">\n</MediaContainer>\n"
    );
    let (_, _, _, json) = call(&app, "GET", uri, Some("application/json")).await;
    assert_eq!(
        json,
        r#"{"MediaContainer":{"size":0,"totalSize":0,"offset":0}}"#
    );
}

#[tokio::test]
async fn no_dvr_device_recording_or_session_exists() {
    // Each captured `404` on the reference, for an id it does not know.
    let app = app();
    for (method, uri) in [
        ("GET", "/livetv/dvrs/999"),
        ("DELETE", "/livetv/dvrs/999"),
        ("POST", "/livetv/dvrs/999/channels/5/tune"),
        ("PUT", "/livetv/dvrs/999/devices/999"),
        ("DELETE", "/livetv/dvrs/999/devices/999"),
        ("PUT", "/livetv/dvrs/999/lineups?lineup=x"),
        ("DELETE", "/livetv/dvrs/999/lineups?lineup=x"),
        ("PUT", "/livetv/dvrs/999/prefs?name=value"),
        ("POST", "/livetv/dvrs/999/reloadGuide"),
        ("DELETE", "/livetv/dvrs/999/reloadGuide"),
        ("GET", "/livetv/sessions/abc"),
        ("GET", "/livetv/sessions/abc/def/index.m3u8"),
        ("GET", "/livetv/sessions/abc/def/0"),
        ("GET", "/media/grabbers/devices/999"),
        ("PUT", "/media/grabbers/devices/999?enabled=1"),
        ("DELETE", "/media/grabbers/devices/999"),
        ("GET", "/media/grabbers/devices/999/channels"),
        ("PUT", "/media/grabbers/devices/999/channelmap"),
        ("PUT", "/media/grabbers/devices/999/prefs?name=value"),
        ("POST", "/media/grabbers/devices/999/scan"),
        ("DELETE", "/media/grabbers/devices/999/scan"),
        ("GET", "/media/grabbers/devices/999/thumb/1"),
        ("DELETE", "/media/grabbers/operations/999"),
        ("DELETE", "/media/subscriptions/999"),
        ("PUT", "/media/subscriptions/999/move"),
        // Under a guide provider this server does not have, as `nope` is on the reference.
        ("GET", "/livetv/epg/countries/fra/nope/lineups"),
        ("GET", "/livetv/epg/countries/fra/nope/regions"),
        ("GET", "/livetv/epg/countries/fra/nope/regions/x/lineups"),
        ("GET", "/livetv/epg/channelmap?device=x&lineup=y"),
        ("GET", "/livetv/epg/channels?lineup=lineup://nope/x"),
        ("GET", "/livetv/epg/lineupchannels?lineup=lineup://nope/x"),
    ] {
        let (status, content_type, _, _) = call(&app, method, uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        assert_eq!(content_type, "text/html", "{method} {uri}");
    }

    // A guide provider's own routes, which the reference answers `404` without a DVR.
    for uri in [
        "/tv.plex.providers.epg.cloud:1/grid",
        "/tv.plex.providers.epg.cloud:1/lineups/dvr/channels",
        "/tv.plex.providers.epg.cloud:1/watchnow",
        "/tv.plex.providers.epg.cloud:1/watchnow/all",
    ] {
        let (status, _, _, _) = call(&app, "GET", uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn a_guide_lookup_missing_what_it_names_is_a_bad_request() {
    // Captured `400`s on the reference.
    let app = app();
    for uri in [
        "/livetv/epg/channelmap",
        "/livetv/epg/channelmap?device=x",
        "/livetv/epg/channels",
        "/livetv/epg/lineupchannels",
        "/livetv/epg/lineup",
        "/livetv/epg/lineup?device=x",
        "/media/subscriptions/template",
    ] {
        let (status, content_type, _, _) = call(&app, "GET", uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(content_type, "text/html", "{uri}");
    }
}

#[tokio::test]
async fn nothing_can_be_set_up_or_recorded() {
    let app = app();
    for (method, uri, expected) in [
        ("POST", "/livetv/dvrs", StatusCode::BAD_REQUEST),
        (
            "POST",
            "/livetv/dvrs?device=device://tv.plex.grabbers.hdhomerun/x",
            StatusCode::NOT_FOUND,
        ),
        (
            "POST",
            "/media/grabbers/devices?uri=http://10.0.0.2",
            StatusCode::BAD_REQUEST,
        ),
        ("POST", "/media/subscriptions", StatusCode::BAD_REQUEST),
    ] {
        let (status, _, _, _) = call(&app, method, uri, None).await;
        assert_eq!(status, expected, "{method} {uri}");
    }
    let (status, content_type, _, body) =
        call(&app, "POST", "/media/subscriptions/process", None).await;
    assert_eq!(
        (status, content_type.as_str(), body.as_str()),
        (StatusCode::OK, "text/html", "")
    );
}

/// The container the reference sends around no item of a kind a library does not hold,
/// captured on its "VF - Films" with `type=8`, `9` and `10` and a window. `mediaTagVersion`
/// is the reference's media flag bundle, which this server reports as 0.
const NO_MUSIC_IN_FILMS: &str = r#"{"MediaContainer":{"size":0,"totalSize":0,"offset":0,"allowSync":true,"art":"/:/resources/movie-fanart.jpg","identifier":"com.plexapp.plugins.library","librarySectionID":4,"librarySectionTitle":"VF - Films","librarySectionUUID":"2cd48489-54c0-41e9-ae62-f3dcb0f3c249","mediaTagPrefix":"/system/bundle/media/flags/","mediaTagVersion":1791210760,"thumb":"/:/resources/movie.png","title1":"VF - Films","title2":"All VF - Films","viewGroup":"movie","viewMode":65592}}"#;

fn with_our_flag_bundle(captured: &str) -> Value {
    let mut value: Value = serde_json::from_str(captured).expect("captured json");
    value["MediaContainer"]["mediaTagVersion"] = Value::from(0);
    value
}

#[tokio::test]
async fn a_film_library_lists_no_artist_album_or_track() {
    for kind in [8, 9, 10] {
        let uri = format!("/library/sections/4/all?type={kind}");
        assert_eq!(
            listing(&uri).await,
            with_our_flag_bundle(NO_MUSIC_IN_FILMS),
            "{uri}"
        );
    }
    // And every view of the library agrees: no letter, no genre, no year.
    for uri in [
        "/library/sections/4/firstCharacter?type=10",
        "/library/sections/4/genre?type=10",
        "/library/sections/4/year?type=9",
    ] {
        let body = listing(uri).await;
        assert_eq!(body["MediaContainer"]["size"], 0, "{uri}");
        assert!(body["MediaContainer"].get("Directory").is_none(), "{uri}");
    }
}

#[tokio::test]
async fn albums_are_an_empty_view_of_any_library() {
    // Captured on the reference's film and series libraries, windowed as this server always
    // windows.
    let films = r#"{"MediaContainer":{"size":0,"totalSize":0,"offset":0,"allowSync":false,"art":"/:/resources/movie-fanart.jpg","identifier":"com.plexapp.plugins.library","mediaTagPrefix":"/system/bundle/media/flags/","mediaTagVersion":1791210760,"mixedParents":true,"thumb":"/:/resources/movie.png","title1":"VF - Films","title2":"By Album","viewGroup":"album","viewMode":65592}}"#;
    assert_eq!(
        listing("/library/sections/4/albums").await,
        with_our_flag_bundle(films)
    );
    let series = listing("/library/sections/5/albums").await;
    assert_eq!(series["MediaContainer"]["nocache"], true);
    assert_eq!(series["MediaContainer"]["title2"], "By Album");

    let (status, xml) = get_from(items_app(), "/library/sections/4/albums", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(xml.contains(r#"allowSync="0""#) && xml.contains(r#"mixedParents="1""#));

    let (status, _) = get_from(items_app(), "/library/sections/999/albums", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_music_library_is_listed_and_holds_nothing() {
    let app = app_with(vec![section(30, SectionKind::Artist, "Musique", false)]);
    let (_, sections) = get_from(app.clone(), "/library/sections", Some("application/json")).await;
    let sections: Value = serde_json::from_str(&sections).expect("json");
    assert_eq!(sections["MediaContainer"]["Directory"][0]["type"], "artist");

    for uri in [
        "/library/sections/30/all",
        "/library/sections/30/all?type=8",
        "/library/sections/30/all?type=10",
        "/library/sections/30/albums",
        "/hubs/sections/30",
    ] {
        let (status, body) = get_from(app.clone(), uri, Some("application/json")).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let body: Value = serde_json::from_str(&body).expect("json");
        assert_eq!(body["MediaContainer"]["size"], 0, "{uri}");
    }
}

#[tokio::test]
async fn a_search_of_one_type_finds_that_kind_only_and_no_track() {
    let everything = listing("/search?query=avatar").await;
    assert_eq!(
        everything["MediaContainer"]["Provider"]
            .as_array()
            .map(Vec::len),
        Some(4)
    );

    // As the reference answers `type=1`: the films alone, and no other source advertised.
    let films = listing("/search?query=avatar&type=1").await;
    assert_eq!(films["MediaContainer"]["Metadata"][0]["title"], "Avatar");
    assert!(films["MediaContainer"].get("Provider").is_none());
    assert!(
        listing("/search?query=avatar&type=2").await["MediaContainer"]
            .get("Metadata")
            .is_none()
    );

    // Captured with `type=10`, and identical with `type=99`.
    let none = r#"{"MediaContainer":{"size":0,"identifier":"com.plexapp.plugins.library","mediaTagPrefix":"/system/bundle/media/flags/","mediaTagVersion":1791210760}}"#;
    for kind in [10, 99] {
        assert_eq!(
            listing(&format!("/search?query=avatar&type={kind}")).await,
            with_our_flag_bundle(none),
            "{kind}"
        );
    }
}

/// The attribute names of a JSON element, in order.
fn keys_of(element: &Value) -> Vec<&str> {
    element
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

/// Avatar played past the end, and the first episode of Severance marked watched.
async fn watched_app() -> axum::Router {
    let app = watching_app();
    timeline_report(&app, 100, 4_945_024, 9_890_048).await;
    timeline_report(&app, 100, 9_000_000, 9_890_048).await;
    let (status, _) = get_from(
        app.clone(),
        "/:/scrobble?identifier=com.plexapp.plugins.library&key=202",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    app
}

#[tokio::test]
async fn history_lists_what_was_played_with_the_captured_attributes() {
    let app = watched_app().await;
    let (status, content_type, paging, body) = call(
        &app,
        "GET",
        "/status/sessions/history/all",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    let body: Value = serde_json::from_str(&body).unwrap();
    let container = &body["MediaContainer"];
    // What was marked is not in the default listing, as on the reference.
    assert_eq!(container["size"], 1);
    assert_eq!(container["totalSize"], 1);
    assert_eq!(container["offset"], 0);
    assert!(paging.contains(&("x-plex-container-total-size".to_owned(), "1".to_owned())));

    let entry = &container["Metadata"][0];
    // The captured order, less `thumb` and `deviceID`.
    assert_eq!(
        keys_of(entry),
        [
            "historyKey",
            "key",
            "ratingKey",
            "librarySectionID",
            "title",
            "type",
            "originallyAvailableAt",
            "viewedAt",
            "accountID"
        ]
    );
    assert!(entry["historyKey"]
        .as_str()
        .unwrap()
        .starts_with("/status/sessions/history/"));
    assert_eq!(entry["key"], "/library/metadata/100");
    assert_eq!(entry["ratingKey"], "100");
    // A string, where `accountID` and `viewedAt` are numbers.
    assert_eq!(entry["librarySectionID"], "4");
    assert_eq!(entry["accountID"], 1);
    assert!(entry["viewedAt"].is_i64());
    assert_eq!(entry["originallyAvailableAt"], "2009-12-16");
}

#[tokio::test]
async fn history_of_what_was_marked_carries_the_view_type_and_the_series() {
    let app = watched_app().await;
    let body = json_from(
        &app,
        "/status/sessions/history/all?type=1&metadataItemID=200",
    )
    .await;
    let entry = &body["MediaContainer"]["Metadata"][0];
    assert_eq!(
        keys_of(entry),
        [
            "historyKey",
            "key",
            "ratingKey",
            "librarySectionID",
            "parentKey",
            "grandparentKey",
            "title",
            "grandparentTitle",
            "type",
            "index",
            "parentIndex",
            "viewedAt",
            "accountID",
            "viewType"
        ]
    );
    assert_eq!(entry["parentKey"], "/library/metadata/201");
    assert_eq!(entry["grandparentKey"], "/library/metadata/200");
    assert_eq!(entry["grandparentTitle"], "Severance");
    assert_eq!(entry["viewType"], 1);

    // The XML element is named after the item, as in any listing.
    let (_, content_type, _, xml) =
        call(&app, "GET", "/status/sessions/history/all?type=1", None).await;
    assert_eq!(content_type, "text/xml;charset=utf-8");
    assert!(
        xml.contains("<Video historyKey=\"/status/sessions/history/"),
        "{xml}"
    );
    assert!(xml.contains("viewType=\"1\""), "{xml}");
}

#[tokio::test]
async fn history_filters_and_refuses_as_the_reference_does() {
    let app = watched_app().await;
    let size = |uri: &'static str| {
        let app = app.clone();
        async move { json_from(&app, uri).await["MediaContainer"]["size"].clone() }
    };
    assert_eq!(
        size("/status/sessions/history/all?librarySectionID=5").await,
        0
    );
    assert_eq!(
        size("/status/sessions/history/all?librarySectionID=4,5").await,
        1
    );
    assert_eq!(size("/status/sessions/history/all?accountID=2").await, 0);
    assert_eq!(
        size("/status/sessions/history/all?accountID=1&deviceID=7").await,
        1
    );
    assert_eq!(size("/status/sessions/history/all?type=4").await, 0);
    assert_eq!(
        size("/status/sessions/history/all?sort=viewedAt:desc,accountID").await,
        1
    );
    assert_eq!(
        size("/status/sessions/history/all?viewedAt%3E=4000000000").await,
        0
    );

    let status = |uri: &'static str| {
        let app = app.clone();
        async move { call(&app, "GET", uri, None).await.0 }
    };
    for refused in [
        "/status/sessions/history/all?sort=title",
        "/status/sessions/history/all?librarySectionID=abc",
        "/status/sessions/history/all?type=movie",
    ] {
        assert_eq!(status(refused).await, StatusCode::BAD_REQUEST, "{refused}");
    }
    assert_eq!(
        status("/status/sessions/history/all?metadataItemID=999").await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_history_entry_is_read_without_its_keys_and_deleted_without_the_state() {
    let app = watched_app().await;
    let listed = json_from(&app, "/status/sessions/history/all").await;
    let key = listed["MediaContainer"]["Metadata"][0]["historyKey"]
        .as_str()
        .unwrap()
        .to_owned();

    let one = json_from(&app, &key).await;
    let entry = &one["MediaContainer"]["Metadata"][0];
    assert_eq!(one["MediaContainer"]["size"], 1);
    assert!(one["MediaContainer"].get("totalSize").is_none());
    assert_eq!(
        keys_of(entry),
        [
            "historyKey",
            "librarySectionID",
            "title",
            "type",
            "originallyAvailableAt",
            "viewedAt",
            "accountID"
        ]
    );

    let (status, _, _, body) = call(&app, "DELETE", &key, Some("application/json")).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, r#"{"MediaContainer":{"size":0}}"#)
    );
    assert_eq!(call(&app, "GET", &key, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        call(&app, "DELETE", &key, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "DELETE", "/status/sessions/history/abc", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let film = json_from(&app, "/library/metadata/100").await;
    assert_eq!(film["MediaContainer"]["Metadata"][0]["viewCount"], 1);
}

/// Jason Statham's page as the reference writes it, but for the generation of the flags.
const STATHAM: &str = r#"{"MediaContainer":{"size":1,"identifier":"com.plexapp.plugins.library","mediaTagPrefix":"/system/bundle/media/flags/","mediaTagVersion":0,"Directory":[{"id":472942,"filter":"actor=472942","tag":"Jason Statham","tagType":6,"tagKey":"5d776826e6d55c002040af58","thumb":"https://metadata-static.plex.tv/c/people/c540bdd39873ee8e8612b0af06fc0d4c.jpg"}]}}"#;

#[tokio::test]
async fn a_person_page_is_the_tag_with_its_kind_number() {
    let (status, body) = get_from(
        items_app(),
        "/library/people/472942",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, STATHAM);

    // Any tag answers; what the provider does not know is absent.
    let genre = listing("/library/people/38").await;
    assert_eq!(
        genre["MediaContainer"]["Directory"][0],
        serde_json::json!({"id": 38, "filter": "genre=38", "tag": "Action", "tagType": 1})
    );
    let director = listing("/library/people/705517").await;
    let director = &director["MediaContainer"]["Directory"][0];
    assert_eq!(director["filter"], "director=705517");
    assert_eq!(director["tagType"], 4);
    assert!(director.get("thumb").is_none());
}

#[tokio::test]
async fn a_person_page_is_a_directory_in_xml() {
    let (status, body) = get_from(items_app(), "/library/people/473529", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"<Directory id="473529" filter="writer=473529" tag="Luc Besson" tagType="5" tagKey="5d776824103a2d001f563ac4""#),
        "{body}"
    );
}

#[tokio::test]
async fn a_person_is_addressed_by_tag_id_only() {
    for uri in [
        "/library/people/999999",
        // The spec's other form, the provider's key, which the reference refuses too.
        "/library/people/5d776826e6d55c002040af58",
        "/library/people/abc",
        "/library/people/999999/media",
        "/library/people/abc/media",
    ] {
        let (status, _) = get_from(items_app(), uri, Some("application/json")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn a_persons_media_lists_a_series_for_a_credit_on_its_episode() {
    let media = listing("/library/people/705517/media").await;
    let container = &media["MediaContainer"];
    assert_eq!(container["size"], 1);
    // Whole and unpaged, as the reference answers: no window to state.
    assert!(container.get("totalSize").is_none());
    assert!(container.get("offset").is_none());
    // Only an actor's page carries a view.
    assert!(container.get("viewGroup").is_none());
    let show = &container["Metadata"][0];
    assert_eq!(show["ratingKey"], "200");
    assert_eq!(show["type"], "show");
    assert_eq!(show["librarySectionTitle"], "VF - Series");
    assert_eq!(show["librarySectionID"], 5);
    assert_eq!(show["librarySectionKey"], "/library/sections/5");
}

#[tokio::test]
async fn an_actors_media_carries_the_grid_view_and_names_each_library() {
    let media = listing("/library/people/472942/media").await;
    let container = &media["MediaContainer"];
    assert_eq!(container["viewGroup"], "movie");
    assert_eq!(container["viewMode"], 65_592);
    let film = &container["Metadata"][0];
    assert_eq!(film["ratingKey"], "100");
    // After the title, as on the reference; the sort title equal to it is omitted.
    let keys: Vec<&str> = film
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let at = |name: &str| keys.iter().position(|key| *key == name).unwrap();
    assert!(film.get("titleSort").is_none());
    assert_eq!(at("librarySectionTitle"), at("title") + 1);
    assert_eq!(at("librarySectionKey"), at("librarySectionID") + 1);

    let (_, xml) = get_from(items_app(), "/library/people/472942/media", None).await;
    assert!(
        xml.contains(r#"viewGroup="movie" viewMode="65592""#),
        "{xml}"
    );
    assert!(xml.contains(r#"<Video ratingKey="100""#), "{xml}");
}

#[tokio::test]
async fn a_persons_media_pages_with_both_container_parameters() {
    let response = items_app()
        .oneshot(
            Request::builder()
                .uri(
                    "/library/people/472942/media?X-Plex-Container-Start=0&X-Plex-Container-Size=0",
                )
                .header("X-Plex-Token", TOKEN)
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["X-Plex-Container-Start"], "0");
    assert_eq!(response.headers()["X-Plex-Container-Total-Size"], "1");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["MediaContainer"]["size"], 0);
    assert_eq!(body["MediaContainer"]["totalSize"], 1);
    assert_eq!(body["MediaContainer"]["offset"], 0);

    // One parameter alone is ignored, as on the reference.
    let alone = listing("/library/people/472942/media?X-Plex-Container-Size=0").await;
    assert_eq!(alone["MediaContainer"]["size"], 1);
}

#[tokio::test]
async fn a_persons_media_honors_filters_and_ignores_sort_and_type() {
    let none = listing("/library/people/472942/media?year=1990").await;
    assert_eq!(none["MediaContainer"]["size"], 0);
    let kept = listing("/library/people/472942/media?genre=38").await;
    assert_eq!(kept["MediaContainer"]["size"], 1);
    // A sort `/all` refuses, and a type naming episodes: neither changes the list here.
    let ignored = listing("/library/people/472942/media?sort=bogus:desc&type=4").await;
    assert_eq!(ignored["MediaContainer"]["Metadata"][0]["ratingKey"], "100");

    let (status, _) = get_from(
        items_app(),
        "/library/people/472942/media?label=1",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn a_person_a_user_was_not_given_does_not_exist() {
    assert_eq!(user_get("/library/people/472942").await.0, StatusCode::OK);
    for uri in ["/library/people/705517", "/library/people/705517/media"] {
        let (status, body) = user_get(uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
}

/// The shelves of a search, by `hubIdentifier`, with their sizes.
fn search_shelves(body: &Value) -> Vec<(String, u64)> {
    body["MediaContainer"]["Hub"]
        .as_array()
        .expect("hubs")
        .iter()
        .map(|hub| {
            (
                hub["hubIdentifier"]
                    .as_str()
                    .expect("identifier")
                    .to_owned(),
                hub["size"].as_u64().expect("size"),
            )
        })
        .collect()
}

fn search_shelf<'a>(body: &'a Value, identifier: &str) -> &'a Value {
    body["MediaContainer"]["Hub"]
        .as_array()
        .expect("hubs")
        .iter()
        .find(|hub| hub["hubIdentifier"] == identifier)
        .expect("every shelf is listed")
}

#[tokio::test]
async fn a_hub_search_answers_seventeen_shelves_even_when_empty() {
    let body = listing("/hubs/search?query=zzzz").await;
    assert_eq!(body["MediaContainer"]["size"], 17);
    // The order the reference lists them in when nothing ties them apart, as for `zzqxw`.
    let identifiers: Vec<String> = search_shelves(&body)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        identifiers,
        [
            "show",
            "artist",
            "album",
            "movie",
            "episode",
            "track",
            "photoalbum",
            "autotag",
            "photo",
            "tag",
            "actor",
            "director",
            "genre",
            "collection",
            "playlist",
            "shared",
            "place"
        ]
    );
    // An empty shelf as captured: no item list at all, `more` false, the title in the
    // client's language.
    let shelf = search_shelf(&body, "show");
    assert_eq!(
        shelf.to_string(),
        r#"{"title":"Shows","type":"show","hubIdentifier":"show","context":"","size":0,"more":false,"style":"shelf"}"#
    );
    let french = listing("/hubs/search?query=zzzz&X-Plex-Language=fr").await;
    let titles: Vec<&str> = french["MediaContainer"]["Hub"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hub| hub["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles[..4], ["Séries", "Artistes", "Albums", "Films"]);
    assert_eq!(titles[9..13], ["Tags", "Acteurs", "Réalisateurs", "Genres"]);
}

#[tokio::test]
async fn a_hub_search_puts_the_best_matching_shelf_first() {
    let body = listing("/hubs/search?query=avatar").await;
    assert_eq!(search_shelves(&body)[0], ("movie".to_owned(), 1));
    let film = &search_shelf(&body, "movie")["Metadata"][0];
    // An exact title, its audience rating of 7.9 breaking ties, written as the reference
    // writes a score.
    assert_eq!(film["score"], "0.93079");
    assert_eq!(film["ratingKey"], "100");
    // Unlike `/library/search`, a film says where its library is.
    assert_eq!(film["librarySectionID"], 4);
    assert_eq!(film["librarySectionKey"], "/library/sections/4");
    assert_eq!(film["librarySectionTitle"], "VF - Films");
    // Alone with its title on the shelf, so nothing tells it apart by library.
    assert!(film.get("reason").is_none(), "{film}");

    // A later word of an episode's title: the lowest rung, two hundredths under a film's.
    let body = listing("/hubs/search?query=hell").await;
    assert_eq!(search_shelves(&body)[0], ("episode".to_owned(), 1));
    let episode = &search_shelf(&body, "episode")["Metadata"][0];
    assert_eq!(episode["score"], "0.31000");
    assert_eq!(episode["key"], "/library/metadata/202");
    assert_eq!(episode["librarySectionID"], 5);
}

#[tokio::test]
async fn a_hub_search_offers_a_person_in_each_library_crediting_them() {
    let body = listing("/hubs/search?query=jason%20statham").await;
    assert_eq!(search_shelves(&body)[0], ("actor".to_owned(), 1));
    let actor = &search_shelf(&body, "actor")["Directory"][0];
    // The reference's attributes in its order, captured for Tom Hanks.
    assert_eq!(
        actor.to_string(),
        r#"{"key":"/library/sections/4/all?actor=472942","librarySectionID":4,"librarySectionKey":"/library/sections/4","librarySectionTitle":"VF - Films","librarySectionType":1,"reason":"section","reasonID":4,"reasonTitle":"VF - Films","score":"0.92000","type":"tag","id":472942,"filter":"actor=472942","tag":"Jason Statham","tagType":6,"tagKey":"5d776826e6d55c002040af58","thumb":"https://metadata-static.plex.tv/c/people/c540bdd39873ee8e8612b0af06fc0d4c.jpg","count":1}"#
    );

    // A name starting with the term is a rung below the name itself.
    let body = listing("/hubs/search?query=corey").await;
    let director = &search_shelf(&body, "director")["Directory"][0];
    assert_eq!(director["score"], "0.52000");
    assert_eq!(director["filter"], "director=623027");
    assert_eq!(director["tagType"], 4);

    // A genre carries no person: no provider key, no portrait.
    let body = listing("/hubs/search?query=thriller").await;
    let genre = &search_shelf(&body, "genre")["Directory"][0];
    assert_eq!(genre["key"], "/library/sections/4/all?genre=39");
    assert_eq!(genre["tagType"], 1);
    assert!(genre.get("tagKey").is_none() && genre.get("thumb").is_none());
    // Writers and producers are not searched, as on the reference: Luc Besson is neither.
    let body = listing("/hubs/search?query=besson").await;
    assert!(search_shelves(&body).iter().all(|(_, size)| *size == 0));
}

#[tokio::test]
async fn a_hub_search_finds_films_through_the_person_a_term_names() {
    // As the reference lists Tom Hanks' films for `tom hanks`: no title answers the term, so
    // the film shelf holds the actor's films, naming the credit as the reason and scored as
    // a match anywhere, 0.33 and 7.9 for the audience rating.
    let body = listing("/hubs/search?query=jason%20statham").await;
    let shelf = search_shelf(&body, "movie");
    assert_eq!(shelf["size"], 1);
    let film = &shelf["Metadata"][0];
    assert_eq!(film["ratingKey"], "100");
    assert_eq!(film["reason"], "actor");
    assert_eq!(film["reasonID"], 472_942);
    assert_eq!(film["reasonTitle"], "Jason Statham");
    assert_eq!(film["score"], "0.33079");
    // The director the term names counts too, when the actor leaves room.
    let body = listing("/hubs/search?query=corey%20yuen").await;
    assert_eq!(
        search_shelf(&body, "movie")["Metadata"][0]["reason"],
        "director"
    );
    // A one-character term finds nobody, and so nothing through anybody.
    let body = listing("/hubs/search?query=j").await;
    assert_eq!(search_shelf(&body, "movie")["size"], 0);
}

/// The people of `/library/search` as the reference writes them, captured for Luc Besson.
const STATHAM_RESULT: &str = r#"{"score":0.92,"Directory":{"key":"/library/sections/4/all?actor=472942","librarySectionID":4,"librarySectionKey":"/library/sections/4","librarySectionTitle":"VF - Films","librarySectionType":1,"type":"tag","id":472942,"filter":"actor=472942","tag":"Jason Statham","tagType":6,"tagKey":"5d776826e6d55c002040af58","thumb":"https://metadata-static.plex.tv/c/people/c540bdd39873ee8e8612b0af06fc0d4c.jpg","count":1}}"#;

fn search_results(body: &Value) -> Vec<Value> {
    body["MediaContainer"]["SearchResult"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn the_search_field_finds_people_in_the_references_shape() {
    let body = listing("/library/search?query=jason%20statham&limit=10").await;
    let results = search_results(&body);
    assert_eq!(results.len(), 1, "{body}");
    assert_eq!(results[0].to_string(), STATHAM_RESULT);
}

#[tokio::test]
async fn people_come_from_the_libraries_the_other_search_types_name() {
    // Measured for `luc besson`, credited in film libraries alone.
    for (search_types, found) in [
        ("people", 1),
        ("movies,people", 1),
        ("movies,tv,people", 1),
        ("tv,people", 0),
        ("people,bogus", 0),
        ("music,people", 0),
        ("movies", 0),
    ] {
        let body = listing(&format!(
            "/library/search?query=jason%20statham&searchTypes={search_types}"
        ))
        .await;
        assert_eq!(search_results(&body).len(), found, "{search_types}");
    }
}

#[tokio::test]
async fn the_search_field_lists_items_then_people_then_genres() {
    let body = listing("/library/search?query=thriller&searchTypes=movies").await;
    let results = search_results(&body);
    let genre = &results.last().unwrap()["Directory"];
    assert_eq!(genre["tagType"], 1);
    assert_eq!(genre["filter"], "genre=39");
    assert!(genre.get("reason").is_none() && genre.get("score").is_none());
    // A genre comes with the items of its libraries, not with the people.
    let body = listing("/library/search?query=thriller&searchTypes=people").await;
    assert!(search_results(&body).is_empty());

    // An exact title before anything else, scored with its audience rating.
    let body = listing("/library/search?query=avatar").await;
    let film = &search_results(&body)[0];
    assert_eq!(film["score"], 0.93079);
    assert_eq!(film["Metadata"]["ratingKey"], "100");
}

#[tokio::test]
async fn a_single_character_finds_only_titles_equal_to_it() {
    // `s` begins Severance, Shu Qi and more, none of them named `s`.
    let body = listing("/hubs/search?query=s").await;
    assert!(search_shelves(&body).iter().all(|(_, size)| *size == 0));
    let body = listing("/hubs/search?query=se").await;
    assert_eq!(search_shelf(&body, "show")["size"], 1);
}

#[tokio::test]
async fn a_hub_search_refuses_what_the_reference_refuses() {
    for uri in [
        "/hubs/search",
        "/hubs/search?query=",
        "/hubs/search?query=%20",
        "/hubs/search?query=avatar&limit=abc",
        "/hubs/search?query=avatar&limit=0",
        // A library that does not exist; one that does narrows nothing.
        "/hubs/search?query=avatar&sectionId=999",
    ] {
        let (status, _) = get_from(items_app(), uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
    let everywhere = listing("/hubs/search?query=hell").await;
    for scoped in [
        "/hubs/search?query=hell&sectionId=4",
        "/hubs/search?query=hell&sectionId=abc",
    ] {
        assert_eq!(listing(scoped).await, everywhere, "{scoped}");
    }
}

#[tokio::test]
async fn a_hub_search_names_its_elements_by_kind_in_xml() {
    let (status, body) = get_from(items_app(), "/hubs/search?query=avatar", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"<Hub title="Movies" type="movie" hubIdentifier="movie" context="" size="1" more="0" style="shelf">"#),
        "{body}"
    );
    assert!(
        body.contains(r#"<Video score="0.93079" ratingKey="100""#),
        "{body}"
    );
    let (_, body) = get_from(items_app(), "/hubs/search?query=thriller", None).await;
    assert!(
        body.contains(r#"<Directory key="/library/sections/4/all?genre=39""#),
        "{body}"
    );
}
/// An app with one session playing under `key`, its sessions and its stub engine.
async fn playing(
    key: &str,
    playback_id: Option<&str>,
) -> (
    axum::Router,
    std::sync::Arc<open_plex_core::testing::InMemoryPlayback>,
    std::sync::Arc<open_plex_core::testing::StubStreams>,
) {
    use open_plex_core::now_epoch;
    use open_plex_core::playback::{
        NewPlaybackSession, PlaybackOutput, PlaybackRepository, StreamDecision,
    };

    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let (library, metadata) = catalog();
    let playback = std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default());
    let streams = std::sync::Arc::new(open_plex_core::testing::StubStreams::default());
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
        metadata: std::sync::Arc::new(metadata),
        playback: playback.clone(),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: streams.clone(),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    playback
        .start(
            NewPlaybackSession {
                key: open_plex_core::testing::owner().session_key(key),
                user: open_plex_core::testing::owner().user,
                playback_id: playback_id.map(|id| open_plex_core::testing::owner().session_key(id)),
                item: RatingKey::new(100),
                part: PartId::new(1),
                client: None,
                product: None,
                video: StreamDecision::Copy,
                audio: StreamDecision::Copy,
                position_ms: 0,
                output: PlaybackOutput::default(),
            },
            now_epoch(),
        )
        .await
        .expect("start");
    let app = authenticated(router(PlexState::new(identity, schema)));
    (app, playback, streams)
}

async fn send(app: axum::Router, method: &str, uri: &str) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Plex-Token", TOKEN)
        .body(Body::empty())
        .expect("request builds");
    app.oneshot(request)
        .await
        .expect("router responds")
        .status()
}

#[tokio::test]
async fn an_owner_terminates_a_session_and_its_player_hears_why() {
    use open_plex_core::playback::PlaybackRepository;

    let (app, playback, streams) = playing("transcode-session", Some("tab")).await;

    // As on the reference: the session's id is the player's, and its key a number.
    let (_, body) = get_from(app.clone(), "/status/sessions", Some("application/json")).await;
    let listed: Value = serde_json::from_str(&body).expect("json body");
    let listed = &listed["MediaContainer"]["Metadata"][0];
    assert_eq!(listed["Session"]["id"], "tab");
    assert_eq!(listed["sessionKey"], "1");
    assert_eq!(
        listed["TranscodeSession"]["key"],
        "/transcode/sessions/transcode-session"
    );

    let terminate = "/status/sessions/terminate";
    assert_eq!(
        send(app.clone(), "POST", terminate).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(app.clone(), "POST", &format!("{terminate}?sessionId=")).await,
        StatusCode::FORBIDDEN
    );
    // The conversion's key is not the session's id.
    assert_eq!(
        send(
            app.clone(),
            "POST",
            &format!("{terminate}?sessionId=transcode-session")
        )
        .await,
        StatusCode::NOT_FOUND
    );
    // Plex Web sends it as `GET`.
    assert_eq!(
        send(
            app.clone(),
            "GET",
            &format!("{terminate}?sessionId=tab&reason=Bedtime%20now")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        *streams.stopped.lock().expect("lock"),
        ["u1000~transcode-session"]
    );
    assert_eq!(
        get_from(app.clone(), "/status/sessions", Some("application/json"))
            .await
            .1,
        r#"{"MediaContainer":{"size":0}}"#
    );
    assert_eq!(
        send(app.clone(), "GET", &format!("{terminate}?sessionId=tab")).await,
        StatusCode::NOT_FOUND
    );
    // Its conversion is gone: a ping says so.
    assert_eq!(
        send(
            app.clone(),
            "GET",
            "/video/:/transcode/universal/ping?session=transcode-session"
        )
        .await,
        StatusCode::NOT_FOUND
    );

    let (_, answer) = timeline(
        app.clone(),
        "GET",
        "ratingKey=100&state=playing&time=2000&X-Plex-Session-Identifier=tab",
        "application/json",
    )
    .await;
    let answer: Value = serde_json::from_str(&answer).expect("json body");
    assert_eq!(answer["MediaContainer"]["terminationCode"], 2006);
    assert_eq!(answer["MediaContainer"]["terminationText"], "Bedtime now");
    assert!(answer["MediaContainer"].get("TranscodeSession").is_none());

    // The player stops, having shown the reason: the session is gone for good.
    let (_, answer) = timeline(
        app.clone(),
        "GET",
        "ratingKey=100&state=stopped&time=2000&X-Plex-Session-Identifier=tab",
        "application/json",
    )
    .await;
    assert!(!answer.contains("termination"));
    assert!(playback
        .keys_of_playback(&open_plex_core::testing::owner().session_key("tab"))
        .await
        .expect("keys")
        .is_empty());
}

#[tokio::test]
async fn ping_and_stop_answer_from_the_official_api() {
    let (app, _, streams) = playing("conversion", None).await;

    let ping = "/video/:/transcode/universal/ping?session=conversion";
    let stop = "/video/:/transcode/universal/stop?session=conversion";
    assert_eq!(send(app.clone(), "GET", ping).await, StatusCode::OK);
    assert_eq!(send(app.clone(), "GET", stop).await, StatusCode::OK);
    // The engine knows the session by the name it is stored under, the caller's own.
    assert_eq!(*streams.stopped.lock().expect("lock"), ["u1000~conversion"]);
    // As on the reference once it is stopped.
    assert_eq!(send(app.clone(), "GET", stop).await, StatusCode::NOT_FOUND);
    assert_eq!(send(app.clone(), "GET", ping).await, StatusCode::NOT_FOUND);
    assert_eq!(
        send(app, "GET", "/video/:/transcode/universal/ping").await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_fallback_is_refused_as_the_reference_refuses_a_software_conversion() {
    let (app, _, _) = playing("conversion", None).await;
    let fallback =
        |kind: &str, query: &str| format!("/{kind}/:/transcode/universal/fallback{query}");

    // Measured on the reference, which reads `session` and not the spec's `transcodeSessionId`.
    for (method, uri, expected) in [
        ("POST", fallback("video", ""), StatusCode::BAD_REQUEST),
        ("POST", fallback("music", ""), StatusCode::BAD_REQUEST),
        (
            "POST",
            fallback("video", "?transcodeSessionId=conversion"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            fallback("video", "?session="),
            StatusCode::NOT_FOUND,
        ),
        (
            "POST",
            fallback("video", "?session=nope"),
            StatusCode::NOT_FOUND,
        ),
        (
            "POST",
            fallback("photo", "?session=conversion"),
            StatusCode::NOT_FOUND,
        ),
        (
            "GET",
            fallback("video", "?session=conversion"),
            StatusCode::NOT_FOUND,
        ),
        ("GET", fallback("video", ""), StatusCode::NOT_FOUND),
        (
            "PUT",
            fallback("video", "?session=conversion"),
            StatusCode::NOT_FOUND,
        ),
        // Whatever kind the path names.
        (
            "POST",
            fallback("subtitles", "?session=conversion"),
            StatusCode::PRECONDITION_FAILED,
        ),
    ] {
        assert_eq!(
            send(app.clone(), method, &uri).await,
            expected,
            "{method} {uri}"
        );
    }

    let request = Request::builder()
        .method("POST")
        .uri(fallback("video", "?session=conversion"))
        .header("X-Plex-Token", TOKEN)
        .header("Accept", "application/json")
        .body(Body::empty())
        .expect("request builds");
    let response = app.clone().oneshot(request).await.expect("router responds");
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(response.headers()["content-type"], "text/html");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    // The reference's `500` page, 109 bytes, under its `412`.
    assert_eq!(
        body,
        "<html><head><title>Internal Server Error</title></head><body><h1>500 Internal Server Error</h1></body></html>"
    );

    // The session plays on.
    let ping = "/video/:/transcode/universal/ping?session=conversion";
    assert_eq!(send(app, "GET", ping).await, StatusCode::OK);
}

#[tokio::test]
async fn background_tasks_answer_the_empty_container_the_reference_sends_when_idle() {
    // Captured on the reference with no optimizer or download running, in both formats.
    let (status, _, body) = get("/status/sessions/background", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"MediaContainer":{"size":0}}"#);
    let (_, _, xml) = get("/status/sessions/background", None).await;
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MediaContainer size=\"0\">\n</MediaContainer>\n"
    );
}

/// The reference's test libraries for markers: Soul in 27 with its summary locked, as it was
/// when its bookmarks were measured, and Bull's first episode in 28 carrying the credits and
/// intro the reference found on an episode of *The Farewell*.
fn markers_store() -> std::sync::Arc<InMemoryMetadata> {
    use open_plex_core::edit::ItemField;
    use open_plex_core::marker::{Marker, MarkerKind};
    let marker = |id: i64, kind, start_ms, end_ms, is_final, version| Marker {
        id: open_plex_core::MarkerId::new(id),
        item: RatingKey::new(578_173),
        kind,
        owner: None,
        start_ms,
        end_ms: Some(end_ms),
        is_final,
        version: Some(version),
        title: None,
        color: None,
    };
    let metadata = InMemoryMetadata::new(vec![
        item(577_986, 27, ItemKind::Movie, None, "Soul"),
        item(577_987, 28, ItemKind::Show, None, "Bull"),
        item(577_988, 28, ItemKind::Season, Some(577_987), "Season 1"),
        item(578_173, 28, ItemKind::Episode, Some(577_988), "Episode 1"),
    ])
    .with_locked_fields(vec![(RatingKey::new(577_986), ItemField::Summary)])
    .with_markers(vec![
        marker(1, MarkerKind::Credits, 2_233_609, 2_397_344, true, 4),
        marker(2, MarkerKind::Intro, 136_180, 176_381, false, 5),
    ]);
    std::sync::Arc::new(metadata)
}

fn markers_app(principal: Principal) -> axum::Router {
    markers_app_over(markers_store(), principal)
}

/// The test libraries over a given store, for several callers to share one.
fn markers_app_over(store: std::sync::Arc<InMemoryMetadata>, principal: Principal) -> axum::Router {
    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let library = InMemoryLibrary::new(vec![
        section(27, SectionKind::Movie, "open-plex tests", false),
        section(28, SectionKind::Show, "open-plex tests series", false),
    ]);
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
        metadata: store,
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    router(PlexState::new(identity, schema)).layer(axum::Extension(principal))
}

/// A user given the test libraries as a viewer.
fn test_libraries_viewer(user: i64) -> Principal {
    Principal {
        name: format!("user{user}"),
        role: open_plex_core::ServerRole::User,
        libraries: LibraryAccess::Only(
            [27, 28]
                .into_iter()
                .map(|section| (SectionId::new(section), LibraryRole::Viewer))
                .collect(),
        ),
        user: UserId::new(user),
        server_identifier: None,
    }
}

async fn marker_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    accept: Option<&str>,
) -> (StatusCode, String, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Plex-Token", TOKEN);
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

/// The `Marker` elements of a body, each with its children, one line per element.
fn marker_lines(xml: &str) -> Vec<&str> {
    let mut inside = false;
    xml.lines()
        .filter(|line| {
            if line.starts_with("<Marker") {
                inside = true;
            }
            let keep = inside;
            if line.starts_with("</Marker>") {
                inside = false;
            }
            keep
        })
        .collect()
}

#[tokio::test]
async fn markers_are_published_on_request_only() {
    let app = markers_app(open_plex_core::testing::owner());
    let (_, _, plain) = marker_request(&app, "GET", "/library/metadata/578173", None).await;
    assert!(!plain.contains("<Marker"), "{plain}");
    let (_, _, zero) = marker_request(
        &app,
        "GET",
        "/library/metadata/578173?includeMarkers=0",
        None,
    )
    .await;
    assert!(!zero.contains("<Marker"), "{zero}");

    let (status, _, xml) = marker_request(
        &app,
        "GET",
        "/library/metadata/578173?includeMarkers=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        marker_lines(&xml),
        [
            "<Marker id=\"1\" type=\"credits\" startTimeOffset=\"2233609\" endTimeOffset=\"2397344\" final=\"1\">",
            "<Attributes id=\"1\" version=\"4\" />",
            "</Marker>",
            "<Marker id=\"2\" type=\"intro\" startTimeOffset=\"136180\" endTimeOffset=\"176381\">",
            "<Attributes id=\"2\" version=\"5\" />",
            "</Marker>",
        ]
    );

    let (_, _, json) = marker_request(
        &app,
        "GET",
        "/library/metadata/578173?includeMarkers=1",
        Some("application/json"),
    )
    .await;
    let json: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        json["MediaContainer"]["Metadata"][0]["Marker"],
        serde_json::json!([
            { "id": 1, "type": "credits", "startTimeOffset": 2_233_609,
              "endTimeOffset": 2_397_344, "final": true,
              "Attributes": { "id": 1, "version": 4 } },
            { "id": 2, "type": "intro", "startTimeOffset": 136_180,
              "endTimeOffset": 176_381, "Attributes": { "id": 2, "version": 5 } },
        ])
    );
}

#[tokio::test]
async fn an_episodes_markers_come_with_it_where_it_is_listed() {
    let app = markers_app(open_plex_core::testing::owner());
    for uri in [
        "/library/metadata/577988/children?includeMarkers=1",
        "/library/metadata/577987/grandchildren?includeMarkers=1",
        "/library/metadata/577987/allLeaves?includeMarkers=1",
        "/library/sections/28/all?type=4&includeMarkers=1",
    ] {
        let (status, _, xml) = marker_request(&app, "GET", uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(marker_lines(&xml).len(), 6, "{uri}: {xml}");
        let without = uri.replace("includeMarkers=1", "includeMarkers=0");
        let (_, _, xml) = marker_request(&app, "GET", &without, None).await;
        assert!(!xml.contains("<Marker"), "{without}");
    }
}

#[tokio::test]
async fn a_bookmark_is_added_read_moved_and_removed_as_the_reference_answers() {
    let app = markers_app(open_plex_core::testing::owner());
    // Captured on Soul: a new bookmark comes back before it has an id.
    let (status, content_type, xml) = marker_request(
        &app,
        "POST",
        "/library/metadata/577986/marker?type=bookmark&startTimeOffset=10000&endTimeOffset=20000",
        None,
    )
    .await;
    assert_eq!(
        (status, content_type.as_str()),
        (StatusCode::OK, "text/xml;charset=utf-8")
    );
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<MediaContainer size=\"1\" identifier=\"com.plexapp.plugins.library\" mediaTagPrefix=\"/system/bundle/media/flags/\" mediaTagVersion=\"0\">\n\
<Marker type=\"bookmark\" startTimeOffset=\"10000\" endTimeOffset=\"20000\">\n\
<Attributes />\n\
</Marker>\n\
</MediaContainer>\n"
    );

    // Read back after the tags and before the locked fields, with its id.
    let (_, _, lookup) = marker_request(
        &app,
        "GET",
        "/library/metadata/577986?includeMarkers=1",
        None,
    )
    .await;
    assert!(
        lookup.contains(
            "<Marker type=\"bookmark\" startTimeOffset=\"10000\" endTimeOffset=\"20000\">\n\
<Attributes id=\"3\" />\n\
</Marker>\n\
<Field locked=\"1\" name=\"summary\" />\n\
</Video>"
        ),
        "{lookup}"
    );

    // The same span again is that bookmark, with its id.
    let (_, _, again) = marker_request(
        &app,
        "POST",
        "/library/metadata/577986/marker?type=bookmark&startTimeOffset=10000&endTimeOffset=20000&attributes%5Btitle%5D=Y",
        None,
    )
    .await;
    assert!(again.contains("<Attributes id=\"3\" />"), "{again}");

    // Captured: `PUT` ignores `type`, keeps the end, and answers every attribute.
    let (status, _, json) = marker_request(
        &app,
        "PUT",
        "/library/metadata/577986/marker/3?type=intro&startTimeOffset=12000&attributes%5Btitle%5D=Edited",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let json: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "MediaContainer": {
            "size": 1,
            "identifier": "com.plexapp.plugins.library",
            "mediaTagPrefix": "/system/bundle/media/flags/",
            "mediaTagVersion": 0,
            "Marker": [{ "type": "bookmark", "startTimeOffset": 12000, "endTimeOffset": 20000,
                         "Attributes": { "id": 3, "title": "Edited" } }],
        } })
    );

    let (status, content_type, body) =
        marker_request(&app, "DELETE", "/library/metadata/577986/marker/3", None).await;
    assert_eq!(
        (status, content_type.as_str(), body.as_str()),
        (StatusCode::OK, "text/html", "")
    );
    let (status, _, _) =
        marker_request(&app, "DELETE", "/library/metadata/577986/marker/3", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_bookmarks_attributes_come_in_alphabetical_order() {
    let app = markers_app(open_plex_core::testing::owner());
    // As captured on Bull's first episode: a point, named and coloured.
    let (status, _, _) = marker_request(
        &app,
        "POST",
        "/library/metadata/578173/marker?type=bookmark&startTimeOffset=5000&attributes%5Btitle%5D=Spot&attributes%5Bcolor%5D=ff0000&attributes%5Bfoo%5D=bar",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, xml) = marker_request(
        &app,
        "GET",
        "/library/metadata/578173?includeMarkers=1",
        None,
    )
    .await;
    assert_eq!(
        marker_lines(&xml)[6..],
        [
            "<Marker type=\"bookmark\" startTimeOffset=\"5000\">",
            "<Attributes color=\"ff0000\" id=\"3\" title=\"Spot\" />",
            "</Marker>",
        ]
    );
}

#[tokio::test]
async fn a_marker_route_refuses_what_the_reference_refuses() {
    let app = markers_app(open_plex_core::testing::owner());
    for (method, uri, expected) in [
        // A type by number, an unknown name, no start, a negative one, an empty span.
        (
            "POST",
            "/library/metadata/577986/marker?type=3&startTimeOffset=1",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/library/metadata/577986/marker?type=foo&startTimeOffset=1",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/library/metadata/577986/marker?type=bookmark",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/library/metadata/577986/marker?type=bookmark&startTimeOffset=x",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/library/metadata/577986/marker?type=bookmark&startTimeOffset=-5",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/library/metadata/577986/marker?type=bookmark&startTimeOffset=50&endTimeOffset=50",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/library/metadata/99999999/marker?type=bookmark&startTimeOffset=1000",
            StatusCode::NOT_FOUND,
        ),
        // The work's markers are not written here.
        (
            "POST",
            "/library/metadata/578173/marker?type=intro&startTimeOffset=0&endTimeOffset=10",
            StatusCode::BAD_REQUEST,
        ),
        (
            "PUT",
            "/library/metadata/578173/marker/1?startTimeOffset=0",
            StatusCode::BAD_REQUEST,
        ),
        (
            "DELETE",
            "/library/metadata/578173/marker/2",
            StatusCode::BAD_REQUEST,
        ),
        // A marker named under another item, and one that does not exist.
        (
            "PUT",
            "/library/metadata/577986/marker/1?startTimeOffset=0",
            StatusCode::BAD_REQUEST,
        ),
        (
            "DELETE",
            "/library/metadata/577986/marker/2",
            StatusCode::BAD_REQUEST,
        ),
        (
            "PUT",
            "/library/metadata/577986/marker/999?type=bookmark&startTimeOffset=1",
            StatusCode::NOT_FOUND,
        ),
        (
            "DELETE",
            "/library/metadata/577986/marker/999",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, content_type, _) = marker_request(&app, method, uri, None).await;
        assert_eq!(
            (status, content_type.as_str()),
            (expected, "text/html"),
            "{method} {uri}"
        );
    }
    // Nothing above was written.
    let (_, _, xml) = marker_request(
        &app,
        "GET",
        "/library/metadata/578173?includeMarkers=1",
        None,
    )
    .await;
    assert_eq!(marker_lines(&xml).len(), 6);
}

#[tokio::test]
async fn a_bookmark_is_its_keepers_and_nobody_elses() {
    let store = markers_store();
    let first = markers_app_over(std::sync::Arc::clone(&store), test_libraries_viewer(3));
    let (status, _, _) = marker_request(
        &first,
        "POST",
        "/library/metadata/577986/marker?type=bookmark&startTimeOffset=10000",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, mine) = marker_request(
        &first,
        "GET",
        "/library/metadata/577986?includeMarkers=1",
        None,
    )
    .await;
    assert!(mine.contains("<Attributes id=\"3\" />"), "{mine}");

    for other in [test_libraries_viewer(4), open_plex_core::testing::owner()] {
        let app = markers_app_over(std::sync::Arc::clone(&store), other);
        let (_, _, theirs) = marker_request(
            &app,
            "GET",
            "/library/metadata/577986?includeMarkers=1",
            None,
        )
        .await;
        assert!(!theirs.contains("<Marker"), "{theirs}");
        let (status, _, _) =
            marker_request(&app, "DELETE", "/library/metadata/577986/marker/3", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn a_play_queue_carries_the_markers_a_player_skips_by() {
    let app = markers_app(open_plex_core::testing::owner());
    let uri = "server%3A%2F%2Fdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef%2Fcom.plexapp.plugins.library%2Flibrary%2Fmetadata%2F578173";
    // What Plex Web sends to start playing an episode, and to read the queue back.
    let (status, _, created) = marker_request(
        &app,
        "POST",
        &format!("/playQueues?type=video&uri={uri}&includeChapters=1&includeMarkers=1"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(marker_lines(&created).len(), 6, "{created}");
    let queue = created
        .split("playQueueID=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    let (_, _, read) = marker_request(
        &app,
        "GET",
        &format!("/playQueues/{queue}?own=1&includeChapters=1&includeMarkers=1"),
        None,
    )
    .await;
    assert_eq!(marker_lines(&read).len(), 6, "{read}");
    let (_, _, plain) = marker_request(&app, "GET", &format!("/playQueues/{queue}"), None).await;
    assert!(!plain.contains("<Marker"), "{plain}");
}

const RATE: &str = "/:/rate?identifier=com.plexapp.plugins.library";

/// The scalar attributes of an item, in order.
fn attribute_names(item: &Value) -> Vec<String> {
    item.as_object()
        .unwrap()
        .iter()
        .filter(|(_, value)| !value.is_array() && !value.is_object())
        .map(|(name, _)| name.clone())
        .collect()
}

#[tokio::test]
async fn a_rating_is_published_on_lookups_and_listings_as_the_reference_does() {
    let app = editing_app(open_plex_core::testing::owner());
    assert_eq!(
        put_status(&app, &format!("{RATE}&key=577986&rating=9.5")).await,
        (StatusCode::OK, Some("text/html".to_owned()))
    );
    let film = film_after(&app).await;
    assert_eq!(film["userRating"], 9.5);
    assert!(film["lastRatedAt"].as_i64().unwrap() > 1_794_000_000);
    // Captured on the reference after rating it 9.5: `librarySectionKey, subtype, summary,
    // userRating, lastRatedAt, thumb`. This copy has no subtype, summary or poster.
    let names = attribute_names(&film);
    let rated = names.iter().position(|name| name == "userRating").unwrap();
    assert_eq!(names[rated + 1], "lastRatedAt", "{names:?}");
    assert_eq!(names[rated - 1], "librarySectionKey", "{names:?}");

    // A series is rated on its own, and its listing says so; one decimal, as on the reference.
    put_status(&app, &format!("{RATE}&key=577987&rating=7.26")).await;
    let listing = json_from(&app, "/library/sections/28/all").await;
    assert_eq!(listing["MediaContainer"]["Metadata"][0]["userRating"], 7.3);
    let season = json_from(&app, "/library/metadata/577987/children").await;
    let season = &season["MediaContainer"]["Metadata"][0];
    assert!(season.get("userRating").is_none(), "{season}");

    // `8.0` in XML, as the reference writes it.
    put_status(&app, &format!("{RATE}&key=577986&rating=8")).await;
    let (_, xml) = get_from(app.clone(), "/library/metadata/577986", None).await;
    assert!(xml.contains(" userRating=\"8.0\" lastRatedAt=\""), "{xml}");

    // An older change is ignored; `-1` clears, and both attributes go.
    put_status(
        &app,
        &format!("{RATE}&key=577986&rating=2&ratedAt=1600000000"),
    )
    .await;
    assert_eq!(film_after(&app).await["userRating"], 8.0);
    put_status(&app, &format!("{RATE}&key=577986&rating=-1")).await;
    let film = film_after(&app).await;
    assert!(film.get("userRating").is_none(), "{film}");
    assert!(film.get("lastRatedAt").is_none(), "{film}");
}

#[tokio::test]
async fn a_rating_is_refused_as_the_reference_refuses_it() {
    let app = editing_app(open_plex_core::testing::owner());
    for (uri, status) in [
        (format!("{RATE}&key=577986"), StatusCode::BAD_REQUEST),
        (
            format!("{RATE}&key=577986&rating=abc"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{RATE}&key=577986&rating=4&ratedAt=1900000000"),
            StatusCode::BAD_REQUEST,
        ),
        // Before the item, as on the reference.
        (
            format!("{RATE}&key=999&rating=4&ratedAt=1900000000"),
            StatusCode::BAD_REQUEST,
        ),
        // Kept by the reference, off the scale here.
        (
            format!("{RATE}&key=577986&rating=11"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{RATE}&key=577986&rating=-5"),
            StatusCode::BAD_REQUEST,
        ),
        (format!("{RATE}&key=999&rating=4"), StatusCode::NOT_FOUND),
        (
            "/:/rate?identifier=tv.plex.provider.discover&key=577986&rating=4".to_owned(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(put_status(&app, &uri).await.0, status, "{uri}");
    }
    // `GET`, as the reference answers it too.
    let (status, _) = get_from(app.clone(), &format!("{RATE}&key=577986&rating=6"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(film_after(&app).await["userRating"], 6.0);
}

#[tokio::test]
async fn a_listing_sorts_and_filters_by_the_callers_rating() {
    let app = editing_app(open_plex_core::testing::owner());
    for uri in [
        "/library/sections/27/all?sort=userRating:desc",
        "/library/sections/27/all?userRating=-1",
        "/library/sections/27/all?userRating%3E=5&userRating%3C%3C=9",
    ] {
        let (status, body) = get_from(app.clone(), uri, Some("application/json")).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    }
    let (status, _) = get_from(app, "/library/sections/27/all?userRating=abc", None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn library_tags_lists_a_kind_across_libraries_in_id_order() {
    let body = listing("/library/tags?type=4").await;
    let container = &body["MediaContainer"];
    // Complete, so unpaged: no `totalSize`, as the reference answers a list it holds whole.
    assert_eq!(
        keys_of(container),
        [
            "size",
            "identifier",
            "mediaTagPrefix",
            "mediaTagVersion",
            "Directory"
        ]
    );
    assert_eq!(container["size"], 2);
    let directors = container["Directory"].as_array().unwrap();
    // The film's director, then an episode's: every depth counts.
    assert_eq!(directors[0]["id"], 623_027);
    assert_eq!(directors[1]["id"], 705_517);
    // The captured order of a director's attributes.
    assert_eq!(
        keys_of(&directors[0]),
        ["id", "filter", "tag", "tagType", "tagKey", "thumb"]
    );
    assert_eq!(directors[0]["filter"], "director=623027");
    assert_eq!(directors[0]["tag"], "Corey Yuen");
    assert_eq!(directors[0]["tagType"], 4);
    assert_eq!(directors[0]["tagKey"], "5d77682d103a2d001f565d0f");

    // A country, captured: no person, so no `tagKey` or `thumb`.
    let (status, xml) = get_from(items_app(), "/library/tags?type=8", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        xml.contains(r#"<Directory id="10097" filter="country=10097" tag="France" tagType="8" />"#),
        "{xml}"
    );
}

#[tokio::test]
async fn library_tags_reads_its_type_as_the_reference_does() {
    let status = |uri: &'static str| async move { get_from(items_app(), uri, None).await.0 };
    for uri in [
        "/library/tags",
        "/library/tags?type=",
        "/library/tags?type=abc",
        "/library/tags?type=8.5",
        "/library/tags?type=%208",
        "/library/tags?type=99999999999",
        "/library/tags?TYPE=8",
    ] {
        assert_eq!(status(uri).await, StatusCode::BAD_REQUEST, "{uri}");
    }
    // Collections are items here, with no tag to list.
    assert_eq!(
        status("/library/tags?type=2").await,
        StatusCode::NOT_IMPLEMENTED
    );

    // The last `type` wins, and leading zeros read.
    let countries = listing("/library/tags?type=1&type=08").await;
    assert_eq!(countries["MediaContainer"]["Directory"][0]["tagType"], 8);

    // A type with no tag, captured with `type=3`, `999` and `-1` alike.
    let none = r#"{"MediaContainer":{"size":0,"identifier":"com.plexapp.plugins.library","mediaTagPrefix":"/system/bundle/media/flags/","mediaTagVersion":1791210760}}"#;
    for kind in [0, 3, 11, 999, -1] {
        assert_eq!(
            listing(&format!("/library/tags?type={kind}")).await,
            with_our_flag_bundle(none),
            "{kind}"
        );
    }
}

#[tokio::test]
async fn library_tags_pages_with_both_container_parameters() {
    let (status, _, paging, body) = call(
        &items_app(),
        "GET",
        "/library/tags?type=6&X-Plex-Container-Start=1&X-Plex-Container-Size=2",
        Some("application/json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_str(&body).unwrap();
    let container = &body["MediaContainer"];
    assert_eq!(container["size"], 2);
    assert_eq!(container["totalSize"], 4);
    assert_eq!(container["offset"], 1);
    let ids: Vec<&Value> = container["Directory"]
        .as_array()
        .unwrap()
        .iter()
        .map(|actor| &actor["id"])
        .collect();
    assert_eq!(ids, [480_672, 603_622]);
    assert!(paging.contains(&("x-plex-container-start".to_owned(), "1".to_owned())));
    assert!(paging.contains(&("x-plex-container-total-size".to_owned(), "4".to_owned())));

    // One parameter alone pages nothing, as on the reference.
    let whole = listing("/library/tags?type=6&X-Plex-Container-Size=2").await;
    assert_eq!(whole["MediaContainer"]["size"], 4);
    assert!(whole["MediaContainer"].get("totalSize").is_none());
}

#[tokio::test]
async fn library_tags_leaves_out_what_only_another_users_libraries_carry() {
    // The user reads the films alone: the episode's director is nobody it can see.
    let app = app_as_user(9, LibraryRole::Viewer);
    let (status, body) = get_from(app, "/library/tags?type=4", Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["MediaContainer"]["size"], 1);
    assert_eq!(body["MediaContainer"]["Directory"][0]["id"], 623_027);
}

#[tokio::test]
async fn top_users_names_the_caller_as_its_servers_account() {
    let app = watched_app().await;
    let json = |uri: &'static str| {
        let app = app.clone();
        async move {
            let (status, _, _, body) = call(&app, "GET", uri, Some("application/json")).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
            serde_json::from_str::<Value>(&body).unwrap()
        }
    };
    assert_eq!(
        json("/library/metadata/100/users/top").await,
        serde_json::json!({"MediaContainer": {"size": 1, "Account": [{"id": 1, "globalViewCount": 1}]}})
    );
    // A series and its season count their episodes' viewings.
    for uri in [
        "/library/metadata/200/users/top",
        "/library/metadata/201/users/top",
    ] {
        assert_eq!(
            json(uri).await["MediaContainer"]["Account"][0]["globalViewCount"],
            1,
            "{uri}"
        );
    }
    // Nothing watched: an empty container.
    assert_eq!(
        json("/library/metadata/205/users/top").await,
        serde_json::json!({"MediaContainer": {"size": 0}})
    );

    let (_, _, _, xml) = call(&app, "GET", "/library/metadata/100/users/top", None).await;
    assert!(
        xml.contains(r#"<Account id="1" globalViewCount="1" />"#),
        "{xml}"
    );
    for uri in [
        "/library/metadata/999/users/top",
        "/library/metadata/100,202/users/top",
        "/library/metadata/abc/users/top",
    ] {
        assert_eq!(
            call(&app, "GET", uri, None).await.0,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

/// A film whose chapters came from the reference's agent, with images, and an episode whose
/// file named none of its chapters, as captured on the reference.
fn chapters_app() -> axum::Router {
    use open_plex_core::chapter::{Chapter, ChapterSource};
    let chapter = |id: i64, item: i64, index: u32, start_ms, end_ms, title: Option<&str>| Chapter {
        id: open_plex_core::ChapterId::new(id),
        item: RatingKey::new(item),
        index,
        start_ms,
        end_ms,
        title: title.map(str::to_owned),
        image_version: (item == 51_797).then(|| MediaId::new(109_578)),
    };
    let mut film = item(51_797, 27, ItemKind::Movie, None, "Groundhog Day");
    film.chapter_source = Some(ChapterSource::Agent);
    let mut episode = item(578_173, 28, ItemKind::Episode, Some(577_988), "Episode 1");
    episode.chapter_source = Some(ChapterSource::Media);
    let version = MediaVersion {
        id: MediaId::new(109_578),
        item_id: RatingKey::new(51_797),
        width: Some(1280),
        height: Some(720),
        aspect_ratio: None,
        duration_ms: Some(6_060_000),
        bitrate: None,
        container: Some("mkv".to_owned()),
        video_codec: Some("h264".to_owned()),
        audio_codec: Some("aac".to_owned()),
        audio_channels: Some(2),
        frame_rate: None,
        interlaced: false,
        optimized_for_streaming: false,
    };
    let metadata = InMemoryMetadata::new(vec![
        film,
        item(577_987, 28, ItemKind::Show, None, "Bull"),
        item(577_988, 28, ItemKind::Season, Some(577_987), "Season 1"),
        episode,
    ])
    .with_media(vec![version], Vec::new(), Vec::new())
    .with_chapters(vec![
        chapter(1, 51_797, 1, 0, 162_000, Some("A Day in a Life")),
        chapter(2, 51_797, 2, 162_000, 409_000, Some("Day 10,909")),
        chapter(3, 578_173, 1, 80, 21_200, None),
    ]);
    markers_app_over(
        std::sync::Arc::new(metadata),
        open_plex_core::testing::owner(),
    )
}

fn chapter_lines(xml: &str) -> Vec<&str> {
    xml.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("<Chapter "))
        .collect()
}

#[tokio::test]
async fn chapters_are_published_on_a_lookup_that_asks_for_them() {
    let app = chapters_app();
    let (status, _, xml) = marker_request(
        &app,
        "GET",
        "/library/metadata/51797?includeChapters=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(xml.contains("chapterSource=\"agent\""), "{xml}");
    assert_eq!(
        chapter_lines(&xml),
        [
            "<Chapter id=\"1\" filter=\"thumb=1\" tag=\"A Day in a Life\" index=\"1\" startTimeOffset=\"0\" endTimeOffset=\"162000\" thumb=\"/library/media/109578/chapterImages/1\" />",
            "<Chapter id=\"2\" filter=\"thumb=2\" tag=\"Day 10,909\" index=\"2\" startTimeOffset=\"162000\" endTimeOffset=\"409000\" thumb=\"/library/media/109578/chapterImages/2\" />",
        ]
    );

    let (_, _, json) = marker_request(
        &app,
        "GET",
        "/library/metadata/578173?includeChapters=1",
        Some("application/json"),
    )
    .await;
    let json: Value = serde_json::from_str(&json).unwrap();
    let episode = &json["MediaContainer"]["Metadata"][0];
    assert_eq!(episode["chapterSource"], "media");
    assert_eq!(
        episode["Chapter"],
        serde_json::json!([
            { "id": 3, "filter": "thumb=3", "index": 1, "startTimeOffset": 80,
              "endTimeOffset": 21_200 },
        ])
    );

    // Only `1` asks, as on the reference.
    for uri in [
        "/library/metadata/51797",
        "/library/metadata/51797?includeChapters=0",
        "/library/metadata/51797?includeChapters=true",
    ] {
        let (_, _, xml) = marker_request(&app, "GET", uri, None).await;
        assert!(chapter_lines(&xml).is_empty(), "{uri}: {xml}");
        assert!(xml.contains("chapterSource=\"agent\""), "{uri}: {xml}");
    }
}

#[tokio::test]
async fn a_listing_says_where_chapters_came_from_and_lists_none() {
    let app = chapters_app();
    let (status, _, xml) = marker_request(
        &app,
        "GET",
        "/library/sections/27/all?includeChapters=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(xml.contains("chapterSource=\"agent\""), "{xml}");
    assert!(chapter_lines(&xml).is_empty(), "{xml}");
}
