//! Storage tests for library contents.
//!
//! The subject is keyset pagination. An offset-based pager is easy and wrong — it skips and
//! repeats rows when the set changes under it — so the tests here deliberately include items
//! that share a sort title, because the tiebreaker is the whole point.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::metadata::{
    ItemDepth, ItemFilter, ItemKind, ItemQuery, ItemSort, MetadataError, MetadataRepository,
};
use open_plex_core::page::{Cursor, CursorKey, Direction, Window};
use open_plex_core::{MediaId, PartId, RatingKey, SectionId};
use open_plex_db::entity::{library_section, media, media_part, media_stream, metadata_item};
use open_plex_db::write;
use open_plex_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const SECTION: i64 = 4;

async fn repository() -> SqlMetadataRepository {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    SqlMetadataRepository::new(connection)
}

#[derive(Clone, Copy)]
struct Seed<'a> {
    id: i64,
    kind: &'a str,
    parent: Option<i64>,
    title: &'a str,
    sort_title: &'a str,
    position: Option<i32>,
    added_at: i64,
    year: Option<i32>,
    studio: Option<&'a str>,
    content_rating: Option<&'a str>,
}

fn item(
    id: i64,
    kind: &str,
    parent: Option<i64>,
    title: &str,
    sort_title: &str,
    position: Option<i32>,
    added_at: i64,
) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(SECTION),
        parent_id: ActiveValue::Set(parent),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("plex://{kind}/{id}")),
        title: ActiveValue::Set((!title.is_empty()).then(|| title.to_owned())),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set(sort_title.to_lowercase()),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(position),
        year: ActiveValue::Set(None),
        released_on: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(added_at),
        updated_at: ActiveValue::Set(0),
    }
}

/// A movie with the fields the filter tests need.
fn movie(seed: Seed<'_>) -> metadata_item::ActiveModel {
    let mut model = item(
        seed.id,
        seed.kind,
        seed.parent,
        seed.title,
        seed.sort_title,
        seed.position,
        seed.added_at,
    );
    model.year = ActiveValue::Set(seed.year);
    model.studio = ActiveValue::Set(seed.studio.map(ToOwned::to_owned));
    model.content_rating = ActiveValue::Set(seed.content_rating.map(ToOwned::to_owned));
    model
}

fn film(
    id: i64,
    title: &str,
    sort_title: &str,
    added_at: i64,
    year: i32,
    studio: &str,
    content_rating: &str,
) -> metadata_item::ActiveModel {
    movie(Seed {
        id,
        kind: "movie",
        parent: None,
        title,
        sort_title,
        position: None,
        added_at,
        year: Some(year),
        studio: Some(studio),
        content_rating: Some(content_rating),
    })
}

/// Five movies, two of which share a sort title, plus one show with two seasons and three
/// episodes, plus one movie's media chain.
async fn seed(connection: &DatabaseConnection) {
    seed_section(connection).await;
    seed_items(connection).await;
    seed_media(connection).await;
    seed_parts(connection).await;
    seed_streams(connection).await;
}

