//! Item artwork: `/library/metadata/{ids}/{element}/{timestamp}`, bound onto `Item.images`, and
//! the same addresses as a source of `/photo/:/transcode`.
//!
//! As captured on the reference, an item publishes `thumb` and `art` at these addresses, an
//! `Image` list naming them by their roles (`coverPoster`, `background`, `clearLogo`), and the
//! timestamp is the image's: it changes with the image so that clients refetch it. Any
//! timestamp is served the current image.

use async_graphql::Variables;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use klapz_api::artwork::ARTWORK_PREFIX;
use klapz_api::item::ITEM_TYPE;
use klapz_api::GlobalId;
use serde::Deserialize;
use serde_json::json;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::PlexState;

/// Images do not change under one address: a new one is a new timestamp.
const CACHE_CONTROL: &str = "max-age=31536000, private";

pub fn router() -> Router<PlexState> {
    Router::new().route(
        "/library/metadata/{ids}/{element}/{timestamp}",
        get(read_element),
    )
}

const IMAGES: &str = r"
query PlexImages($id: ID!) {
  node(id: $id) {
    __typename
    ... on Item { images { kind url } }
  }
}
";

#[derive(Debug, Deserialize)]
struct ImagesData {
    node: Option<ItemImages>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemImages {
    #[serde(rename = "__typename")]
    typename: String,
    #[serde(default)]
    images: Vec<ItemImage>,
}

#[derive(Debug, Deserialize)]
struct ItemImage {
    kind: String,
    url: String,
}

/// The kinds of image an element of an item names, best first: an episode's `thumb` is its
/// still, anything else's its poster.
fn kinds_for(element: &str, typename: &str) -> &'static [&'static str] {
    match element {
        "thumb" if typename == "Episode" => &["THUMB", "POSTER"],
        "thumb" | "poster" => &["POSTER"],
        "art" => &["BACKGROUND"],
        "clearLogo" => &["CLEAR_LOGO"],
        "banner" => &["BANNER"],
        _ => &[],
    }
}

/// The bytes and type of an item's element, as the caller may read it.
pub(crate) async fn element_bytes(
    state: &PlexState,
    item: i64,
    element: &str,
) -> Result<(Vec<u8>, &'static str), ApiError> {
    let store = state.artwork.as_ref().ok_or(ApiError::NotFound)?;
    let variables = Variables::from_json(json!({ "id": GlobalId::encode(ITEM_TYPE, item) }));
    let data: ImagesData = gql::execute(&state.schema, IMAGES, variables).await?;
    let node = data.node.ok_or(ApiError::NotFound)?;
    let file = kinds_for(element, &node.typename)
        .iter()
        .find_map(|kind| node.images.iter().find(|image| image.kind == *kind))
        .and_then(|image| image.url.strip_prefix(ARTWORK_PREFIX))
        .ok_or(ApiError::NotFound)?;
    store.read(file).await.map_err(|_| ApiError::NotFound)
}

/// An address of the shape items publish, `/library/metadata/{id}/{element}/{timestamp}`: the
/// item and the element. `None` for any other path.
pub(crate) fn parse_element_path(path: &str) -> Option<(i64, &str)> {
    let rest = path.strip_prefix("/library/metadata/")?;
    let mut segments = rest.split('/');
    let id = segments.next()?.parse().ok()?;
    let element = segments.next()?;
    let _timestamp = segments.next()?;
    segments.next().is_none().then_some((id, element))
}

/// `GET /library/metadata/{ids}/{element}/{timestamp}`.
async fn read_element(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((ids, element, _timestamp)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let id: i64 = ids.parse().map_err(|_| ApiError::NotFound)?;
    let (bytes, content_type) = element_bytes(&state, id, &element).await?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, CACHE_CONTROL),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_addresses_items_publish() {
        assert_eq!(
            parse_element_path("/library/metadata/415073/thumb/1698529158"),
            Some((415_073, "thumb"))
        );
        assert_eq!(parse_element_path("/library/metadata/415073/thumb"), None);
        assert_eq!(parse_element_path("/library/metadata/x/thumb/1"), None);
        assert_eq!(
            parse_element_path("/library/metadata/1/thumb/1/extra"),
            None
        );
        assert_eq!(parse_element_path("https://plex.tv/users/a/avatar"), None);
    }

    #[test]
    fn an_episode_thumb_is_its_still_and_falls_back_on_a_poster() {
        assert_eq!(kinds_for("thumb", "Episode"), ["THUMB", "POSTER"]);
        assert_eq!(kinds_for("thumb", "Movie"), ["POSTER"]);
        assert!(kinds_for("squareArt", "Movie").is_empty());
    }
}
