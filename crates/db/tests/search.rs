//! Storage tests for the search across libraries: ranking, and paging a ranked search.
//!
//! Titles, sort titles, original titles and audience ratings are the reference library's, as
//! `/hubs/search` scored them for `dune` and `kaamelott`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::metadata::{
    ItemKind, MetadataItem, MetadataRepository, SearchOrder, SearchQuery,
};
use open_plex_core::page::Window;
use open_plex_core::search;
use open_plex_db::entity::{library_section, metadata_item};
use open_plex_db::{write, SqlMetadataRepository};
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

struct Seed {
    id: i64,
    section: i64,
    kind: &'static str,
    parent: Option<i64>,
    title: &'static str,
    original: Option<&'static str>,
    audience_rating: Option<f64>,
}

const SEEDS: [Seed; 11] = [
    film(470_473, 4, "Dune", None, Some(6.5)),
    film(470_476, 6, "Dune", None, Some(6.5)),
    film(
        440_223,
        4,
        "Dune : Deuxième partie",
        Some("Dune: Part Two"),
        Some(9.5),
    ),
    film(439_274, 6, "Dune: Part Two", None, Some(9.5)),
    film(433_653, 4, "Anatomie d'une chute", None, Some(9.0)),
    film(261_195, 4, "Dune, la naissance", None, None),
    Seed {
        id: 453_809,
        section: 5,
        kind: "show",
        parent: None,
        title: "Dune : Prophecy",
        original: Some("Dune: Prophecy"),
        audience_rating: Some(7.3),
    },
    Seed {
        id: 453_810,
        section: 5,
        kind: "season",
        parent: Some(453_809),
        title: "",
        original: None,
        audience_rating: None,
    },
    Seed {
        id: 455_949,
        section: 5,
        kind: "episode",
        parent: Some(453_810),
        title: "L’ennemi suprême",
        original: Some("Dune: Prophecy"),
        audience_rating: Some(6.9),
    },
    Seed {
        id: 200_641,
        section: 5,
        kind: "season",
        parent: Some(200_640),
        title: "",
        original: None,
        audience_rating: None,
    },
    Seed {
        id: 200_642,
        section: 5,
        kind: "episode",
        parent: Some(200_641),
        title: "Heat",
        original: None,
        audience_rating: Some(8.7),
    },
];

const fn film(
    id: i64,
    section: i64,
    title: &'static str,
    original: Option<&'static str>,
    audience_rating: Option<f64>,
) -> Seed {
    Seed {
        id,
        section,
        kind: "movie",
        parent: None,
        title,
        original,
        audience_rating,
    }
}

fn model(seed: &Seed) -> metadata_item::ActiveModel {
    let title = (!seed.title.is_empty()).then(|| seed.title.to_owned());
    metadata_item::ActiveModel {
        id: ActiveValue::Set(seed.id),
        library_section_id: ActiveValue::Set(seed.section),
        parent_id: ActiveValue::Set(seed.parent),
        kind: ActiveValue::Set(seed.kind.to_owned()),
        guid: ActiveValue::Set(format!("plex://{}/{}", seed.kind, seed.id)),
        sort_key: ActiveValue::Set(seed.title.to_lowercase()),
        title: ActiveValue::Set(title),
        title_sort: ActiveValue::Set(None),
        original_title: ActiveValue::Set(seed.original.map(ToOwned::to_owned)),
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
        audience_rating: ActiveValue::Set(seed.audience_rating),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
    }
}

