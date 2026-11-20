//! A series' seasons and episodes, and what to watch next.

use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::dto::{item_dto, Fields, Item, ItemsResult, Shaping, ITEM_FIELDS};
use crate::error::JellyfinError;
use crate::gql;
use crate::id;
use crate::items::continue_watching;
use crate::listing::MAX_PAGE;
use crate::query::Query;
use crate::response::json;
use crate::JellyfinState;

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/shows/{id}/seasons", get(seasons))
        .route("/shows/{id}/episodes", get(episodes))
        .route("/shows/nextup", get(next_up))
}

#[derive(Deserialize)]
struct NodeData<T> {
    node: Option<T>,
}

#[derive(Deserialize)]
struct Connection<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct ShowSeasons {
    seasons: Option<Connection<Item>>,
}

#[derive(Deserialize)]
struct ShowEpisodes {
    episodes: Option<Connection<Item>>,
}

/// The window a request asks of a list the official API gave whole, as a page.
fn answer(
    items: &[Item],
    query: &Query,
    state: &JellyfinState,
    fields: &Fields,
) -> Result<Response, JellyfinError> {
    let start = query.count("startIndex")?.unwrap_or(0);
    let limit = query.count("limit")?.unwrap_or(MAX_PAGE);
    let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
    let shaping = Shaping {
        server_id: &state.server.id,
        fields,
    };
    let shown: Vec<Value> = items
        .iter()
        .skip(usize::try_from(start).unwrap_or(usize::MAX))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .map(|item| item_dto(item, &shaping))
        .collect();
    Ok(json(&ItemsResult {
        items: shown,
        total_record_count: total,
        start_index: start,
    }))
}

async fn seasons(
    State(state): State<JellyfinState>,
    Path(show): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    let show = id::global_of_type(&show, open_plex_api::item::ITEM_TYPE)?;
    let fields = Fields::named(query.list("fields"));
    let seasons_query = format!(
        r"query($id: ID!, $people: Boolean!) {{
            node(id: $id) {{ ... on Show {{
                seasons(first: {MAX_PAGE}) {{ nodes {{ ...JellyfinItem }} }}
            }} }}
        }}
        {ITEM_FIELDS}"
    );
    let data: NodeData<ShowSeasons> = gql::query(
        &state.schema,
        &seasons_query,
        json!({ "id": show, "people": fields.wants_people() }),
    )
    .await?;
    let seasons = data
        .node
        .and_then(|show| show.seasons)
        .ok_or(JellyfinError::NotFound)?
        .nodes;
    answer(&seasons, &query, &state, &fields)
}

/// A series' episodes in order, or one season's: by `seasonId`, or by the season's number.
async fn episodes(
    State(state): State<JellyfinState>,
    Path(show): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    let show = id::global_of_type(&show, open_plex_api::item::ITEM_TYPE)?;
    let fields = Fields::named(query.list("fields"));
    let episodes_query = format!(
        r"query($id: ID!, $people: Boolean!) {{
            node(id: $id) {{ ... on Show {{
                episodes(first: {MAX_PAGE}) {{ nodes {{ ...JellyfinItem }} }}
            }} }}
        }}
        {ITEM_FIELDS}"
    );
    let data: NodeData<ShowEpisodes> = gql::query(
        &state.schema,
        &episodes_query,
        json!({ "id": show, "people": fields.wants_people() }),
    )
    .await?;
    let mut episodes = data
        .node
        .and_then(|show| show.episodes)
        .ok_or(JellyfinError::NotFound)?
        .nodes;
    if let Some(season) = query.get("seasonId") {
        let season = id::global_of_type(season, open_plex_api::item::ITEM_TYPE)?;
        episodes.retain(|episode| {
            episode
                .season
                .as_ref()
                .is_some_and(|candidate| candidate.id == season)
        });
    } else if let Some(number) = query.get("season") {
        let number: i32 = number
            .parse()
            .map_err(|_| JellyfinError::BadRequest("a season number is not a number"))?;
        episodes.retain(|episode| {
            episode
                .season
                .as_ref()
                .is_some_and(|season| season.number == Some(number))
        });
    }
    answer(&episodes, &query, &state, &fields)
}

/// Each series' next episode, as the official API's continue-watching rule finds it.
async fn next_up(
    State(state): State<JellyfinState>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    let fields = Fields::named(query.list("fields"));
    let library = query
        .get("parentId")
        .map(|parent| id::global_of_type(parent, open_plex_api::library::LIBRARY_TYPE))
        .transpose()?;
    let series = query
        .get("seriesId")
        .map(|series| id::global_of_type(series, open_plex_api::item::ITEM_TYPE))
        .transpose()?;
    let next: Vec<Item> = continue_watching(&state, library, &fields)
        .await?
        .into_iter()
        .filter(|(item, reason)| reason == "UP_NEXT" && item.type_name == "Episode")
        .map(|(item, _)| item)
        .filter(|item| {
            series
                .as_ref()
                .is_none_or(|series| item.show.as_ref().is_some_and(|show| show.id == *series))
        })
        .collect();
    answer(&next, &query, &state, &fields)
}
