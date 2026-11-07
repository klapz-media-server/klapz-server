//! Storage tests for tags: their order on an item, and filtering a listing by them.
//!
//! Values are the reference server's own: The Transporter's genres in billing order, and Luc
//! Besson credited as writer (473529) and as producer (475888) — one person, two tags.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::metadata::{
    ItemFilter, ItemKind, ItemQuery, MetadataRepository, Tag, TagCondition, TagKind, TagRef,
};
use open_plex_core::{RatingKey, SectionId, TagId};
use open_plex_db::entity::{item_tag, library_section, metadata_item};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const SECTION: i64 = 6;
const TRANSPORTER: i64 = 21_945;
const TRANSPORTER_2: i64 = 213_686;
const LE_NOUVEAU_JOUET: i64 = 415_073;

const ACTION: i64 = 38;
const CRIME: i64 = 198;
const THRILLER: i64 = 39;
const COMEDIE: i64 = 4025;
const BESSON_WRITER: i64 = 473_529;
const BESSON_PRODUCER: i64 = 475_888;
const STATHAM: i64 = 472_942;
/// A second person sharing an actor's name, which the source tells apart by provider key.
const STATHAM_HOMONYM: i64 = 999_001;

async fn repository() -> SqlMetadataRepository {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    SqlMetadataRepository::new(connection)
}

fn film(id: i64, title: &str) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(SECTION),
        parent_id: ActiveValue::Set(None),
        kind: ActiveValue::Set("movie".to_owned()),
        guid: ActiveValue::Set(format!("plex://movie/{id}")),
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
    }
}

fn tag(id: i64, kind: TagKind, name: &str, provider_key: Option<&str>) -> Tag {
    Tag {
        id: TagId::new(id),
        kind,
        name: name.to_owned(),
        provider_key: provider_key.map(ToOwned::to_owned),
        thumb_url: provider_key.map(|key| format!("https://metadata-static.plex.tv/{key}.jpg")),
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

async fn seed(connection: &DatabaseConnection) {
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(SECTION),
        kind: ActiveValue::Set("movie".to_owned()),
        title: ActiveValue::Set("VO - Movies".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("en-US".to_owned()),
        uuid: ActiveValue::Set("uuid-6".to_owned()),
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
            film(TRANSPORTER, "The Transporter"),
            film(TRANSPORTER_2, "Transporter 2"),
            film(LE_NOUVEAU_JOUET, "Le Nouveau Jouet"),
        ],
    )
    .await
    .expect("items");

    // Through the write path, which derives the folded name a filter compares against.
    write::insert_tags(
        connection,
        &[
            tag(ACTION, TagKind::Genre, "Action", None),
            tag(CRIME, TagKind::Genre, "Crime", None),
            tag(THRILLER, TagKind::Genre, "Thriller", None),
            tag(COMEDIE, TagKind::Genre, "Comédie", None),
            tag(
                BESSON_WRITER,
                TagKind::Writer,
                "Luc Besson",
                Some("5d776824103a2d001f563ac4"),
            ),
            tag(
                BESSON_PRODUCER,
                TagKind::Producer,
                "Luc Besson",
                Some("5d776824103a2d001f563ac4"),
            ),
            tag(
                STATHAM,
                TagKind::Actor,
                "Jason Statham",
                Some("5d776826e6d55c002040af58"),
            ),
            tag(
                STATHAM_HOMONYM,
                TagKind::Actor,
                "Jason Statham",
                Some("homonym"),
            ),
        ],
    )
    .await
    .expect("tags");

    item_tag::Entity::insert_many(vec![
        // Deliberately inserted out of billing order: the order is the position, not the
        // insertion sequence.
        attach(TRANSPORTER, THRILLER, 2, None),
        attach(TRANSPORTER, ACTION, 0, None),
        attach(TRANSPORTER, CRIME, 1, None),
        attach(TRANSPORTER, BESSON_WRITER, 0, None),
        attach(TRANSPORTER, BESSON_PRODUCER, 0, None),
        attach(TRANSPORTER, STATHAM, 0, Some("Frank Martin")),
        attach(TRANSPORTER_2, ACTION, 0, None),
        attach(TRANSPORTER_2, STATHAM_HOMONYM, 0, Some("Frank Martin")),
        attach(LE_NOUVEAU_JOUET, COMEDIE, 0, None),
    ])
    .exec(connection)
    .await
    .expect("attachments");
}

fn filtered(conditions: Vec<TagCondition>) -> ItemQuery {
    ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        kind: Some(ItemKind::Movie),
        tags: conditions,
        ..ItemFilter::default()
    })
}

fn condition(kind: TagKind, any_of: Vec<TagRef>) -> TagCondition {
    TagCondition { kind, any_of }
}

fn by_id(id: i64) -> TagRef {
    TagRef::Id(TagId::new(id))
}

fn by_name(name: &str) -> TagRef {
    TagRef::Name(name.to_owned())
}

