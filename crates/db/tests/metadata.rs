//! Storage tests for library contents.
//!
//! The subject is keyset pagination. An offset-based pager is easy and wrong — it skips and
//! repeats rows when the set changes under it — so the tests here deliberately include items
//! that share a sort title, because the tiebreaker is the whole point.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use klapz_core::metadata::{FrameRange, FrameSize, ScanType};
use klapz_core::metadata::{
    ItemDepth, ItemFilter, ItemKind, ItemQuery, ItemSort, Listed, MetadataError, MetadataRepository,
};
use klapz_core::page::{Cursor, CursorKey, Direction, Window};
use klapz_core::{MediaId, PartId, RatingKey, SectionId, StreamId};
use klapz_db::entity::{
    library_section, media, media_part, media_stream, metadata_item, stream_dolby_vision,
    stream_loudness,
};
use klapz_db::write;
use klapz_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const SECTION: i64 = 4;

async fn repository() -> SqlMetadataRepository {
    let connection = klapz_db::connect("sqlite::memory:")
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
        refreshed_at: ActiveValue::Set(None),
        chapter_source: ActiveValue::Set(None),
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
            has_thumbnail: ActiveValue::Set(false),
            has_chapter_text_stream: ActiveValue::Set(false),
            content_hash: ActiveValue::Set(None),
            open_subtitles_hash: ActiveValue::Set(None),
            ..Default::default()
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
            has_thumbnail: ActiveValue::Set(true),
            has_chapter_text_stream: ActiveValue::Set(false),
            content_hash: ActiveValue::Set(None),
            open_subtitles_hash: ActiveValue::Set(None),
            ..Default::default()
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
        // Set rather than left to the column default: a batch insert writes NULL for a
        // column one row sets and another does not.
        has_closed_captions: ActiveValue::Set(false),
        has_separate_fields: ActiveValue::Set(false),
        has_hdr10_plus: ActiveValue::Set(false),
        is_embedded_in_video: ActiveValue::Set(false),
        is_original: ActiveValue::Set(false),
        is_dub: ActiveValue::Set(false),
        is_commentary: ActiveValue::Set(false),
        is_hearing_impaired: ActiveValue::Set(false),
        is_visual_impaired: ActiveValue::Set(false),
        has_header_compression: ActiveValue::Set(false),
        required_bandwidths: ActiveValue::Set(None),
        ..Default::default()
    }
}

fn movies() -> ItemQuery {
    ItemQuery::new(SectionId::new(SECTION)).of_kind(ItemKind::Movie)
}

fn page(n: u32) -> Window {
    Window::new(None, None, Some(n), None).expect("window")
}

fn titles(items: &[Listed]) -> Vec<String> {
    items
        .iter()
        .map(|listed| listed.item.title.clone().unwrap_or_default())
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

    let cursor = first.items[1].cursor.clone();
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

    let cursor = first.items[1].cursor.clone();
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
async fn a_term_of_several_words_needs_them_all() {
    let repository = repository().await;

    let found = repository
        .items(&movies().matching("dune part"), page(10))
        .await
        .expect("page");
    assert_eq!(titles(&found.items), ["Dune: Part Two"]);

    let none = repository
        .items(&movies().matching("dune world"), page(10))
        .await
        .expect("page");
    assert!(none.items.is_empty());
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
    // A title cursor holds one value, a cursor by date added two: the date, then the title
    // that breaks its ties. Neither can say where it stands in the other.
    let by_title = movies();
    let by_added = movies().sorted_by(ItemSort::AddedAt, Direction::Ascending);
    let repository = repository().await;

    let title_cursor = repository
        .items(&by_title, page(1))
        .await
        .expect("page")
        .items[0]
        .cursor
        .clone();
    assert!(matches!(title_cursor.keys.as_slice(), [CursorKey::Text(_)]));
    let added_cursor = repository
        .items(&by_added, page(1))
        .await
        .expect("page")
        .items[0]
        .cursor
        .clone();
    assert!(matches!(
        added_cursor.keys.as_slice(),
        [CursorKey::Int(_), CursorKey::Text(_)]
    ));

    for (query, cursor) in [(&by_added, title_cursor), (&by_title, added_cursor)] {
        let window = Window::new(Some(cursor), None, Some(2), None).expect("window");
        let error = repository.items(query, window).await.expect_err("mismatch");
        assert!(matches!(error, MetadataError::CursorMismatch));
    }
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
async fn a_kind_not_held_matches_nothing_in_any_view() {
    let repository = repository().await;
    let query = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
        kind_not_held: true,
        ..ItemFilter::default()
    });
    assert!(repository
        .items(&query, page(10))
        .await
        .expect("page")
        .items
        .is_empty());
    assert_eq!(repository.item_count(&query).await.expect("count"), 0);
    assert!(repository
        .first_characters(&query)
        .await
        .expect("characters")
        .is_empty());
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
        frame: Some(FrameRange {
            reaching: Some(FrameSize {
                width: 2880,
                height: 1540,
            }),
            short_of: None,
        }),
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
        frame: Some(FrameRange {
            reaching: None,
            short_of: Some(FrameSize {
                width: i32::MAX,
                height: i32::MAX,
            }),
        }),
        ..ItemFilter::default()
    });
    let items = repository()
        .await
        .items(&query, page(50))
        .await
        .expect("page")
        .items;
    assert!(items
        .iter()
        .all(|listed| listed.item.kind == ItemKind::Movie));
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
    assert!(items.iter().all(|listed| listed.item.parent_id.is_none()));
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

