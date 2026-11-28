//! `GET /library/sections/{sectionId}/autocomplete`: what a filter field offers as it is
//! typed into.
//!
//! One `{field}.query` names the field and the text. `title` completes items, listed as
//! `/all` lists them with `title.queryRange` added; a tag field completes the tags the
//! library's items carry, as `Directory` entries. Both read the official API: a listing
//! filtered with `ItemFilterInput.titleCompletion`, and `Library.tags(completing:)`. The rule
//! is `core::completion`'s, measured on the reference and listed in `docs/plex/spec-gaps.md`.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use klapz_api::library::LIBRARY_TYPE;
use klapz_api::GlobalId;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::filter_values::with_fields;
use crate::gql;
use crate::gql_items::{database_id, SectionItemsData, SECTION_ITEMS_QUERY};
use crate::items::{
    MetadataEntry, ViewStyle, GRID_VIEW_MODE, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX,
    MEDIA_TAG_VERSION,
};
use crate::language::Language;
use crate::library::item_type;
use crate::listing;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    // A static segment, so it wins over `/library/sections/{sectionId}/{filter}`.
    Router::new().route(
        "/library/sections/{sectionId}/autocomplete",
        get(autocomplete),
    )
}

/// How many tags a completion lists at most, as the reference does: `actor.query=a` answers
/// `totalSize="100"` of the thousands matching.
const TAG_LIMIT: u32 = 100;

/// The field a `{field}.query` completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    /// `kind` is the schema's `TagType` value, `tag_type` Plex's number for it.
    Tag {
        filter: &'static str,
        kind: &'static str,
        tag_type: u8,
    },
}

impl Field {
    /// `Ok(None)` for a field the reference completes and this server does not yet, which
    /// answers `501`; `Err` for one the reference refuses.
    fn named(name: &str) -> Result<Option<Self>, ApiError> {
        let tag = |filter, kind, tag_type| {
            Ok(Some(Self::Tag {
                filter,
                kind,
                tag_type,
            }))
        };
        match name {
            "title" => Ok(Some(Self::Title)),
            "genre" => tag("genre", "GENRE", 1),
            "director" => tag("director", "DIRECTOR", 4),
            "writer" => tag("writer", "WRITER", 5),
            "actor" => tag("actor", "ACTOR", 6),
            "producer" => tag("producer", "PRODUCER", 7),
            "country" => tag("country", "COUNTRY", 8),
            "titleSort" | "originalTitle" | "studio" | "contentRating" | "summary" | "tagline"
            | "year" | "collection" | "label" | "mood" | "style" => Ok(None),
            _ => Err(ApiError::BadRequest),
        }
    }
}

