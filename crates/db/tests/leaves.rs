//! Storage tests for a library's leaves and the order episodes are read in.
//!
//! The fixture holds what makes that order more than "by series": two series sharing a sort
//! title, whose episodes interleave as on the reference, and episodes with no number.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::metadata::{
    ItemDepth, ItemFilter, ItemKind, ItemQuery, ItemSort, MetadataRepository,
};
use open_plex_core::page::{Direction, Window};
use open_plex_core::SectionId;
use open_plex_db::entity::{library_section, metadata_item};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, EntityTrait};

const SECTION: i64 = 5;

struct Row<'a> {
    id: i64,
    kind: &'a str,
    parent: Option<i64>,
    title: &'a str,
    position: Option<i32>,
    released_on: Option<i64>,
}

fn row(row: &Row<'_>) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(row.id),
        library_section_id: ActiveValue::Set(SECTION),
        parent_id: ActiveValue::Set(row.parent),
        kind: ActiveValue::Set(row.kind.to_owned()),
        guid: ActiveValue::Set(format!("plex://{}/{}", row.kind, row.id)),
        title: ActiveValue::Set((!row.title.is_empty()).then(|| row.title.to_owned())),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set(row.title.to_lowercase()),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(row.position),
        year: ActiveValue::Set(None),
        released_on: ActiveValue::Set(row.released_on),
        duration_ms: ActiveValue::Set(None),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        refreshed_at: ActiveValue::Set(None),
    }
}

fn series(id: i64, title: &str) -> metadata_item::ActiveModel {
    row(&Row {
        id,
        kind: "show",
        parent: None,
        title,
        position: None,
        released_on: None,
    })
}

fn season(id: i64, show: i64, number: Option<i32>) -> metadata_item::ActiveModel {
    row(&Row {
        id,
        kind: "season",
        parent: Some(show),
        title: "",
        position: number,
        released_on: None,
    })
}

fn episode(
    id: i64,
    season: i64,
    title: &str,
    number: Option<i32>,
    released_on: Option<i64>,
) -> metadata_item::ActiveModel {
    row(&Row {
        id,
        kind: "episode",
        parent: Some(season),
        title,
        position: number,
        released_on,
    })
}

async fn repository() -> SqlMetadataRepository {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(SECTION),
        kind: ActiveValue::Set("show".to_owned()),
        title: ActiveValue::Set("Series".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("en-US".to_owned()),
        uuid: ActiveValue::Set("uuid-5".to_owned()),
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
        vec![
            // Ids out of title order, so that an order by id would show.
            series(60, "Alf"),
            season(61, 60, Some(1)),
            episode(62, 61, "Zeta", Some(1), None),
            // Two series titled alike: their episodes interleave by number.
            series(40, "Good Doctor"),
            season(41, 40, Some(2)),
            season(42, 40, Some(1)),
            episode(43, 41, "Second season opener", Some(1), None),
            episode(44, 42, "Burnt Food", Some(2), None),
            episode(45, 42, "Pilot", Some(1), None),
            series(50, "Good Doctor"),
            season(51, 50, Some(1)),
            episode(52, 51, "Episode 1", Some(1), None),
            // Unnumbered episodes come after the numbered ones of their season, in release
            // order, and so before the next season of the series titled alike.
            episode(53, 51, "Special B", None, Some(2_000)),
            episode(54, 51, "Special A", None, Some(1_000)),
            // A season without a number comes after the numbered ones.
            season(55, 50, None),
            episode(56, 55, "Extra", Some(1), None),
        ],
    )
    .await
    .expect("items");
    SqlMetadataRepository::new(connection)
}

fn leaves() -> ItemQuery {
    ItemQuery::new(SectionId::new(SECTION))
        .filtered(ItemFilter {
            depth: ItemDepth::Leaves,
            ..ItemFilter::default()
        })
        .sorted_by(ItemSort::Episodes, Direction::Ascending)
}

async fn ids(repository: &SqlMetadataRepository, query: &ItemQuery, window: Window) -> Vec<i64> {
    repository
        .items(query, window)
        .await
        .expect("items")
        .items
        .iter()
        .map(|listed| listed.item.id.get())
        .collect()
}

fn first(n: u32) -> Window {
    Window::new(None, None, Some(n), None).expect("window")
}

#[tokio::test]
async fn reads_episodes_by_series_season_and_number_interleaving_alike_series() {
    let repository = repository().await;
    assert_eq!(
        ids(&repository, &leaves(), first(50)).await,
        [62, 52, 45, 44, 54, 53, 43, 56]
    );
}

#[tokio::test]
async fn pages_through_the_episode_order_by_cursor() {
    let repository = repository().await;
    let query = leaves();
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = repository
            .items(
                &query,
                Window::new(after, None, Some(3), None).expect("window"),
            )
            .await
            .expect("page");
        seen.extend(page.items.iter().map(|listed| listed.item.id.get()));
        if !page.has_next {
            break;
        }
        after = page.items.last().map(|listed| listed.cursor.clone());
    }
    assert_eq!(seen, [62, 52, 45, 44, 54, 53, 43, 56]);
}

#[tokio::test]
async fn the_leaves_are_counted_as_listed() {
    let repository = repository().await;
    assert_eq!(repository.item_count(&leaves()).await.expect("count"), 8);
}

#[tokio::test]
async fn a_kind_narrows_the_leaves_rather_than_replacing_them() {
    let repository = repository().await;
    let narrowed = |kind| {
        ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
            depth: ItemDepth::Leaves,
            kind: Some(kind),
            ..ItemFilter::default()
        })
    };
    let count = |query| {
        let repository = &repository;
        async move { repository.item_count(&query).await.expect("count") }
    };
    assert_eq!(count(narrowed(ItemKind::Episode)).await, 8);
    assert_eq!(count(narrowed(ItemKind::Show)).await, 0);
    assert_eq!(count(narrowed(ItemKind::Season)).await, 0);
}

#[tokio::test]
async fn anything_but_an_episode_follows_its_title_in_the_episode_order() {
    let repository = repository().await;
    let query = ItemQuery::new(SectionId::new(SECTION))
        .of_kind(ItemKind::Show)
        .sorted_by(ItemSort::Episodes, Direction::Ascending);
    assert_eq!(ids(&repository, &query, first(10)).await, [60, 40, 50]);
}
