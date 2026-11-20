//! Storage tests for markers: the work's, as the importer writes them, and the bookmarks
//! viewers keep.
//!
//! The work's markers are the reference's own for an episode of *The Farewell*: credits found
//! first, at 2,233,609–2,397,344 ms and final, then the intro at 136,180–176,381 ms. The
//! bookmarks are the ones measured on the reference's test film, Soul.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::marker::{MarkerKind, MarkerValues, NewMarker};
use open_plex_core::metadata::{MetadataItem, MetadataRepository};
use open_plex_core::{MarkerId, RatingKey, UserId};
use open_plex_db::entity::{item_marker, library_section, metadata_item};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const EPISODE: i64 = 423_845;
const FILM: i64 = 577_986;
const VIEWER: UserId = UserId::new(3);
const OTHER: UserId = UserId::new(4);

fn item(id: i64, kind: &str) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(27),
        parent_id: ActiveValue::Set(None),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("plex://{kind}/{id}")),
        title: ActiveValue::Set(Some("The Farewell".to_owned())),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set("farewell".to_owned()),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(None),
        year: ActiveValue::Set(None),
        released_on: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(Some(3_514_208)),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        refreshed_at: ActiveValue::Set(None),
        chapter_source: ActiveValue::Set(None),
    }
}

fn work_marker(
    kind: &str,
    start: i64,
    end: i64,
    is_final: bool,
    version: i32,
) -> item_marker::ActiveModel {
    item_marker::ActiveModel {
        id: ActiveValue::NotSet,
        item_id: ActiveValue::Set(EPISODE),
        owner: ActiveValue::Set(None),
        kind: ActiveValue::Set(kind.to_owned()),
        start_ms: ActiveValue::Set(start),
        end_ms: ActiveValue::Set(Some(end)),
        is_final: ActiveValue::Set(is_final),
        version: ActiveValue::Set(Some(version)),
        title: ActiveValue::Set(None),
        color: ActiveValue::Set(None),
    }
}

