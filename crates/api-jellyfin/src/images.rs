//! Item images: `/Items/{itemId}/Images/{imageType}` and `/{imageIndex}`, bound onto
//! `Item.images` and the artwork store.
//!
//! Jellyfin's clients fetch images without a token — Jellyfin Web writes plain `<img>`
//! addresses — and the reference serves them anonymously. Here an image is served without a
//! token only when its `tag` is the one a lookup published for that item, the name of the
//! file it shows: a caller learns it by reading the item. Without a `tag`, the item is looked
//! up as the caller, rights applying. `maxWidth`, `maxHeight`, `fillWidth`, `fillHeight`,
//! `width` and `height` scale the image down into the box they draw.

use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use klapz_core::source_metadata::artwork::ArtworkReader;
use klapz_core::source_metadata::{ImageKind, ShownImage, SourceMetadataRepository};
use klapz_core::RatingKey;
use serde::Deserialize;
use serde_json::json;

use crate::error::JellyfinError;
use crate::query::Query;
use crate::{gql, id, JellyfinState};

/// What serving images needs: the store's files, and which file each item shows.
#[derive(Debug, Clone)]
pub struct JellyfinArtwork {
    pub reader: Arc<dyn ArtworkReader>,
    pub repository: Arc<dyn SourceMetadataRepository>,
}

/// A tagged image never changes: a new one has a new tag.
const CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// The largest box an image is scaled into.
const MAX_SIDE: u32 = 3840;

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/items/{id}/images/{kind}", get(image))
        .route("/items/{id}/images/{kind}/{index}", get(indexed_image))
}

/// Whether `path` (lower case, below the base) names an item's image: public, as on the
/// reference, the handler deciding what a request without a token may have.
pub fn is_image_path(path: &str) -> bool {
    let mut segments = path.trim_start_matches('/').split('/');
    segments.next() == Some("items")
        && segments.next().is_some_and(|id| !id.is_empty())
        && segments.next() == Some("images")
        && segments.next().is_some_and(|kind| !kind.is_empty())
        && segments
            .next()
            .is_none_or(|index| index.bytes().all(|b| b.is_ascii_digit()))
        && segments.next().is_none()
}

/// The name a lookup publishes for an image: its file's, the hash of where it came from.
pub fn tag_of(url_or_file: &str) -> &str {
    let name = url_or_file.rsplit('/').next().unwrap_or(url_or_file);
    name.split('.').next().unwrap_or(name)
}

/// Our kinds of image for one of Jellyfin's types, best first. An episode's primary image is
/// its still.
fn kinds_for(kind: &str, episode: bool) -> &'static [ImageKind] {
    match kind.to_ascii_lowercase().as_str() {
        "primary" if episode => &[ImageKind::Thumb, ImageKind::Poster],
        "primary" => &[ImageKind::Poster],
        "backdrop" => &[ImageKind::Background],
        "logo" => &[ImageKind::ClearLogo],
        "banner" => &[ImageKind::Banner],
        "thumb" => &[ImageKind::Thumb, ImageKind::Background],
        _ => &[],
    }
}

async fn image(
    State(state): State<JellyfinState>,
    Path((item, kind)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    serve(&state, &item, &kind, 0, query.as_deref()).await
}

async fn indexed_image(
    State(state): State<JellyfinState>,
    Path((item, kind, index)): Path<(String, String, u32)>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    serve(&state, &item, &kind, index, query.as_deref()).await
}

const IMAGES: &str = r"
query JellyfinImages($id: ID!) {
  node(id: $id) { __typename ... on Item { images { kind url } } }
}
";

#[derive(Debug, Deserialize)]
struct ImagesData {
    node: Option<Node>,
}

#[derive(Debug, Deserialize)]
struct Node {
    #[serde(rename = "__typename")]
    typename: String,
    #[serde(default)]
    images: Vec<NodeImage>,
}

#[derive(Debug, Deserialize)]
struct NodeImage {
    kind: String,
    url: String,
}

async fn serve(
    state: &JellyfinState,
    item: &str,
    kind: &str,
    index: u32,
    query: Option<&str>,
) -> Result<Response, JellyfinError> {
    let artwork = state.artwork.as_ref().ok_or(JellyfinError::NotFound)?;
    // One image of each kind: a backdrop past the first is missing, as on an item with one.
    if index > 0 {
        return Err(JellyfinError::NotFound);
    }
    let global = id::decode(item).map_err(|_| JellyfinError::NotFound)?;
    if global.type_name != klapz_api::item::ITEM_TYPE {
        return Err(JellyfinError::NotFound);
    }
    let query = Query::parse(query);
    let file = match query.get("tag").filter(|tag| !tag.is_empty()) {
        Some(tag) => tagged_file(artwork, RatingKey::new(global.key), kind, tag).await?,
        None => current_file(state, &global, kind).await?,
    };
    let scale = scale_of(&query)?;
    let (bytes, content_type) = artwork
        .reader
        .read(&file, scale)
        .await
        .map_err(|_| JellyfinError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, CACHE_CONTROL.to_owned()),
        ],
        bytes,
    )
        .into_response())
}

