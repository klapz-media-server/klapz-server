//! Storage tests for the viewing history: written with the watch state, read per viewer or
//! for everyone, by item and its episodes, by library, kind and date, in three orders.
//!
//! Items are the reference's test libraries: Bull, one season of two episodes, in library
//! 28, and Soul in library 27.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::history::{
    HistoryEntry, HistoryOrder, HistoryQuery, HistoryScope, NewViewing, ViewerCount, ViewingKind,
};
use open_plex_core::page::{Direction, Window};
use open_plex_core::watch::{WatchRepository, WatchState};
use open_plex_core::{HistoryId, RatingKey, SectionId, UserId};
use open_plex_db::entity::{library_section, metadata_item};
use open_plex_db::write;
use open_plex_db::SqlWatchRepository;
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

const SOUL: i64 = 577_986;
const SHOW: i64 = 577_987;
const SEASON: i64 = 577_988;
const EPISODE_1: i64 = 577_989;
const EPISODE_2: i64 = 577_990;
const NOW: i64 = 1_794_318_000;

fn guid(id: i64) -> String {
    format!("tv.plex.agents.none://{id}")
}

fn item(id: i64, section: i64, kind: &str, parent: Option<i64>) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(section),
        parent_id: ActiveValue::Set(parent),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(guid(id)),
        title: ActiveValue::Set(Some(format!("item {id}"))),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set(format!("item {id}")),
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