fn section(id: i64, kind: &str) -> library_section::ActiveModel {
    library_section::ActiveModel {
        id: ActiveValue::Set(id),
        kind: ActiveValue::Set(kind.to_owned()),
        title: ActiveValue::Set(format!("Library {id}")),
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

async fn seeded() -> (DatabaseConnection, SqlMetadataRepository) {
    let connection = open_plex_db::connect("sqlite::memory:").await.unwrap();
    library_section::Entity::insert_many([
        section(4, "movie"),
        section(5, "show"),
        section(6, "movie"),
    ])
    .exec(&connection)
    .await
    .unwrap();
    let kaamelott = Seed {
        id: 200_640,
        section: 5,
        kind: "show",
        parent: None,
        title: "Kaamelott",
        original: None,
        audience_rating: Some(8.3),
    };
    let models: Vec<_> = std::iter::once(&kaamelott)
        .chain(SEEDS.iter())
        .map(model)
        .collect();
    write::insert_items(&connection, models).await.unwrap();
    (connection.clone(), SqlMetadataRepository::new(connection))
}

async fn search_all(repository: &SqlMetadataRepository, query: &SearchQuery) -> Vec<MetadataItem> {
    repository
        .search(query, Window::new(None, None, Some(100), None).unwrap())
        .await
        .unwrap()
        .items
}

fn ids(items: &[MetadataItem]) -> Vec<i64> {
    items.iter().map(|item| item.id.get()).collect()
}

fn scores(query: &SearchQuery, items: &[MetadataItem]) -> Vec<f64> {
    items
        .iter()
        .map(|item| search::score(item.kind, query.rung(item), item.audience_rating))
        .collect()
}

#[tokio::test]
async fn ranks_by_the_score_the_core_gives() {
    let (_, repository) = seeded().await;
    let query = SearchQuery::new("dune");
    let found = search_all(&repository, &query).await;
    // The exact titles, then a first word, then a title starting with the term, each rung
    // by audience rating; the episode two hundredths under its rung. `Anatomie d'une chute`
    // is not found: `d'une` is one word, which `dune` does not begin.
    assert_eq!(
        ids(&found),
        [470_473, 470_476, 440_223, 453_809, 439_274, 261_195, 455_949]
    );
    let scores = scores(&query, &found);
    assert_eq!(
        scores,
        [0.93065, 0.93065, 0.73095, 0.73073, 0.53095, 0.53, 0.51069]
    );
    assert!(scores.windows(2).all(|pair| pair[0] >= pair[1]));
}

#[tokio::test]
async fn pages_a_ranked_search_without_skipping_or_repeating() {
    let (_, repository) = seeded().await;
    let query = SearchQuery::new("dune");
    let whole = ids(&search_all(&repository, &query).await);

    let mut walked = Vec::new();
    let mut after = None;
    loop {
        let window = Window::new(after.clone(), None, Some(3), None).unwrap();
        let page = repository.search(&query, window).await.unwrap();
        walked.extend(ids(&page.items));
        after = page.items.last().map(|item| query.cursor(item));
        if !page.has_next {
            break;
        }
    }
    assert_eq!(walked, whole);

    // Backwards from the end, the same order read the other way.
    let mut walked_back = Vec::new();
    let mut before = None;
    loop {
        let window = Window::new(None, before.clone(), None, Some(3)).unwrap();
        let page = repository.search(&query, window).await.unwrap();
        walked_back.splice(0..0, ids(&page.items));
        before = page.items.first().map(|item| query.cursor(item));
        if !page.has_previous {
            break;
        }
    }
    assert_eq!(walked_back, whole);
}

#[tokio::test]
async fn a_cursor_from_the_other_order_is_refused() {
    let (_, repository) = seeded().await;
    let by_title = SearchQuery {
        order: SearchOrder::Title,
        ..SearchQuery::new("dune")
    };
    let first = search_all(&repository, &by_title).await;
    let cursor = by_title.cursor(&first[0]);
    let ranked = SearchQuery::new("dune");
    let window = Window::new(Some(cursor), None, Some(3), None).unwrap();
    assert!(repository.search(&ranked, window).await.is_err());
}

#[tokio::test]
async fn still_orders_by_title_when_asked() {
    let (_, repository) = seeded().await;
    let query = SearchQuery {
        order: SearchOrder::Title,
        kinds: vec![ItemKind::Movie],
        ..SearchQuery::new("dune")
    };
    let titles: Vec<String> = search_all(&repository, &query)
        .await
        .into_iter()
        .filter_map(|item| item.title)
        .collect();
    assert_eq!(
        titles,
        [
            "Dune",
            "Dune",
            "Dune : Deuxième partie",
            "Dune, la naissance",
            "Dune: Part Two"
        ]
    );
}

#[tokio::test]
async fn finds_an_episode_through_its_series_only_when_asked() {
    let (_, repository) = seeded().await;
    let episodes = SearchQuery {
        kinds: vec![ItemKind::Episode],
        ..SearchQuery::new("kaamelott")
    };
    assert!(search_all(&repository, &episodes).await.is_empty());

    let through_series = SearchQuery {
        episodes_by_series: true,
        ..episodes
    };
    let found = search_all(&repository, &through_series).await;
    assert_eq!(ids(&found), [200_642]);
    // A match through the series ranks as a match anywhere: 0.31, and 8.7 for its rating.
    assert_eq!(scores(&through_series, &found), [0.31087]);
}

#[tokio::test]
async fn an_edit_moves_an_item_along_the_ranking() {
    let (connection, repository) = seeded().await;
    let query = SearchQuery {
        kinds: vec![ItemKind::Movie],
        ..SearchQuery::new("anatomie d'une chute")
    };
    let mut film = search_all(&repository, &query).await.remove(0);
    assert_eq!(scores(&query, std::slice::from_ref(&film)), [0.9309]);

    film.title = Some("Anatomy of a Fall".to_owned());
    film.original_title = Some("Anatomie d'une chute".to_owned());
    write::update_item(&connection, &film, &std::collections::BTreeSet::new())
        .await
        .unwrap();
    let found = search_all(&repository, &query).await;
    // Now the original title alone holds the term: a leading word, not an exact title.
    assert_eq!(scores(&query, &found), [0.7309]);
}
