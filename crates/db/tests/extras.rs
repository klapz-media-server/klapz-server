//! Storage tests for extras, with the reference's own rows for *Les Croods*: two trailers its
//! metadata provider supplied, each with three versions of one part and two tracks, and a
//! behind-the-scenes clip added after them at position 1.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use klapz_core::extra::ExtraKind;
use klapz_core::language::LanguageTag;
use klapz_core::metadata::{MetadataRepository, StreamKind};
use klapz_core::{ExtraId, MediaId, PartId, RatingKey, StreamId};
use klapz_db::entity::{
    extra, extra_media, extra_track, library_section, media, media_part, metadata_item,
};
use klapz_db::write;
use klapz_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILM: i64 = 576_947;
const OTHER_FILM: i64 = 577_986;
const FRENCH: i64 = 576_949;
const SUBTITLED: i64 = 576_950;
const MAKING_OF: i64 = 576_960;

fn item(id: i64) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(4),
        parent_id: ActiveValue::Set(None),
        kind: ActiveValue::Set("movie".to_owned()),
        guid: ActiveValue::Set(format!("plex://movie/{id}")),
        title: ActiveValue::Set(Some("Les Croods".to_owned())),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set("croods".to_owned()),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(None),
        year: ActiveValue::Set(None),
        released_on: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        refreshed_at: ActiveValue::Set(None),
        chapter_source: ActiveValue::Set(None),
    }
}

fn trailer(id: i64, position: Option<i32>, kind: &str, title: &str) -> extra::ActiveModel {
    extra::ActiveModel {
        id: ActiveValue::Set(id),
        item_id: ActiveValue::Set(FILM),
        kind: ActiveValue::Set(kind.to_owned()),
        position: ActiveValue::Set(position),
        guid: ActiveValue::Set(format!(
            "iva://api.internetvideoarchive.com/2.0/DataService/VideoAssets({id})"
        )),
        title: ActiveValue::Set(Some(title.to_owned())),
        title_sort: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        year: ActiveValue::Set(Some(2012)),
        released_on: ActiveValue::Set(Some(1_352_678_400)),
        duration_ms: ActiveValue::Set(Some(136_000)),
        added_at: ActiveValue::Set(Some(1_352_756_325)),
        updated_at: ActiveValue::Set(None),
    }
}

/// A provider's version: its part is a stream at an address, on no disk.
fn version(id: i64, extra: i64, part: i64, bitrate: i32, height: i32) -> extra_media::ActiveModel {
    extra_media::ActiveModel {
        id: ActiveValue::Set(id),
        extra_id: ActiveValue::Set(extra),
        part_id: ActiveValue::Set(part),
        width: ActiveValue::Set(Some(1920)),
        height: ActiveValue::Set(Some(height)),
        aspect_ratio: ActiveValue::Set(Some(2.352_941_274_642_944)),
        duration_ms: ActiveValue::Set(Some(136_000)),
        bitrate: ActiveValue::Set(Some(bitrate)),
        container: ActiveValue::Set(Some("mp4".to_owned())),
        video_codec: ActiveValue::Set(Some("h264".to_owned())),
        audio_codec: ActiveValue::Set(Some("aac".to_owned())),
        audio_channels: ActiveValue::Set(None),
        optimized_for_streaming: ActiveValue::Set(true),
        path: ActiveValue::Set(None),
        size_bytes: ActiveValue::Set(None),
        stream_url: ActiveValue::Set(Some(format!(
            "/services/iva/assets/200752/video.mp4?fmt=4&bitrate={}",
            bitrate / 1000
        ))),
        file_optimized_for_streaming: ActiveValue::Set(None),
    }
}

