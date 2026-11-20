//! A library's codec and layout choices, its filter and sort menus, and the fields common to
//! a set of its items, against responses captured on the reference (1.43.5):
//! `tests/fixtures/library_menus`. The items reproduce the captured ones field for field.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use open_plex_api::{build_schema, ApiContext, ApiSchema};
use open_plex_api_plex::{router, PlexState};
use open_plex_core::edit::{CalendarDate, ItemField};
use open_plex_core::metadata::{
    AudioAnalysis, FileLayout, ItemKind, MediaFile, MediaStream, MediaVersion, MetadataItem,
    StreamKind, TrackTraits,
};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{
    LibrarySection, MediaId, PartId, RatingKey, SectionId, SectionKind, ServerCapabilities,
    ServerIdentity, StreamId,
};
use serde_json::Value;
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/library_menus/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn section(id: i64, kind: SectionKind, title: &str) -> LibrarySection {
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
        hidden: false,
    }
}

struct Item {
    id: i64,
    section: i64,
    kind: ItemKind,
    parent: Option<i64>,
    title: Option<&'static str>,
    title_sort: Option<&'static str>,
    original_title: Option<&'static str>,
    summary: Option<&'static str>,
    content_rating: Option<&'static str>,
    released: Option<&'static str>,
}

impl Item {
    const fn film(id: i64, title: &'static str, released: &'static str) -> Self {
        Self {
            id,
            section: 4,
            kind: ItemKind::Movie,
            parent: None,
            title: Some(title),
            title_sort: Some(title),
            original_title: None,
            summary: None,
            content_rating: None,
            released: Some(released),
        }
    }

    fn build(self) -> MetadataItem {
        let released = self.released.and_then(CalendarDate::parse);
        MetadataItem {
            id: RatingKey::new(self.id),
            section_id: SectionId::new(self.section),
            parent_id: self.parent.map(RatingKey::new),
            kind: self.kind,
            guid: format!("plex://item/{}", self.id),
            title: self.title.map(ToOwned::to_owned),
            title_sort: self.title_sort.map(ToOwned::to_owned),
            original_title: self.original_title.map(ToOwned::to_owned),
            summary: self.summary.map(ToOwned::to_owned),
            tagline: None,
            studio: None,
            content_rating: self.content_rating.map(ToOwned::to_owned),
            content_rating_age: None,
            position: Some(1),
            year: self.released.map(|date| date[..4].parse().unwrap()),
            released_on: released.map(CalendarDate::midnight),
            duration_ms: None,
            rating: None,
            audience_rating: None,
            added_at: 0,
            updated_at: 0,
            refreshed_at: None,
            chapter_source: None,
        }
    }
}

/// An episode as the reference's two are stored: numbered 1 in a season numbered 1 and
/// untitled, which is how the source keeps seasons.
fn episode(
    ids: (i64, i64, i64),
    show: &'static str,
    title: &'static str,
    title_sort: &'static str,
    original_title: &'static str,
    content_rating: Option<&'static str>,
    released: &'static str,
) -> [MetadataItem; 3] {
    let (show_id, season_id, episode_id) = ids;
    let mut show_item = Item::film(show_id, show, released);
    show_item.section = 5;
    show_item.kind = ItemKind::Show;
    let season = Item {
        id: season_id,
        section: 5,
        kind: ItemKind::Season,
        parent: Some(show_id),
        title: None,
        title_sort: Some("Saison 1"),
        original_title: None,
        summary: None,
        content_rating: None,
        released: None,
    };
    let episode = Item {
        id: episode_id,
        section: 5,
        kind: ItemKind::Episode,
        parent: Some(season_id),
        title: Some(title),
        title_sort: Some(title_sort),
        original_title: Some(original_title),
        summary: Some(title),
        content_rating,
        released: Some(released),
    };
    [show_item.build(), season.build(), episode.build()]
}

