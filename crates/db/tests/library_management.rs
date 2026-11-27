//! Creating, editing and deleting a library in storage: roots kept by path, and a deletion
//! that leaves nothing of the library's items behind and nothing of the others touched.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use klapz_core::language::LanguageTag;
use klapz_core::library::{LibraryEdit, NewLibrary};
use klapz_core::playback::PlaybackRepository;
use klapz_core::{LibraryError, LibraryRepository, SectionId, SectionKind};
use klapz_db::entity::{
    item_marker, item_title_word, library_permission, library_section, metadata_item,
    playback_session, section_root,
};
use klapz_db::{write, SqlLibraryRepository, SqlPlaybackRepository};
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
};

const NOW: i64 = 1_794_406_080;

fn item(
    id: i64,
    section: i64,
    parent: Option<i64>,
    kind: &str,
    title: &str,
) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(section),
        parent_id: ActiveValue::Set(parent),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("local://{id}")),
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
        refreshed_at: ActiveValue::Set(None),
        chapter_source: ActiveValue::Set(None),
    }
}

fn marker(id: i64, item: i64) -> item_marker::ActiveModel {
    item_marker::ActiveModel {
        id: ActiveValue::Set(id),
        item_id: ActiveValue::Set(item),
        owner: ActiveValue::Set(None),
        kind: ActiveValue::Set("intro".to_owned()),
        start_ms: ActiveValue::Set(0),
        end_ms: ActiveValue::Set(Some(1_000)),
        is_final: ActiveValue::Set(false),
        version: ActiveValue::Set(None),
        title: ActiveValue::Set(None),
        color: ActiveValue::Set(None),
    }
}

fn permission(id: i64, section: i64) -> library_permission::ActiveModel {
    library_permission::ActiveModel {
        id: ActiveValue::Set(id),
        user_id: ActiveValue::Set(5),
        section_id: ActiveValue::Set(section),
        role: ActiveValue::Set("viewer".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
    }
}

fn session(id: i64, item: i64) -> playback_session::ActiveModel {
    playback_session::ActiveModel {
        id: ActiveValue::Set(id),
        session_key: ActiveValue::Set(format!("u5~session-{id}")),
        user_id: ActiveValue::Set(5),
        playback_id: ActiveValue::Set(None),
        item_id: ActiveValue::Set(item),
        part_id: ActiveValue::Set(1),
        client: ActiveValue::Set(None),
        product: ActiveValue::Set(None),
        state: ActiveValue::Set("playing".to_owned()),
        video_decision: ActiveValue::Set("directplay".to_owned()),
        audio_decision: ActiveValue::Set("directplay".to_owned()),
        position_ms: ActiveValue::Set(0),
        video_bitrate: ActiveValue::Set(None),
        audio_bitrate: ActiveValue::Set(None),
        width: ActiveValue::Set(None),
        height: ActiveValue::Set(None),
        started_at: ActiveValue::Set(NOW),
        updated_at: ActiveValue::Set(NOW),
        expires_at: ActiveValue::Set(NOW + 300),
        terminated_at: ActiveValue::Set(None),
        termination_reason: ActiveValue::Set(None),
    }
}

/// A film library and a series library, each with what refers to its items.
async fn seeded() -> (
    DatabaseConnection,
    SqlLibraryRepository,
    SectionId,
    SectionId,
) {
    let connection = klapz_db::connect("sqlite::memory:").await.unwrap();
    let library = SqlLibraryRepository::new(connection.clone());
    let new = |kind, title: &str, root: &str| NewLibrary {
        kind,
        title: title.to_owned(),
        language: LanguageTag::parse("fr-FR").unwrap(),
        roots: vec![root.to_owned()],
    };
    let films = library
        .create(new(SectionKind::Movie, "Films", "/media/films"), NOW)
        .await
        .unwrap()
        .id;
    let series = library
        .create(new(SectionKind::Show, "Séries", "/media/series"), NOW)
        .await
        .unwrap()
        .id;
    write::insert_items(
        &connection,
        vec![
            item(100, films.get(), None, "movie", "Soul"),
            item(200, series.get(), None, "show", "Bull"),
            item(201, series.get(), Some(200), "season", "Season 1"),
            item(202, series.get(), Some(201), "episode", "Pilot"),
        ],
    )
    .await
    .unwrap();
    item_marker::Entity::insert_many([marker(1, 100), marker(2, 202)])
        .exec(&connection)
        .await
        .unwrap();
    library_permission::Entity::insert_many([
        permission(1, films.get()),
        permission(2, series.get()),
    ])
    .exec(&connection)
    .await
    .unwrap();
    playback_session::Entity::insert_many([session(1, 100), session(2, 202)])
        .exec(&connection)
        .await
        .unwrap();
    (connection, library, films, series)
}

#[tokio::test]
async fn a_new_library_takes_the_next_number_and_its_roots() {
    let (_, library, films, series) = seeded().await;
    assert_eq!((films.get(), series.get()), (1, 2));
    let created = library.section(series).await.unwrap();
    assert_eq!(created.title, "Séries");
    assert_eq!(created.kind, SectionKind::Show);
    assert_eq!(created.created_at, NOW);
    let roots = library.roots_for(&[films, series]).await.unwrap();
    let paths: Vec<&str> = roots.iter().map(|root| root.path.as_str()).collect();
    assert_eq!(paths, ["/media/films", "/media/series"]);
}

#[tokio::test]
async fn an_edit_keeps_the_roots_it_names_and_clears_the_sort_title() {
    let (connection, library, films, _) = seeded().await;
    let mut sorted: library_section::ActiveModel = library_section::Entity::find_by_id(films.get())
        .one(&connection)
        .await
        .unwrap()
        .unwrap()
        .into();
    sorted.title_sort = ActiveValue::Set(Some("films".to_owned()));
    library_section::Entity::update(sorted)
        .exec(&connection)
        .await
        .unwrap();
    let before = library.roots_for(&[films]).await.unwrap();

    let edited = library
        .edit(
            films,
            &LibraryEdit {
                title: Some("Films VF".to_owned()),
                language: LanguageTag::parse("en-US"),
                roots: Some(vec!["/media/films".to_owned(), "/media/vf".to_owned()]),
            },
            NOW + 1,
        )
        .await
        .unwrap();
    assert_eq!(edited.title, "Films VF");
    assert_eq!(edited.title_sort, None);
    assert_eq!(edited.language.to_string(), "en-US");
    assert_eq!(edited.updated_at, NOW + 1);
    let roots = library.roots_for(&[films]).await.unwrap();
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0], before[0], "a kept root keeps its id and date");
    assert_eq!(roots[1].path, "/media/vf");

    library
        .edit(
            films,
            &LibraryEdit {
                roots: Some(vec!["/media/vf".to_owned()]),
                ..LibraryEdit::default()
            },
            NOW + 2,
        )
        .await
        .unwrap();
    let roots = library.roots_for(&[films]).await.unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path, "/media/vf");

    let missing = library
        .edit(SectionId::new(99), &LibraryEdit::default(), NOW)
        .await;
    assert!(matches!(missing, Err(LibraryError::SectionNotFound(id)) if id.get() == 99));
}

