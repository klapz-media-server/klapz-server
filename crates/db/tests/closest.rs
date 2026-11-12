//! Storage tests for the voice search and for completions.
//!
//! The titles are the reference's, with the sort titles it gave them: the films and series a
//! few edits from `dune`, `the office` and `kaamelott`, which is where the length bands start
//! and stop. Every ranking is checked against the same titles ranked whole, so a band left
//! unread shows up as a missing or misplaced match.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::closest::{self, ClosestTitles, Distance};
use open_plex_core::edit::{FieldEdit, ItemEdit};
use open_plex_core::metadata::{
    ItemFilter, ItemKind, ItemQuery, MetadataRepository, TagKind, TagScope,
};
use open_plex_core::page::Window;
use open_plex_core::{RatingKey, SectionId};
use open_plex_db::entity::{item_tag, library_section, metadata_item, person, tag};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILMS: i64 = 4;
const SERIES: i64 = 5;

/// `(id, section, kind, title, sort title)`, as the reference holds them.
const TITLES: &[(i64, i64, &str, &str, Option<&str>)] = &[
    (470_473, FILMS, "movie", "Dune", None),
    (279_798, FILMS, "movie", "Cube", None),
    (333_633, FILMS, "movie", "Juno", None),
    (465_594, FILMS, "movie", "Duel", None),
    (
        440_223,
        FILMS,
        "movie",
        "Dune : Deuxième partie",
        Some("Dune : Deuxieme partie"),
    ),
    (
        396_710,
        FILMS,
        "movie",
        "Dune : Première partie",
        Some("Dune : Premiere partie"),
    ),
    (306_682, FILMS, "movie", "Breaking In", None),
    (409_687, FILMS, "movie", "Charlotte", None),
    (213_416, FILMS, "movie", "Hamlet", None),
    (312_480, FILMS, "movie", "La Meute", Some("Meute")),
    (550_310, FILMS, "movie", "La Momie", None),
    (261_076, FILMS, "movie", "Ève", Some("Eve")),
    (511_259, FILMS, "movie", "Le mal-aimé", None),
    (
        309_731,
        FILMS,
        "movie",
        "À tous les garçons que j'ai aimés",
        Some("A tous les garcons que j'ai aimes"),
    ),
    (465_488, FILMS, "movie", "Mange, prie, aime", None),
    (
        438_966,
        FILMS,
        "movie",
        "The First Slam Dunk",
        Some("First Slam Dunk"),
    ),
    (323_535, FILMS, "movie", "Crocodile Dundee", None),
    (433_060, FILMS, "movie", "100% bio", None),
    (200_640, SERIES, "show", "Kaamelott", None),
    (260_220, SERIES, "show", "The Office", Some("Office")),
    (305_531, SERIES, "show", "Dope", None),
    (24_569, SERIES, "show", "Breaking Bad", None),
    (497_485, SERIES, "show", "Ángela", Some("Angela")),
];

/// An episode of Breaking Bad, to keep series and episodes apart.
const PILOT: i64 = 24_570;

const TOM_HANKS: i64 = 469_791;
const COLIN_HANKS: i64 = 481_281;
const TOM_HARDY: i64 = 478_655;
const HANKS_DIRECTOR: i64 = 999_002;

fn item(
    id: i64,
    section: i64,
    kind: &str,
    title: &str,
    title_sort: Option<&str>,
    parent: Option<i64>,
) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(section),
        parent_id: ActiveValue::Set(parent),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("plex://{kind}/{id}")),
        title: ActiveValue::Set(Some(title.to_owned())),
        title_sort: ActiveValue::Set(title_sort.map(ToOwned::to_owned)),
        sort_key: ActiveValue::Set(open_plex_core::metadata::sort_key(title_sort, Some(title))),
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
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set(format!("uuid-{id}")),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    }
}

