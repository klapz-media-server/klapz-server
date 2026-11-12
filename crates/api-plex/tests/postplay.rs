//! The shelves at the end of a playback and of the home screen, against shapes captured on
//! the reference (1.43.5): the films of its "Terminator - Saga" and "The Terminator
//! Collection", read only, and its test series Bull, whose watch state was changed and put
//! back for the purpose.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use open_plex_api::{build_schema, ApiContext, ApiSchema};
use open_plex_api_plex::{router, PlexState};
use open_plex_core::edit::CalendarDate;
use open_plex_core::metadata::{ItemKind, MetadataItem};
use open_plex_core::testing::{InMemoryCollections, InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{
    LibrarySection, RatingKey, SectionId, SectionKind, ServerCapabilities, ServerIdentity,
};
use serde_json::Value;
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const SHOW: i64 = 577_987;
const SEASON: i64 = 577_988;
const EPISODE_1: i64 = 578_173;
const EPISODE_2: i64 = 577_990;

/// "Terminator - Saga" in the reference's film library, by release date.
const SAGA: [(i64, &str, &str); 6] = [
    (338_698, "Terminator", "1984-10-26"),
    (427_139, "Terminator 2 : Le Jugement dernier", "1991-07-03"),
    (
        445_826,
        "Terminator 3 : Le Soulèvement des machines",
        "2003-07-02",
    ),
    (444_367, "Terminator Renaissance", "2009-05-21"),
    (107_760, "Terminator Genisys", "2015-06-23"),
    (356_821, "Terminator: Dark Fate", "2019-10-23"),
];

fn section(id: i64, kind: SectionKind, title: &str, hidden: bool) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: title.to_owned(),
        title_sort: None,
        language: "fr-FR".to_owned(),
        uuid: format!("uuid-{id}"),
        created_at: 0,
        updated_at: 0,
        scanned_at: None,
        content_changed_at: 0,
        hidden,
    }
}

fn item(
    id: i64,
    section: i64,
    kind: ItemKind,
    parent: Option<i64>,
    title: &str,
    released: Option<&str>,
) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: parent.map(RatingKey::new),
        kind,
        guid: format!("plex://item/{id}"),
        title: Some(title.to_owned()),
        title_sort: None,
        original_title: None,
        summary: None,
        tagline: None,
        studio: None,
        content_rating: None,
        content_rating_age: None,
        position: None,
        year: released.map(|date| date[..4].parse().unwrap()),
        released_on: released.map(|date| CalendarDate::parse(date).unwrap().midnight()),
        duration_ms: Some(2_467_160),
        rating: None,
        audience_rating: None,
        added_at: 1_794_320_860,
        updated_at: 1_794_320_862,
    }
}

fn schema() -> ApiSchema {
    let mut items: Vec<MetadataItem> = SAGA
        .iter()
        .map(|(id, title, released)| item(*id, 4, ItemKind::Movie, None, title, Some(released)))
        .collect();
    let mut season = item(SEASON, 28, ItemKind::Season, Some(SHOW), "", None);
    season.title = None;
    season.position = Some(5);
    let mut first = item(
        EPISODE_1,
        28,
        ItemKind::Episode,
        Some(SEASON),
        "Episode 1",
        Some("2021-04-30"),
    );
    first.position = Some(1);
    let mut second = item(
        EPISODE_2,
        28,
        ItemKind::Episode,
        Some(SEASON),
        "Episode 2",
        None,
    );
    second.position = Some(2);
    items.extend([
        item(SHOW, 28, ItemKind::Show, None, "Bull", None),
        season,
        first,
        second,
    ]);
    let collections = InMemoryCollections::new(items.clone());
    let metadata = InMemoryMetadata::new(items.clone()).with_collections(&collections);
    build_schema(ApiContext {
        capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0-dev"), 0),
        library: std::sync::Arc::new(InMemoryLibrary::new(vec![
            section(4, SectionKind::Movie, "VF - Films", false),
            section(9, SectionKind::Movie, "VF - Films 3D", true),
            section(28, SectionKind::Show, "open-plex tests series", false),
        ])),
        metadata: std::sync::Arc::new(metadata),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(collections),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::over(items)),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
    })
}

fn app() -> axum::Router {
    router(PlexState::new(
        ServerIdentity::new(MACHINE, "0.1.0-dev"),
        schema(),
    ))
    .layer(axum::Extension(open_plex_core::testing::owner()))
}

