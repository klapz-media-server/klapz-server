//! Stored playlists: ownership, order and moves, repeats, and what a reader may see.
//!
//! Items are the reference's test libraries: Soul in library 27, two episodes of Bull in 28.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::page::{Direction, Window};
use open_plex_core::playlist::{
    Destination, NewPlaylist, PlaylistEdit, PlaylistError, PlaylistField, PlaylistFilter,
    PlaylistKind, PlaylistQuery, PlaylistRepository, PlaylistSort, SmartRule,
};
use open_plex_core::watch::Viewer;
use open_plex_core::{PlaylistEntryId, RatingKey, SectionId, UserId};
use open_plex_db::entity::{library_section, metadata_item};
use open_plex_db::{write, SqlPlaylistRepository};
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILM: i64 = 577_986;
const EPISODE_1: i64 = 578_173;
const EPISODE_2: i64 = 577_990;
const NOW: i64 = 1_794_398_967;

fn section(id: i64, kind: &str) -> library_section::ActiveModel {
    library_section::ActiveModel {
        id: ActiveValue::Set(id),
        kind: ActiveValue::Set(kind.to_owned()),
        title: ActiveValue::Set(format!("library {id}")),
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

fn item(id: i64, section: i64, kind: &str, duration_ms: i64) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(section),
        parent_id: ActiveValue::Set(None),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("tv.plex.agents.none://{id}")),
        title: ActiveValue::Set(Some(format!("item {id}"))),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set(format!("item {id}")),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(None),
        year: ActiveValue::Set(None),
        released_on: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(Some(duration_ms)),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
    }
}

async fn seed(connection: &DatabaseConnection) {
    library_section::Entity::insert_many([section(27, "movie"), section(28, "show")])
        .exec(connection)
        .await
        .unwrap();
    write::insert_items(
        connection,
        vec![
            item(FILM, 27, "movie", 6_031_568),
            item(EPISODE_1, 28, "episode", 2_467_160),
            item(EPISODE_2, 28, "episode", 2_467_480),
        ],
    )
    .await
    .unwrap();
}

async fn repository() -> SqlPlaylistRepository {
    let connection = open_plex_db::connect("sqlite::memory:").await.unwrap();
    seed(&connection).await;
    SqlPlaylistRepository::new(connection)
}

fn keys(ids: &[i64]) -> Vec<RatingKey> {
    ids.iter().copied().map(RatingKey::new).collect()
}

fn plain(owner: Viewer, title: &str, items: &[i64]) -> NewPlaylist {
    NewPlaylist {
        owner,
        title: title.to_owned(),
        summary: String::new(),
        kind: PlaylistKind::Video,
        items: keys(items),
        rule: None,
        source_uri: None,
    }
}

fn page(size: u32) -> Window {
    Window::new(None, None, Some(size), None).unwrap()
}

async fn order(
    repository: &SqlPlaylistRepository,
    playlist: open_plex_core::PlaylistId,
) -> Vec<i64> {
    repository
        .entries(playlist, None, page(50))
        .await
        .unwrap()
        .items
        .iter()
        .map(|entry| entry.item.get())
        .collect()
}

fn query(owner: Viewer, sort: PlaylistSort, direction: Direction) -> PlaylistQuery {
    PlaylistQuery {
        owner,
        filter: PlaylistFilter::default(),
        sort,
        direction,
    }
}

#[tokio::test]
async fn a_new_playlist_keeps_its_items_once_in_order_and_counts_its_creation() {
    let repository = repository().await;
    let created = repository
        .create(
            plain(Viewer::Operator, "OP Dup", &[FILM, FILM, EPISODE_1]),
            NOW,
        )
        .await
        .unwrap();
    assert_eq!((created.view_count, created.last_viewed_at), (1, NOW));
    assert_eq!(created.guid.len(), 36);
    assert_eq!(order(&repository, created.id).await, [FILM, EPISODE_1]);

    let totals = repository.totals(&[created.id], None).await.unwrap();
    assert_eq!(totals[0].1.entries, 2);
    assert_eq!(totals[0].1.duration_ms, 6_031_568 + 2_467_160);
}

#[tokio::test]
async fn appending_skips_what_the_playlist_holds_already() {
    let repository = repository().await;
    let created = repository
        .create(plain(Viewer::Operator, "OP", &[FILM, EPISODE_1]), NOW)
        .await
        .unwrap();
    let added = repository
        .append(created.id, &keys(&[EPISODE_2, FILM, EPISODE_2]), NOW + 1)
        .await
        .unwrap();
    assert_eq!(added, 1);
    assert_eq!(
        order(&repository, created.id).await,
        [FILM, EPISODE_1, EPISODE_2]
    );
    let reread = repository.playlist(created.id).await.unwrap().unwrap();
    assert_eq!(reread.updated_at, NOW + 1);
}