async fn repository() -> (SqlMetadataRepository, DatabaseConnection) {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(27),
        kind: ActiveValue::Set("movie".to_owned()),
        title: ActiveValue::Set("open-plex tests".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("en-US".to_owned()),
        uuid: ActiveValue::Set("16785456-a589-4342-a96d-58911d98d98b".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(&connection)
    .await
    .expect("section");
    write::insert_items(
        &connection,
        vec![item(EPISODE, "episode"), item(FILM, "movie")],
    )
    .await
    .expect("items");
    item_marker::Entity::insert_many([
        work_marker("credits", 2_233_609, 2_397_344, true, 4),
        work_marker("intro", 136_180, 176_381, false, 5),
    ])
    .exec(&connection)
    .await
    .expect("markers");
    (SqlMetadataRepository::new(connection.clone()), connection)
}

async fn film(repository: &SqlMetadataRepository) -> MetadataItem {
    repository
        .items_by_id(&[RatingKey::new(FILM)])
        .await
        .unwrap()
        .remove(0)
}

fn bookmark(
    film: &MetadataItem,
    viewer: UserId,
    start: i64,
    end: Option<i64>,
    title: Option<&str>,
) -> NewMarker {
    NewMarker::new(
        film,
        MarkerKind::Bookmark,
        Some(viewer),
        MarkerValues {
            start_ms: start,
            end_ms: end,
            is_final: false,
            title: title.map(str::to_owned),
            color: None,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn the_works_markers_read_back_in_the_order_they_were_found() {
    let (repository, _) = repository().await;
    let markers = repository
        .markers(&[RatingKey::new(EPISODE)], None)
        .await
        .unwrap();
    let read: Vec<_> = markers
        .iter()
        .map(|marker| {
            (
                marker.kind,
                marker.start_ms,
                marker.end_ms,
                marker.is_final,
                marker.version,
                marker.owner,
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            (
                MarkerKind::Credits,
                2_233_609,
                Some(2_397_344),
                true,
                Some(4),
                None
            ),
            (
                MarkerKind::Intro,
                136_180,
                Some(176_381),
                false,
                Some(5),
                None
            ),
        ]
    );
}

#[tokio::test]
async fn a_bookmark_is_seen_by_its_owner_and_by_nobody_else() {
    let (repository, _) = repository().await;
    let film = film(&repository).await;
    let (added, new) = repository
        .add_marker(&bookmark(&film, VIEWER, 10_000, Some(20_000), Some("Spot")))
        .await
        .unwrap();
    assert!(new);
    assert_eq!(
        (added.owner, added.title.as_deref()),
        (Some(VIEWER), Some("Spot"))
    );

    let seen = |viewer| {
        let repository = &repository;
        async move {
            repository
                .markers(&[RatingKey::new(FILM)], viewer)
                .await
                .unwrap()
                .len()
        }
    };
    assert_eq!(seen(Some(VIEWER)).await, 1);
    assert_eq!(seen(Some(OTHER)).await, 0);
    assert_eq!(seen(Some(UserId::new(1))).await, 0);
    assert_eq!(seen(None).await, 0);
}

#[tokio::test]
async fn an_identical_add_returns_the_bookmark_already_there_unchanged() {
    let (repository, _) = repository().await;
    let film = film(&repository).await;
    let (first, _) = repository
        .add_marker(&bookmark(&film, VIEWER, 42_000, Some(50_000), None))
        .await
        .unwrap();
    let (again, new) = repository
        .add_marker(&bookmark(&film, VIEWER, 42_000, Some(50_000), Some("Y")))
        .await
        .unwrap();
    assert!(!new);
    assert_eq!((again.id, again.title), (first.id, None));

    // A start alone is another span, and another viewer's bookmark another marker.
    let (point, new) = repository
        .add_marker(&bookmark(&film, VIEWER, 42_000, None, None))
        .await
        .unwrap();
    assert!(new && point.id != first.id);
    let (theirs, new) = repository
        .add_marker(&bookmark(&film, OTHER, 42_000, Some(50_000), None))
        .await
        .unwrap();
    assert!(new && theirs.id != first.id);
}

#[tokio::test]
async fn an_update_writes_the_values_and_leaves_kind_and_owner() {
    let (repository, _) = repository().await;
    let intro = repository
        .markers(&[RatingKey::new(EPISODE)], None)
        .await
        .unwrap()
        .remove(1);
    let moved = intro
        .edited(MarkerValues {
            start_ms: 140_000,
            ..intro.values()
        })
        .unwrap();
    let stored = repository.update_marker(&moved).await.unwrap().unwrap();
    assert_eq!(
        (stored.kind, stored.owner, stored.start_ms, stored.version),
        (MarkerKind::Intro, None, 140_000, None)
    );
    assert_eq!(repository.marker(intro.id).await.unwrap(), Some(stored));

    let gone = open_plex_core::marker::Marker {
        id: MarkerId::new(9_999),
        ..moved
    };
    assert_eq!(repository.update_marker(&gone).await.unwrap(), None);
}

#[tokio::test]
async fn a_delete_removes_one_marker_once() {
    let (repository, _) = repository().await;
    let credits = repository
        .markers(&[RatingKey::new(EPISODE)], None)
        .await
        .unwrap()
        .remove(0);
    assert!(repository.delete_marker(credits.id).await.unwrap());
    assert!(!repository.delete_marker(credits.id).await.unwrap());
    assert_eq!(repository.marker(credits.id).await.unwrap(), None);
    assert_eq!(
        repository
            .markers(&[RatingKey::new(EPISODE)], None)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn no_marker_goes_with_the_items_a_reimport_clears() {
    let (repository, connection) = repository().await;
    let film = film(&repository).await;
    repository
        .add_marker(&bookmark(&film, VIEWER, 5_000, None, None))
        .await
        .unwrap();
    // A re-import clears the sections, and every item with them; it deletes the work's
    // markers itself, and a viewer's bookmarks stay.
    library_section::Entity::delete_many()
        .exec(&connection)
        .await
        .unwrap();
    assert_eq!(
        item_marker::Entity::find()
            .all(&connection)
            .await
            .unwrap()
            .len(),
        3
    );
}
