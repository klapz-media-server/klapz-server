//! Storage tests for what a set of items has in common, and for filtering and browsing by
//! what their tracks hold.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use klapz_core::collection::{CollectionRepository, NewCollection};
use klapz_core::common::Shared;
use klapz_core::edit::ItemField;
use klapz_core::metadata::{
    FieldValue, ItemDepth, ItemFilter, ItemKind, ItemQuery, MetadataRepository, TagKind,
    TrackCondition, TrackField, ValueField,
};
use klapz_core::page::Window;
use klapz_core::{RatingKey, SectionId};
use klapz_db::entity::{
    item_field_lock, item_tag, library_section, media, media_part, media_stream, metadata_item, tag,
};
use klapz_db::{write, SqlCollectionRepository, SqlMetadataRepository};
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILMS: i64 = 4;
const SERIES: i64 = 5;

fn item(
    id: i64,
    section: i64,
    kind: &str,
    parent: Option<i64>,
    title: &str,
) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(section),
        parent_id: ActiveValue::Set(parent),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("plex://{kind}/{id}")),
        title: ActiveValue::Set((!title.is_empty()).then(|| title.to_owned())),
        title_sort: ActiveValue::Set((!title.is_empty()).then(|| title.to_owned())),
        sort_key: ActiveValue::Set(title.to_lowercase()),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(Some(1)),
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

fn film(id: i64, title: &str, year: i32, studio: Option<&str>) -> metadata_item::ActiveModel {
    let mut model = item(id, FILMS, "movie", None, title);
    model.year = ActiveValue::Set(Some(year));
    model.studio = ActiveValue::Set(studio.map(ToOwned::to_owned));
    model.content_rating = ActiveValue::Set(Some("fr/TP".to_owned()));
    model
}

fn section(id: i64, kind: &str) -> library_section::ActiveModel {
    library_section::ActiveModel {
        id: ActiveValue::Set(id),
        kind: ActiveValue::Set(kind.to_owned()),
        title: ActiveValue::Set(format!("Library {id}")),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set(format!("uuid-{id}")),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    }
}

fn version(id: i64, item: i64) -> media::ActiveModel {
    media::ActiveModel {
        id: ActiveValue::Set(id),
        metadata_item_id: ActiveValue::Set(item),
        width: ActiveValue::Set(Some(1920)),
        height: ActiveValue::Set(Some(1080)),
        aspect_ratio: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        bitrate: ActiveValue::Set(None),
        container: ActiveValue::Set(Some("mkv".to_owned())),
        video_codec: ActiveValue::Set(None),
        audio_codec: ActiveValue::Set(None),
        audio_channels: ActiveValue::Set(None),
        frame_rate: ActiveValue::Set(None),
        interlaced: ActiveValue::Set(false),
        optimized_for_streaming: ActiveValue::Set(false),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
    }
}

fn file(id: i64, version: i64) -> media_part::ActiveModel {
    media_part::ActiveModel {
        id: ActiveValue::Set(id),
        media_id: ActiveValue::Set(version),
        position: ActiveValue::Set(0),
        path: ActiveValue::Set(format!("/mnt/{id}.mkv")),
        size_bytes: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        has_thumbnail: ActiveValue::Set(false),
        has_chapter_text_stream: ActiveValue::Set(false),
        content_hash: ActiveValue::Set(None),
        open_subtitles_hash: ActiveValue::Set(None),
        ..Default::default()
    }
}

