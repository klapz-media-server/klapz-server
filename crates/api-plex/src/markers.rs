//! Markers on the Plex surface: `includeMarkers=1` on the item routes, and the bookmark
//! routes `POST /library/metadata/{ids}/marker`, `PUT` and `DELETE …/marker/{marker}`.
//!
//! Those routes write the caller's own markers, which the reference calls bookmarks and keeps
//! per account. Measured on the reference's test libraries:
//!
//! - `type` is a name, `bookmark`; a number or an unknown name is a `400`, as is a missing
//!   `startTimeOffset`, a negative one, and an `endTimeOffset` not after it. The end is
//!   optional;
//! - `attributes[title]` and `attributes[color]` are kept, any other attribute ignored;
//! - an add identical in span to a bookmark of the item answers that bookmark, its title
//!   unchanged; a new one is answered with an empty `Attributes`, a found one with its id;
//! - `PUT` ignores `type`, keeps the end and the title when they are not given, and answers
//!   the bookmark with its attributes; `DELETE` answers an empty `200`;
//! - a marker of another item is a `400`, one that does not exist a `404`.
//!
//! Each goes through the official API's `addMarker`, `updateMarker` or `deleteMarker`. Where we
//! differ: a `type` other than `bookmark` is refused with `400` — the intro, credits and
//! commercials are the work's, written on the official API by the library's editors, and the
//! reference, which could not be asked without creating a marker its `DELETE` refuses, was not
//! measured there; a bookmark on a series or a season is a `400`, where the reference keeps
//! one nothing can play.

use std::sync::LazyLock;

use async_graphql::Variables;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{post, put};
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::marker::MARKER_TYPE;
use open_plex_api::GlobalId;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as Json};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{with_shared_fragments, GqlMarker};
use crate::items::{
    MarkerAttributes, MarkerEntry, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION,
};
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/metadata/{ids}/marker", post(add))
        .route(
            "/library/metadata/{ids}/marker/{marker}",
            put(update).delete(delete),
        )
}

/// Whether a request asks for markers: `includeMarkers=1`, which Plex Web sends on every
/// item it opens and every play queue.
pub fn requested(pairs: &[(String, String)]) -> bool {
    pairs
        .iter()
        .any(|(key, value)| key == "includeMarkers" && value == "1")
}

/// The answer to an add or an edit: the bookmark, alone in its container.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkerContainer {
    size: u32,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(rename = "Marker")]
    marker: Vec<MarkerEntry>,
}

impl MarkerContainer {
    fn of(marker: MarkerEntry) -> Self {
        Self {
            size: 1,
            identifier: LIBRARY_IDENTIFIER,
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            marker: vec![marker],
        }
    }
}

/// What a request says a bookmark is.
#[derive(Debug, Default, PartialEq, Eq)]
struct Bookmark {
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    title: Option<String>,
    color: Option<String>,
}

impl Bookmark {
    /// Reads the parameters; a number that does not parse is a `400`, as on the reference.
    fn read(pairs: &[(String, String)]) -> Result<Self, ApiError> {
        let offset = |value: &str| value.parse::<i64>().map_err(|_| ApiError::BadRequest);
        let mut bookmark = Self::default();
        for (key, value) in pairs {
            match key.as_str() {
                "startTimeOffset" => bookmark.start_ms = Some(offset(value)?),
                "endTimeOffset" => bookmark.end_ms = Some(offset(value)?),
                "attributes[title]" => bookmark.title = Some(value.clone()),
                "attributes[color]" => bookmark.color = Some(value.clone()),
                _ => {}
            }
        }
        Ok(bookmark)
    }

    /// The input fields it sets, an absent one left out.
    fn fields(&self) -> Map<String, Json> {
        let mut fields = Map::new();
        if let Some(start) = self.start_ms {
            fields.insert("startMs".to_owned(), json!(start));
        }
        if let Some(end) = self.end_ms {
            fields.insert("endMs".to_owned(), json!(end));
        }
        if let Some(title) = &self.title {
            fields.insert("title".to_owned(), json!(title));
        }
        if let Some(color) = &self.color {
            fields.insert("color".to_owned(), json!(color));
        }
        fields
    }
}

/// The item a path names: one number, as a client sends it.
fn item_of(ids: &str) -> Result<i64, ApiError> {
    ids.trim().parse().map_err(|_| ApiError::NotFound)
}

/// A refusal of the official API, as the status the reference answers.
fn refusal(errors: &[UserError]) -> Option<ApiError> {
    let error = errors.first()?;
    tracing::info!(code = error.code, "refused a marker");
    Some(match error.code.as_str() {
        "ITEM_NOT_FOUND" | "MARKER_NOT_FOUND" => ApiError::NotFound,
        _ => ApiError::BadRequest,
    })
}

#[derive(Debug, Deserialize)]
struct UserError {
    code: String,
}

