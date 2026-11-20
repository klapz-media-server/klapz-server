//! Listing by what a viewer watched: the rules measured on the reference's test libraries.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::metadata::{ItemFilter, ItemKind, ItemQuery, MetadataRepository};
use open_plex_core::page::Window;
use open_plex_core::user_rating::RatingCondition;
use open_plex_core::{SectionId, UserId};
use open_plex_db::entity::{library_section, metadata_item, user_rating, watch_state};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const LIBRARY: i64 = 4;

fn item(id: i64, kind: &str, parent: Option<i64>, title: &str) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(LIBRARY),
        parent_id: ActiveValue::Set(parent),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("plex://{kind}/{id}")),
        title: ActiveValue::Set(Some(title.to_owned())),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set(title.to_lowercase()),
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

fn state(
    viewer: i64,
    guid: &str,
    view_count: i32,
    offset_ms: Option<i64>,
) -> watch_state::ActiveModel {
    watch_state::ActiveModel {
        viewer: ActiveValue::Set(viewer),
        guid: ActiveValue::Set(guid.to_owned()),
        view_count: ActiveValue::Set(view_count),
        offset_ms: ActiveValue::Set(offset_ms),
        last_viewed_at: ActiveValue::Set(Some(1)),
    }
}

/// The reference's test films and series, in the states the measurements put them in.
async fn seed(connection: &DatabaseConnection) {
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(LIBRARY),
        kind: ActiveValue::Set("movie".to_owned()),
        title: ActiveValue::Set("Tests".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("en-US".to_owned()),
        uuid: ActiveValue::Set("uuid-4".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(connection)
    .await
    .expect("section");
    write::insert_items(
        connection,
        vec![
            item(1, "movie", None, "Soul"),
            item(2, "movie", None, "Coco"),
            item(3, "movie", None, "Luca"),
            item(4, "movie", None, "Onward"),
            // Bull: both episodes watched. Dark: its one watched episode unmarked since, so
            // the series keeps its own viewing. Lost: one of two episodes watched.
            item(10, "show", None, "Bull"),
            item(11, "season", Some(10), "Season 5"),
            item(12, "episode", Some(11), "Bull 1"),
            item(13, "episode", Some(11), "Bull 2"),
            item(20, "show", None, "Dark"),
            item(21, "season", Some(20), "Season 1"),
            item(22, "episode", Some(21), "Dark 1"),
            item(30, "show", None, "Lost"),
            item(31, "season", Some(30), "Season 1"),
            item(32, "episode", Some(31), "Lost 1"),
            item(33, "episode", Some(31), "Lost 2"),
        ],
    )
    .await
    .expect("items");
    watch_state::Entity::insert_many([
        // Coco watched, Luca in progress, Onward watched then restarted; Soul untouched.
        state(1, "plex://movie/2", 1, None),
        state(1, "plex://movie/3", 0, Some(600_000)),
        state(1, "plex://movie/4", 1, Some(900_000)),
        state(1, "plex://show/10", 2, None),
        state(1, "plex://episode/12", 1, None),
        state(1, "plex://episode/13", 1, None),
        state(1, "plex://show/20", 1, None),
        state(1, "plex://episode/22", 0, None),
        state(1, "plex://show/30", 1, None),
        state(1, "plex://episode/32", 1, None),
        // Another viewer's state is not user 1's.
        state(7, "plex://movie/1", 1, None),
    ])
    .exec(connection)
    .await
    .expect("watch state");

    // Soul rated 8 and Coco 6, Luca's rating cleared; another viewer rated Onward.
    let rated = |viewer: i64, item: i64, rating: Option<f64>| user_rating::ActiveModel {
        viewer: ActiveValue::Set(viewer),
        guid: ActiveValue::Set(format!("plex://movie/{item}")),
        rating: ActiveValue::Set(rating),
        rated_at: ActiveValue::Set(1),
    };
    user_rating::Entity::insert_many([
        rated(1, 1, Some(8.0)),
        rated(1, 2, Some(6.0)),
        rated(1, 3, None),
        rated(7, 4, Some(9.0)),
    ])
    .exec(connection)
    .await
    .expect("ratings");
}

async fn titles(filter: ItemFilter) -> Vec<String> {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    let repository = SqlMetadataRepository::new(connection);
    let query = ItemQuery::new(SectionId::new(LIBRARY)).filtered(filter);
    let window = Window::new(None, None, Some(50), None).expect("window");
    let count = repository.item_count(&query).await.expect("count");
    let titles: Vec<String> = repository
        .items(&query, window)
        .await
        .expect("page")
        .items
        .into_iter()
        .map(|listed| listed.item.title.unwrap_or_default())
        .collect();
    assert_eq!(count, titles.len() as u64, "the count follows the filter");
    titles
}

fn films(viewer: Option<UserId>) -> ItemFilter {
    ItemFilter {
        kind: Some(ItemKind::Movie),
        viewer,
        ..ItemFilter::default()
    }
}

fn shows() -> ItemFilter {
    ItemFilter {
        kind: Some(ItemKind::Show),
        viewer: Some(UserId::new(1)),
        ..ItemFilter::default()
    }
}

const VIEWER: Option<UserId> = Some(UserId::new(1));

#[tokio::test]
async fn a_film_in_progress_is_unwatched_and_a_restarted_one_watched() {
    let unwatched = ItemFilter {
        watched: Some(false),
        ..films(VIEWER)
    };
    assert_eq!(titles(unwatched).await, ["Luca", "Soul"]);
    let watched = ItemFilter {
        watched: Some(true),
        ..films(VIEWER)
    };
    assert_eq!(titles(watched).await, ["Coco", "Onward"]);
}

#[tokio::test]
async fn in_progress_is_a_kept_position_whatever_the_viewings() {
    let started = ItemFilter {
        in_progress: Some(true),
        ..films(VIEWER)
    };
    assert_eq!(titles(started).await, ["Luca", "Onward"]);
    let not_started = ItemFilter {
        in_progress: Some(false),
        ..films(VIEWER)
    };
    assert_eq!(titles(not_started).await, ["Coco", "Soul"]);
    let both = ItemFilter {
        in_progress: Some(true),
        watched: Some(false),
        ..films(VIEWER)
    };
    assert_eq!(titles(both).await, ["Luca"]);
}

#[tokio::test]
async fn each_viewer_filters_on_its_own_state() {
    let watched = ItemFilter {
        watched: Some(true),
        ..films(Some(UserId::new(7)))
    };
    assert_eq!(titles(watched).await, ["Soul"]);
    // A caller with no watch state has watched nothing.
    let unwatched = ItemFilter {
        watched: Some(false),
        ..films(None)
    };
    assert_eq!(titles(unwatched).await, ["Coco", "Luca", "Onward", "Soul"]);
    let watched = ItemFilter {
        watched: Some(true),
        ..films(None)
    };
    assert!(titles(watched).await.is_empty());
}

#[tokio::test]
async fn a_series_is_watched_by_its_own_viewings() {
    // Dark's one episode was unmarked; the series stays watched, as on the reference.
    let watched = ItemFilter {
        watched: Some(true),
        ..shows()
    };
    assert_eq!(titles(watched).await, ["Bull", "Dark", "Lost"]);
}

#[tokio::test]
async fn a_series_with_an_unwatched_episode_has_unwatched_leaves() {
    let unwatched = ItemFilter {
        episodes_watched: Some(false),
        ..shows()
    };
    assert_eq!(titles(unwatched).await, ["Dark", "Lost"]);
    let complete = ItemFilter {
        episodes_watched: Some(true),
        ..shows()
    };
    assert_eq!(titles(complete).await, ["Bull"]);
}

#[tokio::test]
async fn a_last_viewed_range_keeps_what_was_played_within_it() {
    // Every state in the fixture was last played at second 1; Soul never.
    let within = ItemFilter {
        last_viewed_from: Some(0),
        last_viewed_to: Some(5),
        ..films(VIEWER)
    };
    assert_eq!(titles(within).await, ["Coco", "Luca", "Onward"]);
    let later = ItemFilter {
        last_viewed_from: Some(2),
        ..films(VIEWER)
    };
    assert!(titles(later).await.is_empty());
    let earlier = ItemFilter {
        last_viewed_to: Some(0),
        ..films(VIEWER)
    };
    assert!(titles(earlier).await.is_empty());
}

fn rated(condition: RatingCondition) -> ItemFilter {
    ItemFilter {
        user_rating: Some(condition),
        ..films(VIEWER)
    }
}

#[tokio::test]
async fn a_rating_filter_keeps_the_unrated_out_of_every_comparison() {
    assert_eq!(
        titles(rated(RatingCondition {
            any_of: vec![8.0],
            ..RatingCondition::default()
        }))
        .await,
        ["Soul"]
    );
    // `userRating=-1`: a cleared rating and another viewer's are none of the caller's.
    assert_eq!(
        titles(rated(RatingCondition {
            unrated: Some(true),
            ..RatingCondition::default()
        }))
        .await,
        ["Luca", "Onward"]
    );
    assert_eq!(
        titles(rated(RatingCondition {
            any_of: vec![6.0],
            unrated: Some(true),
            ..RatingCondition::default()
        }))
        .await,
        ["Coco", "Luca", "Onward"]
    );
    assert_eq!(
        titles(rated(RatingCondition {
            none_of: vec![8.0],
            ..RatingCondition::default()
        }))
        .await,
        ["Coco"]
    );
    assert_eq!(
        titles(rated(RatingCondition {
            at_most: Some(8.0),
            above: Some(6.0),
            ..RatingCondition::default()
        }))
        .await,
        ["Soul"]
    );
    assert_eq!(
        titles(rated(RatingCondition {
            unrated: Some(false),
            ..RatingCondition::default()
        }))
        .await,
        ["Coco", "Soul"]
    );
}

#[tokio::test]
async fn a_caller_with_no_watch_state_has_rated_nothing() {
    let unrated = ItemFilter {
        user_rating: Some(RatingCondition {
            unrated: Some(true),
            ..RatingCondition::default()
        }),
        ..films(None)
    };
    assert_eq!(titles(unrated).await, ["Coco", "Luca", "Onward", "Soul"]);
    let at_least = ItemFilter {
        user_rating: Some(RatingCondition {
            at_least: Some(0.0),
            ..RatingCondition::default()
        }),
        ..films(None)
    };
    assert!(titles(at_least).await.is_empty());
}