async fn send(app: &axum::Router, method: Method, uri: &str, json: bool) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(uri);
    if json {
        request = request.header(header::ACCEPT, "application/json");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn json(app: &axum::Router, uri: &str) -> Value {
    let (status, body) = send(app, Method::GET, uri, true).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    serde_json::from_str(&body).unwrap()
}

/// A collection of these films, as Plex Web makes one.
async fn collect(app: &axum::Router, title: &str, films: &[i64]) -> String {
    let keys: Vec<String> = films.iter().map(ToString::to_string).collect();
    let target = format!(
        "server://{MACHINE}/com.plexapp.plugins.library/library/metadata/{}",
        keys.join(",")
    );
    let uri: String = form_urlencoded::byte_serialize(target.as_bytes()).collect();
    let title: String = form_urlencoded::byte_serialize(title.as_bytes()).collect();
    let (status, body) = send(
        app,
        Method::POST,
        &format!("/library/collections?type=1&title={title}&smart=0&sectionId=4&uri={uri}"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let created: Value = serde_json::from_str(&body).unwrap();
    created["MediaContainer"]["Metadata"][0]["index"]
        .as_i64()
        .unwrap()
        .to_string()
}

async fn terminator() -> (axum::Router, String, String) {
    let app = app();
    let films: Vec<i64> = SAGA.iter().map(|(id, _, _)| *id).collect();
    let saga = collect(&app, "Terminator - Saga", &films).await;
    let trilogy = collect(&app, "The Terminator Collection", &films[1..4]).await;
    (app, saga, trilogy)
}

fn hubs(body: &Value) -> &Vec<Value> {
    body["MediaContainer"]["Hub"].as_array().unwrap()
}

fn rating_keys(hub: &Value) -> Vec<&str> {
    hub["Metadata"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["ratingKey"].as_str().unwrap())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn after_a_film_come_the_next_of_its_saga_and_a_shelf_per_collection() {
    let (app, saga, trilogy) = terminator().await;
    let body = json(&app, "/hubs/metadata/427139/postplay").await;
    let container = &body["MediaContainer"];
    // Captured: bare, and `allowSync` off.
    assert_eq!(container["allowSync"], false);
    assert_eq!(container["identifier"], "com.plexapp.plugins.library");
    assert!(container.get("librarySectionID").is_none(), "{container}");

    let hubs = hubs(&body);
    let up_next = &hubs[0];
    let attributes: Vec<&str> = up_next
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    // In the reference's order, and with no `key`: the shelf leads nowhere.
    assert_eq!(
        attributes,
        [
            "hubKey",
            "upNext",
            "title",
            "type",
            "hubIdentifier",
            "context",
            "size",
            "more",
            "style",
            "Metadata"
        ]
    );
    assert_eq!(up_next["hubKey"], "/library/metadata/445826");
    assert_eq!(up_next["upNext"], true);
    assert_eq!(up_next["title"], "Up Next in Terminator - Saga");
    assert_eq!(up_next["hubIdentifier"], "movie.upnext");
    assert_eq!(up_next["context"], "hub.movie.upnext");
    assert_eq!(up_next["Metadata"][0]["librarySectionID"], 4);

    let shelf = &hubs[1];
    assert_eq!(
        shelf["key"],
        format!("/library/sections/4/all?collection={saga}&sort=originallyAvailableAt")
    );
    assert_eq!(shelf["title"], "Terminator - Saga Collection");
    assert_eq!(shelf["hubIdentifier"], format!("movie.collection.{saga}"));
    assert_eq!(shelf["context"], "hub.movie.collection");
    assert_eq!(shelf["size"], 6);
    assert_eq!(shelf["more"], false);
    assert_eq!(
        rating_keys(shelf),
        ["338698", "427139", "445826", "444367", "107760", "356821"]
    );
    assert_eq!(hubs[2]["title"], "The Terminator Collection Collection");
    assert_eq!(
        hubs[2]["hubIdentifier"],
        format!("movie.collection.{trilogy}")
    );
}

#[tokio::test]
async fn a_film_past_its_sagas_shelf_is_followed_in_a_smaller_collection() {
    // Measured: the seventh James Bond is offered the next of the smaller saga that shows
    // it, not the eighth of the one listing it seventh.
    let (app, _, _) = terminator().await;
    let body = json(&app, "/hubs/metadata/445826/postplay?count=2").await;
    let up_next = &hubs(&body)[0];
    assert_eq!(up_next["title"], "Up Next in The Terminator Collection");
    assert_eq!(up_next["hubKey"], "/library/metadata/444367");
    // The shelves are cut to the count, and say there is more.
    assert_eq!(hubs(&body)[1]["size"], 2);
    assert_eq!(hubs(&body)[1]["more"], true);

    // The last of every collection is followed by nothing.
    let last = json(&app, "/hubs/metadata/356821/postplay").await;
    assert!(hubs(&last)
        .iter()
        .all(|hub| hub["hubIdentifier"] != "movie.upnext"));
}

#[tokio::test]
async fn after_an_episode_comes_its_series_entry_and_nothing_once_it_is_that_entry() {
    let app = app();
    // The reference's test series as captured: its first episode at 78 seconds.
    send(
        &app,
        Method::GET,
        &format!(
            "/:/progress?identifier=com.plexapp.plugins.library&key={EPISODE_1}&time=78000&state=stopped"
        ),
        false,
    )
    .await;
    let second = json(&app, &format!("/hubs/metadata/{EPISODE_2}/postplay")).await;
    let hub = &hubs(&second)[0];
    assert_eq!(hub["hubIdentifier"], "tv.upnext");
    assert_eq!(hub["title"], "Up Next");
    assert_eq!(hub["type"], "episode");
    assert_eq!(hub["context"], "hub.tv.upnext");
    assert_eq!(hub["hubKey"], format!("/library/metadata/{EPISODE_1}"));
    assert!(hub.get("key").is_none(), "{hub}");
    assert_eq!(hub["Metadata"][0]["viewOffset"], 78_000);

    let first = json(&app, &format!("/hubs/metadata/{EPISODE_1}/postplay")).await;
    assert_eq!(first["MediaContainer"]["size"], 0);
    assert!(first["MediaContainer"].get("Hub").is_none());

    // Watched to the end, it offers the next.
    send(
        &app,
        Method::GET,
        &format!("/:/scrobble?identifier=com.plexapp.plugins.library&key={EPISODE_1}"),
        false,
    )
    .await;
    let after = json(&app, &format!("/hubs/metadata/{EPISODE_1}/postplay")).await;
    assert_eq!(
        hubs(&after)[0]["hubKey"],
        format!("/library/metadata/{EPISODE_2}")
    );
    // Not dated by itself, never played, but by its series.
    assert!(hubs(&after)[0]["Metadata"][0].get("lastViewedAt").is_some());

    // The series' last watched, nothing is left: no shelf at all.
    send(
        &app,
        Method::GET,
        &format!("/:/scrobble?identifier=com.plexapp.plugins.library&key={EPISODE_2}"),
        false,
    )
    .await;
    let done = json(&app, &format!("/hubs/metadata/{EPISODE_2}/postplay")).await;
    assert_eq!(done["MediaContainer"]["size"], 0);
}

#[tokio::test]
async fn a_series_or_a_season_has_no_postplay_and_an_unknown_item_is_a_404() {
    let app = app();
    for id in [SHOW, SEASON] {
        let body = json(&app, &format!("/hubs/metadata/{id}/postplay")).await;
        assert_eq!(body["MediaContainer"]["size"], 0);
    }
    for uri in [
        "/hubs/metadata/99999999/postplay",
        "/hubs/metadata/abc/postplay",
        "/hubs/metadata/99999999",
        "/hubs/metadata/99999999/related",
    ] {
        assert_eq!(
            send(&app, Method::GET, uri, true).await.0,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn an_items_music_shelves_are_the_artists_four_and_empty() {
    let app = app();
    let body = json(&app, &format!("/hubs/metadata/{SHOW}")).await;
    let identifiers: Vec<&str> = hubs(&body)
        .iter()
        .map(|hub| hub["hubIdentifier"].as_str().unwrap())
        .collect();
    assert_eq!(
        identifiers,
        [
            "artist.mostplayedtracks",
            "artist.mostpopulartracks",
            "artist.albums",
            "music.videos.popular.new"
        ]
    );
    assert_eq!(
        hubs(&body)[1]["key"],
        format!(
            "/library/sections/28/all?artist.id={SHOW}&type=10&sort=ratingCount:desc&group=title"
        )
    );
    assert!(hubs(&body).iter().all(|hub| hub["size"] == 0));

    // Captured: an empty shelf keeps the long form in XML.
    let (_, xml) = send(&app, Method::GET, &format!("/hubs/metadata/{SHOW}"), false).await;
    assert!(xml.contains("style=\"shelf\">\n</Hub>"), "{xml}");
}

#[tokio::test]
async fn the_home_hubs_are_the_references_eight_and_filter_by_identifier() {
    let app = app();
    let body = json(&app, "/hubs").await;
    let identifiers: Vec<&str> = hubs(&body)
        .iter()
        .map(|hub| hub["hubIdentifier"].as_str().unwrap())
        .collect();
    assert_eq!(
        identifiers,
        [
            "home.continue",
            "home.ondeck",
            "home.movies.recent",
            "home.television.recent",
            "home.music.recent",
            "home.photos.recent",
            "home.videos.recent",
            "home.playlists"
        ]
    );
    let movies = &hubs(&body)[2];
    assert_eq!(movies["key"], "/hubs/home/recentlyAdded?type=1");
    assert_eq!(movies["title"], "Recently Added Movies");
    assert_eq!(movies["promoted"], true);
    assert_eq!(movies["size"], 6);
    let playlists = &hubs(&body)[7];
    assert_eq!(
        playlists["key"],
        "/playlists/all?type=15&sort=lastViewedAt:desc&playlistType=video,audio"
    );
    assert_eq!(playlists["context"], "hub.home.playlists");

    let some = json(
        &app,
        "/hubs?identifier=home.ondeck,home.movies.recent&count=2",
    )
    .await;
    let identifiers: Vec<&str> = hubs(&some)
        .iter()
        .map(|hub| hub["hubIdentifier"].as_str().unwrap())
        .collect();
    assert_eq!(identifiers, ["home.ondeck", "home.movies.recent"]);
    assert_eq!(hubs(&some)[1]["size"], 2);
    assert_eq!(hubs(&some)[1]["more"], true);
}

#[tokio::test]
async fn an_episode_in_progress_is_both_started_and_on_deck() {
    let app = app();
    send(
        &app,
        Method::GET,
        &format!(
            "/:/progress?identifier=com.plexapp.plugins.library&key={EPISODE_1}&time=78000&state=stopped"
        ),
        false,
    )
    .await;
    let body = json(&app, "/hubs").await;
    let started = &hubs(&body)[0];
    assert_eq!(started["style"], "hero");
    assert_eq!(started["key"], "/hubs/home/continueWatching");
    assert_eq!(rating_keys(started), [EPISODE_1.to_string()]);
    assert_eq!(rating_keys(&hubs(&body)[1]), [EPISODE_1.to_string()]);
    let deck = json(&app, "/hubs/home/onDeck").await;
    assert_eq!(
        deck["MediaContainer"]["Metadata"][0]["ratingKey"],
        EPISODE_1.to_string()
    );
}

#[tokio::test]
async fn the_promoted_hubs_hold_each_shown_librarys_recent_shelf_by_title() {
    let app = app();
    let body = json(&app, "/hubs/promoted").await;
    let identifiers: Vec<&str> = hubs(&body)
        .iter()
        .map(|hub| hub["hubIdentifier"].as_str().unwrap())
        .collect();
    // The hidden library has none, and titles order the rest, case aside.
    assert_eq!(
        identifiers,
        [
            "home.continue",
            "home.ondeck",
            "tv.recentlyadded.28",
            "movie.recentlyadded.4",
            "home.playlists"
        ]
    );
    let films = &hubs(&body)[3];
    assert_eq!(films["key"], "/library/sections/4/all?sort=addedAt:desc");
    assert_eq!(films["title"], "Recently Added in VF - Films");
    assert_eq!(films["promoted"], true);
}

#[tokio::test]
async fn the_related_hubs_are_the_items_shelves_without_the_media_tags() {
    let app = app();
    let body = json(&app, "/hubs/metadata/427139/related").await;
    let container = body["MediaContainer"].as_object().unwrap();
    assert_eq!(container["librarySectionID"], 4);
    assert_eq!(container["librarySectionTitle"], "VF - Films");
    assert!(!container.contains_key("mediaTagPrefix"));
    assert!(!container.contains_key("mediaTagVersion"));
    let under_library = json(&app, "/library/metadata/427139/related").await;
    assert_eq!(
        body["MediaContainer"]["Hub"],
        under_library["MediaContainer"]["Hub"]
    );
}

#[tokio::test]
async fn the_official_api_names_the_next_film_and_the_collection_it_follows_in() {
    let schema = schema();
    let app = router(PlexState::new(
        ServerIdentity::new(MACHINE, "0.1.0-dev"),
        schema.clone(),
    ))
    .layer(axum::Extension(open_plex_core::testing::owner()));
    let films: Vec<i64> = SAGA.iter().map(|(id, _, _)| *id).collect();
    collect(&app, "Terminator - Saga", &films).await;
    collect(&app, "The Terminator Collection", &films[1..4]).await;

    let query = r"
query Next($id: ID!) {
  node(id: $id) {
    ... on Movie {
      upNext { movie { title } collection { title } }
      shown: upNext(within: 2) { movie { title } collection { title } }
    }
  }
}";
    let request = async_graphql::Request::new(query)
        .variables(async_graphql::Variables::from_json(serde_json::json!({
            "id": open_plex_api::GlobalId::encode("Item", 445_826),
        })))
        .data(open_plex_core::testing::owner());
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(
        data["node"]["upNext"],
        serde_json::json!({
            "movie": { "title": "Terminator Renaissance" },
            "collection": { "title": "Terminator - Saga" },
        })
    );
    assert_eq!(
        data["node"]["shown"]["collection"]["title"],
        "The Terminator Collection"
    );
}