/// The file the item shows for `kind` when its name is `tag`, read from storage directly: the
/// tag is what stands for the token here.
async fn tagged_file(
    artwork: &JellyfinArtwork,
    item: RatingKey,
    kind: &str,
    tag: &str,
) -> Result<String, JellyfinError> {
    let shown: Vec<ShownImage> = artwork
        .repository
        .images_of(&[item])
        .await
        .map_err(|_| JellyfinError::NotFound)?;
    let wanted_kinds = [kinds_for(kind, true), kinds_for(kind, false)].concat();
    shown
        .into_iter()
        .find(|image| wanted_kinds.contains(&image.kind) && tag_of(&image.file) == tag)
        .map(|image| image.file)
        .ok_or(JellyfinError::NotFound)
}

/// The file the item shows for `kind` now, looked up as the caller.
async fn current_file(
    state: &JellyfinState,
    global: &klapz_api::GlobalId,
    kind: &str,
) -> Result<String, JellyfinError> {
    let id = klapz_api::GlobalId::encode(&global.type_name, global.key);
    let data: ImagesData = gql::query(&state.schema, IMAGES, json!({ "id": id })).await?;
    let node = data.node.ok_or(JellyfinError::NotFound)?;
    let kinds = kinds_for(kind, node.typename == "Episode");
    kinds
        .iter()
        .find_map(|wanted| {
            node.images
                .iter()
                .find(|image| ImageKind::parse(&graphql_kind(&image.kind)) == Some(*wanted))
        })
        .and_then(|image| image.url.strip_prefix(klapz_api::artwork::ARTWORK_PREFIX))
        .map(str::to_owned)
        .ok_or(JellyfinError::NotFound)
}

/// `CLEAR_LOGO` → `clearLogo`: the official API's enum as storage spells the kind.
fn graphql_kind(kind: &str) -> String {
    let mut spelled = String::new();
    for (index, word) in kind.split('_').enumerate() {
        let lower = word.to_ascii_lowercase();
        if index == 0 {
            spelled.push_str(&lower);
        } else {
            let mut letters = lower.chars();
            if let Some(first) = letters.next() {
                spelled.push(first.to_ascii_uppercase());
                spelled.extend(letters);
            }
        }
    }
    spelled
}

/// The box Jellyfin's parameters draw: `fill*` the size it will be shown at, `max*` a bound,
/// `width`/`height` exact. A side alone bounds that side; none leaves the image whole.
fn scale_of(query: &Query) -> Result<Option<(u32, u32)>, JellyfinError> {
    let number = |name: &str| -> Result<Option<u32>, JellyfinError> {
        query
            .get(name)
            .map(|value| {
                value
                    .parse::<u32>()
                    .map_err(|_| JellyfinError::BadRequest("image size"))
            })
            .transpose()
    };
    let width = number("fillWidth")?
        .or(number("maxWidth")?)
        .or(number("width")?);
    let height = number("fillHeight")?
        .or(number("maxHeight")?)
        .or(number("height")?);
    Ok(match (width, height) {
        (None, None) => None,
        (width, height) => Some((
            width.unwrap_or(MAX_SIDE).clamp(1, MAX_SIDE),
            height.unwrap_or(MAX_SIDE).clamp(1, MAX_SIDE),
        )),
    })
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn knows_an_image_path() {
        assert!(is_image_path("/items/abc/images/primary"));
        assert!(is_image_path("/items/abc/images/backdrop/0"));
        assert!(!is_image_path("/items/abc/images"));
        assert!(!is_image_path("/items/abc"));
        assert!(!is_image_path("/items/abc/images/primary/x"));
        assert!(!is_image_path("/items/abc/images/primary/0/more"));
    }

    #[test]
    fn a_tag_is_the_file_name_without_extension() {
        assert_eq!(tag_of("/artwork/provider/ab/abcdef.jpg"), "abcdef");
        assert_eq!(tag_of("provider/ab/abcdef.png"), "abcdef");
    }

    #[test]
    fn an_episode_primary_image_is_its_still() {
        assert_eq!(
            kinds_for("Primary", true),
            [ImageKind::Thumb, ImageKind::Poster]
        );
        assert_eq!(kinds_for("Primary", false), [ImageKind::Poster]);
        assert!(kinds_for("Disc", false).is_empty());
    }

    #[test]
    fn spells_the_official_kinds_as_storage_does() {
        assert_eq!(graphql_kind("CLEAR_LOGO"), "clearLogo");
        assert_eq!(graphql_kind("POSTER"), "poster");
    }

    #[test]
    fn reads_jellyfins_size_parameters() {
        let query = Query::parse(Some("fillHeight=396&fillWidth=264&quality=96"));
        assert_eq!(scale_of(&query).unwrap(), Some((264, 396)));
        let query = Query::parse(Some("maxWidth=800"));
        assert_eq!(scale_of(&query).unwrap(), Some((800, MAX_SIDE)));
        assert_eq!(scale_of(&Query::parse(None)).unwrap(), None);
        assert!(scale_of(&Query::parse(Some("maxWidth=big"))).is_err());
    }
}
