//! `GET /library/sections/{sectionId}/common`: what a set of items has in common.
//!
//! What Plex Web loads to edit several items at once, with `type`, `id` (a comma list) and
//! any listing filter. One `common` element: the values every item shares, `mixedFields`
//! naming those that differ, the tags every item carries and the fields every one locked.
//! Measured on the reference's films, series and episodes; the rules are in
//! `docs/spec-gaps.md`.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use klapz_api::item::ITEM_TYPE;
use klapz_api::library::LIBRARY_TYPE;
use klapz_api::GlobalId;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{database_id, GqlCollectionTag, GqlConnection, GqlTag};
use crate::items::{
    one_decimal, CollectionTagEntry, FieldEntry, TagEntry, ViewStyle, GRID_VIEW_MODE,
    LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION,
};
use crate::library::item_type;
use crate::listing;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

const COMMON_QUERY: &str = r"
fragment SharedTag on Tag {
  id
  name
  person { providerKey thumbUrl }
}

query PlexCommon($id: ID!, $filter: ItemFilterInput) {
  node(id: $id) {
    ... on Library {
      title
      kind
      common(filter: $filter) {
        mixed
        item { id }
        parent { id }
        grandparent { id }
        parentTitle
        grandparentTitle
        title
        titleSort
        originalTitle
        tagline
        summary
        studio
        contentRating
        year
        releasedOn
        rating
        number
        genres: tags(kind: GENRE, first: 500) { edges { node { ...SharedTag } } }
        countries: tags(kind: COUNTRY, first: 500) { edges { node { ...SharedTag } } }
        directors: tags(kind: DIRECTOR, first: 500) { edges { node { ...SharedTag } } }
        writers: tags(kind: WRITER, first: 500) { edges { node { ...SharedTag } } }
        cast: tags(kind: ACTOR, first: 500) { edges { node { ...SharedTag } character } }
        producers: tags(kind: PRODUCER, first: 500) { edges { node { ...SharedTag } } }
        collections(first: 500) { edges { node { tagNumber title guid summary } } }
        lockedFields
      }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct CommonData {
    node: Option<GqlLibrary>,
}

#[derive(Debug, Deserialize)]
struct GqlLibrary {
    title: String,
    kind: String,
    common: GqlCommon,
}

#[derive(Debug, Deserialize)]
struct GqlRef {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCommon {
    mixed: Vec<String>,
    item: Option<GqlRef>,
    parent: Option<GqlRef>,
    grandparent: Option<GqlRef>,
    parent_title: Option<String>,
    grandparent_title: Option<String>,
    title: Option<String>,
    title_sort: Option<String>,
    original_title: Option<String>,
    tagline: Option<String>,
    summary: Option<String>,
    studio: Option<String>,
    content_rating: Option<String>,
    year: Option<i32>,
    #[serde(
        default,
        deserialize_with = "crate::gql::date_time::optional_calendar_date"
    )]
    released_on: Option<String>,
    rating: Option<f64>,
    number: Option<i32>,
    genres: GqlConnection<GqlTag>,
    countries: GqlConnection<GqlTag>,
    directors: GqlConnection<GqlTag>,
    writers: GqlConnection<GqlTag>,
    cast: GqlConnection<GqlTag>,
    producers: GqlConnection<GqlTag>,
    collections: GqlConnection<GqlCollectionTag>,
    locked_fields: Vec<String>,
}

/// The fields a type's `mixedFields` can name, in the reference's order, by the official
/// API's `CommonField`. The reference also names `parentIndex`, `isAdult` and `userRating`,
/// which we do not hold.
const MOVIE_FIELDS: &[(&str, &str)] = &[
    ("title", "TITLE"),
    ("ratingKey", "ITEM"),
    ("titleSort", "TITLE_SORT"),
    ("tagline", "TAGLINE"),
    ("rating", "RATING"),
    ("summary", "SUMMARY"),
    ("year", "YEAR"),
    ("studio", "STUDIO"),
    ("originallyAvailableAt", "RELEASED_ON"),
    ("originalTitle", "ORIGINAL_TITLE"),
    ("contentRating", "CONTENT_RATING"),
];

const SEASON_FIELDS: &[(&str, &str)] = &[
    ("title", "TITLE"),
    ("parentTitle", "PARENT_TITLE"),
    ("ratingKey", "ITEM"),
    ("parentRatingKey", "PARENT"),
    ("titleSort", "TITLE_SORT"),
    ("index", "NUMBER"),
    ("summary", "SUMMARY"),
    ("year", "YEAR"),
    ("originallyAvailableAt", "RELEASED_ON"),
];

const EPISODE_FIELDS: &[(&str, &str)] = &[
    ("title", "TITLE"),
    ("parentTitle", "PARENT_TITLE"),
    ("grandparentTitle", "GRANDPARENT_TITLE"),
    ("ratingKey", "ITEM"),
    ("parentRatingKey", "PARENT"),
    ("grandparentRatingKey", "GRANDPARENT"),
    ("titleSort", "TITLE_SORT"),
    ("index", "NUMBER"),
    ("rating", "RATING"),
    ("summary", "SUMMARY"),
    ("year", "YEAR"),
    ("originallyAvailableAt", "RELEASED_ON"),
    ("originalTitle", "ORIGINAL_TITLE"),
    ("contentRating", "CONTENT_RATING"),
];

/// The fields `type` compares, a series' being a film's.
fn fields_of(plex_type: i32) -> &'static [(&'static str, &'static str)] {
    match plex_type {
        3 => SEASON_FIELDS,
        4 => EPISODE_FIELDS,
        _ => MOVIE_FIELDS,
    }
}

/// Response body: always one entry, of type `common`, even for a set of no items.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonContainer {
    size: u32,
    allow_sync: bool,
    art: &'static str,
    content: &'static str,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    nocache: Option<bool>,
    thumb: &'static str,
    title1: String,
    title2: &'static str,
    view_group: &'static str,
    view_mode: u32,
    #[serde(rename = "Metadata")]
    metadata: [CommonEntry; 1],
}