fn track(
    id: i64,
    file: i64,
    kind: &str,
    codec: &str,
    layout: Option<&str>,
) -> media_stream::ActiveModel {
    media_stream::ActiveModel {
        id: ActiveValue::Set(id),
        media_part_id: ActiveValue::Set(file),
        kind: ActiveValue::Set(kind.to_owned()),
        position: ActiveValue::Set(0),
        codec: ActiveValue::Set(Some(codec.to_owned())),
        channel_layout: ActiveValue::Set(layout.map(ToOwned::to_owned)),
        is_default: ActiveValue::Set(true),
        is_forced: ActiveValue::Set(false),
        has_closed_captions: ActiveValue::Set(false),
        has_separate_fields: ActiveValue::Set(false),
        has_hdr10_plus: ActiveValue::Set(false),
        is_embedded_in_video: ActiveValue::Set(false),
        is_original: ActiveValue::Set(false),
        is_dub: ActiveValue::Set(false),
        is_commentary: ActiveValue::Set(false),
        is_hearing_impaired: ActiveValue::Set(false),
        is_visual_impaired: ActiveValue::Set(false),
        has_header_compression: ActiveValue::Set(false),
        required_bandwidths: ActiveValue::Set(None),
        ..Default::default()
    }
}

fn attach(item: i64, tag: i64, position: i32, role: Option<&str>) -> item_tag::ActiveModel {
    item_tag::ActiveModel {
        item_id: ActiveValue::Set(item),
        tag_id: ActiveValue::Set(tag),
        position: ActiveValue::Set(position),
        role: ActiveValue::Set(role.map(ToOwned::to_owned)),
    }
}

fn lock(item: i64, field: ItemField) -> item_field_lock::ActiveModel {
    item_field_lock::ActiveModel {
        item_id: ActiveValue::Set(item),
        field: ActiveValue::Set(write::field_name(field).to_owned()),
    }
}

/// Avatar in two versions (h264 then hevc, a 5.1 and a stereo track), Avatar 2 in one, a
/// film with no media, and a series whose two episodes share a season.
async fn connection() -> DatabaseConnection {
    let connection = klapz_db::connect("sqlite::memory:").await.unwrap();
    library_section::Entity::insert_many([section(FILMS, "movie"), section(SERIES, "show")])
        .exec(&connection)
        .await
        .unwrap();
    write::insert_items(
        &connection,
        vec![
            film(1, "Avatar", 2009, Some("Lightstorm")),
            film(2, "Avatar 2", 2022, Some("Lightstorm")),
            film(3, "Brazil", 1985, None),
            item(10, SERIES, "show", None, "Severance"),
            item(11, SERIES, "season", Some(10), ""),
            item(12, SERIES, "episode", Some(11), "Good News About Hell"),
            item(13, SERIES, "episode", Some(11), "Half Loop"),
        ],
    )
    .await
    .unwrap();
    media::Entity::insert_many([version(100, 1), version(101, 1), version(102, 2)])
        .exec(&connection)
        .await
        .unwrap();
    media_part::Entity::insert_many([file(200, 100), file(201, 101), file(202, 102)])
        .exec(&connection)
        .await
        .unwrap();
    media_stream::Entity::insert_many([
        track(300, 200, "video", "h264", None),
        track(301, 200, "audio", "dca", Some("5.1(side)")),
        track(302, 201, "video", "hevc", None),
        track(303, 201, "audio", "truehd", Some("7.1")),
        track(304, 202, "video", "h264", None),
        track(305, 202, "audio", "aac", Some("stereo")),
        track(306, 202, "subtitle", "srt", None),
    ])
    .exec(&connection)
    .await
    .unwrap();
    write::insert_tags(
        &connection,
        [
            (38, TagKind::Genre, "Action"),
            (122, TagKind::Genre, "Adventure"),
            (4034, TagKind::Genre, "Fantastique"),
            (900, TagKind::Actor, "Sam Worthington"),
            (901, TagKind::Actor, "Zoe Saldaña"),
        ]
        .into_iter()
        .map(|(id, kind, name)| tag::ActiveModel {
            id: ActiveValue::Set(id),
            kind: ActiveValue::Set(tag::kind_name(kind).to_owned()),
            name: ActiveValue::Set(name.to_owned()),
            person_id: ActiveValue::Set(None),
        })
        .collect(),
    )
    .await
    .unwrap();
    item_tag::Entity::insert_many([
        attach(1, 4034, 0, None),
        attach(1, 38, 1, None),
        attach(1, 122, 2, None),
        attach(1, 901, 0, Some("Neytiri")),
        attach(1, 900, 1, Some("Jake Sully")),
        attach(2, 38, 0, None),
        attach(2, 4034, 1, None),
        attach(2, 900, 0, Some("Jake Sully")),
        attach(2, 901, 1, Some("Neytiri")),
    ])
    .exec(&connection)
    .await
    .unwrap();
    item_field_lock::Entity::insert_many([
        lock(1, ItemField::Title),
        lock(1, ItemField::Poster),
        lock(2, ItemField::Poster),
    ])
    .exec(&connection)
    .await
    .unwrap();
    connection
}

