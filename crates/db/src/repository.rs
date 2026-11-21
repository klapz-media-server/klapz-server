//! The storage adapter: implements `core`'s port in terms of our entities.
//!
//! This is the only place that knows both vocabularies. Mapping here rather than returning
//! entities is what lets `core` and the bindings stay unaware of the schema.

pub use klapz_core::library::NewLibrary;
use klapz_core::library::{
    LibraryEdit, LibraryError, LibraryRepository, LibrarySection, SectionKind, SectionRoot,
};
use klapz_core::page::{Page, Window};
use klapz_core::SectionId;
use sea_orm::sea_query::{Query, SelectStatement};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, QueryTrait,
    TransactionTrait,
};

use crate::entity::{
    collection_entry, item_marker, library_permission, library_section, metadata_item,
    play_queue_entry, play_queue_source, playback_session, playlist_entry, section_root, viewing,
};

#[derive(Debug, Clone)]
pub struct SqlLibraryRepository {
    connection: DatabaseConnection,
}

impl SqlLibraryRepository {
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl LibraryRepository for SqlLibraryRepository {
    /// The window becomes `WHERE`, `ORDER BY` and `LIMIT`, so the database returns one page
    /// rather than the whole table. The same shape will serve metadata collections, where
    /// reading everything is not an option.
    async fn sections(
        &self,
        only: Option<&[SectionId]>,
        window: Window,
    ) -> Result<Page<LibrarySection>, LibraryError> {
        let mut query = library_section::Entity::find();
        if let Some(only) = only {
            query = query.filter(library_section::Column::Id.is_in(only.iter().map(|id| id.get())));
        }

        // Sections are ordered by id, so the cursor's sort key is unused and only its
        // tiebreaker applies.
        if let Some(after) = window.after.as_ref() {
            query = query.filter(library_section::Column::Id.gt(after.id));
        }
        if let Some(before) = window.before.as_ref() {
            query = query.filter(library_section::Column::Id.lt(before.id));
        }

        // A backward page is read from the far end, then flipped back: the caller always
        // wants ascending order, whichever end the window was taken from.
        query = if window.is_reversed() {
            query.order_by_desc(library_section::Column::Id)
        } else {
            query.order_by_asc(library_section::Column::Id)
        };

        let rows = query
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let mut page = Page::from_overfetch(rows, &window).map(to_domain);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn section(&self, id: SectionId) -> Result<LibrarySection, LibraryError> {
        library_section::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_domain)
            .ok_or(LibraryError::SectionNotFound(id))
    }

    /// One `IN` query regardless of how many sections are asked for, which is what makes the
    /// API's `DataLoader` worth having.
    async fn roots_for(&self, sections: &[SectionId]) -> Result<Vec<SectionRoot>, LibraryError> {
        if sections.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<i64> = sections.iter().map(|id| id.get()).collect();
        let roots = section_root::Entity::find()
            .filter(section_root::Column::SectionId.is_in(ids))
            .all(&self.connection)
            .await
            .map_err(storage)?;

        Ok(roots.into_iter().map(root_to_domain).collect())
    }

    async fn create(&self, library: NewLibrary, now: i64) -> Result<LibrarySection, LibraryError> {
        let id = create_library_at(&self.connection, library, now)
            .await
            .map_err(storage)?;
        self.section(id).await
    }

    async fn edit(
        &self,
        id: SectionId,
        edit: &LibraryEdit,
        now: i64,
    ) -> Result<LibrarySection, LibraryError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let section = library_section::Entity::find_by_id(id.get())
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or(LibraryError::SectionNotFound(id))?;
        let mut changed = section.into_active_model();
        if let Some(title) = &edit.title {
            changed.title = ActiveValue::Set(title.clone());
            changed.title_sort = ActiveValue::Set(None);
        }
        if let Some(language) = &edit.language {
            changed.language = ActiveValue::Set(language.clone());
        }
        changed.updated_at = ActiveValue::Set(now);
        let section = changed.update(&transaction).await.map_err(storage)?;
        if let Some(roots) = &edit.roots {
            replace_roots(&transaction, id, roots, now)
                .await
                .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(to_domain(section))
    }

    /// The section's foreign keys cascade to its roots, items and collections, and the items'
    /// to their files, tracks, tags, indexes, history and extras. What refers to an item or a
    /// library without one goes by hand first, as when a scan removes an item: a deleted
    /// number could otherwise come back on the next item added.
    async fn delete(&self, id: SectionId) -> Result<bool, LibraryError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let backend = transaction.get_database_backend();
        let items = || -> SelectStatement {
            Query::select()
                .column(metadata_item::Column::Id)
                .from(metadata_item::Entity)
                .and_where(metadata_item::Column::LibrarySectionId.eq(id.get()))
                .to_owned()
        };
        let by_hand = [
            item_marker::Entity::delete_many()
                .filter(item_marker::Column::ItemId.in_subquery(items()))
                .build(backend),
            collection_entry::Entity::delete_many()
                .filter(collection_entry::Column::ItemId.in_subquery(items()))
                .build(backend),
            playlist_entry::Entity::delete_many()
                .filter(playlist_entry::Column::ItemId.in_subquery(items()))
                .build(backend),
            play_queue_entry::Entity::delete_many()
                .filter(play_queue_entry::Column::ItemId.in_subquery(items()))
                .build(backend),
            play_queue_source::Entity::delete_many()
                .filter(play_queue_source::Column::ItemId.in_subquery(items()))
                .build(backend),
            viewing::Entity::delete_many()
                .filter(viewing::Column::ItemId.in_subquery(items()))
                .build(backend),
            playback_session::Entity::delete_many()
                .filter(playback_session::Column::ItemId.in_subquery(items()))
                .build(backend),
            library_permission::Entity::delete_many()
                .filter(library_permission::Column::SectionId.eq(id.get()))
                .build(backend),
        ];
        for statement in by_hand {
            transaction.execute(statement).await.map_err(storage)?;
        }
        let deleted = library_section::Entity::delete_by_id(id.get())
            .exec(&transaction)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        Ok(deleted.rows_affected > 0)
    }
}

