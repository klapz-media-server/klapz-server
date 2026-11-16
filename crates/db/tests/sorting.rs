//! Listing orders over nullable and computed values: where absence sorts, how ties break, and
//! that cursors walk each order without skipping or repeating an item.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::edit::CalendarDate;
use open_plex_core::metadata::{ItemFilter, ItemKind, ItemQuery, ItemSort, MetadataRepository};
use open_plex_core::page::{Direction, Window};
use open_plex_core::{SectionId, UserId};
use open_plex_db::entity::{library_section, media, metadata_item, user_rating, watch_state};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILMS: i64 = 4;
const SERIES: i64 = 5;

struct Film {
    id: i64,
    title: &'static str,
    rating: Option<f64>,
    released_on: Option<&'static str>,
    added_at: i64,
    /// `(height, duration)` of each version.
    versions: &'static [(i32, i64)],
}

const CATALOG: &[Film] = &[
    Film {
        id: 1,
        title: "Alien",
        rating: Some(8.5),
        released_on: Some("1979-05-25"),
        added_at: 100,
        versions: &[(1080, 7_000)],
    },
    Film {
        id: 2,
        title: "Brazil",
        rating: None,
        released_on: None,
        added_at: 100,
        versions: &[(720, 5_000), (2160, 9_000)],
    },
    Film {
        id: 3,
        title: "Casablanca",
        rating: Some(8.5),
        released_on: Some("1942-11-26"),
        added_at: 200,
        versions: &[(1080, 6_000)],
    },
    Film {
        id: 4,
        title: "Dune",
        rating: None,
        released_on: Some("2021-10-22"),
        added_at: 300,
        versions: &[],
    },
    Film {
        id: 5,
        title: "Eraserhead",
        rating: Some(6.1),
        released_on: None,
        added_at: 100,
        versions: &[(480, 8_000)],
    },
];

fn item(
    id: i64,
    section: i64,
    kind: &str,
    parent: Option<i64>,
    title: &str,
    added_at: i64,
) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(section),
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
        added_at: ActiveValue::Set(added_at),
        updated_at: ActiveValue::Set(0),
        refreshed_at: ActiveValue::Set(None),
    }
}

