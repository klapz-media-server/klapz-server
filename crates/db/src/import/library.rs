//! Importing library structure: the sections and the folders they scan.

use sea_orm::{
    ActiveValue, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Statement,
    TransactionTrait,
};

use super::{source_error, ImportError, PlexRow};
use crate::entity::{library_section, section_root};
use crate::language::canonical;

#[derive(Debug, Default, Clone, Copy)]
pub struct LibrarySummary {
    pub sections: usize,
    pub roots: usize,
}

/// Replaces the stored library structure with the source's.
///
/// One transaction: a partial import would leave sections without their roots, and a section
/// with no root is indistinguishable from a misconfigured one.
///
/// # Errors
///
/// Fails if the source cannot be read or a write fails.
pub async fn import(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
) -> Result<LibrarySummary, ImportError> {
    // `section_type` is an integer in Plex and a string in its API; our schema stores the
    // domain discriminant, so the mapping happens here once.
    let sections = source
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT id, section_type, name, name_sort, language, uuid, created_at, \
             updated_at, scanned_at, changed_at, user_fields FROM library_sections ORDER BY id",
        ))
        .await
        .map_err(|error| source_error(source_path, error))?;

    let roots = source
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT id, library_section_id, root_path, created_at FROM section_locations \
             ORDER BY id",
        ))
        .await
        .map_err(|error| source_error(source_path, error))?;

    let mut section_rows = Vec::with_capacity(sections.len());
    for row in &sections {
        let kind = section_kind(row.plex_get::<i32>("section_type")?);
        section_rows.push(library_section::ActiveModel {
            id: ActiveValue::Set(row.plex_get("id")?),
            kind: ActiveValue::Set(kind.to_owned()),
            title: ActiveValue::Set(row.plex_get("name")?),
            title_sort: ActiveValue::Set(row.plex_text("name_sort")),
            language: ActiveValue::Set({
                let stored: String = row.plex_get("language")?;
                canonical(Some(&stored)).unwrap_or(stored)
            }),
            uuid: ActiveValue::Set(row.plex_get("uuid")?),
            created_at: ActiveValue::Set(row.plex_get("created_at")?),
            updated_at: ActiveValue::Set(row.plex_get("updated_at")?),
            scanned_at: ActiveValue::Set(row.plex_opt("scanned_at")),
            content_changed_at: ActiveValue::Set(row.plex_get("changed_at")?),
            hidden: ActiveValue::Set(hidden_flag(&row.plex_get::<String>("user_fields")?)),
        });
    }

    let mut root_rows = Vec::with_capacity(roots.len());
    for row in &roots {
        root_rows.push(section_root::ActiveModel {
            id: ActiveValue::Set(row.plex_get("id")?),
            section_id: ActiveValue::Set(row.plex_get("library_section_id")?),
            path: ActiveValue::Set(row.plex_get("root_path")?),
            created_at: ActiveValue::Set(row.plex_get("created_at")?),
        });
    }

    let summary = LibrarySummary {
        sections: section_rows.len(),
        roots: root_rows.len(),
    };

    let transaction = destination.begin().await?;
    // Sections cascade to everything below them, so clearing them clears the contents too.
    section_root::Entity::delete_many()
        .exec(&transaction)
        .await?;
    library_section::Entity::delete_many()
        .exec(&transaction)
        .await?;
    if !section_rows.is_empty() {
        library_section::Entity::insert_many(section_rows)
            .exec(&transaction)
            .await?;
    }
    if !root_rows.is_empty() {
        section_root::Entity::insert_many(root_rows)
            .exec(&transaction)
            .await?;
    }
    transaction.commit().await?;

    Ok(summary)
}

/// Extracts `pr:hidden` from Plex's packed `user_fields` blob.
///
/// This is the denormalized design `docs/plex-db-schema.md` records as one we reject: a
/// per-section preference that should be a column lives inside a JSON string, so it cannot be
/// queried or indexed. We unpack it once here and store it as a real column; the key is absent
/// entirely on sections that were never hidden, which reads as `false`.
fn hidden_flag(user_fields: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(user_fields)
        .ok()
        .and_then(|fields| {
            fields
                .get("pr:hidden")
                .and_then(serde_json::Value::as_str)
                .map(|value| value == "1")
        })
        .unwrap_or(false)
}

/// Plex's `section_type` discriminants, as observed in a real database.
const fn section_kind(section_type: i32) -> &'static str {
    match section_type {
        2 => "show",
        8 => "artist",
        13 => "photo",
        _ => "movie",
    }
}
