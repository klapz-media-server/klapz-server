//! Stored collections: numbering, entries and their orders, memberships, searching, and the
//! collection filter of a listing.
//!
//! Items are the reference's test libraries: Soul in library 27, two episodes of Bull in 28.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::collection::{
    CollectionCondition, CollectionEdit, CollectionError, CollectionField, CollectionMode,
    CollectionOrder, CollectionQuery, CollectionRef, CollectionRepository, CollectionSort,
    Destination, NewCollection,
};
use open_plex_core::edit::CalendarDate;
use open_plex_core::metadata::{
    ItemDepth, ItemFilter, ItemKind, ItemQuery, ItemSort, MetadataRepository,
};
use open_plex_core::page::{Direction, Window};
use open_plex_core::{CollectionId, RatingKey, SectionId};
use open_plex_db::entity::{library_section, metadata_item};
use open_plex_db::{write, SqlCollectionRepository, SqlMetadataRepository};
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILM: i64 = 577_986;
const EPISODE_1: i64 = 578_173;
const EPISODE_2: i64 = 577_990;
const NOW: i64 = 1_794_406_080;

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

fn item(
    id: i64,
    section: i64,
    kind: &str,
    title: &str,
    year: Option<i32>,
) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(section),
        parent_id: ActiveValue::Set(None),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("tv.plex.agents.none://{id}")),
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
        year: ActiveValue::Set(year),
        released_on: ActiveValue::Set(year.map(|year| {
            CalendarDate::parse(&format!("{year}-04-30"))
                .unwrap()
                .midnight()
        })),
        duration_ms: ActiveValue::Set(None),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        refreshed_at: ActiveValue::Set(None),
        chapter_source: ActiveValue::Set(None),
    }
}

async fn connection() -> DatabaseConnection {
    let connection = open_plex_db::connect("sqlite::memory:").await.unwrap();
    library_section::Entity::insert_many([section(27, "movie"), section(28, "show")])
        .exec(&connection)
        .await
        .unwrap();
    write::insert_items(
        &connection,
        vec![
            item(FILM, 27, "movie", "Soul", None),
            item(EPISODE_1, 28, "episode", "Episode 1", Some(2021)),
            item(EPISODE_2, 28, "episode", "Episode 2", None),
        ],
    )
    .await
    .unwrap();
    connection
}

fn keys(ids: &[i64]) -> Vec<RatingKey> {
    ids.iter().copied().map(RatingKey::new).collect()
}

fn episodes(title: &str, items: &[i64]) -> NewCollection {
    NewCollection {
        section: SectionId::new(28),
        kind: ItemKind::Episode,
        title: title.to_owned(),
        items: keys(items),
        rule: None,
        source_uri: None,
    }
}

fn page(size: u32) -> Window {
    Window::new(None, None, Some(size), None).unwrap()
}

async fn custom_order(repository: &SqlCollectionRepository, collection: CollectionId) -> Vec<i64> {
    repository
        .entries(collection, page(50))
        .await
        .unwrap()
        .items
        .iter()
        .map(|entry| entry.item.get())
        .collect()
}

#[tokio::test]
async fn a_new_collection_is_numbered_past_the_items_and_tagged_past_the_others() {
    let repository = SqlCollectionRepository::new(connection().await);
    let first = repository
        .create(episodes("OP Coll D", &[EPISODE_1, EPISODE_2]), NOW)
        .await
        .unwrap();
    let second = repository
        .create(episodes("The Second", &[]), NOW)
        .await
        .unwrap();

    assert_eq!(first.id, CollectionId::new(EPISODE_1 + 1));
    assert_eq!(second.id, CollectionId::new(EPISODE_1 + 2));
    assert_eq!(second.tag_number, first.tag_number + 1);
    assert!(first.guid.starts_with("collection://"));
    // The sort title is derived as an item's is: the article goes.
    assert_eq!(second.title_sort.as_deref(), Some("Second"));
    assert_eq!(
        repository.collection(first.id).await.unwrap(),
        Some(first.clone())
    );
}

