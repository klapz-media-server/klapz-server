//! Storage tests against a real database.
//!
//! `SQLite` in memory, with the schema created by running the migrations — so every test run
//! exercises those too. Mocking a query builder would only test the mock; what needs proving
//! here is that the SQL we generate means what we think it means.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::page::Window;
use open_plex_core::{LibraryError, LibraryRepository, SectionId, SectionKind};
use open_plex_db::entity::{library_section, section_root};
use open_plex_db::SqlLibraryRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

async fn repository() -> SqlLibraryRepository {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    SqlLibraryRepository::new(connection)
}

/// Six libraries with ids 4, 5, 6, 8, 9, 17 — deliberately not contiguous, so a cursor
/// cannot be confused with an offset.
async fn seed(connection: &DatabaseConnection) {
    let sections = [
        (4, "movie", "VF - Films", false),
        (5, "show", "VF - Series", false),
        (6, "movie", "VO - Movies", false),
        (8, "movie", "VO - Movies 3D", true),
        (9, "movie", "VF - Films 3D", true),
        (17, "movie", "VF - Films UHD", false),
    ];

    library_section::Entity::insert_many(sections.map(|(id, kind, title, hidden)| {
        library_section::ActiveModel {
            id: ActiveValue::Set(id),
            kind: ActiveValue::Set(kind.to_owned()),
            title: ActiveValue::Set(title.to_owned()),
            title_sort: ActiveValue::Set(None),
            language: ActiveValue::Set("fr-FR".to_owned()),
            uuid: ActiveValue::Set(format!("uuid-{id}")),
            created_at: ActiveValue::Set(1_477_835_115),
            updated_at: ActiveValue::Set(1_792_916_300),
            scanned_at: ActiveValue::Set(Some(1_792_916_245)),
            content_changed_at: ActiveValue::Set(8_457_612_157_739_110_435),
            hidden: ActiveValue::Set(hidden),
        }
    }))
    .exec(connection)
    .await
    .expect("sections inserted");

    // Two roots on section 4, one on section 5, none on the rest.
    section_root::Entity::insert_many(
        [
            (17_i64, 4_i64, "/mnt/movies/Multi"),
            (18, 4, "/mnt/movies/VF"),
            (19, 5, "/mnt/shows/Multi"),
        ]
        .map(|(id, section_id, path)| section_root::ActiveModel {
            id: ActiveValue::Set(id),
            section_id: ActiveValue::Set(section_id),
            path: ActiveValue::Set(path.to_owned()),
            created_at: ActiveValue::Set(1_477_835_115),
        }),
    )
    .exec(connection)
    .await
    .expect("roots inserted");
}

fn window(
    after: Option<i64>,
    before: Option<i64>,
    first: Option<u32>,
    last: Option<u32>,
) -> Window {
    Window::by_id(after, before, first, last).expect("valid window")
}

fn ids(sections: &[open_plex_core::LibrarySection]) -> Vec<i64> {
    sections.iter().map(|section| section.id.get()).collect()
}

#[tokio::test]
async fn migrations_create_a_usable_schema() {
    // Connecting runs them; a failure here means the migration itself is broken.
    let page = repository()
        .await
        .sections(window(None, None, Some(100), None))
        .await
        .expect("query succeeds");
    assert_eq!(page.items.len(), 6);
}

#[tokio::test]
async fn pages_forward_in_ascending_id_order() {
    let repository = repository().await;

    let first = repository
        .sections(window(None, None, Some(2), None))
        .await
        .expect("first page");
    assert_eq!(ids(&first.items), vec![4, 5]);
    assert!(first.has_next);
    assert!(!first.has_previous);

    let second = repository
        .sections(window(Some(5), None, Some(2), None))
        .await
        .expect("second page");
    assert_eq!(ids(&second.items), vec![6, 8]);
    assert!(second.has_next);
    assert!(second.has_previous);
}