async fn seed(connection: &DatabaseConnection) {
    library_section::Entity::insert_many([section(FILMS, "movie"), section(SERIES, "show")])
        .exec(connection)
        .await
        .unwrap();
    let mut items: Vec<_> = TITLES
        .iter()
        .map(|&(id, section, kind, title, sort)| item(id, section, kind, title, sort, None))
        .collect();
    items.push(item(PILOT, SERIES, "episode", "Pilot", None, Some(24_569)));
    write::insert_items(connection, items).await.unwrap();

    person::Entity::insert_many([person::ActiveModel {
        id: ActiveValue::Set(1),
        name: ActiveValue::Set("Tom Hanks".to_owned()),
        provider_key: ActiveValue::Set(Some("5d776826e6d55c002040af60".to_owned())),
        thumb_url: ActiveValue::Set(None),
    }])
    .exec(connection)
    .await
    .unwrap();
    let label = |id, kind, name: &str, person| tag::ActiveModel {
        id: ActiveValue::Set(id),
        kind: ActiveValue::Set(tag::kind_name(kind).to_owned()),
        name: ActiveValue::Set(name.to_owned()),
        person_id: ActiveValue::Set(person),
    };
    write::insert_tags(
        connection,
        vec![
            label(TOM_HANKS, TagKind::Actor, "Tom Hanks", Some(1)),
            label(COLIN_HANKS, TagKind::Actor, "Colin Hanks", None),
            label(TOM_HARDY, TagKind::Actor, "Tom Hardy", None),
            label(HANKS_DIRECTOR, TagKind::Director, "Tom Hanks", Some(1)),
        ],
    )
    .await
    .unwrap();
    let attach = |item, tag| item_tag::ActiveModel {
        item_id: ActiveValue::Set(item),
        tag_id: ActiveValue::Set(tag),
        position: ActiveValue::Set(0),
        role: ActiveValue::Set(None),
    };
    item_tag::Entity::insert_many([
        attach(470_473, TOM_HANKS),
        attach(279_798, COLIN_HANKS),
        attach(333_633, TOM_HARDY),
        attach(465_594, HANKS_DIRECTOR),
    ])
    .exec(connection)
    .await
    .unwrap();
}

async fn repository() -> SqlMetadataRepository {
    let connection = open_plex_db::connect("sqlite::memory:").await.unwrap();
    seed(&connection).await;
    SqlMetadataRepository::new(connection)
}

fn forward(n: u32) -> Window {
    Window::new(None, None, Some(n), None).unwrap()
}

fn search(term: &str, kinds: &[ItemKind]) -> ClosestTitles {
    ClosestTitles {
        term: term.to_owned(),
        kinds: kinds.to_vec(),
        sections: None,
    }
}

/// Every title of these kinds ranked whole: what the bands must agree with.
fn ranked_whole(term: &str, kinds: &[&str]) -> Vec<(u32, i64)> {
    let distance = Distance::new(&closest::term_key(term));
    let mut ranked: Vec<(u32, String, i64)> = TITLES
        .iter()
        .filter(|(_, _, kind, _, _)| kinds.contains(kind))
        .map(|&(id, _, _, title, sort)| {
            let key = closest::title_key(sort, Some(title));
            (distance.to(&key), key, id)
        })
        .collect();
    ranked.sort();
    ranked
        .into_iter()
        .map(|(distance, _, id)| (distance, id))
        .collect()
}

async fn ranked(
    repository: &SqlMetadataRepository,
    query: &ClosestTitles,
    window: Window,
) -> Vec<(u32, i64)> {
    repository
        .closest_titles(query, window)
        .await
        .unwrap()
        .items
        .iter()
        .map(|found| (found.distance, found.item.id.get()))
        .collect()
}

#[tokio::test]
async fn the_bands_find_what_ranking_every_title_finds() {
    let repository = repository().await;
    let films_and_series = [ItemKind::Movie, ItemKind::Show];
    for term in [
        "dune",
        "the office",
        "kaamelott",
        "breaking bad",
        "la meute",
        "zzqxw",
        " ",
    ] {
        let whole = ranked_whole(term, &["movie", "show"]);
        for limit in [1_u32, 3, 10, 50] {
            let expected: Vec<_> = whole
                .iter()
                .copied()
                .take(usize::try_from(limit).unwrap())
                .collect();
            assert_eq!(
                ranked(
                    &repository,
                    &search(term, &films_and_series),
                    forward(limit)
                )
                .await,
                expected,
                "{term:?} at {limit}"
            );
        }
    }
}

#[tokio::test]
async fn ranks_by_the_measured_distances() {
    let repository = repository().await;
    let found = ranked(&repository, &search("dune", &[ItemKind::Movie]), forward(3)).await;
    // As the reference publishes them: Cube and Duel both 2, then in folded title order.
    assert_eq!(found, [(0, 470_473), (2, 279_798), (2, 465_594)]);
    // The article leaves the term, and the sort title is compared.
    let found = ranked(
        &repository,
        &search("the office", &[ItemKind::Show]),
        forward(1),
    )
    .await;
    assert_eq!(found, [(0, 260_220)]);
}