#[tokio::test]
async fn a_deleted_library_takes_its_items_and_what_refers_to_them() {
    let (connection, library, films, series) = seeded().await;
    let playback = SqlPlaybackRepository::new(connection.clone());
    assert_eq!(
        playback.keys_in_library(series).await.unwrap(),
        ["u5~session-2"]
    );

    assert!(library.delete(series).await.unwrap());
    assert!(!library.delete(series).await.unwrap());
    assert!(matches!(
        library.section(series).await,
        Err(LibraryError::SectionNotFound(_))
    ));

    let left = |section: SectionId| {
        metadata_item::Entity::find()
            .filter(metadata_item::Column::LibrarySectionId.eq(section.get()))
            .count(&connection)
    };
    assert_eq!(left(series).await.unwrap(), 0);
    assert_eq!(left(films).await.unwrap(), 1);
    let words: Vec<i64> = item_title_word::Entity::find()
        .all(&connection)
        .await
        .unwrap()
        .into_iter()
        .map(|word| word.item_id)
        .collect();
    assert!(words.iter().all(|item| *item == 100), "{words:?}");
    let markers: Vec<i64> = item_marker::Entity::find()
        .all(&connection)
        .await
        .unwrap()
        .into_iter()
        .map(|marker| marker.item_id)
        .collect();
    assert_eq!(markers, [100]);
    let permissions: Vec<i64> = library_permission::Entity::find()
        .all(&connection)
        .await
        .unwrap()
        .into_iter()
        .map(|permission| permission.section_id)
        .collect();
    assert_eq!(permissions, [films.get()]);
    let sessions: Vec<i64> = playback_session::Entity::find()
        .all(&connection)
        .await
        .unwrap()
        .into_iter()
        .map(|session| session.item_id)
        .collect();
    assert_eq!(sessions, [100]);
    let roots: Vec<i64> = section_root::Entity::find()
        .all(&connection)
        .await
        .unwrap()
        .into_iter()
        .map(|root| root.section_id)
        .collect();
    assert_eq!(roots, [films.get()]);
}
