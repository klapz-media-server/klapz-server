//! Storage tests for tags, the people they credit, and external identifiers and ratings.
//!
//! Values are the reference server's own: The Transporter's genres in billing order, Luc
//! Besson credited as writer (473529) and as producer (475888) — one person, two tags — and a
//! Simpsons episode directed by David Silverman.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::metadata::{
    ItemFilter, ItemKind, ItemQuery, MetadataRepository, RatingKind, TagCondition, TagKind, TagRef,
};
use open_plex_core::page::Window;
use open_plex_core::{PersonId, RatingKey, SectionId, TagId};
use open_plex_db::entity::{
    item_external_id, item_rating, item_tag, library_section, metadata_item, person, tag,
};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILMS: i64 = 6;
const SERIES: i64 = 7;

const TRANSPORTER: i64 = 21_945;
const TRANSPORTER_2: i64 = 213_686;
const LE_NOUVEAU_JOUET: i64 = 415_073;
const SIMPSONS: i64 = 22_053;
const SEASON_14: i64 = 22_077;
const TREEHOUSE_XIII: i64 = 22_078;
const TREEHOUSE_XIV: i64 = 22_079;

const ACTION: i64 = 38;
const CRIME: i64 = 198;
const THRILLER: i64 = 39;
const COMEDIE: i64 = 4025;
const BESSON_WRITER: i64 = 473_529;
const BESSON_PRODUCER: i64 = 475_888;
const STATHAM: i64 = 472_942;
/// A second person sharing an actor's name, told apart by the provider key.
const STATHAM_HOMONYM: i64 = 999_001;
const SILVERMAN: i64 = 705_517;
const CASTELLANETA: i64 = 486_047;

const BESSON: i64 = 1;
const JASON_STATHAM: i64 = 2;
const OTHER_JASON_STATHAM: i64 = 3;
const DAVID_SILVERMAN: i64 = 4;
const DAN_CASTELLANETA: i64 = 5;

async fn repository() -> SqlMetadataRepository {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    SqlMetadataRepository::new(connection)
}

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
        title: ActiveValue::Set(Some(title.to_owned())),
        title_sort: ActiveValue::Set(None),
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
    }
}