#[tokio::test]
async fn an_entry_moves_after_another_or_to_the_start() {
    let repository = repository().await;
    let created = repository
        .create(
            plain(Viewer::Operator, "OP", &[FILM, EPISODE_1, EPISODE_2]),
            NOW,
        )
        .await
        .unwrap();
    let entries = repository
        .entries(created.id, None, page(10))
        .await
        .unwrap()
        .items;
    let (film, first, second) = (entries[0].id, entries[1].id, entries[2].id);

    repository
        .move_entry(second, Destination::After(film), NOW)
        .await
        .unwrap();
    assert_eq!(
        order(&repository, created.id).await,
        [FILM, EPISODE_2, EPISODE_1]
    );
    repository
        .move_entry(first, Destination::First, NOW)
        .await
        .unwrap();
    assert_eq!(
        order(&repository, created.id).await,
        [EPISODE_1, FILM, EPISODE_2]
    );
    repository
        .move_entry(first, Destination::After(second), NOW)
        .await
        .unwrap();
    assert_eq!(
        order(&repository, created.id).await,
        [FILM, EPISODE_2, EPISODE_1]
    );

    let missing = repository
        .move_entry(
            first,
            Destination::After(PlaylistEntryId::new(999_999)),
            NOW,
        )
        .await;
    assert!(matches!(missing, Err(PlaylistError::EntryNotFound(_))));
}

#[tokio::test]
async fn moves_keep_working_once_neighbours_leave_no_room() {
    let repository = repository().await;
    let created = repository
        .create(
            plain(Viewer::Operator, "OP", &[FILM, EPISODE_1, EPISODE_2]),
            NOW,
        )
        .await
        .unwrap();
    let entries = repository
        .entries(created.id, None, page(10))
        .await
        .unwrap()
        .items;
    let (film, first, second) = (entries[0].id, entries[1].id, entries[2].id);
    // Each move halves the gap after the film; eleven exhaust a gap of 1024.
    for round in 0..24 {
        let moved = if round % 2 == 0 { second } else { first };
        repository
            .move_entry(moved, Destination::After(film), NOW)
            .await
            .unwrap();
    }
    assert_eq!(
        order(&repository, created.id).await,
        [FILM, EPISODE_1, EPISODE_2]
    );
}