/// The shared values, in the reference's attribute order. A field the items differ on, or
/// that none holds, is absent; `guid` and `titleSort` are always written, empty when not
/// shared, and so is an episode's `parentTitle`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommonEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    grandparent_rating_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grandparent_title: Option<String>,
    mixed_fields: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_rating_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    guid: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    studio: Option<String>,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    title_sort: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tagline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    originally_available_at: Option<String>,
    #[serde(rename = "Genre", skip_serializing_if = "Vec::is_empty")]
    genre: Vec<TagEntry>,
    #[serde(rename = "Country", skip_serializing_if = "Vec::is_empty")]
    country: Vec<TagEntry>,
    #[serde(rename = "Collection", skip_serializing_if = "Vec::is_empty")]
    collection: Vec<CollectionTagEntry>,
    #[serde(rename = "Director", skip_serializing_if = "Vec::is_empty")]
    director: Vec<TagEntry>,
    #[serde(rename = "Writer", skip_serializing_if = "Vec::is_empty")]
    writer: Vec<TagEntry>,
    #[serde(rename = "Role", skip_serializing_if = "Vec::is_empty")]
    role: Vec<TagEntry>,
    #[serde(rename = "Producer", skip_serializing_if = "Vec::is_empty")]
    producer: Vec<TagEntry>,
    #[serde(rename = "Field", skip_serializing_if = "Vec::is_empty")]
    field: Vec<FieldEntry>,
}