static ADD: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
mutation PlexAddMarker($input: AddMarkerInput!) {
  result: addMarker(input: $input) {
    added
    marker { ...MarkerFields }
    userErrors { code }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
struct AddData {
    result: AddResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddResult {
    added: bool,
    marker: Option<GqlMarker>,
    user_errors: Vec<UserError>,
}

/// `POST /library/metadata/{ids}/marker`.
async fn add(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<MarkerContainer>, ApiError> {
    let item = item_of(&ids)?;
    let kind = pairs
        .iter()
        .rev()
        .find(|(key, _)| key == "type")
        .map(|(_, value)| value.as_str());
    if kind != Some("bookmark") {
        return Err(ApiError::BadRequest);
    }
    let bookmark = Bookmark::read(&pairs)?;
    if bookmark.start_ms.is_none() {
        return Err(ApiError::BadRequest);
    }

    let mut input = bookmark.fields();
    input.insert(
        "itemId".to_owned(),
        json!(GlobalId::encode(ITEM_TYPE, item)),
    );
    input.insert("kind".to_owned(), json!("BOOKMARK"));
    let variables = Variables::from_json(json!({ "input": input }));
    let data: AddData = gql::execute(&state.schema, &ADD, variables).await?;
    if let Some(error) = refusal(&data.result.user_errors) {
        return Err(error);
    }
    let marker = data
        .result
        .marker
        .as_ref()
        .and_then(MarkerEntry::from_graphql)
        .ok_or(ApiError::NotFound)?;
    // The reference answers a new bookmark before it has an id, and with none of the
    // attributes it was just given.
    let marker = if data.result.added {
        MarkerEntry {
            attributes: MarkerAttributes::default(),
            ..marker
        }
    } else {
        marker
    };
    Ok(PlexResponse::new(MarkerContainer::of(marker), format))
}

const TARGET: &str = r"
query PlexMarkerTarget($id: ID!) {
  node(id: $id) { ... on Marker { kind item { id } } }
}";

#[derive(Debug, Deserialize)]
struct TargetData {
    node: Option<Target>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Target {
    kind: Option<String>,
    item: Option<TargetItem>,
}

#[derive(Debug, Deserialize)]
struct TargetItem {
    id: String,
}

/// The bookmark a path names, as its global id, once it is known to be the caller's bookmark
/// on that item: `404` when the caller has no such marker, `400` when it is another item's
/// or not a bookmark.
async fn bookmark_of(state: &PlexState, ids: &str, marker: &str) -> Result<String, ApiError> {
    let item = item_of(ids)?;
    let marker: i64 = marker.trim().parse().map_err(|_| ApiError::NotFound)?;
    let id = GlobalId::encode(MARKER_TYPE, marker);
    let variables = Variables::from_json(json!({ "id": id }));
    let data: TargetData = gql::execute(&state.schema, TARGET, variables).await?;
    let target = data.node.unwrap_or_default();
    let (Some(kind), Some(on)) = (target.kind, target.item) else {
        return Err(ApiError::NotFound);
    };
    if on.id != GlobalId::encode(ITEM_TYPE, item) || kind != "BOOKMARK" {
        return Err(ApiError::BadRequest);
    }
    Ok(id)
}

static UPDATE: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
mutation PlexUpdateMarker($input: UpdateMarkerInput!) {
  result: updateMarker(input: $input) {
    marker { ...MarkerFields }
    userErrors { code }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
struct UpdateData {
    result: UpdateResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateResult {
    marker: Option<GqlMarker>,
    user_errors: Vec<UserError>,
}

/// `PUT /library/metadata/{ids}/marker/{marker}`.
async fn update(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((ids, marker)): Path<(String, String)>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<MarkerContainer>, ApiError> {
    let bookmark = Bookmark::read(&pairs)?;
    if bookmark.start_ms.is_none() {
        return Err(ApiError::BadRequest);
    }
    let id = bookmark_of(&state, &ids, &marker).await?;

    let mut input = bookmark.fields();
    input.insert("id".to_owned(), json!(id));
    let variables = Variables::from_json(json!({ "input": input }));
    let data: UpdateData = gql::execute(&state.schema, &UPDATE, variables).await?;
    if let Some(error) = refusal(&data.result.user_errors) {
        return Err(error);
    }
    let marker = data
        .result
        .marker
        .as_ref()
        .and_then(MarkerEntry::from_graphql)
        .ok_or(ApiError::NotFound)?;
    Ok(PlexResponse::new(MarkerContainer::of(marker), format))
}

const DELETE: &str = r"
mutation PlexDeleteMarker($input: DeleteMarkerInput!) {
  result: deleteMarker(input: $input) { userErrors { code } }
}";

#[derive(Debug, Deserialize)]
struct DeleteData {
    result: DeleteResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteResult {
    user_errors: Vec<UserError>,
}

/// `DELETE /library/metadata/{ids}/marker/{marker}`.
async fn delete(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((ids, marker)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let id = bookmark_of(&state, &ids, &marker).await?;
    let variables = Variables::from_json(json!({ "input": { "id": id } }));
    let data: DeleteData = gql::execute(&state.schema, DELETE, variables).await?;
    if let Some(error) = refusal(&data.result.user_errors) {
        return Err(error);
    }
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn markers_are_asked_for_with_a_one_and_nothing_else() {
        assert!(requested(&pairs(&[("includeMarkers", "1")])));
        assert!(!requested(&pairs(&[("includeMarkers", "0")])));
        assert!(!requested(&pairs(&[("includeMarkers", "true")])));
        assert!(!requested(&pairs(&[])));
    }

    #[test]
    fn reads_a_bookmark_and_its_two_attributes() {
        let bookmark = Bookmark::read(&pairs(&[
            ("type", "bookmark"),
            ("startTimeOffset", "5000"),
            ("attributes[title]", "Spot"),
            ("attributes[color]", "ff0000"),
            ("attributes[foo]", "bar"),
        ]))
        .unwrap();
        assert_eq!(
            bookmark,
            Bookmark {
                start_ms: Some(5000),
                end_ms: None,
                title: Some("Spot".to_owned()),
                color: Some("ff0000".to_owned()),
            }
        );
        assert_eq!(
            Json::Object(bookmark.fields()),
            json!({ "startMs": 5000, "title": "Spot", "color": "ff0000" })
        );
    }

    #[test]
    fn an_offset_that_is_not_a_number_is_a_bad_request() {
        assert!(matches!(
            Bookmark::read(&pairs(&[("startTimeOffset", "x")])),
            Err(ApiError::BadRequest)
        ));
    }
}