fn section(id: i64, kind: &str) -> library_section::ActiveModel {
    library_section::ActiveModel {
        id: ActiveValue::Set(id),
        kind: ActiveValue::Set(kind.to_owned()),
        title: ActiveValue::Set(format!("section {id}")),
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

fn someone(id: i64, name: &str, key: Option<&str>) -> person::ActiveModel {
    person::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(name.to_owned()),
        provider_key: ActiveValue::Set(key.map(ToOwned::to_owned)),
        thumb_url: ActiveValue::Set(
            key.map(|key| format!("https://metadata-static.plex.tv/{key}.jpg")),
        ),
    }
}

fn label(id: i64, kind: TagKind, name: &str, person: Option<i64>) -> tag::ActiveModel {
    tag::ActiveModel {
        id: ActiveValue::Set(id),
        kind: ActiveValue::Set(tag::kind_name(kind).to_owned()),
        name: ActiveValue::Set(name.to_owned()),
        person_id: ActiveValue::Set(person),
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
    seed_catalog(connection).await;
    seed_tags(connection).await;
    seed_references(connection).await;
}

async fn seed_catalog(connection: &DatabaseConnection) {
    library_section::Entity::insert_many([section(FILMS, "movie"), section(SERIES, "show")])
        .exec(connection)
        .await
        .expect("sections");

    write::insert_items(
        connection,
        vec![
            item(TRANSPORTER, FILMS, "movie", None, "The Transporter"),
            item(TRANSPORTER_2, FILMS, "movie", None, "Transporter 2"),
            item(LE_NOUVEAU_JOUET, FILMS, "movie", None, "Le Nouveau Jouet"),
            item(SIMPSONS, SERIES, "show", None, "The Simpsons"),
            item(SEASON_14, SERIES, "season", Some(SIMPSONS), ""),
            item(
                TREEHOUSE_XIII,
                SERIES,
                "episode",
                Some(SEASON_14),
                "Treehouse of Horror XIII",
            ),
            item(
                TREEHOUSE_XIV,
                SERIES,
                "episode",
                Some(SEASON_14),
                "Treehouse of Horror XIV",
            ),
        ],
    )
    .await
    .expect("items");
}

async fn seed_tags(connection: &DatabaseConnection) {
    person::Entity::insert_many([
        someone(BESSON, "Luc Besson", Some("5d776824103a2d001f563ac4")),
        someone(
            JASON_STATHAM,
            "Jason Statham",
            Some("5d776826e6d55c002040af58"),
        ),
        someone(OTHER_JASON_STATHAM, "Jason Statham", Some("homonym")),
        someone(
            DAVID_SILVERMAN,
            "David Silverman",
            Some("5d7768267e9a3c0020c6ab00"),
        ),
        someone(
            DAN_CASTELLANETA,
            "Dan Castellaneta",
            Some("5d7768267e9a3c0020c6ab0c"),
        ),
    ])
    .exec(connection)
    .await
    .expect("people");

    tag::Entity::insert_many([
        label(ACTION, TagKind::Genre, "Action", None),
        label(CRIME, TagKind::Genre, "Crime", None),
        label(THRILLER, TagKind::Genre, "Thriller", None),
        label(COMEDIE, TagKind::Genre, "Comédie", None),
        label(BESSON_WRITER, TagKind::Writer, "Luc Besson", Some(BESSON)),
        label(
            BESSON_PRODUCER,
            TagKind::Producer,
            "Luc Besson",
            Some(BESSON),
        ),
        label(
            STATHAM,
            TagKind::Actor,
            "Jason Statham",
            Some(JASON_STATHAM),
        ),
        label(
            STATHAM_HOMONYM,
            TagKind::Actor,
            "Jason Statham",
            Some(OTHER_JASON_STATHAM),
        ),
        label(
            SILVERMAN,
            TagKind::Director,
            "David Silverman",
            Some(DAVID_SILVERMAN),
        ),
        label(
            CASTELLANETA,
            TagKind::Actor,
            "Dan Castellaneta",
            Some(DAN_CASTELLANETA),
        ),
    ])
    .exec(connection)
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
        attach(SIMPSONS, CASTELLANETA, 0, Some("Homer Simpson")),
        attach(TREEHOUSE_XIII, SILVERMAN, 0, None),
        attach(TREEHOUSE_XIII, CASTELLANETA, 0, Some("Homer Simpson")),
        attach(TREEHOUSE_XIV, CASTELLANETA, 0, Some("Homer Simpson")),
    ])
    .exec(connection)
    .await
    .expect("attachments");
}

async fn seed_references(connection: &DatabaseConnection) {
    item_external_id::Entity::insert_many([
        external(TRANSPORTER, "tvdb", "1601", 2),
        external(TRANSPORTER, "imdb", "tt0293662", 0),
        external(TRANSPORTER, "tmdb", "4108", 1),
    ])
    .exec(connection)
    .await
    .expect("external ids");

    item_rating::Entity::insert_many([
        item_rating::ActiveModel {
            item_id: ActiveValue::Set(TRANSPORTER),
            source: ActiveValue::Set("imdb".to_owned()),
            kind: ActiveValue::Set("audience".to_owned()),
            value: ActiveValue::Set(6.8),
            image: ActiveValue::Set("imdb://image.rating".to_owned()),
            position: ActiveValue::Set(0),
        },
        item_rating::ActiveModel {
            item_id: ActiveValue::Set(TRANSPORTER),
            source: ActiveValue::Set("rottentomatoes".to_owned()),
            kind: ActiveValue::Set("critic".to_owned()),
            value: ActiveValue::Set(5.4),
            image: ActiveValue::Set("rottentomatoes://image.rating.rotten".to_owned()),
            position: ActiveValue::Set(1),
        },
    ])
    .exec(connection)
    .await
    .expect("ratings");
}