fn films(filter: ItemFilter) -> ItemQuery {
    ItemQuery::new(SectionId::new(FILMS)).filtered(filter)
}

fn ids(keys: &[i64]) -> ItemFilter {
    ItemFilter {
        ids: keys.iter().copied().map(RatingKey::new).collect(),
        ..ItemFilter::default()
    }
}

fn track_filter(field: TrackField, values: &[&str], negated: bool) -> ItemFilter {
    ItemFilter {
        tracks: vec![TrackCondition {
            field,
            any_of: values.iter().map(|value| (*value).to_owned()).collect(),
            negated,
        }],
        ..ItemFilter::default()
    }
}

async fn titles(repository: &SqlMetadataRepository, query: &ItemQuery) -> Vec<String> {
    let window = Window::new(None, None, Some(50), None).unwrap();
    repository
        .items(query, window)
        .await
        .unwrap()
        .items
        .into_iter()
        .filter_map(|listed| listed.item.title)
        .collect()
}

#[tokio::test]
async fn a_track_filter_reads_every_version_and_negation_keeps_what_has_none() {
    let repository = SqlMetadataRepository::new(connection().await);
    let found = |field, values: &'static [&'static str], negated| {
        let repository = &repository;
        async move { titles(repository, &films(track_filter(field, values, negated))).await }
    };
    // Avatar's second version is the hevc one.
    assert_eq!(
        found(TrackField::VideoCodec, &["hevc"], false).await,
        ["Avatar"]
    );
    assert_eq!(
        found(TrackField::VideoCodec, &["h264"], false).await,
        ["Avatar", "Avatar 2"]
    );
    // Exact, case included, as on the reference.
    assert!(found(TrackField::VideoCodec, &["H264"], false)
        .await
        .is_empty());
    assert_eq!(
        found(TrackField::AudioLayout, &["7.1", "stereo"], false).await,
        ["Avatar", "Avatar 2"]
    );
    // A film with no media holds no hevc track.
    assert_eq!(
        found(TrackField::VideoCodec, &["hevc"], true).await,
        ["Avatar 2", "Brazil"]
    );
    // A codec is read on tracks of its kind only: no video track is `srt`.
    assert!(found(TrackField::VideoCodec, &["srt"], false)
        .await
        .is_empty());
    assert_eq!(
        found(TrackField::SubtitleCodec, &["srt"], false).await,
        ["Avatar 2"]
    );
}