#[tokio::test]
async fn pages_forward_and_backward_through_the_ranking() {
    let repository = repository().await;
    let query = search("dune", &[ItemKind::Movie, ItemKind::Show]);
    let whole = ranked_whole("dune", &["movie", "show"]);

    let first = repository.closest_titles(&query, forward(4)).await.unwrap();
    assert!(first.has_next && !first.has_previous);
    let after = first.items.last().unwrap().cursor();
    let next = Window::new(Some(after), None, Some(4), None).unwrap();
    assert_eq!(ranked(&repository, &query, next).await, whole[4..8]);

    // The farthest, read from the far end.
    let last = Window::new(None, None, None, Some(2)).unwrap();
    assert_eq!(
        ranked(&repository, &query, last).await,
        whole[whole.len() - 2..]
    );
    // Before a cursor, the ones just closer than it.
    let before = first.items[3].cursor();
    let window = Window::new(None, Some(before), None, Some(2)).unwrap();
    assert_eq!(ranked(&repository, &query, window).await, whole[1..3]);
    // A skip lands in the middle.
    let skipped = forward(2).skipping(5);
    assert_eq!(ranked(&repository, &query, skipped).await, whole[5..7]);
}

#[tokio::test]
async fn keeps_to_the_kinds_and_libraries_asked() {
    let repository = repository().await;
    let episodes = ranked(
        &repository,
        &search("pilot", &[ItemKind::Episode]),
        forward(5),
    )
    .await;
    assert_eq!(episodes, [(0, PILOT)]);
    let mut query = search("dune", &[ItemKind::Movie, ItemKind::Show]);
    query.sections = Some(vec![SectionId::new(SERIES)]);
    let found = ranked(&repository, &query, forward(50)).await;
    assert!(found
        .iter()
        .all(|(_, id)| [200_640, 260_220, 305_531, 24_569, 497_485].contains(id)));
    assert!(ranked(&repository, &search("dune", &[]), forward(5))
        .await
        .is_empty());
}

#[tokio::test]
async fn an_edited_title_is_compared_as_edited() {
    let repository = repository().await;
    let edit = ItemEdit::new(
        ItemKind::Movie,
        vec![FieldEdit::Title(Some(
            "Kaamelott : Premier volet".to_owned(),
        ))],
        [],
        [],
    )
    .unwrap();
    repository
        .edit_item(RatingKey::new(213_416), &edit)
        .await
        .unwrap();
    let found = ranked(
        &repository,
        &search("kaamelott : premier volet", &[ItemKind::Movie]),
        forward(1),
    )
    .await;
    assert_eq!(found, [(0, 213_416)]);
}

async fn completed_titles(repository: &SqlMetadataRepository, text: &str) -> Vec<i64> {
    let filter = ItemFilter {
        kind: Some(ItemKind::Movie),
        title_completion: Some(text.to_owned()),
        ..ItemFilter::default()
    };
    let query = ItemQuery::new(SectionId::new(FILMS)).filtered(filter);
    repository
        .items(&query, forward(50))
        .await
        .unwrap()
        .items
        .iter()
        .map(|listed| listed.item.id.get())
        .collect()
}

#[tokio::test]
async fn completes_a_phrase_beginning_a_word_of_the_title() {
    let repository = repository().await;
    // In title order: Crocodile Dundee, Dune, the two parts of Dune, The First Slam Dunk.
    assert_eq!(
        completed_titles(&repository, "DUN").await,
        [323_535, 470_473, 440_223, 396_710, 438_966]
    );
    assert_eq!(completed_titles(&repository, "first sl").await, [438_966]);
    assert!(completed_titles(&repository, "dun ").await.is_empty());
    // A hyphen begins no word, and accents count.
    assert!(completed_titles(&repository, "aim")
        .await
        .contains(&309_731));
    assert!(!completed_titles(&repository, "aim")
        .await
        .contains(&511_259));
    assert_eq!(completed_titles(&repository, "aime").await, [465_488]);
    // A capital outside ASCII, typed in lower case.
    assert_eq!(completed_titles(&repository, "à tous").await, [309_731]);
    assert_eq!(completed_titles(&repository, "ève").await, [261_076]);
    // Wildcards are text.
    assert_eq!(completed_titles(&repository, "100%").await, [433_060]);
    assert!(completed_titles(&repository, "%").await.is_empty());
    assert!(completed_titles(&repository, "_une").await.is_empty());
    assert!(completed_titles(&repository, "").await.is_empty());
}

#[tokio::test]
async fn completes_the_names_of_one_kind_of_tag() {
    let repository = repository().await;
    let query = ItemQuery::new(SectionId::new(FILMS));
    let actors = TagScope {
        kind: TagKind::Actor,
        billed_within: None,
    };
    let names = |text: &'static str| {
        let repository = &repository;
        let query = &query;
        async move {
            repository
                .tags_completing(query, actors, text, forward(10))
                .await
                .unwrap()
                .items
                .into_iter()
                .map(|tag| tag.name)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(names("hanks").await, ["Colin Hanks", "Tom Hanks"]);
    assert_eq!(names("TOM H").await, ["Tom Hanks", "Tom Hardy"]);
    assert!(names("anks").await.is_empty());
}