#[tokio::test]
async fn the_last_forward_page_reports_no_next() {
    let page = repository()
        .await
        .sections(window(Some(9), None, Some(2), None))
        .await
        .expect("page");
    assert_eq!(ids(&page.items), vec![17]);
    assert!(!page.has_next);
    assert!(page.has_previous);
}

#[tokio::test]
async fn pages_backward_but_returns_ascending_items() {
    // `last` reads from the far end, yet a caller always wants the page in reading order.
    let page = repository()
        .await
        .sections(window(None, None, None, Some(2)))
        .await
        .expect("page");
    assert_eq!(ids(&page.items), vec![9, 17]);
    assert!(page.has_previous);
    assert!(!page.has_next);
}

#[tokio::test]
async fn a_cursor_is_exclusive_and_not_an_offset() {
    // Ids are non-contiguous on purpose: `after: 6` must mean "greater than 6", not "skip 6
    // rows".
    let page = repository()
        .await
        .sections(window(Some(6), None, Some(10), None))
        .await
        .expect("page");
    assert_eq!(ids(&page.items), vec![8, 9, 17]);
}

#[tokio::test]
async fn before_and_after_narrow_the_range_together() {
    let page = repository()
        .await
        .sections(window(Some(4), Some(17), Some(10), None))
        .await
        .expect("page");
    assert_eq!(ids(&page.items), vec![5, 6, 8, 9]);
}

#[tokio::test]
async fn maps_stored_values_onto_the_domain() {
    let page = repository()
        .await
        .sections(window(None, None, Some(100), None))
        .await
        .expect("page");

    let shows: Vec<_> = page
        .items
        .iter()
        .filter(|section| section.kind == SectionKind::Show)
        .collect();
    assert_eq!(shows.len(), 1);
    assert_eq!(shows[0].title, "VF - Series");

    let hidden: Vec<i64> = page
        .items
        .iter()
        .filter(|section| section.hidden)
        .map(|section| section.id.get())
        .collect();
    assert_eq!(hidden, vec![8, 9]);

    // Wide enough to exceed 32 bits, which is why it is not an i32 anywhere in the stack.
    assert_eq!(page.items[0].content_changed_at, 8_457_612_157_739_110_435);
}

#[tokio::test]
async fn loads_roots_for_several_sections_at_once() {
    let roots = repository()
        .await
        .roots_for(&[SectionId::new(4), SectionId::new(5), SectionId::new(6)])
        .await
        .expect("roots");

    assert_eq!(roots.len(), 3);
    // Each root knows its section, which is what lets the DataLoader group them.
    let for_four: Vec<&str> = roots
        .iter()
        .filter(|root| root.section_id == SectionId::new(4))
        .map(|root| root.path.as_str())
        .collect();
    assert_eq!(for_four, vec!["/mnt/movies/Multi", "/mnt/movies/VF"]);

    // Section 6 has none, and is simply absent rather than an error.
    assert!(!roots
        .iter()
        .any(|root| root.section_id == SectionId::new(6)));
}

#[tokio::test]
async fn asking_for_no_sections_queries_nothing() {
    let roots = repository().await.roots_for(&[]).await.expect("roots");
    assert!(roots.is_empty());
}

#[tokio::test]
async fn reports_a_missing_section_by_id() {
    let error = repository()
        .await
        .section(SectionId::new(9999))
        .await
        .expect_err("absent");
    assert!(matches!(error, LibraryError::SectionNotFound(id) if id.get() == 9999));
}

#[tokio::test]
async fn finds_a_section_by_id() {
    let section = repository()
        .await
        .section(SectionId::new(17))
        .await
        .expect("present");
    assert_eq!(section.title, "VF - Films UHD");
    assert_eq!(section.kind, SectionKind::Movie);
}

#[tokio::test]
async fn an_unset_sort_title_is_null_rather_than_empty() {
    // Plex stores `name_sort = ''` for "unset"; publishing that as an empty string would
    // contradict the schema, which documents `titleSort` as null when the title already
    // sorts correctly.
    let section = repository()
        .await
        .section(SectionId::new(4))
        .await
        .expect("present");
    assert_eq!(section.title_sort, None);
}