#[tokio::test]
async fn a_reader_sees_only_the_entries_of_its_libraries() {
    let repository = repository().await;
    let created = repository
        .create(
            plain(Viewer::Operator, "OP", &[FILM, EPISODE_1, EPISODE_2]),
            NOW,
        )
        .await
        .unwrap();
    let series = [SectionId::new(28)];
    let seen: Vec<i64> = repository
        .entries(created.id, Some(&series), page(10))
        .await
        .unwrap()
        .items
        .iter()
        .map(|entry| entry.item.get())
        .collect();
    assert_eq!(seen, [EPISODE_1, EPISODE_2]);
    let totals = repository
        .totals(&[created.id], Some(&series))
        .await
        .unwrap();
    assert_eq!(totals[0].1.entries, 2);
    assert!(repository
        .entries(created.id, Some(&[]), page(10))
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn entries_page_from_a_cursor() {
    let repository = repository().await;
    let created = repository
        .create(
            plain(Viewer::Operator, "OP", &[FILM, EPISODE_1, EPISODE_2]),
            NOW,
        )
        .await
        .unwrap();
    let first = repository.entries(created.id, None, page(2)).await.unwrap();
    assert!(first.has_next);
    let after = first.items[1].cursor();
    let rest = repository
        .entries(
            created.id,
            None,
            Window::new(Some(after), None, Some(2), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.items[0].item.get(), EPISODE_2);
    assert!(!rest.has_next);
}

#[tokio::test]
async fn a_listing_shows_its_owner_only_smart_playlists_first() {
    let repository = repository().await;
    let user = Viewer::User(UserId::new(7));
    for title in ["OP Renamed", "AA Nothing", "OP Lib"] {
        repository
            .create(plain(Viewer::Operator, title, &[]), NOW)
            .await
            .unwrap();
    }
    repository
        .create(plain(user, "Someone else's", &[FILM]), NOW)
        .await
        .unwrap();
    let mut smart = plain(Viewer::Operator, "ZZ Smart", &[]);
    smart.rule = Some(SmartRule {
        section: SectionId::new(28),
        listing: "{}".to_owned(),
    });
    repository.create(smart, NOW).await.unwrap();

    let titles = |query: PlaylistQuery| {
        let repository = &repository;
        async move {
            repository
                .playlists(&query, page(50))
                .await
                .unwrap()
                .items
                .into_iter()
                .map(|playlist| playlist.title)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        titles(query(
            Viewer::Operator,
            PlaylistSort::Title,
            Direction::Ascending
        ))
        .await,
        ["ZZ Smart", "AA Nothing", "OP Lib", "OP Renamed"]
    );
    assert_eq!(
        titles(query(
            Viewer::Operator,
            PlaylistSort::Title,
            Direction::Descending
        ))
        .await,
        ["ZZ Smart", "OP Renamed", "OP Lib", "AA Nothing"]
    );
    assert_eq!(
        titles(query(user, PlaylistSort::Title, Direction::Ascending)).await,
        ["Someone else's"]
    );

    let mut films = query(Viewer::Operator, PlaylistSort::Title, Direction::Ascending);
    films.filter.library = Some(SectionId::new(27));
    assert_eq!(
        repository.count(&films).await.unwrap(),
        3,
        "a smart playlist of another library is left out"
    );
}

#[tokio::test]
async fn a_listing_pages_from_a_cursor_in_both_directions() {
    let repository = repository().await;
    for (title, at) in [("B", NOW), ("A", NOW + 5), ("C", NOW)] {
        repository
            .create(plain(Viewer::Operator, title, &[]), at)
            .await
            .unwrap();
    }
    let sorted = query(
        Viewer::Operator,
        PlaylistSort::AddedAt,
        Direction::Descending,
    );
    let first = repository.playlists(&sorted, page(2)).await.unwrap();
    let titles: Vec<_> = first.items.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["A", "B"]);
    let after = sorted.cursor(&first.items[1]);
    let rest = repository
        .playlists(
            &sorted,
            Window::new(Some(after.clone()), None, Some(2), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rest.items[0].title, "C");
    let back = repository
        .playlists(
            &sorted,
            Window::new(None, Some(after), None, Some(5)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(back.items[0].title, "A");
}

#[tokio::test]
async fn an_edit_renames_locks_and_unlocks() {
    let repository = repository().await;
    let created = repository
        .create(plain(Viewer::Operator, "OP Dumb", &[]), NOW)
        .await
        .unwrap();
    let edited = repository
        .edit(
            created.id,
            &PlaylistEdit {
                title: Some("OP Renamed".to_owned()),
                summary: Some("Hello".to_owned()),
                lock: vec![PlaylistField::Title],
                unlock: Vec::new(),
            },
            NOW + 7,
        )
        .await
        .unwrap();
    assert_eq!(
        (edited.title.as_str(), edited.summary.as_str()),
        ("OP Renamed", "Hello")
    );
    assert_eq!(edited.locked, [PlaylistField::Title]);
    assert_eq!(edited.updated_at, NOW + 7);
    let unlocked = repository
        .edit(
            created.id,
            &PlaylistEdit {
                unlock: vec![PlaylistField::Title],
                ..PlaylistEdit::default()
            },
            NOW + 8,
        )
        .await
        .unwrap();
    assert!(unlocked.locked.is_empty());
}

#[tokio::test]
async fn clearing_removing_and_deleting() {
    let repository = repository().await;
    let created = repository
        .create(plain(Viewer::Operator, "OP", &[FILM, EPISODE_1]), NOW)
        .await
        .unwrap();
    let entries = repository
        .entries(created.id, None, page(10))
        .await
        .unwrap()
        .items;
    repository.remove(entries[0].id, NOW).await.unwrap();
    assert_eq!(order(&repository, created.id).await, [EPISODE_1]);
    assert!(matches!(
        repository.remove(entries[0].id, NOW).await,
        Err(PlaylistError::EntryNotFound(_))
    ));
    repository.clear(created.id, NOW).await.unwrap();
    assert!(order(&repository, created.id).await.is_empty());
    assert!(repository
        .totals(&[created.id], None)
        .await
        .unwrap()
        .is_empty());

    repository.record_play(created.id, NOW + 30).await.unwrap();
    let played = repository.playlist(created.id).await.unwrap().unwrap();
    assert_eq!((played.view_count, played.last_viewed_at), (2, NOW + 30));

    assert!(repository.delete(created.id).await.unwrap());
    assert!(!repository.delete(created.id).await.unwrap());
    assert!(repository.playlist(created.id).await.unwrap().is_none());
}
