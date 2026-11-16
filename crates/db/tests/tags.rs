//! Storage tests for tags, the people they credit, and external identifiers and ratings.
//!
//! Values are the reference server's own: The Transporter's genres in billing order, Luc
//! Besson credited as writer (473529) and as producer (475888) — one person, two tags — and a
//! Simpsons episode directed by David Silverman.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::metadata::{
    CreditRef, FieldValue, ItemFilter, ItemKind, ItemQuery, ItemSort, MetadataRepository,
    RatingKind, TagCondition, TagKind, TagRef, TagScope, TagSearch, TagUse, ValueField,
};
use open_plex_core::page::{Direction, Window};
use open_plex_core::{PersonId, RatingKey, SectionId, TagId};
use open_plex_db::entity::{
    item_external_id, item_rating, item_tag, library_section, metadata_item, person, tag,
};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait};

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
        refreshed_at: ActiveValue::Set(None),
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

    write::insert_tags(
        connection,
        vec![
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
        .map(|listed| listed.item.id.get())
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

fn first(n: u32) -> Window {
    Window::new(None, None, Some(n), None).unwrap()
}

fn films() -> ItemQuery {
    ItemQuery::new(SectionId::new(FILMS))
}

fn names(page: &open_plex_core::Page<open_plex_core::Tag>) -> Vec<&str> {
    page.items.iter().map(|tag| tag.name.as_str()).collect()
}

#[tokio::test]
async fn lists_a_librarys_genres_once_each_by_name() {
    let repository = repository().await;
    let scope = TagScope {
        kind: TagKind::Genre,
        billed_within: None,
    };

    // Action is on both Transporters and listed once; the series' genres stay out.
    let page = repository
        .tags_in(&films(), scope, first(10))
        .await
        .unwrap();
    assert_eq!(names(&page), ["Action", "Comédie", "Crime", "Thriller"]);
    assert_eq!(repository.tag_count_in(&films(), scope).await.unwrap(), 4);

    // Resuming from a cursor and landing by offset give the same rows.
    let head = repository.tags_in(&films(), scope, first(2)).await.unwrap();
    assert!(head.has_next);
    let cursor = head.items[1].name_cursor();
    let after = Window::new(Some(cursor), None, Some(2), None).unwrap();
    let tail = repository.tags_in(&films(), scope, after).await.unwrap();
    assert_eq!(names(&tail), ["Crime", "Thriller"]);
    assert!(!tail.has_next);
    let skipped = repository
        .tags_in(&films(), scope, first(2).skipping(2))
        .await
        .unwrap();
    assert_eq!(names(&skipped), names(&tail));
}

#[tokio::test]
async fn a_billing_limit_keeps_only_the_first_credits() {
    let repository = repository().await;
    // The Transporter bills Action, Crime, Thriller in that order.
    let scope = TagScope {
        kind: TagKind::Genre,
        billed_within: Some(1),
    };
    let page = repository
        .tags_in(&films(), scope, first(10))
        .await
        .unwrap();
    assert_eq!(names(&page), ["Action", "Comédie"]);
    assert_eq!(repository.tag_count_in(&films(), scope).await.unwrap(), 2);
}

#[tokio::test]
async fn two_people_sharing_a_name_are_two_entries() {
    let repository = repository().await;
    let scope = TagScope {
        kind: TagKind::Actor,
        billed_within: None,
    };
    let page = repository
        .tags_in(&films(), scope, first(10))
        .await
        .unwrap();
    let ids: Vec<i64> = page.items.iter().map(|tag| tag.id.get()).collect();
    assert_eq!(ids, [STATHAM, STATHAM_HOMONYM]);
    assert!(page.items[0].person.is_some());
}

#[tokio::test]
async fn lists_distinct_years_newest_first_and_text_values_ascending() {
    let connection = open_plex_db::connect("sqlite::memory:").await.unwrap();
    seed(&connection).await;
    connection
        .execute_unprepared(&format!(
            "UPDATE metadata_item SET year = 2002, studio = 'EuropaCorp' WHERE id = {TRANSPORTER};
             UPDATE metadata_item SET year = 2005, studio = 'EuropaCorp' WHERE id = {TRANSPORTER_2};
             UPDATE metadata_item SET year = 1976, studio = 'AMLF' WHERE id = {LE_NOUVEAU_JOUET};"
        ))
        .await
        .unwrap();
    let repository = SqlMetadataRepository::new(connection);

    let years = repository
        .values_in(&films(), ValueField::Year, first(10))
        .await
        .unwrap();
    assert_eq!(
        years.items,
        [
            FieldValue::Year(2005),
            FieldValue::Year(2002),
            FieldValue::Year(1976)
        ]
    );
    let after = Window::new(Some(years.items[0].cursor()), None, Some(10), None).unwrap();
    let rest = repository
        .values_in(&films(), ValueField::Year, after)
        .await
        .unwrap();
    assert_eq!(rest.items, [FieldValue::Year(2002), FieldValue::Year(1976)]);

    let studios = repository
        .values_in(&films(), ValueField::Studio, first(10))
        .await
        .unwrap();
    assert_eq!(
        studios.items,
        [
            FieldValue::Text("AMLF".to_owned()),
            FieldValue::Text("EuropaCorp".to_owned())
        ]
    );
    assert_eq!(
        repository
            .value_count_in(&films(), ValueField::Studio)
            .await
            .unwrap(),
        2
    );
    // Nothing set, nothing listed: an absent rating is not a value.
    assert_eq!(
        repository
            .value_count_in(&films(), ValueField::ContentRating)
            .await
            .unwrap(),
        0
    );
}

fn works(credited: CreditRef, sections: &[i64]) -> ItemQuery {
    ItemQuery::across(sections.iter().copied().map(SectionId::new).collect())
        .filtered(ItemFilter {
            credited: Some(credited),
            ..ItemFilter::default()
        })
        .sorted_by(ItemSort::Filmography, Direction::Ascending)
}

async fn listed(repository: &SqlMetadataRepository, query: &ItemQuery, window: Window) -> Vec<i64> {
    repository
        .items(query, window)
        .await
        .expect("items")
        .items
        .iter()
        .map(|listed| listed.item.id.get())
        .collect()
}

#[tokio::test]
async fn a_credit_on_an_episode_lists_its_series_once() {
    let repository = repository().await;
    for credited in [
        CreditRef::Tag(TagId::new(SILVERMAN)),
        // On the series and on both episodes, still once.
        CreditRef::Tag(TagId::new(CASTELLANETA)),
    ] {
        let query = works(credited, &[FILMS, SERIES]);
        assert_eq!(listed(&repository, &query, first(10)).await, [SIMPSONS]);
        assert_eq!(repository.item_count(&query).await.unwrap(), 1);
    }
}

#[tokio::test]
async fn a_person_lists_what_any_of_their_tags_credits_once() {
    let repository = repository().await;
    let query = works(CreditRef::Person(PersonId::new(BESSON)), &[FILMS, SERIES]);
    assert_eq!(listed(&repository, &query, first(10)).await, [TRANSPORTER]);
}

#[tokio::test]
async fn a_filmography_runs_library_by_library_films_by_title_series_by_id() {
    let connection = open_plex_db::connect("sqlite::memory:").await.unwrap();
    seed(&connection).await;
    // A series titled before The Simpsons but added after it.
    write::insert_items(
        &connection,
        vec![item(500_000, SERIES, "show", None, "Archer")],
    )
    .await
    .unwrap();
    item_tag::Entity::insert(attach(500_000, ACTION, 0, None))
        .exec(&connection)
        .await
        .unwrap();
    item_tag::Entity::insert(attach(SIMPSONS, ACTION, 0, None))
        .exec(&connection)
        .await
        .unwrap();
    let repository = SqlMetadataRepository::new(connection);

    // The libraries in the order given, not by id.
    let query = works(CreditRef::Tag(TagId::new(ACTION)), &[SERIES, FILMS]);
    let expected = [SIMPSONS, 500_000, TRANSPORTER, TRANSPORTER_2];
    assert_eq!(listed(&repository, &query, first(10)).await, expected);

    // Cursors resume across a library boundary and inside each ordering.
    let mut walked = Vec::new();
    let mut after = None;
    loop {
        let page = repository
            .items(&query, Window::new(after, None, Some(1), None).unwrap())
            .await
            .unwrap();
        walked.extend(page.items.iter().map(|listed| listed.item.id.get()));
        let Some(last) = page.items.last() else { break };
        after = Some(last.cursor.clone());
        if !page.has_next {
            break;
        }
    }
    assert_eq!(walked, expected);
}

#[tokio::test]
async fn a_persons_credits_come_with_the_libraries_carrying_them() {
    let repository = repository().await;
    let credits = repository
        .credits_of(&[PersonId::new(BESSON), PersonId::new(DAVID_SILVERMAN)])
        .await
        .unwrap();
    let summary: Vec<(i64, Vec<i64>)> = credits
        .iter()
        .map(|credit| {
            (
                credit.tag.id.get(),
                credit
                    .sections
                    .iter()
                    .map(|section| section.get())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (BESSON_WRITER, vec![FILMS]),
            (BESSON_PRODUCER, vec![FILMS]),
            (SILVERMAN, vec![SERIES]),
        ]
    );
}

fn tag_search(term: &str, kinds: Vec<TagKind>) -> TagSearch {
    TagSearch {
        word_prefix: term.to_owned(),
        kinds,
        sections: None,
    }
}

fn uses(page: &[TagUse]) -> Vec<(i64, i64, u64)> {
    page.iter()
        .map(|found| (found.tag.id.get(), found.section.get(), found.item_count))
        .collect()
}

#[tokio::test]
async fn finds_a_tag_by_any_word_of_its_name_once_per_library() {
    let repository = repository().await;

    let found = repository
        .search_tags(&tag_search("castel", vec![]), first(10))
        .await
        .unwrap();
    // The series and two of its episodes credit him: three items of one library.
    assert_eq!(uses(&found.items), [(CASTELLANETA, SERIES, 3)]);
    let person = found.items[0]
        .tag
        .person
        .as_ref()
        .expect("a credit names someone");
    assert_eq!(
        person.provider_key.as_deref(),
        Some("5d7768267e9a3c0020c6ab0c")
    );

    // Every word of the term has to begin a word of the name, accents folded.
    let found = repository
        .search_tags(&tag_search("dan cast", vec![]), first(10))
        .await
        .unwrap();
    assert_eq!(uses(&found.items), [(CASTELLANETA, SERIES, 3)]);
    for missing in ["dan silverman", "aneta", "   "] {
        let found = repository
            .search_tags(&tag_search(missing, vec![]), first(10))
            .await
            .unwrap();
        assert!(found.items.is_empty(), "{missing}");
    }
    let found = repository
        .search_tags(&tag_search("comedie", vec![TagKind::Genre]), first(10))
        .await
        .unwrap();
    assert_eq!(uses(&found.items), [(COMEDIE, FILMS, 1)]);
}

#[tokio::test]
async fn orders_tag_uses_by_item_count_and_pages_them() {
    let repository = repository().await;

    // Castellaneta on three items first; Crime and Comédie on one film each, by id.
    let found = repository
        .search_tags(&tag_search("c", vec![]), first(10))
        .await
        .unwrap();
    assert_eq!(
        uses(&found.items),
        [
            (CASTELLANETA, SERIES, 3),
            (CRIME, FILMS, 1),
            (COMEDIE, FILMS, 1)
        ]
    );

    let search = tag_search("statham", vec![TagKind::Actor]);
    let page = repository.search_tags(&search, first(1)).await.unwrap();
    assert_eq!(uses(&page.items), [(STATHAM, FILMS, 1)]);
    assert!(page.has_next);
    let after = Window::new(Some(page.items[0].cursor()), None, Some(10), None).unwrap();
    let rest = repository.search_tags(&search, after).await.unwrap();
    assert_eq!(uses(&rest.items), [(STATHAM_HOMONYM, FILMS, 1)]);
    assert!(!rest.has_next);
}

#[tokio::test]
async fn a_tag_carried_by_episodes_alone_names_no_library() {
    let repository = repository().await;
    // David Silverman directed an episode: the series list of that library has no one to
    // show for him.
    let found = repository
        .search_tags(&tag_search("silverman", vec![]), first(10))
        .await
        .unwrap();
    assert!(found.items.is_empty());
}

#[tokio::test]
async fn a_tag_search_sees_only_its_kinds_and_libraries() {
    let repository = repository().await;

    let directors = repository
        .search_tags(&tag_search("david", vec![TagKind::Actor]), first(10))
        .await
        .unwrap();
    assert!(directors.items.is_empty());

    let mut search = tag_search("dan", vec![]);
    search.sections = Some(vec![SectionId::new(FILMS)]);
    let found = repository.search_tags(&search, first(10)).await.unwrap();
    assert!(found.items.is_empty());
}