#[tokio::test]
async fn reads_episode_additions_newest_first_and_resumes_after_the_last() {
    // The three episodes share their added time, so the id breaks the tie, downward.
    let repository = repository().await;
    let section = SectionId::new(SECTION);
    let first = repository
        .episode_additions(section, None, 2)
        .await
        .expect("additions");
    let episodes: Vec<i64> = first
        .iter()
        .map(|addition| addition.episode.get())
        .collect();
    assert_eq!(episodes, [32, 31]);
    assert_eq!(first[0].season, RatingKey::new(22));
    assert_eq!(first[0].show, RatingKey::new(20));

    let last = first.last().expect("a last addition");
    let rest = repository
        .episode_additions(section, Some((last.added_at, last.episode)), 2)
        .await
        .expect("additions");
    let episodes: Vec<i64> = rest.iter().map(|addition| addition.episode.get()).collect();
    assert_eq!(episodes, [30]);
}

/// Values from the reference library: a Dolby Vision profile 8 video track and a DTS track
/// measured for loudness, both in the 4K file.
async fn seed_analysed_tracks(connection: &DatabaseConnection) {
    media_stream::Entity::insert_many([
        media_stream::ActiveModel {
            width: ActiveValue::Set(Some(3840)),
            height: ActiveValue::Set(Some(2160)),
            coded_height: ActiveValue::Set(Some(2160)),
            frame_rate: ActiveValue::Set(Some(23.976)),
            level: ActiveValue::Set(Some(153)),
            profile: ActiveValue::Set(Some("main 10".to_owned())),
            scan_type: ActiveValue::Set(Some("progressive".to_owned())),
            color_transfer: ActiveValue::Set(Some("smpte2084".to_owned())),
            has_scaling_matrix: ActiveValue::Set(Some(false)),
            ..stream(1100, 1001, "video", 0, "hevc", Some("en"), false)
        },
        media_stream::ActiveModel {
            sampling_rate: ActiveValue::Set(Some(48_000)),
            channel_layout: ActiveValue::Set(Some("5.1(side)".to_owned())),
            profile: ActiveValue::Set(Some("dts".to_owned())),
            title: ActiveValue::Set(Some("French DTS 754kbps".to_owned())),
            is_original: ActiveValue::Set(true),
            ..stream(1101, 1001, "audio", 1, "dca", Some("fr"), false)
        },
    ])
    .exec(connection)
    .await
    .expect("streams");
    stream_dolby_vision::Entity::insert(stream_dolby_vision::ActiveModel {
        stream_id: ActiveValue::Set(1100),
        version: ActiveValue::Set("1.0".to_owned()),
        profile: ActiveValue::Set(8),
        level: ActiveValue::Set(6),
        base_layer_compatibility: ActiveValue::Set(Some(1)),
        has_base_layer: ActiveValue::Set(true),
        has_enhancement_layer: ActiveValue::Set(false),
        has_rpu: ActiveValue::Set(true),
    })
    .exec(connection)
    .await
    .expect("dolby vision");
    stream_loudness::Entity::insert(stream_loudness::ActiveModel {
        stream_id: ActiveValue::Set(1101),
        analysis_version: ActiveValue::Set("0.02".to_owned()),
        integrated: ActiveValue::Set(-17.79),
        loudness_range: ActiveValue::Set(21.4),
        true_peak: ActiveValue::Set(-0.37),
        threshold: ActiveValue::Set(-29.89),
        gain_offset: ActiveValue::Set(0.42),
    })
    .exec(connection)
    .await
    .expect("loudness");
}