impl CommonEntry {
    fn new(common: GqlCommon, plex_type: i32) -> Self {
        let fields = fields_of(plex_type);
        let mixed_fields = fields
            .iter()
            .filter(|(_, field)| common.mixed.iter().any(|mixed| mixed == field))
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(",");
        let compared = |field: &str| fields.iter().any(|(_, compared)| *compared == field);
        let mixed = |field: &str| common.mixed.iter().any(|mixed| mixed == field);
        let key_of = |reference: Option<&GqlRef>| {
            reference.map(|reference| database_id(&reference.id).to_string())
        };
        let rating_key = key_of(common.item.as_ref());
        // An episode's season title: shared, and empty when no season has one.
        let parent_title = if plex_type == 4 && !mixed("PARENT_TITLE") {
            Some(common.parent_title.unwrap_or_default())
        } else {
            common.parent_title.filter(|_| compared("PARENT_TITLE"))
        };
        Self {
            grandparent_rating_key: key_of(common.grandparent.as_ref())
                .filter(|_| compared("GRANDPARENT")),
            grandparent_title: common
                .grandparent_title
                .filter(|_| compared("GRANDPARENT_TITLE")),
            mixed_fields,
            parent_rating_key: key_of(common.parent.as_ref()).filter(|_| compared("PARENT")),
            parent_title,
            key: rating_key
                .as_ref()
                .map(|key| format!("/library/metadata/{key}/children")),
            rating_key,
            guid: "",
            studio: common.studio,
            kind: "common",
            title: common.title,
            title_sort: common.title_sort.unwrap_or_default(),
            original_title: common.original_title,
            content_rating: common.content_rating,
            summary: common.summary,
            index: common.number,
            rating: common.rating.map(one_decimal),
            year: common.year,
            tagline: common.tagline,
            originally_available_at: common.released_on,
            genre: TagEntry::all(&common.genres, "genre"),
            country: TagEntry::all(&common.countries, "country"),
            collection: CollectionTagEntry::all(&common.collections),
            director: TagEntry::all(&common.directors, "director"),
            writer: TagEntry::all(&common.writers, "writer"),
            role: common
                .cast
                .edges
                .iter()
                .map(TagEntry::from_cast_member)
                .collect(),
            producer: TagEntry::all(&common.producers, "producer"),
            field: FieldEntry::all(&common.locked_fields),
        }
    }
}

/// Reads `id`, a comma list of items. A number that is not one is refused, as is a list
/// longer than any window.
fn item_ids(pairs: &[(String, String)]) -> Result<Option<Value>, ApiError> {
    let Some((_, list)) = pairs.iter().rev().find(|(key, _)| key == "id") else {
        return Ok(None);
    };
    let ids = list
        .split(',')
        .map(|id| {
            id.parse::<i64>()
                .map(|id| Value::String(GlobalId::encode(ITEM_TYPE, id)))
                .map_err(|_| ApiError::BadRequest)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ids.len() > MAX_ITEMS as usize {
        return Err(ApiError::BadRequest);
    }
    Ok(Some(Value::List(ids)))
}

pub async fn common(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<CommonContainer>, ApiError> {
    let request = listing::parse(&pairs)?;
    // Required, as on the reference: the set is of one type.
    let plex_type = request.kind.ok_or(ApiError::BadRequest)?;
    let mut filter = request.filters.to_graphql(item_type(Some(plex_type)));
    if let Some(ids) = item_ids(&pairs)? {
        let mut fields = match filter {
            Value::Object(fields) => fields,
            _ => async_graphql::indexmap::IndexMap::new(),
        };
        fields.insert(Name::new("ids"), ids);
        filter = Value::Object(fields);
    }
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("filter"), filter);

    let data: CommonData = gql::execute(&state.schema, COMMON_QUERY, variables).await?;
    let library = data.node.ok_or(ApiError::NotFound)?;
    let style = ViewStyle::for_kind(&library.kind);
    let body = CommonContainer {
        size: 1,
        allow_sync: false,
        art: style.art,
        content: "secondary",
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        nocache: (library.kind == "SHOW").then_some(true),
        thumb: style.thumb,
        title1: library.title,
        title2: "Common",
        view_group: "secondary",
        view_mode: GRID_VIEW_MODE,
        metadata: [CommonEntry::new(library.common, plex_type)],
    };
    Ok(PlexResponse::new(body, format))
}