/// One version of a film per value, each with one track holding it: the values of the
/// captured lists, which every version of a library contributes to.
fn tracks(
    item: i64,
    first_id: i64,
    kind: StreamKind,
    values: &[&str],
) -> (Vec<MediaVersion>, Vec<MediaFile>, Vec<MediaStream>) {
    let mut media = (Vec::new(), Vec::new(), Vec::new());
    for (offset, value) in (0..).zip(values) {
        let id = first_id + offset;
        media.0.push(MediaVersion {
            id: MediaId::new(id),
            item_id: RatingKey::new(item),
            width: Some(1920),
            height: Some(1080),
            aspect_ratio: None,
            duration_ms: None,
            bitrate: None,
            container: Some("mkv".to_owned()),
            video_codec: None,
            audio_codec: None,
            audio_channels: None,
            frame_rate: None,
            interlaced: false,
            optimized_for_streaming: false,
        });
        media.1.push(MediaFile {
            id: PartId::new(id),
            version_id: MediaId::new(id),
            position: 0,
            path: format!("/media/{id}.mkv"),
            size_bytes: None,
            duration_ms: None,
            layout: FileLayout::default(),
            stream_url: None,
            content_hash: None,
            open_subtitles_hash: None,
            stated: open_plex_core::metadata::StatedFile::default(),
        });
        let audio = kind == StreamKind::Audio;
        media.2.push(MediaStream {
            id: StreamId::new(id),
            file_id: PartId::new(id),
            kind,
            position: 0,
            codec: (!audio).then(|| (*value).to_owned()),
            language: None,
            channels: None,
            bitrate: None,
            is_default: true,
            is_forced: false,
            external_path: None,
            title: None,
            stream_identifier: None,
            profile: None,
            bit_depth: None,
            traits: TrackTraits::default(),
            video: None,
            audio: audio.then(|| AudioAnalysis {
                sampling_rate: None,
                channel_layout: Some((*value).to_owned()),
                loudness: None,
            }),
            subtitle: None,
            required_bandwidths: Vec::new(),
            stated: open_plex_core::metadata::StatedTrack::default(),
        });
    }
    media
}

fn schema() -> ApiSchema {
    let mut items = vec![
        Item::film(81_132, "Les 12 vœux de Noël", "2011-11-27").build(),
        Item::film(212_900, "Le crash", "2012-02-01").build(),
    ];
    items.extend(episode(
        (369_599, 369_600, 369_601),
        "#BlackAF",
        "À cause de l'esclavage",
        "A cause de l'esclavage",
        "#blackAF",
        Some("TV-MA"),
        "2020-04-17",
    ));
    items.extend(episode(
        (508_585, 508_586, 508_587),
        "13 novembre fluctuat nec mergitur",
        "Épisode 1",
        "Episode 1",
        "November 13: Attack on Paris",
        None,
        "2018-06-01",
    ));
    let video_codecs = [
        "av1",
        "h264",
        "hevc",
        "mjpeg",
        "mpeg2video",
        "mpeg4",
        "msmpeg4v3",
        "vc1",
    ];
    // Twice `stereo`, from two versions: a value is listed once.
    let layouts = [
        "stereo",
        "3.0",
        "3.1",
        "4.0",
        "4.1",
        "5.0(side)",
        "5.1",
        "5.1(side)",
        "6.1",
        "7.1",
        "8 channels (FL+FR+FC+LFE+SL+SR+TFL+TFR)",
        "mono",
        "quad(side)",
        "stereo",
    ];
    let (mut versions, mut files, mut streams) =
        tracks(81_132, 1_000, StreamKind::Video, &video_codecs);
    let (more_versions, more_files, more_streams) =
        tracks(212_900, 2_000, StreamKind::Audio, &layouts);
    versions.extend(more_versions);
    files.extend(more_files);
    streams.extend(more_streams);

    let metadata = InMemoryMetadata::new(items)
        .with_media(versions, files, streams)
        .with_locked_fields(vec![
            (RatingKey::new(81_132), ItemField::Title),
            (RatingKey::new(81_132), ItemField::TitleSort),
            (RatingKey::new(212_900), ItemField::Poster),
            (RatingKey::new(212_900), ItemField::Genres),
        ]);
    build_schema(ApiContext {
        capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0-dev"), 0),
        library: std::sync::Arc::new(InMemoryLibrary::new(vec![
            section(4, SectionKind::Movie, "VF - Films"),
            section(5, SectionKind::Show, "VF - Series"),
        ])),
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
    })
}

