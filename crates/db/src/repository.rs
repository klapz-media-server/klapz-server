//! The storage adapter: implements `core`'s port in terms of our entities.
//!
//! This is the only place that knows both vocabularies. Mapping here rather than returning
//! entities is what lets `core` and the bindings stay unaware of the schema.

use open_plex_core::library::{
    LibraryError, LibraryRepository, LibrarySection, SectionKind, SectionRoot,
};
use open_plex_core::page::{Page, Window};
use open_plex_core::SectionId;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

use crate::entity::{library_section, section_root};

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