fn track(id: i64, version: i64, kind: &str, position: i32) -> extra_track::ActiveModel {
    let video = kind == "video";
    extra_track::ActiveModel {
        id: ActiveValue::Set(id),
        extra_media_id: ActiveValue::Set(version),
        kind: ActiveValue::Set(kind.to_owned()),
        position: ActiveValue::Set(position),
        codec: ActiveValue::Set(Some(if video { "h264" } else { "aac" }.to_owned())),
        language: ActiveValue::Set((!video).then(|| "fr".to_owned())),
        channels: ActiveValue::Set((!video).then_some(2)),
        bitrate: ActiveValue::Set(video.then_some(5_000_000)),
        width: ActiveValue::Set(video.then_some(1920)),
        height: ActiveValue::Set(video.then_some(816)),
        is_default: ActiveValue::Set(false),
    }
}

async fn repository() -> (SqlMetadataRepository, DatabaseConnection) {
    let connection = klapz_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(4),
        kind: ActiveValue::Set("movie".to_owned()),
        title: ActiveValue::Set("VF - Films".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set("2cd48489-54c0-41e9-ae62-f3dcb0f3c249".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(&connection)
    .await
    .expect("section");
    write::insert_items(&connection, vec![item(FILM), item(OTHER_FILM)])
        .await
        .expect("items");
    extra::Entity::insert_many([
        trailer(
            SUBTITLED,
            Some(2),
            "trailer",
            "The Croods (French Subtitled)",
        ),
        trailer(FRENCH, Some(1), "trailer", "The Croods (French)"),
        trailer(MAKING_OF, None, "behind_the_scenes", "Making of"),
    ])
    .exec(&connection)
    .await
    .expect("extras");
    extra_media::Entity::insert_many([
        version(5_563_786, FRENCH, 5_569_001, 750_000, 204),
        version(5_563_784, FRENCH, 5_568_999, 5_000_000, 816),
        version(5_563_785, FRENCH, 5_569_000, 2_500_000, 544),
        version(5_563_787, SUBTITLED, 5_569_002, 5_000_000, 816),
    ])
    .exec(&connection)
    .await
    .expect("versions");
    extra_track::Entity::insert_many([
        track(12_257_090, 5_563_784, "audio", 1),
        track(12_257_089, 5_563_784, "video", 0),
    ])
    .exec(&connection)
    .await
    .expect("tracks");
    (SqlMetadataRepository::new(connection.clone()), connection)
}

#[tokio::test]
async fn extras_come_by_position_then_id_with_the_unplaced_last() {
    let (repository, _) = repository().await;
    let extras = repository
        .extras(&[RatingKey::new(FILM), RatingKey::new(OTHER_FILM)])
        .await
        .unwrap();
    let listed: Vec<(i64, ExtraKind)> = extras
        .iter()
        .map(|extra| (extra.id.get(), extra.kind))
        .collect();
    // The extra with no position comes first, as the reference's own `order by index` lists
    // it.
    assert_eq!(
        listed,
        [
            (MAKING_OF, ExtraKind::BehindTheScenes),
            (FRENCH, ExtraKind::Trailer),
            (SUBTITLED, ExtraKind::Trailer),
        ]
    );
    assert_eq!(
        klapz_core::extra::primary(&extras).map(|extra| extra.id.get()),
        Some(FRENCH)
    );
    let french = &extras[1];
    assert_eq!(french.released_on, Some(1_352_678_400));
    assert_eq!(french.updated_at, None);
    assert!(repository
        .extras(&[RatingKey::new(OTHER_FILM)])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_version_names_the_item_its_extra_comes_with() {
    let (repository, _) = repository().await;
    let versions = repository
        .extra_versions(&[ExtraId::new(FRENCH), ExtraId::new(MAKING_OF)])
        .await
        .unwrap();
    let ids: Vec<(i64, i64)> = versions
        .iter()
        .map(|(extra, version)| (extra.get(), version.id.get()))
        .collect();
    assert_eq!(
        ids,
        [
            (FRENCH, 5_563_784),
            (FRENCH, 5_563_785),
            (FRENCH, 5_563_786)
        ]
    );
    assert!(versions
        .iter()
        .all(|(_, version)| version.item_id == RatingKey::new(FILM)));
    assert_eq!(versions[0].1.bitrate, Some(5_000_000));

    let refetched = repository
        .version(MediaId::new(5_563_785))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refetched.item_id, RatingKey::new(FILM));
    assert_eq!(refetched.height, Some(544));
}

#[tokio::test]
async fn an_extras_files_and_tracks_are_found_where_an_items_are() {
    let (repository, connection) = repository().await;
    // An item's own version and file, asked for alongside an extra's.
    media::Entity::insert(media::ActiveModel {
        id: ActiveValue::Set(1),
        metadata_item_id: ActiveValue::Set(FILM),
        width: ActiveValue::Set(None),
        height: ActiveValue::Set(None),
        aspect_ratio: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        bitrate: ActiveValue::Set(None),
        container: ActiveValue::Set(None),
        video_codec: ActiveValue::Set(None),
        audio_codec: ActiveValue::Set(None),
        audio_channels: ActiveValue::Set(None),
        frame_rate: ActiveValue::Set(None),
        interlaced: ActiveValue::Set(false),
        optimized_for_streaming: ActiveValue::Set(false),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
    })
    .exec(&connection)
    .await
    .unwrap();
    media_part::Entity::insert(media_part::ActiveModel {
        id: ActiveValue::Set(2),
        media_id: ActiveValue::Set(1),
        position: ActiveValue::Set(0),
        path: ActiveValue::Set("/films/Les Croods.mkv".to_owned()),
        size_bytes: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        has_thumbnail: ActiveValue::Set(false),
        has_64bit_offsets: ActiveValue::Set(None),
        optimized_for_streaming: ActiveValue::Set(None),
        packet_length: ActiveValue::Set(None),
        has_chapter_text_stream: ActiveValue::Set(false),
        // The hashes and what ffprobe states: none.
        ..Default::default()
    })
    .exec(&connection)
    .await
    .unwrap();

    let files = repository
        .files_for(&[MediaId::new(1), MediaId::new(5_563_784)])
        .await
        .unwrap();
    let paths: Vec<(i64, &str, Option<&str>)> = files
        .iter()
        .map(|file| {
            (
                file.id.get(),
                file.path.as_str(),
                file.stream_url.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        paths,
        [
            (2, "/films/Les Croods.mkv", None),
            (
                5_568_999,
                "",
                Some("/services/iva/assets/200752/video.mp4?fmt=4&bitrate=5000")
            ),
        ]
    );
    assert_eq!(files[1].version_id, MediaId::new(5_563_784));

    let tracks = repository
        .streams_for(&[PartId::new(5_568_999)])
        .await
        .unwrap();
    let kinds: Vec<_> = tracks
        .iter()
        .map(|track| (track.id.get(), track.kind, track.language))
        .collect();
    assert_eq!(
        kinds,
        [
            (12_257_089, StreamKind::Video, None),
            (12_257_090, StreamKind::Audio, LanguageTag::parse("fr"))
        ]
    );
    let video = tracks[0].video.as_ref().unwrap();
    assert_eq!((video.width, video.height), (Some(1920), Some(816)));
    assert!(tracks
        .iter()
        .all(|track| track.file_id == PartId::new(5_568_999)));

    let file = repository
        .file(PartId::new(5_569_000))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.version_id, MediaId::new(5_563_785));
    let stream = repository
        .stream(StreamId::new(12_257_090))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stream.file_id, PartId::new(5_568_999));
}

#[tokio::test]
async fn extras_go_with_their_item() {
    let (repository, connection) = repository().await;
    metadata_item::Entity::delete_by_id(FILM)
        .exec(&connection)
        .await
        .unwrap();
    assert!(repository
        .extras(&[RatingKey::new(FILM)])
        .await
        .unwrap()
        .is_empty());
    assert!(repository
        .version(MediaId::new(5_563_784))
        .await
        .unwrap()
        .is_none());
    assert!(repository
        .stream(StreamId::new(12_257_089))
        .await
        .unwrap()
        .is_none());
}