/// Gives a section these roots, keeping the id and date of each it already had.
async fn replace_roots<C: ConnectionTrait>(
    connection: &C,
    section: SectionId,
    paths: &[String],
    now: i64,
) -> Result<(), DbErr> {
    section_root::Entity::delete_many()
        .filter(section_root::Column::SectionId.eq(section.get()))
        .filter(section_root::Column::Path.is_not_in(paths.iter().cloned()))
        .exec(connection)
        .await?;
    let kept: Vec<String> = section_root::Entity::find()
        .select_only()
        .column(section_root::Column::Path)
        .filter(section_root::Column::SectionId.eq(section.get()))
        .into_tuple()
        .all(connection)
        .await?;
    let mut next = next_root_id(connection).await?;
    for path in paths.iter().filter(|path| !kept.contains(path)) {
        section_root::ActiveModel {
            id: ActiveValue::Set(next),
            section_id: ActiveValue::Set(section.get()),
            path: ActiveValue::Set(path.clone()),
            created_at: ActiveValue::Set(now),
        }
        .insert(connection)
        .await?;
        next += 1;
    }
    Ok(())
}

async fn next_root_id<C: ConnectionTrait>(connection: &C) -> Result<i64, DbErr> {
    Ok(section_root::Entity::find()
        .select_only()
        .column_as(section_root::Column::Id.max(), "id")
        .into_tuple::<Option<i64>>()
        .one(connection)
        .await?
        .flatten()
        .unwrap_or(0)
        + 1)
}

fn to_domain(section: library_section::Model) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(section.id),
        kind: parse_kind(&section.kind),
        title: section.title,
        title_sort: section.title_sort,
        language: section.language,
        uuid: section.uuid,
        created_at: section.created_at,
        updated_at: section.updated_at,
        scanned_at: section.scanned_at,
        content_changed_at: section.content_changed_at,
        hidden: section.hidden,
    }
}

fn root_to_domain(root: section_root::Model) -> SectionRoot {
    SectionRoot {
        id: root.id,
        section_id: SectionId::new(root.section_id),
        path: root.path,
        created_at: root.created_at,
    }
}

/// Maps the stored discriminant back to the domain enum.
///
/// Unknown values fall back to `Movie` rather than failing the whole listing: a single
/// mis-stored row should not make the library unreachable, and the fallback is visible in
/// logs.
fn parse_kind(stored: &str) -> SectionKind {
    match stored {
        "show" => SectionKind::Show,
        "artist" => SectionKind::Artist,
        "photo" => SectionKind::Photo,
        "movie" => SectionKind::Movie,
        other => {
            tracing::warn!(
                kind = other,
                "unknown section kind in storage, treating as movie"
            );
            SectionKind::Movie
        }
    }
}

fn storage(error: sea_orm::DbErr) -> LibraryError {
    LibraryError::Storage(Box::new(error))
}

/// Creates a library and its roots, numbered past the highest ids, as an import would have
/// left them.
///
/// # Errors
///
/// Fails if a statement fails; nothing is written then.
pub async fn create_library(
    connection: &DatabaseConnection,
    library: NewLibrary,
) -> Result<SectionId, DbErr> {
    create_library_at(connection, library, klapz_core::now_epoch()).await
}

async fn create_library_at(
    connection: &DatabaseConnection,
    library: NewLibrary,
    now: i64,
) -> Result<SectionId, DbErr> {
    let transaction = connection.begin().await?;
    let next_section = library_section::Entity::find()
        .select_only()
        .column_as(library_section::Column::Id.max(), "id")
        .into_tuple::<Option<i64>>()
        .one(&transaction)
        .await?
        .flatten()
        .unwrap_or(0)
        + 1;
    library_section::ActiveModel {
        id: ActiveValue::Set(next_section),
        kind: ActiveValue::Set(
            match library.kind {
                SectionKind::Movie => "movie",
                SectionKind::Show => "show",
                SectionKind::Artist => "artist",
                SectionKind::Photo => "photo",
            }
            .to_owned(),
        ),
        title_sort: ActiveValue::Set(None),
        title: ActiveValue::Set(library.title),
        language: ActiveValue::Set(library.language),
        uuid: ActiveValue::Set(uuid::Uuid::new_v4().to_string()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    }
    .insert(&transaction)
    .await?;
    replace_roots(
        &transaction,
        SectionId::new(next_section),
        &library.roots,
        now,
    )
    .await?;
    transaction.commit().await?;
    Ok(SectionId::new(next_section))
}