async fn matches(repository: &SqlMetadataRepository, conditions: Vec<TagCondition>) -> Vec<i64> {
    let window = open_plex_core::page::Window::by_id(None, None, Some(50), None).unwrap();
    let mut ids: Vec<i64> = repository
        .items(&filtered(conditions), window)
        .await
        .expect("items")
        .items
        .iter()
        .map(|item| item.id.get())
        .collect();
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn returns_an_items_tags_in_billing_order() {
    let repository = repository().await;
    let tags = repository
        .tags_for(&[RatingKey::new(TRANSPORTER)])
        .await
        .expect("tags");

    let genres: Vec<&str> = tags
        .iter()
        .filter(|attached| attached.tag.kind == TagKind::Genre)
        .map(|attached| attached.tag.name.as_str())
        .collect();
    assert_eq!(genres, ["Action", "Crime", "Thriller"]);
    assert_eq!(tags.len(), 6);
}

#[tokio::test]
async fn carries_the_character_and_the_portrait_of_a_credit() {
    let repository = repository().await;
    let tags = repository
        .tags_for(&[RatingKey::new(TRANSPORTER)])
        .await
        .expect("tags");
    let statham = tags
        .iter()
        .find(|attached| attached.tag.kind == TagKind::Actor)
        .expect("an actor");
    assert_eq!(statham.role.as_deref(), Some("Frank Martin"));
    assert_eq!(
        statham.tag.provider_key.as_deref(),
        Some("5d776826e6d55c002040af58")
    );
    assert!(statham.tag.thumb_url.is_some());
}

#[tokio::test]
async fn loads_several_items_tags_in_one_call() {
    let repository = repository().await;
    let tags = repository
        .tags_for(&[
            RatingKey::new(TRANSPORTER),
            RatingKey::new(LE_NOUVEAU_JOUET),
        ])
        .await
        .expect("tags");
    assert_eq!(tags.len(), 7);
    assert!(repository.tags_for(&[]).await.expect("tags").is_empty());
}

#[tokio::test]
async fn refetches_a_tag_by_id() {
    let tag = repository()
        .await
        .tag(TagId::new(COMEDIE))
        .await
        .expect("lookup")
        .expect("present");
    assert_eq!(tag.name, "Comédie");
    assert_eq!(tag.kind, TagKind::Genre);
}

#[tokio::test]
async fn filters_by_tag_id() {
    let repository = repository().await;
    let found = matches(
        &repository,
        vec![condition(TagKind::Genre, vec![by_id(ACTION)])],
    )
    .await;
    assert_eq!(found, [TRANSPORTER, TRANSPORTER_2]);
}

#[tokio::test]
async fn filters_by_name_ignoring_ascii_case_only() {
    // The reference's rule, measured: `genre=ACTION` finds Action, `genre=COMÉDIE` does not
    // find Comédie. PostgreSQL's `LOWER` would find it, which is why the folded name is
    // stored rather than computed in the query.
    let repository = repository().await;
    let upper = matches(
        &repository,
        vec![condition(TagKind::Genre, vec![by_name("ACTION")])],
    )
    .await;
    assert_eq!(upper, [TRANSPORTER, TRANSPORTER_2]);

    let accented = matches(
        &repository,
        vec![condition(TagKind::Genre, vec![by_name("COMÉDIE")])],
    )
    .await;
    assert!(accented.is_empty());

    let exact = matches(
        &repository,
        vec![condition(TagKind::Genre, vec![by_name("Comédie")])],
    )
    .await;
    assert_eq!(exact, [LE_NOUVEAU_JOUET]);
}

#[tokio::test]
async fn values_in_one_condition_are_alternatives() {
    let repository = repository().await;
    let found = matches(
        &repository,
        vec![condition(
            TagKind::Genre,
            vec![by_id(CRIME), by_name("Comédie")],
        )],
    )
    .await;
    assert_eq!(found, [TRANSPORTER, LE_NOUVEAU_JOUET]);
}

#[tokio::test]
async fn conditions_must_all_hold() {
    let repository = repository().await;
    let found = matches(
        &repository,
        vec![
            condition(TagKind::Genre, vec![by_id(ACTION)]),
            condition(TagKind::Genre, vec![by_id(CRIME)]),
        ],
    )
    .await;
    assert_eq!(found, [TRANSPORTER]);
}

#[tokio::test]
async fn a_tag_counts_only_under_its_own_kind() {
    // Luc Besson the writer is not Luc Besson the producer: asking for films he produced by
    // the writer tag's id finds nothing.
    let repository = repository().await;
    let found = matches(
        &repository,
        vec![condition(TagKind::Producer, vec![by_id(BESSON_WRITER)])],
    )
    .await;
    assert!(found.is_empty());
}

#[tokio::test]
async fn a_name_shared_by_two_people_matches_both() {
    let repository = repository().await;
    let by_name = matches(
        &repository,
        vec![condition(TagKind::Actor, vec![by_name("jason statham")])],
    )
    .await;
    assert_eq!(by_name, [TRANSPORTER, TRANSPORTER_2]);

    // The id names one person.
    let by_id = matches(
        &repository,
        vec![condition(TagKind::Actor, vec![by_id(STATHAM)])],
    )
    .await;
    assert_eq!(by_id, [TRANSPORTER]);
}

#[tokio::test]
async fn a_condition_naming_nothing_matches_nothing() {
    // An empty alternative must not degrade into no constraint at all.
    let repository = repository().await;
    let found = matches(&repository, vec![condition(TagKind::Genre, vec![])]).await;
    assert!(found.is_empty());
}

#[tokio::test]
async fn the_count_honors_the_tag_filter() {
    let repository = repository().await;
    let query = filtered(vec![condition(TagKind::Genre, vec![by_id(ACTION)])]);
    assert_eq!(repository.item_count(&query).await.expect("count"), 2);
}