const TAGS_QUERY: &str = r"
query PlexTagCompletion($id: ID!, $kind: TagType!, $filter: ItemFilterInput, $text: String!, $first: Int!) {
  node(id: $id) {
    ... on Library {
      title
      kind
      tags(kind: $kind, filter: $filter, completing: $text, first: $first) {
        edges { node { id name person { providerKey thumbUrl } } }
      }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct TagsData {
    node: Option<GqlLibraryTags>,
}

#[derive(Debug, Deserialize)]
struct GqlLibraryTags {
    title: String,
    kind: String,
    tags: GqlEdges,
}

#[derive(Debug, Deserialize)]
struct GqlEdges {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPerson {
    provider_key: Option<String>,
    thumb_url: Option<String>,
}

/// Response body: a "secondary" view of the library, items or tags.
///
/// Always windowed, with `totalSize` and `offset`, for the reason `/all` is: the reference
/// lists the 2,723 films a `d` completes in one response, and `MAX_ITEMS` forbids that.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionContainer {
    size: usize,
    total_size: u64,
    offset: u64,
    allow_sync: bool,
    art: &'static str,
    content: &'static str,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    thumb: &'static str,
    title1: String,
    view_group: &'static str,
    view_mode: u32,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    items: Vec<CompletedItem>,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    tags: Vec<CompletedTag>,
}

impl CompletionContainer {
    fn new(title: String, kind: &str, offset: u64, total: u64) -> Self {
        let style = ViewStyle::for_kind(kind);
        Self {
            size: 0,
            total_size: total,
            offset,
            allow_sync: false,
            art: style.art,
            content: "secondary",
            identifier: LIBRARY_IDENTIFIER,
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            thumb: style.thumb,
            title1: title,
            view_group: "secondary",
            view_mode: GRID_VIEW_MODE,
            items: Vec::new(),
            tags: Vec::new(),
        }
    }
}

/// An item as `/all` lists it, with where the text sits in its title.
#[derive(Debug, Serialize)]
struct CompletedItem {
    #[serde(rename = "title.queryRange", skip_serializing_if = "Option::is_none")]
    query_range: Option<String>,
    #[serde(flatten)]
    metadata: MetadataEntry,
}

/// A tag, as a filter takes it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompletedTag {
    id: i64,
    filter: String,
    tag: String,
    tag_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb: Option<String>,
}

/// Where `text` first occurs in `title`, as Plex's `queryRange`: the first and last byte,
/// both inclusive.
///
/// The first occurrence anywhere, which need not be the word the match was made on: `d`
/// completes *3 Billboards : Les Panneaux de la vengeance* by `de`, and the reference ranges
/// it on the `d` of `Billboards`, at 10. Found by bytes, ASCII case aside, so that a match
/// made on a capital outside ASCII has no range (`à tous` against *À tous les garçons*), as
/// on the reference.
fn query_range(title: &str, text: &str) -> Option<String> {
    let (title, text) = (title.as_bytes(), text.as_bytes());
    let length = text.len();
    (0..title.len())
        .find(|start| {
            title
                .get(*start..start + length)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(text))
        })
        .map(|start| format!("{start},{}", start + length.max(1) - 1))
}

async fn autocomplete(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<CompletionContainer>, ApiError> {
    let mut completions = pairs
        .iter()
        .filter_map(|(key, value)| Some((key.strip_suffix(".query")?, value.as_str())));
    let (name, text) = completions.next().ok_or(ApiError::BadRequest)?;
    if completions.next().is_some() || text.is_empty() {
        return Err(ApiError::BadRequest);
    }
    let field = Field::named(name)?.ok_or(ApiError::NotImplemented)?;
    // `type` is required, and a number.
    pairs
        .iter()
        .find(|(key, _)| key == "type")
        .and_then(|(_, value)| value.parse::<i32>().ok())
        .ok_or(ApiError::BadRequest)?;
    let rest: Vec<(String, String)> = pairs
        .iter()
        .filter(|(key, _)| key.strip_suffix(".query").is_none())
        .cloned()
        .collect();
    let request = listing::parse(&rest)?;
    let window = (
        request.start,
        request.size.unwrap_or(MAX_ITEMS).min(MAX_ITEMS),
    );

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    let body = match field {
        Field::Title => {
            let language = Language::of(&client);
            complete_titles(&state, &request, variables, text, window, language).await?
        }
        Field::Tag {
            filter,
            kind,
            tag_type,
        } => {
            variables.insert(
                Name::new("filter"),
                request.filters.to_graphql(item_type(request.kind)),
            );
            variables.insert(Name::new("kind"), Value::Enum(Name::new(kind)));
            complete_tags(&state, variables, text, (filter, tag_type), window).await?
        }
    };
    let total = body.total_size;
    Ok(PlexResponse::new(body, format).paged(request.start, total))
}

/// The library's items whose title `text` completes, as `/all` lists them, each with its
/// range.
async fn complete_titles(
    state: &PlexState,
    request: &listing::ListingRequest,
    mut variables: Variables,
    text: &str,
    (offset, size): (u64, u32),
    language: Language,
) -> Result<CompletionContainer, ApiError> {
    variables.insert(
        Name::new("filter"),
        with_fields(
            request.filters.to_graphql(item_type(request.kind)),
            [("titleCompletion", Value::String(text.to_owned()))],
        ),
    );
    variables.insert(Name::new("first"), Value::Number(size.into()));
    variables.insert(Name::new("offset"), Value::Number(offset.into()));
    let (order_by, direction) = request.ordering();
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new(order_by)));
    variables.insert(Name::new("direction"), Value::Enum(Name::new(direction)));
    variables.insert(
        Name::new("seed"),
        Value::Number(crate::library::shuffle_seed().into()),
    );
    let data: SectionItemsData =
        gql::execute(&state.schema, &SECTION_ITEMS_QUERY, variables).await?;
    let library = data.node.ok_or(ApiError::NotFound)?;
    let total = library.stats.item_count;
    let mut body = CompletionContainer::new(library.title, &library.kind, offset, total);
    body.items = library
        .items
        .edges
        .iter()
        .filter_map(|edge| {
            let metadata = MetadataEntry::from_graphql(&edge.node, false, language)?;
            let query_range = metadata
                .title
                .as_deref()
                .and_then(|title| query_range(title, text));
            Some(CompletedItem {
                query_range,
                metadata,
            })
        })
        .collect();
    body.size = body.items.len();
    Ok(body)
}

