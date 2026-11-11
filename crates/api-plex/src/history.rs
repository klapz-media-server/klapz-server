//! `/status/sessions/history/…`: what the caller watched, viewing by viewing.
//!
//! Bound onto `viewingHistory` and `deleteHistoryEntry`. Each Plex user has a server of its
//! own here, so the history a token reads is its own viewer's, every entry `accountID` 1 —
//! the account a server's owner has on the reference. The operator's GraphQL view of
//! everyone's history has no counterpart on this surface.
//!
//! Measured on the reference: the listing holds what was played to the end (`viewType` 0)
//! unless `type=1` asks for what was marked watched; entries come in the order written;
//! the item's keys are on a listing and not on a lookup.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::history::HISTORY_ENTRY_TYPE;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/status/sessions/history/all", get(history))
        .route(
            "/status/sessions/history/{history_id}",
            get(entry).delete(delete_entry),
        )
}

const ENTRY_FIELDS: &str = r"
fragment PlexHistoryEntry on HistoryEntry {
  databaseId
  kind
  viewedAt
  libraryId
  item {
    __typename
    id
    title
    ... on Movie { releasedOn }
    ... on Episode { number releasedOn season { id number } show { id title } }
  }
}
";

const HISTORY_QUERY: &str = r"
query PlexHistory(
  $first: Int!
  $offset: Int!
  $filter: HistoryFilterInput!
  $orderBy: HistoryOrderType!
  $direction: SortDirection!
) {
  viewingHistory(
    first: $first
    offset: $offset
    filter: $filter
    orderBy: $orderBy
    direction: $direction
  ) {
    totalCount
    edges { node { ...PlexHistoryEntry } }
  }
}
";

const ENTRY_QUERY: &str = r"
query PlexHistoryEntry($id: ID!) { node(id: $id) { ...PlexHistoryEntry } }
";

const ITEM_EXISTS: &str = r"
query PlexHistoryItem($id: ID!) { node(id: $id) { id } }
";

const DELETE: &str = r"
mutation PlexDeleteHistoryEntry($input: DeleteHistoryEntryInput!) {
  deleteHistoryEntry(input: $input) { deletedEntryId }
}
";

/// The account every entry is published under: the server's owner, which the caller is on
/// the server it sees.
const OWNER_ACCOUNT: i64 = 1;