fn section(id: i64, kind: &str) -> library_section::ActiveModel {
    library_section::ActiveModel {
        id: ActiveValue::Set(id),
        kind: ActiveValue::Set(kind.to_owned()),
        title: ActiveValue::Set(format!("open-plex tests {id}")),
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

async fn repository() -> (SqlWatchRepository, DatabaseConnection) {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    library_section::Entity::insert_many([section(27, "movie"), section(28, "show")])
        .exec(&connection)
        .await
        .expect("sections");
    write::insert_items(
        &connection,
        vec![
            item(SOUL, 27, "movie", None),
            item(SHOW, 28, "show", None),
            item(SEASON, 28, "season", Some(SHOW)),
            item(EPISODE_1, 28, "episode", Some(SEASON)),
            item(EPISODE_2, 28, "episode", Some(SEASON)),
        ],
    )
    .await
    .expect("items");
    (SqlWatchRepository::new(connection.clone()), connection)
}

fn viewing(item: i64, kind: ViewingKind, viewed_at: i64) -> NewViewing {
    NewViewing {
        item: RatingKey::new(item),
        kind,
        viewed_at,
    }
}

const fn watched_once() -> WatchState {
    WatchState {
        view_count: 1,
        offset_ms: None,
        last_viewed_at: Some(NOW + 30),
    }
}

/// User 1 played Soul, marked the season's two episodes, then played episode 1; user
/// 7 played Soul.
async fn watch(repository: &SqlWatchRepository) {
    let user_one = UserId::new(1);
    repository
        .record_viewings(
            user_one,
            &[(guid(SOUL), watched_once())],
            &[viewing(SOUL, ViewingKind::Played, NOW + 30)],
        )
        .await
        .unwrap();
    let marked = [
        viewing(EPISODE_1, ViewingKind::Marked, NOW + 10),
        viewing(EPISODE_2, ViewingKind::Marked, NOW + 10),
    ];
    repository
        .record_viewings(user_one, &[], &marked)
        .await
        .unwrap();
    let played = [viewing(EPISODE_1, ViewingKind::Played, NOW + 20)];
    repository
        .record_viewings(user_one, &[], &played)
        .await
        .unwrap();
    let theirs = [viewing(SOUL, ViewingKind::Played, NOW)];
    repository
        .record_viewings(UserId::new(7), &[], &theirs)
        .await
        .unwrap();
}

fn first(count: u32) -> Window {
    Window::by_id(None, None, Some(count), None).unwrap()
}

fn ids(entries: &[HistoryEntry]) -> Vec<i64> {
    entries.iter().map(|entry| entry.item.get()).collect()
}

async fn items(repository: &SqlWatchRepository, query: &HistoryQuery) -> Vec<i64> {
    ids(&repository.history(query, first(50)).await.unwrap().items)
}

const fn user_one() -> HistoryQuery {
    HistoryQuery::of(HistoryScope::Viewer(UserId::new(1)))
}

#[tokio::test]
async fn writes_the_state_and_the_entries_together() {
    let (repository, _) = repository().await;
    watch(&repository).await;
    assert_eq!(
        repository
            .states(UserId::new(1), &[guid(SOUL)])
            .await
            .unwrap(),
        vec![(guid(SOUL), watched_once())]
    );
    assert_eq!(
        items(&repository, &user_one()).await,
        vec![SOUL, EPISODE_1, EPISODE_2, EPISODE_1]
    );
    assert_eq!(
        items(&repository, &HistoryQuery::of(HistoryScope::Everyone)).await,
        vec![SOUL, EPISODE_1, EPISODE_2, EPISODE_1, SOUL]
    );
    let theirs = HistoryQuery::of(HistoryScope::Viewer(UserId::new(7)));
    let entry = repository.history(&theirs, first(5)).await.unwrap().items[0];
    assert_eq!(entry.section, SectionId::new(27));
    assert_eq!(entry.kind, ViewingKind::Played);
    assert_eq!(entry.viewer, UserId::new(7));
}

#[tokio::test]
async fn a_series_or_a_season_reads_its_episodes_entries() {
    let (repository, _) = repository().await;
    watch(&repository).await;
    for parent in [SHOW, SEASON] {
        let mut query = user_one();
        query.item = Some(RatingKey::new(parent));
        assert_eq!(
            items(&repository, &query).await,
            vec![EPISODE_1, EPISODE_2, EPISODE_1]
        );
        assert_eq!(repository.history_count(&query).await.unwrap(), 3);
    }
    let mut query = user_one();
    query.item = Some(RatingKey::new(EPISODE_2));
    assert_eq!(items(&repository, &query).await, vec![EPISODE_2]);
}

#[tokio::test]
async fn filters_by_library_kind_and_date() {
    let (repository, _) = repository().await;
    watch(&repository).await;
    let mut query = user_one();
    query.sections = Some(vec![SectionId::new(27)]);
    assert_eq!(items(&repository, &query).await, vec![SOUL]);

    let mut query = user_one();
    query.kind = Some(ViewingKind::Played);
    assert_eq!(items(&repository, &query).await, vec![SOUL, EPISODE_1]);

    let mut query = user_one();
    query.viewed_since = Some(NOW + 10);
    query.viewed_until = Some(NOW + 20);
    assert_eq!(
        items(&repository, &query).await,
        vec![EPISODE_1, EPISODE_2, EPISODE_1]
    );
    assert_eq!(repository.history_count(&query).await.unwrap(), 3);
}

#[tokio::test]
async fn pages_through_an_order_by_date_from_either_end() {
    let (repository, _) = repository().await;
    watch(&repository).await;
    let mut query = user_one();
    query.order = HistoryOrder::ViewedAt;
    query.direction = Direction::Descending;
    // Ties on the date go to the order written, reversed with the rest.
    assert_eq!(
        items(&repository, &query).await,
        vec![SOUL, EPISODE_1, EPISODE_2, EPISODE_1]
    );

    let page = repository.history(&query, first(2)).await.unwrap();
    assert!(page.has_next);
    let after = query.cursor(&page.items[1]);
    let rest = repository
        .history(
            &query,
            Window::new(Some(after), None, Some(5), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&rest.items), vec![EPISODE_2, EPISODE_1]);
    assert!(!rest.has_next);

    let before = query.cursor(&rest.items[0]);
    let back = repository
        .history(
            &query,
            Window::new(None, Some(before), None, Some(1)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&back.items), vec![EPISODE_1]);
    assert!(back.has_previous);

    let last = repository
        .history(&query, Window::new(None, None, None, Some(3)).unwrap())
        .await
        .unwrap();
    assert_eq!(ids(&last.items), vec![EPISODE_1, EPISODE_2, EPISODE_1]);
    assert!(last.has_previous);
}

#[tokio::test]
async fn orders_by_library_and_lands_at_an_offset() {
    let (repository, _) = repository().await;
    watch(&repository).await;
    let mut query = user_one();
    query.order = HistoryOrder::Library;
    query.direction = Direction::Descending;
    assert_eq!(
        items(&repository, &query).await,
        vec![EPISODE_1, EPISODE_2, EPISODE_1, SOUL]
    );
    let skipped = repository
        .history(&query, first(2).skipping(3))
        .await
        .unwrap();
    assert_eq!(ids(&skipped.items), vec![SOUL]);
    assert!(skipped.has_previous);
}

#[tokio::test]
async fn deleting_an_entry_leaves_the_state_and_an_entry_goes_with_its_item() {
    let (repository, connection) = repository().await;
    watch(&repository).await;
    let soul = repository
        .history(&user_one(), first(1))
        .await
        .unwrap()
        .items[0];
    assert_eq!(repository.history_entry(soul.id).await.unwrap(), Some(soul));
    assert!(repository.delete_history_entry(soul.id).await.unwrap());
    assert!(!repository.delete_history_entry(soul.id).await.unwrap());
    assert_eq!(repository.history_entry(soul.id).await.unwrap(), None);
    assert_eq!(
        repository
            .states(UserId::new(1), &[guid(SOUL)])
            .await
            .unwrap(),
        vec![(guid(SOUL), watched_once())]
    );
    assert_eq!(
        repository.history_entry(HistoryId::new(404)).await.unwrap(),
        None
    );

    metadata_item::Entity::delete_many()
        .filter(metadata_item::Column::Id.eq(SOUL))
        .exec(&connection)
        .await
        .unwrap();
    let everyone = items(&repository, &HistoryQuery::of(HistoryScope::Everyone)).await;
    assert_eq!(everyone, vec![EPISODE_1, EPISODE_2, EPISODE_1]);
}

fn ranking(page: &open_plex_core::Page<ViewerCount>) -> Vec<(i64, u64)> {
    page.items
        .iter()
        .map(|count| (count.viewer.get(), count.viewings))
        .collect()
}

#[tokio::test]
async fn ranks_viewers_by_viewings_counting_a_series_episodes() {
    let (repository, _) = repository().await;
    watch(&repository).await;
    let page = |items: Vec<i64>, scope| {
        let items: Vec<RatingKey> = items.into_iter().map(RatingKey::new).collect();
        let repository = &repository;
        async move { repository.viewers(&items, scope, first(10)).await.unwrap() }
    };

    // Soul: one viewing each, ties by user.
    let soul = page(vec![SOUL], HistoryScope::Everyone).await;
    assert_eq!(ranking(&soul), [(1, 1), (7, 1)]);
    // The series and its season count the episodes' three viewings; an episode its own.
    for parent in [SHOW, SEASON] {
        assert_eq!(
            ranking(&page(vec![parent], HistoryScope::Everyone).await),
            [(1, 3)]
        );
    }
    assert_eq!(
        ranking(&page(vec![EPISODE_1], HistoryScope::Everyone).await),
        [(1, 2)]
    );
    // Several copies add up, and a scope keeps one viewer.
    let both = page(vec![SOUL, SHOW], HistoryScope::Everyone).await;
    assert_eq!(ranking(&both), [(1, 4), (7, 1)]);
    let theirs = page(vec![SOUL, SHOW], HistoryScope::Viewer(UserId::new(7))).await;
    assert_eq!(ranking(&theirs), [(7, 1)]);
    assert!(page(vec![SOUL], HistoryScope::Viewer(UserId::new(2)))
        .await
        .items
        .is_empty());
}

#[tokio::test]
async fn a_ranking_resumes_after_a_cursor_in_either_direction() {
    let (repository, _) = repository().await;
    watch(&repository).await;
    let items = [RatingKey::new(SOUL), RatingKey::new(SHOW)];
    let head = repository
        .viewers(&items, HistoryScope::Everyone, first(1))
        .await
        .unwrap();
    assert_eq!(ranking(&head), [(1, 4)]);
    assert!(head.has_next);
    let after = Window::new(Some(head.items[0].cursor()), None, Some(5), None).unwrap();
    let tail = repository
        .viewers(&items, HistoryScope::Everyone, after)
        .await
        .unwrap();
    assert_eq!(ranking(&tail), [(7, 1)]);
    assert!(!tail.has_next);
    let before = Window::new(None, Some(tail.items[0].cursor()), None, Some(5)).unwrap();
    let back = repository
        .viewers(&items, HistoryScope::Everyone, before)
        .await
        .unwrap();
    assert_eq!(ranking(&back), [(1, 4)]);
}