async fn seed_section(connection: &DatabaseConnection) {
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(SECTION),
        kind: ActiveValue::Set("movie".to_owned()),
        title: ActiveValue::Set("Films".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set("uuid-4".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(connection)
    .await
    .expect("section");
}

async fn seed_items(connection: &DatabaseConnection) {
    // Through `write::insert_items` rather than the entity directly: that is the one path
    // that also builds the title-word index, and a fixture bypassing it would make the
    // search tests pass against an index production never sees.
    write::insert_items(
        connection,
        vec![
            film(10, "Alien", "Alien", 500, 1979, "Brandywine", "fr/16"),
            // Two films sharing a sort title: the id is what keeps the ordering total.
            film(11, "Dune", "Dune", 400, 2021, "Legendary", "fr/12"),
            film(
                12,
                "Dune: Part Two",
                "Dune",
                300,
                2024,
                "Legendary",
                "fr/12",
            ),
            film(
                13,
                "Jurassic World",
                "Jurassic World",
                200,
                2015,
                "Universal",
                "fr/12",
            ),
            film(14, "Zodiac", "Zodiac", 100, 2007, "Paramount", "fr/16"),
            item(20, "show", None, "The Simpsons", "Simpsons, The", None, 50),
            item(21, "season", Some(20), "", "", Some(1), 50),
            item(22, "season", Some(20), "", "", Some(2), 50),
            item(
                30,
                "episode",
                Some(21),
                "Bart the Genius",
                "Bart the Genius",
                Some(1),
                50,
            ),
            item(
                31,
                "episode",
                Some(21),
                "Homer's Odyssey",
                "Homer's Odyssey",
                Some(2),
                50,
            ),
            item(
                32,
                "episode",
                Some(22),
                "Bart Gets an F",
                "Bart Gets an F",
                Some(1),
                50,
            ),
        ],
    )
    .await
    .expect("items");
}

async fn seed_media(connection: &DatabaseConnection) {
    media::Entity::insert_many([
        media::ActiveModel {
            id: ActiveValue::Set(100),
            metadata_item_id: ActiveValue::Set(13),
            width: ActiveValue::Set(Some(1920)),
            height: ActiveValue::Set(Some(960)),
            aspect_ratio: ActiveValue::Set(Some(2.0)),
            duration_ms: ActiveValue::Set(Some(9_641_185)),
            bitrate: ActiveValue::Set(Some(13_543_444)),
            container: ActiveValue::Set(Some("mkv".to_owned())),
            video_codec: ActiveValue::Set(Some("h264".to_owned())),
            audio_codec: ActiveValue::Set(Some("dca".to_owned())),
            audio_channels: ActiveValue::Set(Some(8)),
            frame_rate: ActiveValue::Set(Some(23.976)),
            interlaced: ActiveValue::Set(false),
            optimized_for_streaming: ActiveValue::Set(false),
            created_at: ActiveValue::Set(0),
            updated_at: ActiveValue::Set(0),
        },
        // A second version of the same film, which 8 of 18,332 films really have.
        media::ActiveModel {
            id: ActiveValue::Set(101),
            metadata_item_id: ActiveValue::Set(13),
            width: ActiveValue::Set(Some(3840)),
            height: ActiveValue::Set(Some(2160)),
            aspect_ratio: ActiveValue::Set(Some(2.0)),
            duration_ms: ActiveValue::Set(Some(9_641_185)),
            bitrate: ActiveValue::Set(Some(60_000_000)),
            container: ActiveValue::Set(Some("mkv".to_owned())),
            video_codec: ActiveValue::Set(Some("hevc".to_owned())),
            audio_codec: ActiveValue::Set(Some("truehd".to_owned())),
            audio_channels: ActiveValue::Set(Some(8)),
            frame_rate: ActiveValue::Set(Some(23.976)),
            interlaced: ActiveValue::Set(false),
            optimized_for_streaming: ActiveValue::Set(true),
            created_at: ActiveValue::Set(0),
            updated_at: ActiveValue::Set(0),
        },
    ])
    .exec(connection)
    .await
    .expect("media");
}

async fn seed_parts(connection: &DatabaseConnection) {
    media_part::Entity::insert_many([
        media_part::ActiveModel {
            id: ActiveValue::Set(1000),
            media_id: ActiveValue::Set(100),
            position: ActiveValue::Set(0),
            path: ActiveValue::Set("/mnt/movies/jurassic.mkv".to_owned()),
            size_bytes: ActiveValue::Set(Some(16_321_856_143)),
            duration_ms: ActiveValue::Set(Some(9_641_185)),
            created_at: ActiveValue::Set(0),
            updated_at: ActiveValue::Set(0),
        },
        media_part::ActiveModel {
            id: ActiveValue::Set(1001),
            media_id: ActiveValue::Set(101),
            position: ActiveValue::Set(0),
            path: ActiveValue::Set("/mnt/movies4k/jurassic.mkv".to_owned()),
            size_bytes: ActiveValue::Set(Some(60_000_000_000)),
            duration_ms: ActiveValue::Set(Some(9_641_185)),
            created_at: ActiveValue::Set(0),
            updated_at: ActiveValue::Set(0),
        },
    ])
    .exec(connection)
    .await
    .expect("parts");
}

async fn seed_streams(connection: &DatabaseConnection) {
    media_stream::Entity::insert_many([
        stream(10_000, 1000, "video", 0, "h264", None, false),
        stream(10_001, 1000, "audio", 1, "dca", Some("fr"), true),
        stream(10_002, 1000, "subtitle", 2, "srt", Some("fr"), true),
    ])
    .exec(connection)
    .await
    .expect("streams");
}

fn stream(
    id: i64,
    part: i64,
    kind: &str,
    position: i32,
    codec: &str,
    language: Option<&str>,
    forced: bool,
) -> media_stream::ActiveModel {
    media_stream::ActiveModel {
        id: ActiveValue::Set(id),
        media_part_id: ActiveValue::Set(part),
        kind: ActiveValue::Set(kind.to_owned()),
        position: ActiveValue::Set(position),
        codec: ActiveValue::Set(Some(codec.to_owned())),
        language: ActiveValue::Set(language.map(ToOwned::to_owned)),
        channels: ActiveValue::Set(None),
        bitrate: ActiveValue::Set(None),
        is_default: ActiveValue::Set(true),
        is_forced: ActiveValue::Set(forced),
        external_path: ActiveValue::Set(None),
    }
}

fn movies() -> ItemQuery {
    ItemQuery::new(SectionId::new(SECTION)).of_kind(ItemKind::Movie)
}

fn page(n: u32) -> Window {
    Window::new(None, None, Some(n), None).expect("window")
}

fn titles(items: &[open_plex_core::MetadataItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| item.title.clone().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn lists_a_library_in_sort_title_order() {
    let page = repository()
        .await
        .items(&movies(), page(10))
        .await
        .expect("page");
    assert_eq!(
        titles(&page.items),
        vec![
            "Alien",
            "Dune",
            "Dune: Part Two",
            "Jurassic World",
            "Zodiac"
        ]
    );
}

#[tokio::test]
async fn pages_through_items_sharing_a_sort_title() {
    // The case an offset pager and a key-only cursor both get wrong: `Dune` and
    // `Dune: Part Two` sort identically, so the id must break the tie.
    let repository = repository().await;
    let query = movies();

    let first = repository.items(&query, page(2)).await.expect("first page");
    assert_eq!(titles(&first.items), vec!["Alien", "Dune"]);
    assert!(first.has_next);

    let cursor = Cursor::new(query.cursor_key(&first.items[1]), first.items[1].id.get());
    let second = repository
        .items(
            &query,
            Window::new(Some(cursor), None, Some(2), None).expect("window"),
        )
        .await
        .expect("second page");

    // `Dune: Part Two` must appear exactly once, and must not be skipped.
    assert_eq!(
        titles(&second.items),
        vec!["Dune: Part Two", "Jurassic World"]
    );
    assert!(second.has_previous);
    assert!(second.has_next);
}

#[tokio::test]
async fn pages_descending() {
    let repository = repository().await;
    let query = movies().sorted_by(ItemSort::Title, Direction::Descending);

    let first = repository.items(&query, page(2)).await.expect("page");
    assert_eq!(titles(&first.items), vec!["Zodiac", "Jurassic World"]);

    let cursor = Cursor::new(query.cursor_key(&first.items[1]), first.items[1].id.get());
    let second = repository
        .items(
            &query,
            Window::new(Some(cursor), None, Some(2), None).expect("window"),
        )
        .await
        .expect("page");
    assert_eq!(titles(&second.items), vec!["Dune: Part Two", "Dune"]);
}

#[tokio::test]
async fn a_backward_page_reads_from_the_end_but_returns_reading_order() {
    let page = repository()
        .await
        .items(
            &movies(),
            Window::new(None, None, None, Some(2)).expect("window"),
        )
        .await
        .expect("page");
    assert_eq!(titles(&page.items), vec!["Jurassic World", "Zodiac"]);
    assert!(page.has_previous);
    assert!(!page.has_next);
}

#[tokio::test]
async fn sorts_by_when_items_were_added() {
    let query = movies().sorted_by(ItemSort::AddedAt, Direction::Descending);
    let page = repository()
        .await
        .items(&query, page(3))
        .await
        .expect("page");
    // Added at 500, 400, 300 — newest first.
    assert_eq!(titles(&page.items), vec!["Alien", "Dune", "Dune: Part Two"]);
}

#[tokio::test]
async fn searches_by_prefix_case_insensitively() {
    let repository = repository().await;
    for term in ["dune", "DUNE", "Dune", "dUnE"] {
        let page = repository
            .items(&movies().matching(term), page(10))
            .await
            .expect("page");
        assert_eq!(
            titles(&page.items),
            vec!["Dune", "Dune: Part Two"],
            "{term}"
        );
    }
}

#[tokio::test]
async fn search_matches_a_word_prefix_not_a_substring() {
    let repository = repository().await;

    // Any word of the title, so a later word counts: this is what the reference server does,
    // and matching only the first word finds 45 of the 75 films it returns for `title=le`.
    let found = repository
        .items(&movies().matching("World"), page(10))
        .await
        .expect("page");
    assert_eq!(titles(&found.items), ["Jurassic World"]);

    // Still anchored at a word boundary, though: a substring in the middle of one does not
    // match, which is what keeps the index a range scan rather than a table scan.
    for term in ["orld", "urassic", "une"] {
        let page = repository
            .items(&movies().matching(term), page(10))
            .await
            .expect("page");
        assert!(page.items.is_empty(), "{term} should not match");
    }
}

#[tokio::test]
async fn search_terms_are_never_patterns() {
    // The search is a range on an index, not a `LIKE`, so a wildcard is just a character
    // that happens to be in no word. Asserted anyway: the day someone reaches for `LIKE`
    // here, `%` matching every row is the failure that follows.
    let repository = repository().await;
    for term in ["%", "_", "D%", "Dun_"] {
        let page = repository
            .items(&movies().matching(term), page(10))
            .await
            .expect("page");
        assert!(page.items.is_empty(), "{term} matched something");
    }
}

#[tokio::test]
async fn counts_what_the_query_matches_not_the_page() {
    let repository = repository().await;
    assert_eq!(repository.item_count(&movies()).await.expect("count"), 5);
    assert_eq!(
        repository
            .item_count(&movies().matching("dune"))
            .await
            .expect("count"),
        2
    );
}

#[tokio::test]
async fn lists_children_in_their_own_order() {
    let seasons = repository()
        .await
        .children(RatingKey::new(20), page(10))
        .await
        .expect("seasons");
    assert_eq!(
        seasons.items.iter().map(|s| s.position).collect::<Vec<_>>(),
        vec![Some(1), Some(2)]
    );
    // Seasons carry no title, which is why the schema makes it nullable.
    assert!(seasons.items.iter().all(|s| s.title.is_none()));
}

#[tokio::test]
async fn lists_grandchildren_season_by_season() {
    let repository = repository().await;
    let episodes = repository
        .grandchildren(RatingKey::new(20), page(10))
        .await
        .expect("episodes");
    // Every episode of the show in one list: season 1's two, then season 2's one.
    let by_season: Vec<(Option<i64>, Option<i32>)> = episodes
        .items
        .iter()
        .map(|e| (e.parent_id.map(RatingKey::get), e.position))
        .collect();
    assert_eq!(
        by_season,
        vec![
            (Some(21), Some(1)),
            (Some(21), Some(2)),
            (Some(22), Some(1))
        ]
    );

    // An offset lands in the middle of the list, which is how a Plex client pages.
    let rest = repository
        .grandchildren(RatingKey::new(20), page(10).skipping(2))
        .await
        .expect("episodes");
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.items[0].parent_id, Some(RatingKey::new(22)));
    assert!(rest.has_previous);
}

#[tokio::test]
async fn counts_children_and_grandchildren_in_one_query_each() {
    let repository = repository().await;
    let show = RatingKey::new(20);

    let children = repository.child_counts(&[show]).await.expect("children");
    assert_eq!(children, vec![(show, 2)]);

    // A show's episode count is two levels down.
    let leaves = repository.leaf_counts(&[show]).await.expect("leaves");
    assert_eq!(leaves, vec![(show, 3)]);
}

#[tokio::test]
async fn loads_the_media_chain_in_one_query_per_level() {
    let repository = repository().await;

    let versions = repository
        .versions_for(&[RatingKey::new(13)])
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].video_codec.as_deref(), Some("h264"));
    assert_eq!(versions[1].video_codec.as_deref(), Some("hevc"));
    // Stored as the measured rate, not as a display label.
    assert_eq!(versions[0].frame_rate, Some(23.976));

    let files = repository
        .files_for(&[MediaId::new(100), MediaId::new(101)])
        .await
        .expect("files");
    assert_eq!(files.len(), 2);

    let streams = repository
        .streams_for(&[PartId::new(1000)])
        .await
        .expect("streams");
    assert_eq!(streams.len(), 3);
    assert!(streams.iter().any(|s| s.is_forced));
}

#[tokio::test]
async fn asking_for_nothing_queries_nothing() {
    let repository = repository().await;
    assert!(repository.items_by_id(&[]).await.expect("items").is_empty());
    assert!(repository.versions_for(&[]).await.expect("v").is_empty());
    assert!(repository.files_for(&[]).await.expect("f").is_empty());
    assert!(repository.streams_for(&[]).await.expect("s").is_empty());
    assert!(repository.child_counts(&[]).await.expect("c").is_empty());
}

#[tokio::test]
async fn rejects_a_cursor_that_cannot_locate_a_position() {
    // An id-only cursor says nothing about where it sits in a title-sorted listing. Reusing
    // one after changing the sort field is the way a client hits this.
    let window = Window::new(Some(Cursor::by_id(11)), None, Some(2), None).expect("window");
    let error = repository()
        .await
        .items(&movies(), window)
        .await
        .expect_err("mismatched cursor");
    assert!(matches!(error, MetadataError::CursorMismatch));
}

#[tokio::test]
async fn reports_a_missing_item() {
    let error = repository()
        .await
        .item(RatingKey::new(9999))
        .await
        .expect_err("absent");
    assert!(matches!(error, MetadataError::ItemNotFound(id) if id.get() == 9999));
}

#[tokio::test]
async fn a_cursor_from_one_ordering_is_not_reused_across_orderings() {
    // The sort key's *type* differs per ordering, and a text cursor cannot resume a numeric
    // one. This is why `ItemQuery::cursor_key` and the SQL live next to each other.
    let query = movies();
    let by_added = movies().sorted_by(ItemSort::AddedAt, Direction::Ascending);
    let repository = repository().await;

    let first = repository.items(&query, page(1)).await.expect("page");
    let text_cursor = Cursor::new(query.cursor_key(&first.items[0]), first.items[0].id.get());
    assert!(matches!(text_cursor.key, Some(CursorKey::Text(_))));

    let numeric = by_added.cursor_key(&first.items[0]);
    assert!(matches!(numeric, CursorKey::Int(_)));
}

#[tokio::test]
async fn filters_by_year_range() {
    let repository = repository().await;
    let in_range = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        kind: Some(ItemKind::Movie),
        year_from: Some(2000),
        year_to: Some(2010),
        ..ItemFilter::default()
    });
    let titles = titles(
        &repository
            .items(&in_range, page(10))
            .await
            .expect("page")
            .items,
    );
    // Alien is 1979, Zodiac 2007, Dune 2021, Dune: Part Two 2024, Jurassic World 2015.
    assert_eq!(titles, vec!["Zodiac"]);
}