/// The reference's `type`: what was played to the end, and what was marked watched.
const PLAYED: i64 = 0;
const MARKED: i64 = 1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryData {
    viewing_history: GqlHistory,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlHistory {
    total_count: u64,
    edges: Vec<GqlEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlEdge {
    node: GqlEntry,
}

#[derive(Debug, Deserialize)]
struct EntryData {
    node: Option<GqlEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlEntry {
    database_id: String,
    kind: String,
    viewed_at: i64,
    library_id: String,
    item: Option<GqlHistoryItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlHistoryItem {
    #[serde(rename = "__typename")]
    typename: String,
    id: String,
    title: Option<String>,
    number: Option<i32>,
    released_on: Option<String>,
    season: Option<GqlParent>,
    show: Option<GqlParent>,
}

#[derive(Debug, Deserialize)]
struct GqlParent {
    id: String,
    number: Option<i32>,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NodeData {
    node: Option<serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteData {
    delete_history_entry: GqlDeleted,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlDeleted {
    deleted_entry_id: Option<String>,
}

/// One entry, in the reference's attribute order.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryEntry {
    history_key: String,
    /// The item's keys: a listing carries them, a lookup does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating_key: Option<String>,
    /// A string, where the other numbers are numbers.
    #[serde(rename = "librarySectionID")]
    library_section_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grandparent_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grandparent_title: Option<String>,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    originally_available_at: Option<String>,
    viewed_at: i64,
    #[serde(rename = "accountID")]
    account_id: i64,
    /// Present only for an item marked watched; what was played has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    view_type: Option<i64>,
}

impl HistoryEntry {
    /// `None` for an entry with no item to show, or one of an item that is not played: a
    /// history holds films and episodes.
    fn from_graphql(entry: GqlEntry, with_keys: bool) -> Option<Self> {
        let item = entry.item?;
        let rating_key = database_id(&item.id)?;
        let kind = match item.typename.as_str() {
            "Movie" => "movie",
            "Episode" => "episode",
            _ => return None,
        };
        let episode = kind == "episode";
        let metadata_key = |id: &str| database_id(id).map(|key| format!("/library/metadata/{key}"));
        let season = item.season.as_ref().filter(|_| episode);
        let show = item.show.as_ref().filter(|_| episode);
        Some(Self {
            history_key: format!("/status/sessions/history/{}", entry.database_id),
            key: with_keys.then(|| format!("/library/metadata/{rating_key}")),
            rating_key: with_keys.then(|| rating_key.to_string()),
            library_section_id: database_id(&entry.library_id)?.to_string(),
            parent_key: season
                .filter(|_| with_keys)
                .and_then(|season| metadata_key(&season.id)),
            grandparent_key: show
                .filter(|_| with_keys)
                .and_then(|show| metadata_key(&show.id)),
            title: item.title,
            grandparent_title: show.and_then(|show| show.title.clone()),
            kind,
            index: item.number.filter(|_| episode),
            parent_index: season.and_then(|season| season.number),
            originally_available_at: item.released_on,
            viewed_at: entry.viewed_at,
            account_id: OWNER_ACCOUNT,
            view_type: (entry.kind == "MARKED").then_some(MARKED),
        })
    }
}

fn database_id(global: &str) -> Option<i64> {
    GlobalId::decode(global).ok().map(|decoded| decoded.key)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryContainer {
    size: usize,
    /// Always stated, with `offset`: the listing always windows, where the reference sends
    /// both only when asked for a window.
    total_size: u64,
    offset: u64,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<HistoryEntry>,
}

#[derive(Debug, Serialize)]
struct EntryContainer {
    size: usize,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<HistoryEntry>,
}

/// A history request, as far as the reference reads it.
#[derive(Debug, Default, PartialEq, Eq)]
struct HistoryRequest {
    start: u64,
    size: Option<u32>,
    /// Some other account's history, which this server holds none of.
    others_only: bool,
    item: Option<i64>,
    libraries: Option<Vec<i64>>,
    viewed_since: Option<i64>,
    viewed_until: Option<i64>,
    /// `None` when the `type` asked for is neither played nor marked.
    kind: Option<&'static str>,
    order: (&'static str, &'static str),
}

fn number(value: &str) -> Result<i64, ApiError> {
    value.parse().map_err(|_| ApiError::BadRequest)
}

fn numbers(value: &str) -> Result<Vec<i64>, ApiError> {
    value.split(',').map(number).collect()
}

/// `sort=viewedAt:desc,accountID`, as the reference reads it: four fields, `asc` or `desc`,
/// anything else a `400`. Every entry here has one account and one `type`, so those two
/// order nothing; ordering by both a date and a library is more than one key, which the
/// official API's history does not take.
fn ordering(value: &str) -> Result<(&'static str, &'static str), ApiError> {
    let mut order = None;
    for term in value.split(',') {
        let (field, direction) = term.split_once(':').unwrap_or((term, "asc"));
        let direction = match direction {
            "asc" => "ASCENDING",
            "desc" => "DESCENDING",
            _ => return Err(ApiError::BadRequest),
        };
        let key = match field {
            "viewedAt" => "VIEWED_AT",
            "librarySectionID" => "LIBRARY",
            "accountID" | "type" => continue,
            _ => return Err(ApiError::BadRequest),
        };
        if order.is_some() {
            return Err(ApiError::NotImplemented);
        }
        order = Some((key, direction));
    }
    Ok(order.unwrap_or(("RECORDED", "ASCENDING")))
}

/// Reads the query pairs. Parameters the reference ignores — `deviceID`, `ratingKey`,
/// `viewedAt>>` — are ignored here too; an operator on a field it honors that we cannot
/// translate answers `501`.
fn parse(pairs: &[(String, String)]) -> Result<HistoryRequest, ApiError> {
    let mut request = HistoryRequest {
        kind: Some("PLAYED"),
        order: ("RECORDED", "ASCENDING"),
        ..HistoryRequest::default()
    };
    for (key, value) in pairs {
        match key.as_str() {
            "X-Plex-Container-Start" => request.start = value.parse().unwrap_or(0),
            "X-Plex-Container-Size" => request.size = value.parse().ok(),
            "accountID" => request.others_only |= !numbers(value)?.contains(&OWNER_ACCOUNT),
            "accountID!" => request.others_only |= numbers(value)?.contains(&OWNER_ACCOUNT),
            "librarySectionID" => request.libraries = Some(numbers(value)?),
            "metadataItemID" => request.item = Some(number(value)?),
            "viewedAt" => {
                let at = number(value)?;
                request.viewed_since = Some(at);
                request.viewed_until = Some(at);
            }
            // The operator rides on the name: `viewedAt>=100` arrives as (`viewedAt>`, `100`).
            "viewedAt>" => request.viewed_since = Some(number(value)?),
            "viewedAt<" => request.viewed_until = Some(number(value)?),
            "type" => {
                request.kind = match number(value)? {
                    PLAYED => Some("PLAYED"),
                    MARKED => Some("MARKED"),
                    _ => None,
                }
            }
            "sort" => request.order = ordering(value)?,
            "viewedAt!" | "librarySectionID!" | "metadataItemID!" | "type!" => {
                return Err(ApiError::NotImplemented)
            }
            _ => {}
        }
    }
    Ok(request)
}

/// `GET /status/sessions/history/all`. Always windowed, at most `MAX_ITEMS` an answer.
async fn history(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<HistoryContainer>, ApiError> {
    let request = parse(&pairs)?;
    if let Some(item) = request.item {
        let mut variables = Variables::default();
        variables.insert(
            Name::new("id"),
            Value::String(GlobalId::encode(ITEM_TYPE, item)),
        );
        let found: NodeData = gql::execute(&state.schema, ITEM_EXISTS, variables).await?;
        found.node.ok_or(ApiError::NotFound)?;
    }
    let offset = request.start;
    let empty = || {
        let body = HistoryContainer {
            size: 0,
            total_size: 0,
            offset,
            metadata: Vec::new(),
        };
        Ok(PlexResponse::new(body, format).paged(offset, 0))
    };
    let Some(kind) = request.kind.filter(|_| !request.others_only) else {
        return empty();
    };

    let mut filter = vec![
        (Name::new("mine"), Value::Boolean(true)),
        (Name::new("kind"), Value::Enum(Name::new(kind))),
    ];
    if let Some(item) = request.item {
        filter.push((
            Name::new("item"),
            Value::String(GlobalId::encode(ITEM_TYPE, item)),
        ));
    }
    if let Some(libraries) = &request.libraries {
        let ids = libraries
            .iter()
            .map(|section| Value::String(GlobalId::encode(LIBRARY_TYPE, *section)))
            .collect();
        filter.push((Name::new("libraries"), Value::List(ids)));
    }
    if let Some(since) = request.viewed_since {
        filter.push((Name::new("viewedSince"), Value::Number(since.into())));
    }
    if let Some(until) = request.viewed_until {
        filter.push((Name::new("viewedUntil"), Value::Number(until.into())));
    }
    let (order_by, direction) = request.order;
    let mut variables = Variables::default();
    variables.insert(
        Name::new("first"),
        Value::Number(request.size.unwrap_or(MAX_ITEMS).min(MAX_ITEMS).into()),
    );
    variables.insert(Name::new("offset"), Value::Number(offset.into()));
    variables.insert(
        Name::new("filter"),
        Value::Object(filter.into_iter().collect()),
    );
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new(order_by)));
    variables.insert(Name::new("direction"), Value::Enum(Name::new(direction)));

    let query = format!("{HISTORY_QUERY}{ENTRY_FIELDS}");
    let data: HistoryData = gql::execute(&state.schema, &query, variables).await?;
    let history = data.viewing_history;
    let metadata: Vec<HistoryEntry> = history
        .edges
        .into_iter()
        .filter_map(|edge| HistoryEntry::from_graphql(edge.node, true))
        .collect();
    let body = HistoryContainer {
        size: metadata.len(),
        total_size: history.total_count,
        offset,
        metadata,
    };
    Ok(PlexResponse::new(body, format).paged(offset, history.total_count))
}

/// The entry a path names, as a global id. One that is no number names nothing: `404`, as
/// on the reference.
fn entry_id(history_id: &str) -> Result<String, ApiError> {
    let key: i64 = history_id.parse().map_err(|_| ApiError::NotFound)?;
    Ok(GlobalId::encode(HISTORY_ENTRY_TYPE, key))
}

/// `GET /status/sessions/history/{historyId}`: one entry, without the item's keys.
async fn entry(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(history_id): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<EntryContainer>, ApiError> {
    let mut variables = Variables::default();
    variables.insert(Name::new("id"), Value::String(entry_id(&history_id)?));
    let query = format!("{ENTRY_QUERY}{ENTRY_FIELDS}");
    let data: EntryData = gql::execute(&state.schema, &query, variables).await?;
    let entry = data
        .node
        .and_then(|entry| HistoryEntry::from_graphql(entry, false))
        .ok_or(ApiError::NotFound)?;
    let body = EntryContainer {
        size: 1,
        metadata: vec![entry],
    };
    Ok(PlexResponse::new(body, format))
}

/// `DELETE /status/sessions/history/{historyId}`: the empty container, or `404` for an
/// entry the caller cannot see. The watch state stays as it is.
async fn delete_entry(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(history_id): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<EntryContainer>, ApiError> {
    let input = [(Name::new("entry"), Value::String(entry_id(&history_id)?))];
    let mut variables = Variables::default();
    variables.insert(
        Name::new("input"),
        Value::Object(input.into_iter().collect()),
    );
    let data: DeleteData = gql::execute(&state.schema, DELETE, variables).await?;
    data.delete_history_entry
        .deleted_entry_id
        .ok_or(ApiError::NotFound)?;
    let body = EntryContainer {
        size: 0,
        metadata: Vec::new(),
    };
    Ok(PlexResponse::new(body, format))
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn lists_what_was_played_in_the_order_written_by_default() {
        let request = parse(&[]).unwrap();
        assert_eq!(request.kind, Some("PLAYED"));
        assert_eq!(request.order, ("RECORDED", "ASCENDING"));
        assert_eq!(
            parse(&pairs(&[("type", "1")])).unwrap().kind,
            Some("MARKED")
        );
        assert_eq!(parse(&pairs(&[("type", "4")])).unwrap().kind, None);
    }

    #[test]
    fn reads_the_date_bounds_off_the_parameter_name() {
        let request = parse(&pairs(&[("viewedAt>", "100"), ("viewedAt<", "200")])).unwrap();
        assert_eq!(
            (request.viewed_since, request.viewed_until),
            (Some(100), Some(200))
        );
        let exact = parse(&pairs(&[("viewedAt", "150")])).unwrap();
        assert_eq!(
            (exact.viewed_since, exact.viewed_until),
            (Some(150), Some(150))
        );
        // Ignored by the reference, ignored here.
        assert_eq!(
            parse(&pairs(&[("viewedAt>>", "100")]))
                .unwrap()
                .viewed_since,
            None
        );
    }

    #[test]
    fn refuses_what_the_reference_refuses() {
        // Measured: each of these is a `400` there.
        for query in [
            [("librarySectionID", "abc")],
            [("metadataItemID", "abc")],
            [("type", "movie")],
            [("viewedAt>", "-1d")],
            [("sort", "title")],
            [("sort", "viewedAt:DESC")],
            [("sort", "metadataItemID")],
        ] {
            assert!(
                matches!(parse(&pairs(&query)), Err(ApiError::BadRequest)),
                "{query:?}"
            );
        }
        assert!(matches!(
            parse(&pairs(&[("viewedAt!", "100")])),
            Err(ApiError::NotImplemented)
        ));
    }

    #[test]
    fn the_account_and_the_type_order_nothing() {
        assert_eq!(
            ordering("viewedAt:desc,accountID").unwrap(),
            ("VIEWED_AT", "DESCENDING")
        );
        assert_eq!(ordering("type").unwrap(), ("RECORDED", "ASCENDING"));
        assert_eq!(
            ordering("librarySectionID").unwrap(),
            ("LIBRARY", "ASCENDING")
        );
        assert!(matches!(
            ordering("librarySectionID,viewedAt:desc"),
            Err(ApiError::NotImplemented)
        ));
    }

    #[test]
    fn another_account_has_no_history_here() {
        assert!(parse(&pairs(&[("accountID", "2")])).unwrap().others_only);
        assert!(!parse(&pairs(&[("accountID", "1,2")])).unwrap().others_only);
        assert!(parse(&pairs(&[("accountID!", "1")])).unwrap().others_only);
    }
}
