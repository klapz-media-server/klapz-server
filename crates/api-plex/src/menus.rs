//! `GET /library/sections/{sectionId}/filters` and `/sorts`: a library's menus.
//!
//! One type's `Filter` or `Sort` list from the `Meta` block of [`crate::meta`], as
//! `Directory` elements: the same entries, so the menus offer exactly what a listing honors.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::items::{
    ViewStyle, GRID_VIEW_MODE, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION,
};
use crate::language::Language;
use crate::meta::{self, Menus, MetaFilter, MetaSort};
use crate::response::{PlexResponse, ResponseFormat, CONTAINER_START};
use crate::PlexState;

const LIBRARY_QUERY: &str = r"
query PlexLibraryMenus($id: ID!) {
  node(id: $id) { ... on Library { title kind } }
}
";

#[derive(Debug, Deserialize)]
struct LibraryData {
    node: Option<GqlLibrary>,
}

#[derive(Debug, Deserialize)]
struct GqlLibrary {
    title: String,
    kind: String,
}

/// Response body: a menu, whole unless the client asked for a window, as on the reference —
/// these lists are a few dozen entries at most.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuContainer<T> {
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<usize>,
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
    view_group: &'static str,
    view_mode: u32,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    directory: Vec<T>,
}

/// The window the client asked for, which needs both parameters, as everywhere on the
/// reference.
fn window(pairs: &[(String, String)]) -> Option<(usize, usize)> {
    let value = |name: &str| {
        pairs
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| value.parse().ok())
    };
    Some((value(CONTAINER_START)?, value("X-Plex-Container-Size")?))
}

fn requested_type(pairs: &[(String, String)]) -> Option<i32> {
    pairs
        .iter()
        .rev()
        .find(|(key, _)| key == "type")
        .and_then(|(_, value)| value.parse().ok())
}

/// One menu of a library, `pick` choosing which.
async fn menu<T: Serialize>(
    state: &PlexState,
    client: &ClientIdentity,
    section: i64,
    pairs: &[(String, String)],
    format: ResponseFormat,
    pick: fn(Menus) -> Vec<T>,
) -> Result<PlexResponse<MenuContainer<T>>, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    let data: LibraryData = gql::execute(&state.schema, LIBRARY_QUERY, variables).await?;
    let library = data.node.ok_or(ApiError::NotFound)?;
    // A library of a kind with no menus, music, lists none.
    let entries = meta::menus(
        section,
        &library.kind,
        requested_type(pairs),
        Language::of(client),
    )
    .map(pick)
    .unwrap_or_default();

    let total = entries.len();
    let window = window(pairs);
    let directory: Vec<T> = match window {
        Some((offset, size)) => entries.into_iter().skip(offset).take(size).collect(),
        None => entries,
    };
    let style = ViewStyle::for_kind(&library.kind);
    let body = MenuContainer {
        size: directory.len(),
        total_size: window.map(|_| total),
        offset: window.map(|(offset, _)| offset),
        allow_sync: false,
        art: style.art,
        content: "secondary",
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        nocache: (library.kind == "SHOW").then_some(true),
        thumb: style.thumb,
        title1: library.title,
        view_group: "secondary",
        view_mode: GRID_VIEW_MODE,
        directory,
    };
    let response = PlexResponse::new(body, format);
    Ok(match window {
        Some((offset, _)) => response.paged(offset as u64, total as u64),
        None => response,
    })
}

pub async fn filters(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<MenuContainer<MetaFilter>>, ApiError> {
    menu(&state, &client, section, &pairs, format, |menus| {
        menus.filters
    })
    .await
}

pub async fn sorts(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<MenuContainer<MetaSort>>, ApiError> {
    menu(&state, &client, section, &pairs, format, |menus| {
        menus.sorts
    })
    .await
}
