//! `/MediaSegments/{itemId}`: an item's intro, credits and commercials, which players skip.
//!
//! The official API's markers of the work, as Jellyfin's segment types: an intro is an `Intro`,
//! credits an `Outro`, a commercial a `Commercial`. A bookmark is a viewer's own place, no
//! segment of the work.

use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::dto::ItemsResult;
use crate::error::JellyfinError;
use crate::gql;
use crate::id;
use crate::query::Query;
use crate::response::json;
use crate::time;
use crate::JellyfinState;

pub fn router() -> Router<JellyfinState> {
    Router::new().route("/mediasegments/{id}", get(segments))
}

const MARKERS: &str = r"query($id: ID!) { node(id: $id) {
    ... on Movie { markers(first: 500) { nodes { id kind startMs endMs } } }
    ... on Episode { markers(first: 500) { nodes { id kind startMs endMs } } }
} }";

#[derive(Deserialize)]
struct NodeData {
    node: Option<Marked>,
}

#[derive(Deserialize)]
struct Marked {
    markers: Option<Nodes>,
}

#[derive(Deserialize)]
struct Nodes {
    nodes: Vec<Marker>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    id: String,
    kind: String,
    start_ms: i64,
    end_ms: Option<i64>,
}

/// Jellyfin's segment type for a marker of the work; none for a bookmark.
fn segment_type(kind: &str) -> Option<&'static str> {
    match kind {
        "INTRO" => Some("Intro"),
        "CREDITS" => Some("Outro"),
        "COMMERCIAL" => Some("Commercial"),
        _ => None,
    }
}

/// A marker as a segment: one that ends, after it starts.
fn segment(item: &str, marker: &Marker) -> Option<Value> {
    let kind = segment_type(&marker.kind)?;
    let end = marker.end_ms.filter(|end| *end >= marker.start_ms)?;
    Some(json!({
        "Id": id::of_global(&marker.id)?,
        "ItemId": item,
        "Type": kind,
        "StartTicks": time::ticks(marker.start_ms),
        "EndTicks": time::ticks(end),
    }))
}

async fn segments(
    State(state): State<JellyfinState>,
    Path(item): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let global = id::global_of_type(&item, klapz_api::item::ITEM_TYPE)?;
    let item = id::of_global(&global).unwrap_or_default();
    let wanted: Vec<String> = query
        .list("includeSegmentTypes")
        .iter()
        .map(|kind| kind.to_ascii_lowercase())
        .collect();
    let data: NodeData = gql::query(&state.schema, MARKERS, json!({ "id": global })).await?;
    let marked = data.node.ok_or(JellyfinError::NotFound)?;
    let items: Vec<Value> = marked
        .markers
        .map(|markers| markers.nodes)
        .unwrap_or_default()
        .iter()
        .filter_map(|marker| segment(&item, marker))
        .filter(|segment| {
            wanted.is_empty()
                || segment["Type"]
                    .as_str()
                    .is_some_and(|kind| wanted.contains(&kind.to_ascii_lowercase()))
        })
        .collect();
    Ok(json(&ItemsResult {
        total_record_count: i64::try_from(items.len()).unwrap_or(i64::MAX),
        items,
        start_index: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(kind: &str, start_ms: i64, end_ms: Option<i64>) -> Marker {
        Marker {
            id: klapz_api::GlobalId::encode("Marker", 9),
            kind: kind.to_owned(),
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn markers_of_the_work_are_segments_and_a_bookmark_is_none() {
        let intro = segment("i", &marker("INTRO", 1_000, Some(61_000))).unwrap_or_default();
        assert_eq!(intro["Type"], "Intro");
        assert_eq!(intro["StartTicks"], 10_000_000);
        assert_eq!(intro["EndTicks"], 610_000_000);
        assert_eq!(
            segment("i", &marker("CREDITS", 0, Some(1))).unwrap_or_default()["Type"],
            "Outro"
        );
        assert!(segment("i", &marker("BOOKMARK", 0, Some(1))).is_none());
        // Credits the reference keeps ending before they start are no segment.
        assert!(segment("i", &marker("CREDITS", 5_000, Some(4_000))).is_none());
        assert!(segment("i", &marker("INTRO", 0, None)).is_none());
    }
}