fn section(id: i64, kind: &str) -> library_section::ActiveModel {
    library_section::ActiveModel {
        id: ActiveValue::Set(id),
        kind: ActiveValue::Set(kind.to_owned()),
        title: ActiveValue::Set(kind.to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("en-US".to_owned()),
        uuid: ActiveValue::Set(format!("uuid-{id}")),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    }
}

fn version(id: i64, item: i64, height: i32, duration_ms: i64) -> media::ActiveModel {
    media::ActiveModel {
        id: ActiveValue::Set(id),
        metadata_item_id: ActiveValue::Set(item),
        width: ActiveValue::Set(Some(height * 16 / 9)),
        height: ActiveValue::Set(Some(height)),
        aspect_ratio: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(Some(duration_ms)),
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
    }
}

fn viewed(viewer: i64, item: i64, at: i64) -> watch_state::ActiveModel {
    watch_state::ActiveModel {
        viewer: ActiveValue::Set(viewer),
        guid: ActiveValue::Set(format!("plex://movie/{item}")),
        view_count: ActiveValue::Set(1),
        offset_ms: ActiveValue::Set(None),
        last_viewed_at: ActiveValue::Set(Some(at)),
    }
}

async fn seed(connection: &DatabaseConnection) {
    library_section::Entity::insert_many([section(FILMS, "movie"), section(SERIES, "show")])
        .exec(connection)
        .await
        .expect("sections");

    let mut items: Vec<metadata_item::ActiveModel> = CATALOG
        .iter()
        .map(|film| {
            let mut model = item(film.id, FILMS, "movie", None, film.title, film.added_at);
            model.rating = ActiveValue::Set(film.rating);
            model.released_on = ActiveValue::Set(
                film.released_on
                    .map(|date| CalendarDate::parse(date).unwrap().midnight()),
            );
            model
        })
        .collect();
    // Alpha's episodes came first and last, Beta's between them; Gamma has none.
    items.extend([
        item(50, SERIES, "show", None, "Alpha", 1),
        item(51, SERIES, "season", Some(50), "", 1),
        item(52, SERIES, "episode", Some(51), "Pilot", 10),
        item(53, SERIES, "episode", Some(51), "Finale", 90),
        item(60, SERIES, "show", None, "Beta", 2),
        item(61, SERIES, "season", Some(60), "", 2),
        item(62, SERIES, "episode", Some(61), "Only", 50),
        item(70, SERIES, "show", None, "Gamma", 3),
    ]);
    write::insert_items(connection, items).await.expect("items");

    let versions: Vec<media::ActiveModel> = CATALOG
        .iter()
        .flat_map(|film| {
            film.versions
                .iter()
                .zip(0..)
                .map(|((height, duration), n)| {
                    version(film.id * 10 + n, film.id, *height, *duration)
                })
        })
        .collect();
    media::Entity::insert_many(versions)
        .exec(connection)
        .await
        .expect("versions");

    // User 1 watched Casablanca, then Alien; user 7 watched Dune only.
    watch_state::Entity::insert_many([viewed(1, 3, 50), viewed(1, 1, 70), viewed(7, 4, 10)])
        .exec(connection)
        .await
        .expect("watch state");

    // User 1 rated Alien and Eraserhead alike and Casablanca lower, and cleared Brazil; user 7
    // rated Dune only.
    let rated = |viewer: i64, item: i64, rating: Option<f64>| user_rating::ActiveModel {
        viewer: ActiveValue::Set(viewer),
        guid: ActiveValue::Set(format!("plex://movie/{item}")),
        rating: ActiveValue::Set(rating),
        rated_at: ActiveValue::Set(1),
    };
    user_rating::Entity::insert_many([
        rated(1, 1, Some(8.0)),
        rated(1, 5, Some(8.0)),
        rated(1, 3, Some(5.0)),
        rated(1, 2, None),
        rated(7, 4, Some(9.0)),
    ])
    .exec(connection)
    .await
    .expect("ratings");
}

async fn repository() -> SqlMetadataRepository {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    SqlMetadataRepository::new(connection)
}

fn films(sort: ItemSort, direction: Direction) -> ItemQuery {
    ItemQuery::new(SectionId::new(FILMS))
        .of_kind(ItemKind::Movie)
        .sorted_by(sort, direction)
}

/// The whole listing in one page.
async fn listed(repository: &SqlMetadataRepository, query: &ItemQuery) -> Vec<String> {
    let window = Window::new(None, None, Some(50), None).expect("window");
    let page = repository.items(query, window).await.expect("page");
    page.items
        .into_iter()
        .map(|listed| listed.item.title.unwrap_or_default())
        .collect()
}

/// The listing walked one item at a time, forward from each cursor.
async fn walked_forward(repository: &SqlMetadataRepository, query: &ItemQuery) -> Vec<String> {
    let mut titles = Vec::new();
    let mut after = None;
    loop {
        let window = Window::new(after, None, Some(1), None).expect("window");
        let page = repository.items(query, window).await.expect("page");
        let Some(listed) = page.items.into_iter().next() else {
            return titles;
        };
        titles.push(listed.item.title.unwrap_or_default());
        after = Some(listed.cursor);
        assert!(
            titles.len() <= CATALOG.len(),
            "a cursor repeats: {titles:?}"
        );
    }
}

/// The listing walked one item at a time, backward from each cursor.
async fn walked_backward(repository: &SqlMetadataRepository, query: &ItemQuery) -> Vec<String> {
    let mut titles = Vec::new();
    let mut before = None;
    loop {
        let window = Window::new(None, before, None, Some(1)).expect("window");
        let page = repository.items(query, window).await.expect("page");
        let Some(listed) = page.items.into_iter().next() else {
            titles.reverse();
            return titles;
        };
        titles.push(listed.item.title.unwrap_or_default());
        before = Some(listed.cursor);
        assert!(
            titles.len() <= CATALOG.len(),
            "a cursor repeats: {titles:?}"
        );
    }
}

/// Asserts the order, and that both walks reproduce it.
async fn assert_order(sort: ItemSort, direction: Direction, expected: &[&str]) {
    let repository = repository().await;
    let query = films(sort, direction);
    assert_eq!(
        listed(&repository, &query).await,
        expected,
        "{sort:?} {direction:?}"
    );
    assert_eq!(
        walked_forward(&repository, &query).await,
        expected,
        "forward {sort:?} {direction:?}"
    );
    assert_eq!(
        walked_backward(&repository, &query).await,
        expected,
        "backward {sort:?} {direction:?}"
    );
}

#[tokio::test]
async fn an_absent_rating_sorts_first_ascending_and_last_descending() {
    // Ties in title order both ways: Alien and Casablanca share 8.5, Brazil and Dune have none.
    assert_order(
        ItemSort::Rating,
        Direction::Ascending,
        &["Brazil", "Dune", "Eraserhead", "Alien", "Casablanca"],
    )
    .await;
    assert_order(
        ItemSort::Rating,
        Direction::Descending,
        &["Alien", "Casablanca", "Eraserhead", "Brazil", "Dune"],
    )
    .await;
}

#[tokio::test]
async fn orders_by_release_date() {
    assert_order(
        ItemSort::ReleasedOn,
        Direction::Ascending,
        &["Brazil", "Eraserhead", "Casablanca", "Alien", "Dune"],
    )
    .await;
    assert_order(
        ItemSort::ReleasedOn,
        Direction::Descending,
        &["Dune", "Alien", "Casablanca", "Brazil", "Eraserhead"],
    )
    .await;
}

#[tokio::test]
async fn ties_on_the_date_added_follow_the_title() {
    assert_order(
        ItemSort::AddedAt,
        Direction::Descending,
        &["Dune", "Casablanca", "Alien", "Brazil", "Eraserhead"],
    )
    .await;
}

#[tokio::test]
async fn a_film_ranks_by_its_smallest_version_ascending_and_its_largest_descending() {
    // Brazil has a 720-line and a 2160-line version; Dune has none.
    assert_order(
        ItemSort::Height,
        Direction::Ascending,
        &["Dune", "Eraserhead", "Brazil", "Alien", "Casablanca"],
    )
    .await;
    assert_order(
        ItemSort::Height,
        Direction::Descending,
        &["Brazil", "Alien", "Casablanca", "Eraserhead", "Dune"],
    )
    .await;
    assert_order(
        ItemSort::Duration,
        Direction::Ascending,
        &["Dune", "Brazil", "Casablanca", "Alien", "Eraserhead"],
    )
    .await;
    assert_order(
        ItemSort::Duration,
        Direction::Descending,
        &["Brazil", "Eraserhead", "Alien", "Casablanca", "Dune"],
    )
    .await;
}

#[tokio::test]
async fn orders_by_when_this_viewer_last_played() {
    assert_order(
        ItemSort::LastViewedAt(Some(UserId::new(1))),
        Direction::Descending,
        &["Alien", "Casablanca", "Brazil", "Dune", "Eraserhead"],
    )
    .await;
    // Another viewer's history is not user 1's.
    assert_order(
        ItemSort::LastViewedAt(Some(UserId::new(7))),
        Direction::Descending,
        &["Dune", "Alien", "Brazil", "Casablanca", "Eraserhead"],
    )
    .await;
    // A caller with no watch state has played nothing: every film ties, in title order.
    assert_order(
        ItemSort::LastViewedAt(None),
        Direction::Descending,
        &["Alien", "Brazil", "Casablanca", "Dune", "Eraserhead"],
    )
    .await;
}

#[tokio::test]
async fn orders_by_this_viewers_rating_unrated_first_and_ties_by_title() {
    // Measured on the reference: unrated first ascending, last descending, and ties in the
    // same order both ways. A cleared rating is no rating.
    assert_order(
        ItemSort::UserRating(Some(UserId::new(1))),
        Direction::Ascending,
        &["Brazil", "Dune", "Casablanca", "Alien", "Eraserhead"],
    )
    .await;
    assert_order(
        ItemSort::UserRating(Some(UserId::new(1))),
        Direction::Descending,
        &["Alien", "Eraserhead", "Casablanca", "Brazil", "Dune"],
    )
    .await;
    assert_order(
        ItemSort::UserRating(Some(UserId::new(7))),
        Direction::Descending,
        &["Dune", "Alien", "Brazil", "Casablanca", "Eraserhead"],
    )
    .await;
    assert_order(
        ItemSort::UserRating(None),
        Direction::Descending,
        &["Alien", "Brazil", "Casablanca", "Dune", "Eraserhead"],
    )
    .await;
}

#[tokio::test]
async fn a_shuffle_is_the_same_for_a_seed_and_differs_across_seeds() {
    let repository = repository().await;
    let mut orders = Vec::new();
    for seed in 0..8 {
        let query = films(ItemSort::Shuffled(seed), Direction::Ascending);
        let order = listed(&repository, &query).await;
        assert_eq!(listed(&repository, &query).await, order);
        assert_eq!(walked_forward(&repository, &query).await, order);
        assert_eq!(walked_backward(&repository, &query).await, order);
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            ["Alien", "Brazil", "Casablanca", "Dune", "Eraserhead"]
        );
        orders.push(order);
    }
    orders.sort();
    orders.dedup();
    assert!(orders.len() > 4, "eight seeds gave {} orders", orders.len());
}

#[tokio::test]
async fn a_series_ranks_by_its_first_episode_ascending_and_its_latest_descending() {
    let repository = repository().await;
    let query = ItemQuery::new(SectionId::new(SERIES))
        .filtered(ItemFilter::default())
        .sorted_by(ItemSort::EpisodeAddedAt, Direction::Descending);
    assert_eq!(
        listed(&repository, &query).await,
        ["Alpha", "Beta", "Gamma"]
    );
    let query = query.sorted_by(ItemSort::EpisodeAddedAt, Direction::Ascending);
    // By the first episodes, Alpha's at 10 comes before Beta's at 50.
    assert_eq!(
        listed(&repository, &query).await,
        ["Gamma", "Alpha", "Beta"]
    );
}