#[tokio::test]
async fn entries_keep_their_custom_order_through_moves() {
    let repository = SqlCollectionRepository::new(connection().await);
    let collection = repository
        .create(
            episodes("OP Coll D", &[EPISODE_1, EPISODE_2, EPISODE_1]),
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(
        custom_order(&repository, collection.id).await,
        [EPISODE_1, EPISODE_2]
    );

    repository
        .move_item(collection.id, RatingKey::new(EPISODE_2), Destination::First)
        .await
        .unwrap();
    assert_eq!(
        custom_order(&repository, collection.id).await,
        [EPISODE_2, EPISODE_1]
    );
    repository
        .move_item(
            collection.id,
            RatingKey::new(EPISODE_2),
            Destination::After(RatingKey::new(EPISODE_1)),
        )
        .await
        .unwrap();
    assert_eq!(
        custom_order(&repository, collection.id).await,
        [EPISODE_1, EPISODE_2]
    );

    let missing = repository
        .move_item(collection.id, RatingKey::new(FILM), Destination::First)
        .await;
    assert!(matches!(missing, Err(CollectionError::NotInCollection(_))));
    // A move changes the order, not when the collection last changed.
    assert_eq!(
        repository
            .collection(collection.id)
            .await
            .unwrap()
            .unwrap()
            .updated_at,
        NOW
    );
}

#[tokio::test]
async fn adding_and_removing_say_what_changed_and_date_the_change() {
    let repository = SqlCollectionRepository::new(connection().await);
    let collection = repository
        .create(episodes("OP Coll D", &[EPISODE_1]), NOW)
        .await
        .unwrap();

    let later = NOW + 60;
    assert_eq!(
        repository
            .add(collection.id, &keys(&[EPISODE_1, EPISODE_2]), later)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repository
            .add(collection.id, &keys(&[EPISODE_2]), later + 1)
            .await
            .unwrap(),
        0
    );
    let stored = repository.collection(collection.id).await.unwrap().unwrap();
    // A repeat adds nothing and dates nothing.
    assert_eq!(stored.updated_at, later);

    assert!(repository
        .remove(collection.id, RatingKey::new(EPISODE_1), later + 2)
        .await
        .unwrap());
    assert!(!repository
        .remove(collection.id, RatingKey::new(EPISODE_1), later + 3)
        .await
        .unwrap());
    let stored = repository.collection(collection.id).await.unwrap().unwrap();
    assert_eq!(stored.updated_at, later + 2);
    assert_eq!(custom_order(&repository, collection.id).await, [EPISODE_2]);

    repository.clear(collection.id).await.unwrap();
    assert!(custom_order(&repository, collection.id).await.is_empty());
    // Emptied through its tag, the reference leaves the date.
    let stored = repository.collection(collection.id).await.unwrap().unwrap();
    assert_eq!(stored.updated_at, later + 2);
}

#[tokio::test]
async fn totals_count_the_items_and_span_their_years() {
    let repository = SqlCollectionRepository::new(connection().await);
    let both = repository
        .create(episodes("OP Coll D", &[EPISODE_1, EPISODE_2]), NOW)
        .await
        .unwrap();
    let empty = repository
        .create(episodes("Empty", &[]), NOW)
        .await
        .unwrap();

    let totals = repository.totals(&[both.id, empty.id]).await.unwrap();
    assert_eq!(totals.len(), 1);
    let (id, total) = totals[0];
    assert_eq!(id, both.id);
    assert_eq!(total.items, 2);
    assert_eq!((total.min_year, total.max_year), (Some(2021), Some(2021)));
}

#[tokio::test]
async fn an_item_lists_its_collections_in_the_order_it_joined_them() {
    let repository = SqlCollectionRepository::new(connection().await);
    let later = repository.create(episodes("Zed", &[]), NOW).await.unwrap();
    repository
        .create(episodes("Alpha", &[EPISODE_1]), NOW)
        .await
        .unwrap();
    repository
        .add(later.id, &keys(&[EPISODE_1]), NOW)
        .await
        .unwrap();

    let memberships = repository
        .memberships(&keys(&[EPISODE_1, EPISODE_2]))
        .await
        .unwrap();
    let titles: Vec<(i64, &str)> = memberships
        .iter()
        .map(|membership| (membership.item.get(), membership.collection.title.as_str()))
        .collect();
    assert_eq!(titles, [(EPISODE_1, "Alpha"), (EPISODE_1, "Zed")]);
}

#[tokio::test]
async fn edits_rewrite_the_title_words_and_keep_the_date() {
    let repository = SqlCollectionRepository::new(connection().await);
    let collection = repository
        .create(episodes("OP Coll A", &[EPISODE_1]), NOW)
        .await
        .unwrap();
    let edited = repository
        .edit(
            collection.id,
            &CollectionEdit {
                title: Some("Légendes d'hier".to_owned()),
                summary: Some("A summary".to_owned()),
                mode: Some(CollectionMode::InsteadOfItems),
                order: Some(CollectionOrder::Custom),
                lock: vec![CollectionField::Title],
                ..CollectionEdit::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(edited.updated_at, NOW);
    assert_eq!(
        repository.collection(collection.id).await.unwrap(),
        Some(edited)
    );

    let mut query = CollectionQuery::across(vec![SectionId::new(28)]);
    query.title_word_prefix = Some("legende".to_owned());
    assert_eq!(repository.count(&query).await.unwrap(), 1);
    query.title_word_prefix = Some("coll".to_owned());
    assert_eq!(repository.count(&query).await.unwrap(), 0);
}

#[tokio::test]
async fn listings_order_by_title_or_date_and_page_by_cursor() {
    let repository = SqlCollectionRepository::new(connection().await);
    for (title, at) in [("Bravo", 3), ("alpha", 2), ("Charlie", 1)] {
        repository.create(episodes(title, &[]), at).await.unwrap();
    }
    let mut query = CollectionQuery::across(vec![SectionId::new(28), SectionId::new(27)]);
    let titles = |page: &open_plex_core::page::Page<open_plex_core::collection::Collection>| {
        page.items
            .iter()
            .map(|collection| collection.title.clone())
            .collect::<Vec<_>>()
    };
    let first = repository.collections(&query, page(2)).await.unwrap();
    assert_eq!(titles(&first), ["alpha", "Bravo"]);
    assert!(first.has_next);
    let rest = repository
        .collections(
            &query,
            Window::new(Some(query.cursor(&first.items[1])), None, Some(2), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(titles(&rest), ["Charlie"]);

    query.sort = CollectionSort::AddedAt;
    query.direction = Direction::Descending;
    let by_date = repository.collections(&query, page(5)).await.unwrap();
    assert_eq!(titles(&by_date), ["Bravo", "alpha", "Charlie"]);

    query.sections = vec![SectionId::new(27)];
    assert_eq!(repository.count(&query).await.unwrap(), 0);
}

#[tokio::test]
async fn a_listing_keeps_the_items_of_a_collection_by_id_number_or_title() {
    let connection = connection().await;
    let repository = SqlCollectionRepository::new(connection.clone());
    let metadata = SqlMetadataRepository::new(connection);
    let collection = repository
        .create(episodes("OP Coll D", &[EPISODE_2]), NOW)
        .await
        .unwrap();
    for wanted in [
        CollectionRef::Id(collection.id),
        CollectionRef::TagNumber(collection.tag_number),
        CollectionRef::Title("op coll d".to_owned()),
    ] {
        let filter = ItemFilter {
            depth: ItemDepth::All,
            collections: vec![CollectionCondition {
                any_of: vec![wanted.clone()],
            }],
            ..ItemFilter::default()
        };
        let query = ItemQuery::new(SectionId::new(28))
            .filtered(filter)
            .sorted_by(ItemSort::ReleasedOn, Direction::Ascending);
        let listed = metadata.items(&query, page(10)).await.unwrap();
        let ids: Vec<i64> = listed
            .items
            .iter()
            .map(|listed| listed.item.id.get())
            .collect();
        assert_eq!(ids, [EPISODE_2], "{wanted:?}");
    }
}

#[tokio::test]
async fn deleting_takes_the_entries_and_leaves_the_items() {
    let connection = connection().await;
    let repository = SqlCollectionRepository::new(connection.clone());
    let collection = repository
        .create(episodes("OP Coll D", &[EPISODE_1]), NOW)
        .await
        .unwrap();
    assert!(repository.delete(collection.id).await.unwrap());
    assert!(!repository.delete(collection.id).await.unwrap());
    assert!(repository
        .memberships(&keys(&[EPISODE_1]))
        .await
        .unwrap()
        .is_empty());
    let metadata = SqlMetadataRepository::new(connection);
    assert!(metadata.item(RatingKey::new(EPISODE_1)).await.is_ok());
}