#[tokio::test]
async fn lists_and_counts_the_values_the_tracks_hold() {
    let repository = SqlMetadataRepository::new(connection().await);
    let query = films(ItemFilter::default());
    let window = Window::new(None, None, Some(2), None).unwrap();
    let field = ValueField::Track(TrackField::AudioCodec);
    let first = repository.values_in(&query, field, window).await.unwrap();
    assert_eq!(
        first.items,
        [
            FieldValue::Text("aac".to_owned()),
            FieldValue::Text("dca".to_owned())
        ]
    );
    assert!(first.has_next);
    let after = first.items[1].cursor();
    let window = Window::new(Some(after), None, Some(2), None).unwrap();
    let second = repository.values_in(&query, field, window).await.unwrap();
    assert_eq!(second.items, [FieldValue::Text("truehd".to_owned())]);
    assert_eq!(repository.value_count_in(&query, field).await.unwrap(), 3);

    // Narrowed by the filter, like every other list of values.
    let narrowed = films(ids(&[2]));
    assert_eq!(
        repository
            .value_count_in(&narrowed, ValueField::Track(TrackField::AudioLayout))
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn several_items_share_what_they_all_hold_and_differ_on_the_rest() {
    let connection = connection().await;
    let collections = SqlCollectionRepository::new(connection.clone());
    collections
        .create(
            NewCollection {
                section: SectionId::new(FILMS),
                kind: ItemKind::Movie,
                title: "Avatar - Saga".to_owned(),
                items: vec![RatingKey::new(1), RatingKey::new(2)],
                rule: None,
                source_uri: None,
            },
            0,
        )
        .await
        .unwrap();
    let repository = SqlMetadataRepository::new(connection);

    let common = repository
        .common_fields(&films(ids(&[1, 2])))
        .await
        .unwrap();
    assert_eq!(common.items, 2);
    assert!(common.id.is_mixed());
    assert!(common.title.is_mixed());
    assert!(common.year.is_mixed());
    assert_eq!(common.studio, Shared::Same(Some("Lightstorm".to_owned())));
    assert_eq!(
        common.content_rating.value().map(String::as_str),
        Some("fr/TP")
    );
    // Neither holds a tagline: shared, and absent.
    assert_eq!(common.tagline, Shared::Same(None));
    assert_eq!(common.position, Shared::Same(Some(1)));
    // Tags both carry, by id, with no character: the two films bill them differently.
    let shared: Vec<(i64, Option<&str>)> = common
        .tags
        .iter()
        .map(|shared| (shared.tag.id.get(), shared.role.as_deref()))
        .collect();
    assert_eq!(shared, [(38, None), (900, None), (901, None), (4034, None)]);
    assert_eq!(common.locked, [ItemField::Poster]);
    assert_eq!(common.collections.len(), 1);

    // A film one of them has no studio for makes the studio mixed.
    let with_brazil = repository
        .common_fields(&films(ids(&[1, 3])))
        .await
        .unwrap();
    assert!(with_brazil.studio.is_mixed());
    assert!(with_brazil.tags.is_empty());
    assert!(with_brazil.collections.is_empty());
}

#[tokio::test]
async fn one_item_shares_everything_in_its_own_billing_order() {
    let repository = SqlMetadataRepository::new(connection().await);
    let common = repository.common_fields(&films(ids(&[1]))).await.unwrap();
    assert_eq!(common.id, Shared::Same(Some(RatingKey::new(1))));
    let genres: Vec<i64> = common
        .tags
        .iter()
        .filter(|shared| shared.tag.kind == TagKind::Genre)
        .map(|shared| shared.tag.id.get())
        .collect();
    assert_eq!(genres, [4034, 38, 122]);
    let cast: Vec<Option<&str>> = common
        .tags
        .iter()
        .filter(|shared| shared.tag.kind == TagKind::Actor)
        .map(|shared| shared.role.as_deref())
        .collect();
    assert_eq!(cast, [Some("Neytiri"), Some("Jake Sully")]);
    assert_eq!(common.locked, [ItemField::Title, ItemField::Poster]);
}

#[tokio::test]
async fn episodes_share_their_season_and_series() {
    let repository = SqlMetadataRepository::new(connection().await);
    let query = ItemQuery::new(SectionId::new(SERIES)).filtered(ItemFilter {
        kind: Some(ItemKind::Episode),
        depth: ItemDepth::All,
        ..ItemFilter::default()
    });
    let common = repository.common_fields(&query).await.unwrap();
    assert_eq!(common.items, 2);
    assert_eq!(common.parent, Shared::Same(Some(RatingKey::new(11))));
    // The season is untitled.
    assert_eq!(common.parent_title, Shared::Same(None));
    assert_eq!(common.grandparent, Shared::Same(Some(RatingKey::new(10))));
    assert_eq!(
        common.grandparent_title.value().map(String::as_str),
        Some("Severance")
    );
}

#[tokio::test]
async fn an_empty_set_shares_nothing_and_differs_on_nothing() {
    let repository = SqlMetadataRepository::new(connection().await);
    let common = repository.common_fields(&films(ids(&[999]))).await.unwrap();
    assert_eq!(common, klapz_core::common::CommonFields::empty());
}
