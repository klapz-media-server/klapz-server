//! `GET /library/tags?type=` — every tag of one kind, across the caller's libraries.
//!
//! Bound onto the official API's `tags(kind:)`. Measured on the reference: `type` is Plex's
//! tag type number, the last one given wins, anything that does not read as a 32-bit integer
//! is a `400`, and a number naming no tag answers an empty container. Tags come in id order,
//! each with the attributes a lookup gives its credits plus `tagType`.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{database_id, GqlPerson};
use crate::items::{capitalized, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION};
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/library/tags", get(tags))
}

const TAGS_QUERY: &str = r"
query PlexTags($kind: TagType!, $first: Int!, $offset: Int!) {
  tags(kind: $kind, first: $first, offset: $offset) {
    totalCount
    edges { node { id name person { providerKey thumbUrl } } }
  }
}
";

/// Plex's tag type for a collection, which is an item of its own here.
const COLLECTION: i32 = 2;

/// The official API's `TagType` for each of Plex's tag types we keep.
fn kind_of(tag_type: i32) -> Option<&'static str> {
    Some(match tag_type {
        1 => "GENRE",
        4 => "DIRECTOR",
        5 => "WRITER",
        6 => "ACTOR",
        7 => "PRODUCER",
        8 => "COUNTRY",
        _ => return None,
    })
}

#[derive(Debug, Deserialize)]
struct TagsData {
    tags: GqlTagList,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlTagList {
    total_count: u64,
    edges: Vec<GqlEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlEdge {
    node: GqlTag,
}

#[derive(Debug, Deserialize)]
struct GqlTag {
    id: String,
    name: String,
    person: Option<GqlPerson>,
}

/// `totalSize` and `offset` only when the client paged or the cap cut the list, as on a
/// person's page: an answer holding everything is the reference's, attribute for attribute.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagsContainer {
    size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    directory: Vec<TagDirectory>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TagDirectory {
    id: i64,
    filter: String,
    tag: String,
    tag_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb: Option<String>,
}

/// Collections answer `501`: the reference lists their shared tags, which a collection here
/// does not have. Every other number we keep nothing under answers the empty container the
/// reference gives a type it holds no tag of.
async fn tags(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<TagsContainer>, ApiError> {
    let value = |name: &str| {
        pairs
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let tag_type: i32 = value("type")
        .and_then(|number| number.parse().ok())
        .ok_or(ApiError::BadRequest)?;
    if tag_type == COLLECTION {
        return Err(ApiError::NotImplemented);
    }
    // Both parameters or neither, as the reference pages. Without them it lists everything,
    // which the cap turns into as much as one page holds.
    let window = value("X-Plex-Container-Start")
        .zip(value("X-Plex-Container-Size"))
        .map(|(start, size)| {
            (
                start.parse::<u64>().unwrap_or(0),
                size.parse::<u32>().unwrap_or(MAX_ITEMS).min(MAX_ITEMS),
            )
        });
    let (offset, first) = window.unwrap_or((0, MAX_ITEMS));

    let wire = kind_of(tag_type).and_then(|kind| Some((kind, crate::people::wire_kind(kind)?)));
    let (directory, total) = match wire {
        Some((kind, (field, number))) => {
            let mut variables = Variables::default();
            variables.insert(Name::new("kind"), Value::Enum(Name::new(kind)));
            variables.insert(Name::new("first"), Value::Number(first.into()));
            variables.insert(Name::new("offset"), Value::Number(offset.into()));
            let data: TagsData = gql::execute(&state.schema, TAGS_QUERY, variables).await?;
            let directory: Vec<TagDirectory> = data
                .tags
                .edges
                .into_iter()
                .map(|edge| tag_directory(edge.node, field, number))
                .collect();
            (directory, data.tags.total_count)
        }
        None => (Vec::new(), 0),
    };
    let windowed = window.is_some() || offset + directory.len() as u64 != total;
    let body = TagsContainer {
        size: u32::try_from(directory.len()).unwrap_or(u32::MAX),
        total_size: windowed.then_some(total),
        offset: windowed.then_some(offset),
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        directory,
    };
    let response = PlexResponse::new(body, format);
    Ok(if windowed {
        response.paged(offset, total)
    } else {
        response
    })
}

fn tag_directory(tag: GqlTag, field: &str, tag_type: u8) -> TagDirectory {
    let id = database_id(&tag.id);
    let person = tag.person.unwrap_or_default();
    TagDirectory {
        id,
        filter: format!("{field}={id}"),
        tag: capitalized(&tag.name),
        tag_type,
        tag_key: person.provider_key,
        thumb: person.thumb_url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_six_kinds_we_keep_have_their_plex_numbers() {
        assert_eq!(kind_of(1), Some("GENRE"));
        assert_eq!(kind_of(4), Some("DIRECTOR"));
        assert_eq!(kind_of(5), Some("WRITER"));
        assert_eq!(kind_of(6), Some("ACTOR"));
        assert_eq!(kind_of(7), Some("PRODUCER"));
        assert_eq!(kind_of(8), Some("COUNTRY"));
        assert_eq!(kind_of(COLLECTION), None);
        assert_eq!(kind_of(11), None);
    }
}
