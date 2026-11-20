//! `GET /library/people/{personId}` and `/media`: a tag and what carries it.
//!
//! Plex has no people of its own, only one tag per person and role, and `personId` is that
//! tag's id — the `id` a lookup publishes on its `Role`, `Director` or `Writer`. Any tag
//! answers, a genre or a country as well as a credit, which is why the person page of a
//! director and that of the same person as writer list different films. The spec also
//! promises the provider's `tagKey`; the reference answers it `404`, and so do we.

use std::sync::LazyLock;

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use klapz_api::tag::TAG_TYPE;
use klapz_api::GlobalId;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{database_id, with_shared_fragments, GqlItemEdge, GqlPerson};
use crate::items::{MetadataEntry, GRID_VIEW_MODE, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX};
use crate::language::Language;
use crate::listing;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/people/{personId}", get(person))
        .route("/library/people/{personId}/media", get(media))
}

const PERSON_QUERY: &str = r"
query PlexPerson($id: ID!) {
  node(id: $id) {
    ... on Tag { id kind name person { providerKey thumbUrl } }
  }
}
";

static MEDIA_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexPersonMedia($id: ID!, $first: Int!, $offset: Int!, $filter: ItemFilterInput) {
  node(id: $id) {
    ... on Tag {
      kind
      itemCount(filter: $filter)
      items(first: $first, offset: $offset, filter: $filter) {
        edges { node { ...ListedItem } }
      }
    }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
struct PersonData {
    node: Option<GqlTagNode>,
}

#[derive(Debug, Deserialize)]
struct GqlTagNode {
    id: String,
    kind: String,
    name: String,
    person: Option<GqlPerson>,
}

#[derive(Debug, Deserialize)]
struct MediaData {
    node: Option<GqlWorks>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlWorks {
    kind: String,
    item_count: u64,
    items: GqlWorksConnection,
}

#[derive(Debug, Deserialize)]
struct GqlWorksConnection {
    edges: Vec<GqlItemEdge>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersonContainer {
    size: u32,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(rename = "Directory")]
    directory: [PersonDirectory; 1],
}

/// The tag, with the attributes a lookup gives its credits plus `tagType`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersonDirectory {
    id: i64,
    filter: String,
    tag: String,
    tag_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb: Option<String>,
}

/// `totalSize` and `offset` only when the client paged or the cap cut the list: an unpaged
/// answer that holds everything is the reference's, attribute for attribute.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaContainer {
    size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    /// Set on an actor's page only, measured over seven actors and none of the directors,
    /// writers, producers, genres and countries asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    view_group: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    view_mode: Option<u32>,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<MetadataEntry>,
}

/// The listing filter selecting a kind of tag, and Plex's `tag_type` for it.
pub(crate) fn wire_kind(kind: &str) -> Option<(&'static str, u8)> {
    Some(match kind {
        "GENRE" => ("genre", 1),
        "DIRECTOR" => ("director", 4),
        "WRITER" => ("writer", 5),
        "ACTOR" => ("actor", 6),
        "PRODUCER" => ("producer", 7),
        "COUNTRY" => ("country", 8),
        _ => return None,
    })
}

/// The tag's global id, or `None` for a `personId` that is not a tag id. The provider's
/// `tagKey` is one of those: hexadecimal, and refused by the reference.
fn tag_id(person_id: &str) -> Option<Value> {
    let id: i64 = person_id.parse().ok()?;
    Some(Value::String(GlobalId::encode(TAG_TYPE, id)))
}

/// `GET /library/people/{personId}` — the tag, as one `Directory`.
async fn person(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(person_id): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<PersonContainer>, ApiError> {
    let id = tag_id(&person_id).ok_or(ApiError::NotFound)?;
    let mut variables = Variables::default();
    variables.insert(Name::new("id"), id);
    let data: PersonData = gql::execute(&state.schema, PERSON_QUERY, variables).await?;
    // Null for a tag the caller's libraries do not carry, as for an unknown one.
    let tag = data.node.ok_or(ApiError::NotFound)?;
    let (field, tag_type) = wire_kind(&tag.kind).ok_or(ApiError::NotFound)?;
    let id = database_id(&tag.id);
    let person = tag.person.unwrap_or_default();

    Ok(PlexResponse::new(
        PersonContainer {
            size: 1,
            identifier: LIBRARY_IDENTIFIER,
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: crate::items::MEDIA_TAG_VERSION,
            directory: [PersonDirectory {
                id,
                filter: format!("{field}={id}"),
                tag: crate::items::capitalized(&tag.name),
                tag_type,
                tag_key: person.provider_key,
                thumb: person.thumb_url,
            }],
        },
        format,
    ))
}

/// `GET /library/people/{personId}/media` — what carries the tag, across libraries.
///
/// The listing filters apply as they do on `/all`, and one we cannot honor is refused the
/// same way. `sort` and `type` are dropped first: the reference ignores both here, in the
/// order it lists, and so does this list, whose order is the official API's.
async fn media(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(person_id): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<MediaContainer>, ApiError> {
    let id = tag_id(&person_id).ok_or(ApiError::NotFound)?;
    let filters: Vec<(String, String)> = pairs
        .iter()
        .filter(|(key, _)| key != "sort" && key != "type")
        .cloned()
        .collect();
    let request = listing::parse(&filters)?;
    // Both parameters or neither, as the reference pages. Without them it lists everything,
    // which the cap turns into as much as one page holds.
    let has = |name: &str| pairs.iter().any(|(key, _)| key == name);
    let paged = has("X-Plex-Container-Start") && has("X-Plex-Container-Size");
    let (offset, first) = if paged {
        (
            request.start,
            request.size.unwrap_or(MAX_ITEMS).min(MAX_ITEMS),
        )
    } else {
        (0, MAX_ITEMS)
    };

    let mut variables = Variables::default();
    variables.insert(Name::new("id"), id);
    variables.insert(Name::new("first"), Value::Number(first.into()));
    variables.insert(Name::new("offset"), Value::Number(offset.into()));
    variables.insert(Name::new("filter"), request.filters.to_graphql(None));
    let data: MediaData = gql::execute(&state.schema, &MEDIA_QUERY, variables).await?;
    let works = data.node.ok_or(ApiError::NotFound)?;

    let language = Language::of(&client);
    let metadata: Vec<MetadataEntry> = works
        .items
        .edges
        .iter()
        .filter_map(|edge| MetadataEntry::from_graphql(&edge.node, true, language))
        .collect();
    let total = works.item_count;
    let cut = offset + metadata.len() as u64 != total;
    let windowed = paged || cut;
    let actor = works.kind == "ACTOR";
    let body = MediaContainer {
        size: u32::try_from(metadata.len()).unwrap_or(u32::MAX),
        total_size: windowed.then_some(total),
        offset: windowed.then_some(offset),
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: crate::items::MEDIA_TAG_VERSION,
        view_group: actor.then_some("movie"),
        view_mode: actor.then_some(GRID_VIEW_MODE),
        metadata,
    };
    let response = PlexResponse::new(body, format);
    Ok(if windowed {
        response.paged(offset, total)
    } else {
        response
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_person_is_addressed_by_tag_id_not_by_provider_key() {
        assert!(tag_id("531575").is_some());
        assert!(tag_id("5d77682685719b001f3a0b79").is_none());
        assert!(tag_id("").is_none());
    }

    #[test]
    fn each_kind_keeps_the_filter_and_number_plex_gives_it() {
        assert_eq!(wire_kind("ACTOR"), Some(("actor", 6)));
        assert_eq!(wire_kind("DIRECTOR"), Some(("director", 4)));
        assert_eq!(wire_kind("WRITER"), Some(("writer", 5)));
        assert_eq!(wire_kind("PRODUCER"), Some(("producer", 7)));
        assert_eq!(wire_kind("GENRE"), Some(("genre", 1)));
        assert_eq!(wire_kind("COUNTRY"), Some(("country", 8)));
        assert_eq!(wire_kind("COLLECTION"), None);
    }
}