async fn get(uri: &str, json: bool) -> (StatusCode, String) {
    let app = router(PlexState::new(
        ServerIdentity::new(MACHINE, "0.1.0-dev"),
        schema(),
    ))
    .layer(axum::Extension(open_plex_core::testing::owner()));
    let mut request = Request::builder().uri(uri);
    if json {
        request = request.header(header::ACCEPT, "application/json");
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

async fn json(uri: &str) -> Value {
    let (status, body) = get(uri, true).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    serde_json::from_str(&body).unwrap()
}

/// The container attributes, which are the reference's but for the media flags' version,
/// which we do not track.
fn container(body: &Value) -> serde_json::Map<String, Value> {
    let mut attributes = body["MediaContainer"].as_object().unwrap().clone();
    attributes.retain(|key, value| !value.is_array() && key != "mediaTagVersion");
    attributes
}

#[tokio::test]
async fn lists_the_video_codecs_as_the_reference_does() {
    let reference = fixture("video_codec_4");
    let ours = json("/library/sections/4/videoCodec").await;
    assert_eq!(
        ours["MediaContainer"]["Directory"],
        reference["MediaContainer"]["Directory"]
    );
    // Always windowed, like every list of choices here.
    let mut attributes = container(&ours);
    assert_eq!(attributes.remove("totalSize"), Some(Value::from(8)));
    assert_eq!(attributes.remove("offset"), Some(Value::from(0)));
    assert_eq!(attributes, container(&reference));
}

#[tokio::test]
async fn lists_the_audio_layouts_encoded_once() {
    let reference = fixture("audio_layout_4");
    let ours = json("/library/sections/4/audioLayout").await;
    assert_eq!(
        ours["MediaContainer"]["Directory"],
        reference["MediaContainer"]["Directory"]
    );
    let (_, xml) = get("/library/sections/4/audioLayout", false).await;
    assert!(
        xml.contains("<Directory key=\"5%2E1%28side%29\" title=\"5.1(side)\" />"),
        "{xml}"
    );
}

#[tokio::test]
async fn a_list_of_values_ignores_the_type() {
    // Every version of the library, whatever `type` names, as on the reference.
    let codecs = json("/library/sections/4/videoCodec?type=4").await;
    assert_eq!(codecs["MediaContainer"]["size"], 8);
    let none = json("/library/sections/5/subtitleCodec").await;
    assert_eq!(none["MediaContainer"]["size"], 0);
    assert_eq!(none["MediaContainer"]["nocache"], true);
}

#[tokio::test]
async fn every_value_listed_filters_a_listing() {
    for (filter, value, expected) in [
        ("videoCodec", "mjpeg", 1),
        ("videoCodec", "av1,vc1", 1),
        ("videoCodec!", "mjpeg", 1),
        ("audioLayout", "5%2E1%28side%29", 1),
        (
            "audioLayout",
            "8%20channels%20%28FL%2BFR%2BFC%2BLFE%2BSL%2BSR%2BTFL%2BTFR%29",
            1,
        ),
        ("audioCodec", "aac", 0),
        ("subtitleCodec!", "srt", 2),
        // Matched exactly, case included.
        ("videoCodec", "H264", 0),
    ] {
        let body = json(&format!(
            "/library/sections/4/all?{filter}={value}&X-Plex-Container-Start=0&X-Plex-Container-Size=10"
        ))
        .await;
        assert_eq!(
            body["MediaContainer"]["totalSize"], expected,
            "{filter}={value}"
        );
    }
    // A comparison on the text, which the reference makes and we cannot, is refused.
    let (status, _) = get("/library/sections/4/all?videoCodec%3E=h264", true).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

/// The reference's menu, less the entries a listing cannot honor.
fn honored(reference: &Value, offered: &[Value]) -> Vec<Value> {
    let keys: Vec<&Value> = offered.iter().map(|entry| &entry["key"]).collect();
    reference["MediaContainer"]["Directory"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| keys.contains(&&entry["key"]))
        .cloned()
        .collect()
}

#[tokio::test]
async fn the_menus_are_the_references_entries_we_honor_in_its_order() {
    for (uri, name, count) in [
        ("/library/sections/4/filters", "filters_4", 18),
        ("/library/sections/5/filters?type=4", "filters_5_type_4", 9),
        ("/library/sections/4/sorts", "sorts_4", 9),
        ("/library/sections/5/sorts?type=4", "sorts_5_type_4", 6),
    ] {
        let reference = fixture(name);
        let ours = json(uri).await;
        let offered = ours["MediaContainer"]["Directory"].as_array().unwrap();
        assert_eq!(offered.len(), count, "{uri}");
        assert_eq!(*offered, honored(&reference, offered), "{uri}");
        let mut attributes = container(&reference);
        attributes.insert("size".to_owned(), Value::from(count));
        assert_eq!(container(&ours), attributes, "{uri}");
    }
}

#[tokio::test]
async fn a_menu_is_windowed_only_when_asked() {
    let body =
        json("/library/sections/4/filters?X-Plex-Container-Start=2&X-Plex-Container-Size=3").await;
    let container = &body["MediaContainer"];
    assert_eq!(container["size"], 3);
    assert_eq!(container["totalSize"], 18);
    assert_eq!(container["offset"], 2);
    assert_eq!(container["Directory"][0]["filter"], "decade");
    let (status, _) = get("/library/sections/99/filters", true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The reference's `common` element, without what we do not publish: the empty `Rating`
/// placeholders it writes, one per rating some item carries.
fn common_of(body: &Value) -> Value {
    let mut entry = body["MediaContainer"]["Metadata"][0].clone();
    entry.as_object_mut().unwrap().remove("Rating");
    entry
}

#[tokio::test]
async fn one_item_has_everything_in_common_with_itself() {
    let reference = fixture("common_81132");
    let ours = json("/library/sections/4/common?type=1&id=81132&includeExternalMedia=1").await;
    assert_eq!(common_of(&ours), common_of(&reference));
    assert_eq!(container(&ours), container(&reference));
}

#[tokio::test]
async fn two_items_name_what_differs_and_keep_what_is_shared() {
    let reference = fixture("common_81132_212900");
    let ours =
        json("/library/sections/4/common?type=1&id=81132,212900&includeExternalMedia=1").await;
    // Neither locks what the other does: no `Field`.
    assert_eq!(common_of(&ours), common_of(&reference));
}

#[tokio::test]
async fn episodes_of_two_series_share_their_untitled_season() {
    let reference = fixture("common_369601_508587");
    let ours =
        json("/library/sections/5/common?type=4&id=369601,508587&includeExternalMedia=1").await;
    assert_eq!(common_of(&ours), common_of(&reference));
    assert_eq!(container(&ours), container(&reference));
}

#[tokio::test]
async fn a_set_of_nothing_is_one_empty_common_element() {
    let reference = fixture("common_none");
    let ours = json("/library/sections/4/common?type=1&id=999999999").await;
    assert_eq!(common_of(&ours), common_of(&reference));
    // A film is not of the series type: nothing again.
    let other_type = json("/library/sections/4/common?type=2&id=81132").await;
    assert_eq!(common_of(&other_type), common_of(&reference));
    let (_, xml) = get("/library/sections/4/common?type=1&id=999999999", false).await;
    assert!(
        xml.contains("<Common mixedFields=\"\" guid=\"\" type=\"common\" titleSort=\"\">"),
        "{xml}"
    );
}

#[tokio::test]
async fn common_requires_a_type_and_honors_the_listing_filters() {
    let (status, _) = get("/library/sections/4/common?id=81132", true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get("/library/sections/4/common?type=1&id=abc", true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // `videoCodec` narrows the set as it narrows a listing: Le crash has no video track.
    let filtered = json("/library/sections/4/common?type=1&videoCodec=h264").await;
    let entry = &filtered["MediaContainer"]["Metadata"][0];
    assert_eq!(entry["ratingKey"], "81132");
    // A filter we cannot honor is refused rather than dropped, as on a listing.
    let (status, _) = get("/library/sections/4/common?type=1&hdr=1", true).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}