#[tokio::test]
async fn filters_combine_as_conjunctions() {
    let repository = repository().await;
    let query = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        kind: Some(ItemKind::Movie),
        studio_contains: Some("Legendary".to_owned()),
        year_from: Some(2022),
        ..ItemFilter::default()
    });
    // Both Dunes are Legendary, only one is 2022 or later.
    assert_eq!(
        titles(
            &repository
                .items(&query, page(10))
                .await
                .expect("page")
                .items
        ),
        vec!["Dune: Part Two"]
    );
}

#[tokio::test]
async fn filters_by_content_rating() {
    let query = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        content_rating: Some("fr/16".to_owned()),
        ..ItemFilter::default()
    });
    let items = repository()
        .await
        .items(&query, page(10))
        .await
        .expect("page")
        .items;
    // Both Alien and Zodiac carry this rating.
    assert_eq!(titles(&items), vec!["Alien", "Zodiac"]);
}

#[tokio::test]
async fn filters_by_resolution_without_duplicating_multi_version_items() {
    // Jurassic World has a 1920-wide and a 3840-wide version. A join would return it twice;
    // the `EXISTS` subquery returns it once.
    let query = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        min_width: Some(3840),
        ..ItemFilter::default()
    });
    let items = repository()
        .await
        .items(&query, page(10))
        .await
        .expect("page")
        .items;
    assert_eq!(titles(&items), vec!["Jurassic World"]);
}

