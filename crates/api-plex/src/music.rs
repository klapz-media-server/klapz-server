//! `GET /library/sections/{sectionId}/albums`: a music library's albums.
//!
//! This server holds no music yet. The albums are asked of the official API all the same,
//! which answers that kind with nothing, and the container around them is the one the
//! reference sends for a library holding no album — captured on its film and series
//! libraries.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Serialize;

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{SectionItemsData, SECTION_ITEMS_QUERY};
use crate::items::{
    MetadataEntry, ViewStyle, GRID_VIEW_MODE, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX,
    MEDIA_TAG_VERSION,
};
use crate::language::Language;
use crate::library::shuffle_seed;
use crate::listing;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/library/sections/{sectionId}/albums", get(albums))
}

/// The reference's attributes, in its order. Always windowed, for the reason `/all` is.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumsContainer {
    size: u32,
    total_size: u64,
    offset: u64,
    allow_sync: bool,
    art: &'static str,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    mixed_parents: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    nocache: Option<bool>,
    thumb: &'static str,
    title1: String,
    title2: &'static str,
    view_group: &'static str,
    view_mode: u32,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<MetadataEntry>,
}

async fn albums(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<AlbumsContainer>, ApiError> {
    let request = listing::parse(&pairs)?;
    let offset = request.start;
    let first = request.size.unwrap_or(MAX_ITEMS).min(MAX_ITEMS);
    let (order_by, direction) = request.ordering();

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number(first.into()));
    variables.insert(Name::new("offset"), Value::Number(offset.into()));
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new(order_by)));
    variables.insert(Name::new("direction"), Value::Enum(Name::new(direction)));
    variables.insert(Name::new("seed"), Value::Number(shuffle_seed().into()));
    variables.insert(
        Name::new("filter"),
        request.filters.to_graphql(Some("ALBUM")),
    );

    let data: SectionItemsData =
        gql::execute(&state.schema, &SECTION_ITEMS_QUERY, variables).await?;
    let library = data.node.ok_or(ApiError::NotFound)?;

    let language = Language::of(&client);
    let metadata: Vec<MetadataEntry> = library
        .items
        .edges
        .iter()
        .filter_map(|edge| MetadataEntry::from_graphql(&edge.node, false, language))
        .collect();
    let total = library.stats.item_count;
    let style = ViewStyle::for_kind(&library.kind);
    let body = AlbumsContainer {
        size: u32::try_from(metadata.len()).unwrap_or(u32::MAX),
        total_size: total,
        offset,
        allow_sync: false,
        art: style.art,
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        mixed_parents: true,
        nocache: (library.kind == "SHOW").then_some(true),
        thumb: style.thumb,
        title1: library.title,
        title2: "By Album",
        view_group: "album",
        view_mode: GRID_VIEW_MODE,
        metadata,
    };
    Ok(PlexResponse::new(body, format).paged(offset, total))
}