#[tokio::test]
async fn reads_a_tracks_analysis_with_its_dolby_vision_and_loudness() {
    let connection = klapz_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    seed_analysed_tracks(&connection).await;
    let repository = SqlMetadataRepository::new(connection);

    let tracks = repository
        .streams_for(&[PartId::new(1001)])
        .await
        .expect("streams");
    let video = tracks
        .iter()
        .find(|track| track.id == StreamId::new(1100))
        .expect("video");
    let analysis = video
        .video
        .as_ref()
        .expect("a video track has a video analysis");
    assert!(video.audio.is_none() && video.subtitle.is_none());
    assert_eq!(analysis.width, Some(3840));
    assert_eq!(analysis.scan_type, Some(ScanType::Progressive));
    assert_eq!(analysis.color.transfer.as_deref(), Some("smpte2084"));
    // Stored as false, which is not the same as never analysed.
    assert_eq!(analysis.has_scaling_matrix, Some(false));
    assert_eq!(analysis.anamorphic, None);
    let dolby = analysis.dolby_vision.as_ref().expect("dolby vision");
    assert_eq!((dolby.profile, dolby.level), (8, 6));
    assert_eq!(dolby.base_layer_compatibility, Some(1));

    let audio = tracks
        .iter()
        .find(|track| track.id == StreamId::new(1101))
        .expect("audio");
    assert!(audio.traits.original);
    assert_eq!(audio.title.as_deref(), Some("French DTS 754kbps"));
    let sound = audio
        .audio
        .as_ref()
        .expect("an audio track has an audio analysis");
    assert_eq!(sound.channel_layout.as_deref(), Some("5.1(side)"));
    assert_eq!(
        sound.loudness.as_ref().map(|loudness| loudness.integrated),
        Some(-17.79)
    );
    // The other track of the file has neither record.
    assert!(tracks
        .iter()
        .filter(|track| track.id != StreamId::new(1100))
        .all(|track| track
            .video
            .as_ref()
            .is_none_or(|video| video.dolby_vision.is_none())));

    let files = repository
        .files_for(&[MediaId::new(101)])
        .await
        .expect("files");
    assert!(files[0].layout.has_thumbnail);
    assert_eq!(files[0].layout.has_64bit_offsets, None);
}

#[tokio::test]
async fn a_frame_range_keeps_what_reaches_one_size_and_falls_short_of_another() {
    // The 1920-wide version of Jurassic World is 1080p: it reaches 1902×1070 and falls
    // short of 4K. Its 3840-wide version is not, so only one of its versions answers.
    let range = |reaching: (i32, i32), short_of: (i32, i32)| FrameRange {
        reaching: Some(FrameSize {
            width: reaching.0,
            height: reaching.1,
        }),
        short_of: Some(FrameSize {
            width: short_of.0,
            height: short_of.1,
        }),
    };
    let repository = repository().await;
    let listed = |frame: FrameRange| {
        let query = ItemQuery::new(SectionId::new(SECTION)).filtered(ItemFilter {
            frame: Some(frame),
            ..ItemFilter::default()
        });
        let repository = &repository;
        async move {
            titles(
                &repository
                    .items(&query, page(50))
                    .await
                    .expect("page")
                    .items,
            )
        }
    };
    assert!(listed(range((1902, 1070), (2880, 1540)))
        .await
        .contains(&"Jurassic World".to_owned()));
    // Nothing is short of 1×1.
    assert!(listed(range((0, 0), (1, 1))).await.is_empty());
}