fn external(item: i64, source: &str, value: &str, position: i32) -> item_external_id::ActiveModel {
    item_external_id::ActiveModel {
        item_id: ActiveValue::Set(item),
        source: ActiveValue::Set(source.to_owned()),
        value: ActiveValue::Set(value.to_owned()),
        position: ActiveValue::Set(position),
    }
}

fn filtered(conditions: Vec<TagCondition>) -> ItemQuery {
    ItemQuery::new(SectionId::new(FILMS)).filtered(ItemFilter {
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
    let window = Window::by_id(None, None, Some(50), None).unwrap();
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
async fn a_credit_carries_its_person() {
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
    let person = statham.tag.person.as_ref().expect("a person");
    assert_eq!(person.id, PersonId::new(JASON_STATHAM));
    assert_eq!(
        person.provider_key.as_deref(),
        Some("5d776826e6d55c002040af58")
    );
    assert!(person.thumb_url.is_some());

    // A genre credits nobody.
    assert!(tags
        .iter()
        .filter(|attached| attached.tag.kind == TagKind::Genre)
        .all(|attached| attached.tag.person.is_none()));
}

#[tokio::test]
async fn one_person_in_two_roles_is_two_tags_and_one_person() {
    let repository = repository().await;
    let writer = repository
        .tag(TagId::new(BESSON_WRITER))
        .await
        .unwrap()
        .unwrap();
    let producer = repository
        .tag(TagId::new(BESSON_PRODUCER))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(writer.id, producer.id);
    assert_eq!(writer.person, producer.person);
    assert_eq!(
        repository
            .person(PersonId::new(BESSON))
            .await
            .unwrap()
            .unwrap()
            .name,
        "Luc Besson"
    );
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
async fn counts_the_credits_of_the_episodes_below_a_series_and_a_season() {
    let repository = repository().await;
    let credits = repository
        .credits_below(&[RatingKey::new(SIMPSONS), RatingKey::new(SEASON_14)])
        .await
        .expect("credits");

    let summary = |parent: i64| -> Vec<(&str, u32)> {
        credits
            .iter()
            .filter(|credit| credit.parent == RatingKey::new(parent))
            .map(|credit| (credit.tag.name.as_str(), credit.episodes))
            .collect()
    };
    // Most episodes first: Castellaneta is in both, Silverman directed one.
    let expected = [("Dan Castellaneta", 2), ("David Silverman", 1)];
    assert_eq!(summary(SIMPSONS), expected);
    assert_eq!(summary(SEASON_14), expected);
}

#[tokio::test]
async fn returns_external_ids_and_ratings_in_source_order() {
    let repository = repository().await;
    let ids: Vec<(String, String)> = repository
        .external_ids_for(&[RatingKey::new(TRANSPORTER)])
        .await
        .expect("ids")
        .into_iter()
        .map(|id| (id.source, id.value))
        .collect();
    assert_eq!(
        ids,
        [
            ("imdb".to_owned(), "tt0293662".to_owned()),
            ("tmdb".to_owned(), "4108".to_owned()),
            ("tvdb".to_owned(), "1601".to_owned()),
        ]
    );

    let ratings = repository
        .ratings_for(&[RatingKey::new(TRANSPORTER)])
        .await
        .expect("ratings");
    assert_eq!(ratings.len(), 2);
    assert_eq!(ratings[1].kind, RatingKind::Critic);
    assert!((ratings[1].value - 5.4).abs() < f64::EPSILON);
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
async fn filters_by_name_ignoring_case() {
    let repository = repository().await;
    let upper = matches(
        &repository,
        vec![condition(TagKind::Genre, vec![by_name("ACTION")])],
    )
    .await;
    assert_eq!(upper, [TRANSPORTER, TRANSPORTER_2]);

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