/// The tags of one kind the library's items carry whose name `text` completes: the first
/// [`TAG_LIMIT`], from which the client's window is cut.
async fn complete_tags(
    state: &PlexState,
    mut variables: Variables,
    text: &str,
    (filter, tag_type): (&str, u8),
    (offset, size): (u64, u32),
) -> Result<CompletionContainer, ApiError> {
    variables.insert(Name::new("text"), Value::String(text.to_owned()));
    variables.insert(Name::new("first"), Value::Number(TAG_LIMIT.into()));
    let data: TagsData = gql::execute(&state.schema, TAGS_QUERY, variables).await?;
    let library = data.node.ok_or(ApiError::NotFound)?;
    let total = library.tags.edges.len() as u64;
    let mut body = CompletionContainer::new(library.title, &library.kind, offset, total);
    body.tags = library
        .tags
        .edges
        .into_iter()
        .skip(usize::try_from(offset).unwrap_or(usize::MAX))
        .take(size as usize)
        .map(|edge| {
            let id = database_id(&edge.node.id);
            CompletedTag {
                id,
                filter: format!("{filter}={id}"),
                tag: edge.node.name,
                tag_type,
                tag_key: edge
                    .node
                    .person
                    .as_ref()
                    .and_then(|person| person.provider_key.clone()),
                thumb: edge.node.person.and_then(|person| person.thumb_url),
            }
        })
        .collect();
    body.size = body.tags.len();
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_spans_the_bytes_of_the_text() {
        assert_eq!(query_range("Dune", "dun").as_deref(), Some("0,2"));
        assert_eq!(
            query_range("Crocodile Dundee", "dun").as_deref(),
            Some("10,12")
        );
        assert_eq!(
            query_range("The First Slam Dunk", "first sl").as_deref(),
            Some("4,11")
        );
        // Bytes, not characters: `À` and `ç` are two each.
        assert_eq!(
            query_range("À tous les garçons que j'ai aimés", "aim").as_deref(),
            Some("30,32")
        );
        assert_eq!(
            query_range("À tous les garçons", "À TOUS").as_deref(),
            Some("0,6")
        );
        // Inside a word, before the word the match was made on.
        assert_eq!(
            query_range("3 Billboards : Les Panneaux de la vengeance", "d").as_deref(),
            Some("10,10")
        );
    }

    #[test]
    fn a_match_on_a_capital_outside_ascii_has_no_range() {
        assert_eq!(query_range("À tous les garçons", "à tous"), None);
    }

    #[test]
    fn names_the_fields_it_completes_and_refuses_the_unknown() {
        assert_eq!(Field::named("title").ok().flatten(), Some(Field::Title));
        assert!(matches!(
            Field::named("actor"),
            Ok(Some(Field::Tag { tag_type: 6, .. }))
        ));
        assert!(matches!(Field::named("studio"), Ok(None)));
        assert!(matches!(Field::named("foo"), Err(ApiError::BadRequest)));
    }
}
