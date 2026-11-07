//! Response bodies, declared in their JSON form.
//!
//! Field order is significant: it is the attribute order a real server emits, and
//! `serde_json`'s `preserve_order` feature keeps it. Field *types* are significant too, and
//! are taken from captures rather than chosen — see `docs/wire-format.md`. Plex is not
//! self-consistent here, so within one struct a `0` in XML may be `false` or `0` in JSON
//! depending on the field.

use crate::library::{GqlLibrary, GqlRoot};

use serde::{Deserialize, Serialize};

/// The media kinds the official API exposes, as the adapter deserializes them.
///
/// Mirrors the schema's `LibraryKind`, which async-graphql emits in screaming snake case.
/// Declared here because the adapter consumes the API as a client would.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LibraryKind {
    Movie,
    Show,
    Artist,
    Photo,
}

/// Response to the unauthenticated handshake.
#[derive(Debug, Serialize)]
pub struct IdentityContainer {
    pub size: u32,
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub claimed: bool,
    #[serde(rename = "machineIdentifier")]
    pub machine_identifier: String,
    pub version: String,
}

/// Response to `GET /library/sections`.
#[derive(Debug, Serialize)]
pub struct SectionsContainer {
    pub size: u32,
    #[serde(rename = "allowSync")]
    pub allow_sync: bool,
    pub title1: String,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    pub directory: Vec<SectionDirectory>,
}

/// One library in a section listing.
///
/// The flag count mirrors the reference server's attributes; grouping them into a sub-struct
/// would change the serialized shape, which is the one thing that must not move.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionDirectory {
    pub allow_sync: bool,
    pub filters: bool,
    pub refreshing: bool,
    /// A string even though the value is digits, matching the reference server.
    pub key: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: String,
    pub agent: String,
    pub scanner: String,
    pub language: String,
    pub uuid: String,
    pub updated_at: i64,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_at: Option<i64>,
    pub content: bool,
    pub directory: bool,
    /// A change counter, not a timestamp, and wide enough to exceed 32 bits in practice.
    pub content_changed_at: i64,
    /// A number here, unlike the booleans above. Plex's own inconsistency, reproduced
    /// deliberately so clients parsing strictly do not break.
    pub hidden: u8,
    #[serde(rename = "Location", skip_serializing_if = "Vec::is_empty")]
    pub location: Vec<SectionLocation>,
}

#[derive(Debug, Serialize)]
pub struct SectionLocation {
    pub id: i64,
    pub path: String,
}

impl SectionDirectory {
    pub fn from_graphql(library: &GqlLibrary) -> Self {
        Self {
            allow_sync: true,
            filters: true,
            refreshing: false,
            key: decode_key(&library.id).to_string(),
            kind: kind_name(library.kind),
            title: library.title.clone(),
            agent: agent_for(library.kind).to_owned(),
            scanner: scanner_for(library.kind).to_owned(),
            language: library.language.clone(),
            uuid: library.uuid.clone(),
            updated_at: library.updated_at,
            created_at: library.created_at,
            scanned_at: library.scanned_at,
            content: true,
            directory: true,
            // The schema returns this as a string because it exceeds GraphQL's 32-bit Int.
            // A value we cannot parse would be a schema defect, not client input; emitting 0
            // keeps the listing usable and is visible in the response rather than silent.
            content_changed_at: library.content_changed_at.parse().unwrap_or(0),
            hidden: u8::from(library.hidden),
            location: ordered_locations(&library.roots),
        }
    }
}

/// Reads the numeric key out of a Relay global id.
///
/// The adapter lives in this workspace and uses the official API's published codec, so this
/// is a typed call rather than guessing at a string. A value that does not decode is a defect
/// in our own query, not client input; emitting `0` keeps the listing usable and is visible
/// in the response rather than silent.
fn decode_key(global_id: &str) -> i64 {
    open_plex_api::GlobalId::decode(global_id).map_or(0, |decoded| decoded.key)
}

/// Orders a section's roots the way the reference server emits them.
///
/// Inferred from the three sections in the reference library that had several roots with
/// distinguishable values: newest first, with the id breaking ties. Thin evidence for a rule,
/// but it costs nothing and no client is known to depend on the order either way — revisit if
/// a counter-example turns up.
fn ordered_locations(roots: &[GqlRoot]) -> Vec<SectionLocation> {
    let mut ordered: Vec<&GqlRoot> = roots.iter().collect();
    ordered.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| decode_key(&left.id).cmp(&decode_key(&right.id)))
    });
    ordered
        .into_iter()
        .map(|root| SectionLocation {
            id: decode_key(&root.id),
            path: root.path.clone(),
        })
        .collect()
}

/// Plex's listing order for sections: by media type in this fixed order, then by title.
///
/// Ordering is a presentation decision, so it belongs here rather than in storage — `core`
/// has no opinion on how a binding sorts, and the next binding may sort differently.
pub const fn kind_rank(kind: LibraryKind) -> u8 {
    match kind {
        LibraryKind::Movie => 0,
        LibraryKind::Show => 1,
        LibraryKind::Artist => 2,
        LibraryKind::Photo => 3,
    }
}

/// The `type` string the API exposes for a section kind.
pub const fn kind_name(kind: LibraryKind) -> &'static str {
    match kind {
        LibraryKind::Movie => "movie",
        LibraryKind::Show => "show",
        LibraryKind::Artist => "artist",
        LibraryKind::Photo => "photo",
    }
}

const fn agent_for(kind: LibraryKind) -> &'static str {
    match kind {
        LibraryKind::Movie => "tv.plex.agents.movie",
        LibraryKind::Show => "tv.plex.agents.series",
        LibraryKind::Artist => "tv.plex.agents.music",
        LibraryKind::Photo => "com.plexapp.agents.none",
    }
}

const fn scanner_for(kind: LibraryKind) -> &'static str {
    match kind {
        LibraryKind::Movie => "Plex Movie",
        LibraryKind::Show => "Plex TV Series",
        LibraryKind::Artist => "Plex Music",
        LibraryKind::Photo => "Plex Photo Scanner",
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    /// Builds a root carrying the global id the official API would return.
    fn root(database_id: i64, path: &str, created_at: i64) -> GqlRoot {
        GqlRoot {
            id: open_plex_api::GlobalId::encode("Root", database_id),
            path: path.to_owned(),
            created_at,
        }
    }

    #[test]
    fn orders_locations_newest_first_with_id_breaking_ties() {
        // Section 19 of the reference library: root 49 was added later than 45, and the
        // server lists it first.
        let ordered = ordered_locations(&[
            root(45, "/mnt/acd/Plex/TVShows4K/VF", 1_497_008_316),
            root(49, "/mnt/acd/Plex/TVShows4K/Multi", 1_497_009_972),
        ]);
        assert_eq!(
            ordered.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![49, 45]
        );

        // Section 17: both roots share a creation time, and then the id decides.
        let tied = ordered_locations(&[
            root(41, "/mnt/acd/Plex/Movies4K/Multi", 1_497_008_148),
            root(40, "/mnt/acd/Plex/Movies4K/VF", 1_497_008_148),
        ]);
        assert_eq!(tied.iter().map(|l| l.id).collect::<Vec<_>>(), vec![40, 41]);
    }

    #[test]
    fn ranks_section_kinds_in_the_order_the_server_lists_them() {
        assert!(kind_rank(LibraryKind::Movie) < kind_rank(LibraryKind::Show));
        assert!(kind_rank(LibraryKind::Show) < kind_rank(LibraryKind::Artist));
        assert!(kind_rank(LibraryKind::Artist) < kind_rank(LibraryKind::Photo));
    }
}
