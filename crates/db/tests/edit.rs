//! Storage tests for editing items: what an edit writes, and what it derives with it.
//!
//! The items are the reference's test library, as its database held them before being edited
//! there: a film named after its file, and a series with one episode.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use klapz_core::edit::{CalendarDate, FieldEdit, ItemEdit, ItemField};
use klapz_core::metadata::{ItemKind, ItemQuery, MetadataError, MetadataRepository};
use klapz_core::page::Window;
use klapz_core::{RatingKey, SectionId};
use klapz_db::entity::{library_section, metadata_item};
use klapz_db::write;
use klapz_db::SqlMetadataRepository;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

const FILM: i64 = 577_986;
const FILE_NAME: &str = "Soul.2020.MULTi.TRUEFRENCH.1080p.WEB.H264-EXTREME_wWw.Extreme-Down.Video";

async fn repository() -> (SqlMetadataRepository, DatabaseConnection) {
    let connection = klapz_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(27),
        kind: ActiveValue::Set("movie".to_owned()),
        title: ActiveValue::Set("klapz tests".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("en-US".to_owned()),
        uuid: ActiveValue::Set("16785456-a589-4342-a96d-58911d98d98b".to_owned()),
        created_at: ActiveValue::Set(1_794_230_243),
        updated_at: ActiveValue::Set(1_794_230_243),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(&connection)
    .await
    .expect("section");
    write::insert_items(
        &connection,
        vec![metadata_item::ActiveModel {
            id: ActiveValue::Set(FILM),
            library_section_id: ActiveValue::Set(27),
            parent_id: ActiveValue::Set(None),
            kind: ActiveValue::Set("movie".to_owned()),
            guid: ActiveValue::Set("tv.plex.agents.none://577986".to_owned()),
            title: ActiveValue::Set(Some(FILE_NAME.to_owned())),
            title_sort: ActiveValue::Set(Some(FILE_NAME.to_owned())),
            sort_key: ActiveValue::Set(FILE_NAME.to_lowercase()),
            original_title: ActiveValue::Set(None),
            summary: ActiveValue::Set(None),
            tagline: ActiveValue::Set(None),
            studio: ActiveValue::Set(None),
            content_rating: ActiveValue::Set(None),
            content_rating_age: ActiveValue::Set(None),
            position: ActiveValue::Set(Some(1)),
            year: ActiveValue::Set(None),
            released_on: ActiveValue::Set(None),
            duration_ms: ActiveValue::Set(Some(6_031_568)),
            rating: ActiveValue::Set(None),
            audience_rating: ActiveValue::Set(None),
            added_at: ActiveValue::Set(1_610_180_026),
            updated_at: ActiveValue::Set(1_794_332_579),
            refreshed_at: ActiveValue::Set(None),
            chapter_source: ActiveValue::Set(None),
        }],
    )
    .await
    .expect("items");
    (SqlMetadataRepository::new(connection.clone()), connection)
}

fn movie_edit(values: Vec<FieldEdit>, lock: &[ItemField], unlock: &[ItemField]) -> ItemEdit {
    ItemEdit::new(
        ItemKind::Movie,
        values,
        lock.iter().copied(),
        unlock.iter().copied(),
    )
    .expect("a valid edit")
}

async fn found(repository: &SqlMetadataRepository, term: &str) -> usize {
    let query = ItemQuery::new(SectionId::new(27)).matching(term);
    let window = Window::new(None, None, Some(10), None).expect("window");
    repository
        .items(&query, window)
        .await
        .expect("page")
        .items
        .len()
}

#[tokio::test]
async fn a_new_title_rewrites_the_sort_title_its_key_and_the_words_search_reads() {
    let (repository, connection) = repository().await;
    assert_eq!(found(&repository, "soul.2020").await, 1);

    let edited = repository
        .edit_item(
            RatingKey::new(FILM),
            &movie_edit(vec![FieldEdit::Title(Some("The Soul".into()))], &[], &[]),
        )
        .await
        .expect("edited");
    assert_eq!(edited.item.title.as_deref(), Some("The Soul"));
    assert_eq!(edited.item.title_sort.as_deref(), Some("Soul"));

    let row = metadata_item::Entity::find_by_id(FILM)
        .one(&connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            row.title.as_deref(),
            row.title_sort.as_deref(),
            row.sort_key.as_str()
        ),
        (Some("The Soul"), Some("Soul"), "soul")
    );
    assert_eq!(found(&repository, "soul.2020").await, 0);
    assert_eq!(found(&repository, "the").await, 1);
    assert_eq!(found(&repository, "soul").await, 1);
}

#[tokio::test]
async fn locks_are_kept_and_read_back_per_item() {
    let (repository, _) = repository().await;
    let film = RatingKey::new(FILM);
    repository
        .edit_item(
            film,
            &movie_edit(
                vec![FieldEdit::Summary(Some("Probe summary".into()))],
                &[ItemField::Summary, ItemField::TitleSort],
                &[],
            ),
        )
        .await
        .expect("locked");
    let edited = repository
        .edit_item(
            film,
            &movie_edit(
                vec![FieldEdit::Title(Some("A Soul Again".into()))],
                &[],
                &[ItemField::Summary],
            ),
        )
        .await
        .expect("unlocked");
    // The sort title was locked, so the new title derives none.
    assert_eq!(edited.item.title_sort.as_deref(), Some(FILE_NAME));
    assert_eq!(edited.item.summary.as_deref(), Some("Probe summary"));
    assert_eq!(edited.locked, vec![ItemField::TitleSort]);
    assert_eq!(
        repository.locked_fields(&[film]).await.expect("locks"),
        vec![(film, ItemField::TitleSort)]
    );
}

#[tokio::test]
async fn a_release_date_writes_its_year_and_clearing_writes_absence() {
    let (repository, connection) = repository().await;
    let film = RatingKey::new(FILM);
    repository
        .edit_item(
            film,
            &movie_edit(
                vec![
                    FieldEdit::ReleasedOn(CalendarDate::parse("2020-12-25")),
                    FieldEdit::Studio(Some("Pixar".into())),
                    FieldEdit::Rating(Some(8.1)),
                ],
                &[],
                &[],
            ),
        )
        .await
        .expect("dated");
    let row = metadata_item::Entity::find_by_id(FILM)
        .one(&connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.year, row.released_on, row.studio.as_deref(), row.rating),
        (Some(2020), Some(1_608_854_400), Some("Pixar"), Some(8.1))
    );

    repository
        .edit_item(
            film,
            &movie_edit(
                vec![
                    FieldEdit::ReleasedOn(None),
                    FieldEdit::Studio(Some(String::new())),
                    FieldEdit::Rating(None),
                ],
                &[],
                &[],
            ),
        )
        .await
        .expect("cleared");
    let row = metadata_item::Entity::find_by_id(FILM)
        .one(&connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.year, row.released_on, row.studio, row.rating),
        (None, None, None, None)
    );
}

#[tokio::test]
async fn an_edit_for_another_kind_finds_no_item() {
    let (repository, _) = repository().await;
    let edit = ItemEdit::new(
        ItemKind::Show,
        vec![FieldEdit::Summary(Some("wrong type".into()))],
        [],
        [],
    )
    .unwrap();
    assert!(matches!(
        repository.edit_item(RatingKey::new(FILM), &edit).await,
        Err(MetadataError::ItemNotFound(_))
    ));
    assert!(matches!(
        repository.edit_item(RatingKey::new(1), &edit).await,
        Err(MetadataError::ItemNotFound(_))
    ));
}