#[tokio::test]
async fn a_resolution_filter_excludes_items_without_media() {
    // Shows and seasons have none, so a resolution filter must not return them.
    let query = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        min_width: Some(1),
        ..ItemFilter::default()
    });
    let items = repository()
        .await
        .items(&query, page(50))
        .await
        .expect("page")
        .items;
    assert!(items.iter().all(|item| item.kind == ItemKind::Movie));
}

#[tokio::test]
async fn filters_narrow_the_count_too() {
    let repository = repository().await;
    let all = ItemQuery::new(SectionId::new(SECTION)).of_kind(ItemKind::Movie);
    let narrowed = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        kind: Some(ItemKind::Movie),
        studio_contains: Some("Legendary".to_owned()),
        ..ItemFilter::default()
    });
    assert_eq!(repository.item_count(&all).await.expect("count"), 5);
    assert_eq!(repository.item_count(&narrowed).await.expect("count"), 2);
}

#[tokio::test]
async fn an_empty_filter_covers_the_top_level() {
    // The section holds 5 movies, 1 show, 2 seasons and 3 episodes. Six of those eleven
    // rows have no parent, and those are what a library listing shows: a client browsing a
    // series library wants the series, not every episode beside them.
    let repository = repository().await;
    let query = ItemQuery::new(SectionId::new(SECTION));
    assert_eq!(repository.item_count(&query).await.expect("count"), 6);

    let items = repository
        .items(&query, page(50))
        .await
        .expect("items")
        .items;
    assert!(items.iter().all(|item| item.parent_id.is_none()));
}

#[tokio::test]
async fn the_deep_filter_reaches_every_generation() {
    let query = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        depth: ItemDepth::All,
        ..ItemFilter::default()
    });
    assert_eq!(
        repository().await.item_count(&query).await.expect("count"),
        11
    );
}
