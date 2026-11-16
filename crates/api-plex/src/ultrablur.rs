//! `GET /services/ultrablur/colors` and `/image`: Plex's routes onto the transcoder's backdrops.
//!
//! The colors of an image Plex hosts are drawn by the transcoder; those of an item's background
//! art are the ones its analysis stored, read on the official API, since the artwork itself is
//! not served. Any other address answers `404`, as the reference does for an image it cannot
//! read — and the transcoder fetches nothing but Plex's hosts, where the reference fetches
//! whatever it is given, its own loopback included. `docs/spec-gaps.md` has the rules.

use std::collections::HashMap;

use async_graphql::{Name, Value, Variables};
use axum::extract::{RawQuery, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::GlobalId;
use open_plex_transcoder::{CornerColors, Rgb, TranscodeError, MAX_BACKDROP_PIXELS};
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::GqlCornerColors;
use crate::items::UltraBlurColors;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/services/ultrablur/colors", get(colors))
        .route("/services/ultrablur/image", get(image))
}

/// The reference's size when none is asked, each side on its own.
const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;

const CORNER_COLORS_QUERY: &str = r"
query PlexCornerColors($id: ID!) {
  node(id: $id) {
    ... on Item { cornerColors { topLeft topRight bottomRight bottomLeft } }
  }
}";

#[derive(Deserialize)]
struct CornerColorsData {
    node: Option<CornerColorsNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CornerColorsNode {
    corner_colors: Option<GqlCornerColors>,
}

#[derive(Debug, Serialize)]
pub struct ColorsContainer {
    size: u32,
    /// An array here, where an item carries the same element as an object.
    #[serde(rename = "UltraBlurColors")]
    colors: Vec<UltraBlurColors>,
}

/// Where a `url` points, as far as this server will look.
#[derive(Debug, PartialEq, Eq)]
enum Source<'a> {
    /// `/library/metadata/{id}/art`, with or without its change stamp.
    ItemArt(i64),
    /// Absolute: the transcoder decides whether it may be fetched.
    Remote(&'a str),
    /// A local address of anything else, which this server does not serve.
    Elsewhere,
}

fn source(url: &str) -> Source<'_> {
    let Some(local) = url.strip_prefix('/') else {
        return Source::Remote(url);
    };
    let path = local.split(['?', '#']).next().unwrap_or_default();
    let segments: Vec<&str> = path.split('/').collect();
    let id = match segments.as_slice() {
        ["library", "metadata", id, "art"] => id,
        ["library", "metadata", id, "art", stamp]
            if !stamp.is_empty() && stamp.bytes().all(|digit| digit.is_ascii_digit()) =>
        {
            id
        }
        _ => return Source::Elsewhere,
    };
    id.parse().map_or(Source::Elsewhere, Source::ItemArt)
}

/// The colors of what `url` points at. `None` when there is nothing there to read.
async fn colors_of(state: &PlexState, url: &str) -> Result<Option<CornerColors>, ApiError> {
    match source(url) {
        Source::ItemArt(id) => {
            let mut variables = Variables::default();
            variables.insert(
                Name::new("id"),
                Value::String(GlobalId::encode(ITEM_TYPE, id)),
            );
            let data: CornerColorsData =
                gql::execute(&state.schema, CORNER_COLORS_QUERY, variables).await?;
            Ok(data
                .node
                .and_then(|node| node.corner_colors)
                .and_then(|colors| from_graphql(&colors)))
        }
        Source::Remote(url) => match state.transcoder.corner_colors(url).await {
            Ok(colors) => Ok(Some(colors)),
            // Not an address, or nothing behind it: what the reference answers `404` for.
            Err(TranscodeError::InvalidSource | TranscodeError::UnavailableSource) => Ok(None),
            Err(error) => Err(error.into()),
        },
        Source::Elsewhere => Ok(None),
    }
}

fn from_graphql(colors: &GqlCornerColors) -> Option<CornerColors> {
    let css = |value: &str| hex_color(value.strip_prefix('#')?);
    Some(CornerColors {
        top_left: css(&colors.top_left)?,
        top_right: css(&colors.top_right)?,
        bottom_right: css(&colors.bottom_right)?,
        bottom_left: css(&colors.bottom_left)?,
    })
}

/// Six hex digits, either case.
fn hex_color(text: &str) -> Option<Rgb> {
    if text.len() != 6 || !text.bytes().all(|digit| digit.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&text[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn plex_hex([red, green, blue]: Rgb) -> String {
    format!("{red:02x}{green:02x}{blue:02x}")
}

fn params(query: Option<&str>) -> HashMap<String, String> {
    // Collected into a map, so a repeated parameter takes its last value.
    form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .into_owned()
        .collect()
}

async fn colors(
    _caller: Caller,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<ColorsContainer>, ApiError> {
    let params = params(query.as_deref());
    let url = params.get("url").ok_or(ApiError::BadRequest)?;
    let colors = colors_of(&state, url).await?.ok_or(ApiError::NotFound)?;
    Ok(PlexResponse::new(
        ColorsContainer {
            size: 1,
            colors: vec![UltraBlurColors {
                top_left: plex_hex(colors.top_left),
                top_right: plex_hex(colors.top_right),
                bottom_right: plex_hex(colors.bottom_right),
                bottom_left: plex_hex(colors.bottom_left),
            }],
        },
        format,
    ))
}

/// What `/image` was asked to draw.
#[derive(Debug, PartialEq, Eq)]
struct ImageRequest {
    colors: Colors,
    width: u32,
    height: u32,
    noise: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Colors {
    Given(CornerColors),
    /// The colors of an image, as `/colors` would read them.
    Of(String),
}

fn image_request(mut params: HashMap<String, String>) -> Result<ImageRequest, ApiError> {
    let side = |name: &str, default: u32| match params.get(name) {
        None => Ok(default),
        Some(value) => value.parse::<u32>().map_err(|_| ApiError::BadRequest),
    };
    let width = side("width", DEFAULT_WIDTH)?;
    let height = side("height", DEFAULT_HEIGHT)?;
    // The area is what is bounded, not each side: 4000×100 is drawn.
    if u64::from(width) * u64::from(height) > MAX_BACKDROP_PIXELS {
        return Err(ApiError::BadRequest);
    }
    let noise = match params.get("noise").map(String::as_str) {
        None | Some("1") => true,
        Some("0") => false,
        Some(_) => return Err(ApiError::BadRequest),
    };

    let colors = if let Some(url) = params.remove("url") {
        Colors::Of(url)
    } else {
        // The reference reads each with `sscanf`, and draws whatever was on its stack when
        // one does not parse; refused here instead.
        let corner = |name: &str| {
            params
                .get(name)
                .and_then(|value| hex_color(value))
                .ok_or(ApiError::BadRequest)
        };
        Colors::Given(CornerColors {
            top_left: corner("topLeft")?,
            top_right: corner("topRight")?,
            bottom_right: corner("bottomRight")?,
            bottom_left: corner("bottomLeft")?,
        })
    };
    Ok(ImageRequest {
        colors,
        width,
        height,
        noise,
    })
}

async fn image(
    _caller: Caller,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let request = image_request(params(query.as_deref()))?;
    let colors = match request.colors {
        Colors::Given(colors) => colors,
        // An image with nothing to read answers `500` there, not the `404` of `/colors`.
        Colors::Of(url) => colors_of(&state, &url)
            .await?
            .ok_or_else(|| ApiError::Internal(Box::new(TranscodeError::UnavailableSource)))?,
    };
    let png = state
        .transcoder
        .backdrop(colors, request.width, request.height, request.noise)
        .await?;
    Ok(([(header::CONTENT_TYPE, "image/png")], png).into_response())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn request(query: &str) -> Result<ImageRequest, ApiError> {
        image_request(params(Some(query)))
    }

    const COLORS: &str = "topLeft=3f280a&topRight=6b4713&bottomRight=0f2a43&bottomLeft=1C425D";

    #[test]
    fn reads_an_item_art_address_and_nothing_else_local() {
        assert_eq!(
            source("/library/metadata/21812/art/1697263694"),
            Source::ItemArt(21812)
        );
        assert_eq!(
            source("/library/metadata/21812/art"),
            Source::ItemArt(21812)
        );
        assert_eq!(
            source("/library/metadata/21812/art/1697263694?X-Plex-Token=x"),
            Source::ItemArt(21812)
        );
        for elsewhere in [
            "/library/metadata/21812/thumb/1697263694",
            "/library/metadata/21812/art/../../../etc",
            "/library/metadata/abc/art",
            "/library/metadata/21812/art/1/2",
            "/:/resources/movie-fanart.jpg",
            "/photo/:/transcode?url=%2Flibrary%2Fmetadata%2F1%2Fart",
        ] {
            assert_eq!(source(elsewhere), Source::Elsewhere, "{elsewhere}");
        }
        assert_eq!(
            source("https://metadata-static.plex.tv/x.jpg"),
            Source::Remote("https://metadata-static.plex.tv/x.jpg")
        );
    }

    #[test]
    fn draws_1920_by_1080_with_noise_by_default() {
        let drawn = request(COLORS).unwrap();
        assert_eq!((drawn.width, drawn.height, drawn.noise), (1920, 1080, true));
        assert_eq!(
            drawn.colors,
            Colors::Given(CornerColors {
                top_left: [0x3f, 0x28, 0x0a],
                top_right: [0x6b, 0x47, 0x13],
                bottom_right: [0x0f, 0x2a, 0x43],
                bottom_left: [0x1c, 0x42, 0x5d],
            })
        );
        let one_side = request(&format!("{COLORS}&width=640&noise=0")).unwrap();
        assert_eq!(
            (one_side.width, one_side.height, one_side.noise),
            (640, 1080, false)
        );
    }

    #[test]
    fn bounds_the_area_not_the_sides() {
        assert!(request(&format!("{COLORS}&width=4000&height=100")).is_ok());
        assert!(request(&format!("{COLORS}&width=4096&height=2025")).is_ok());
        assert!(matches!(
            request(&format!("{COLORS}&width=4096&height=2026")),
            Err(ApiError::BadRequest)
        ));
    }

    #[test]
    fn refuses_what_the_reference_refuses() {
        for query in [
            format!("{COLORS}&width=-5"),
            format!("{COLORS}&width=2.5"),
            format!("{COLORS}&width=64x"),
            format!("{COLORS}&noise=2"),
            format!("{COLORS}&noise=true"),
            "topLeft=3f280a&topRight=6b4713&bottomRight=0f2a43".to_owned(),
            "topLeft=3f28&topRight=6b4713&bottomRight=0f2a43&bottomLeft=1c425d".to_owned(),
            "topLeft=%233f280a&topRight=6b4713&bottomRight=0f2a43&bottomLeft=1c425d".to_owned(),
        ] {
            assert!(
                matches!(request(&query), Err(ApiError::BadRequest)),
                "{query}"
            );
        }
    }

    #[test]
    fn an_address_replaces_the_colors() {
        let drawn = request("url=https%3A%2F%2Fplex.tv%2Fa.jpg&width=64&height=48").unwrap();
        assert_eq!(drawn.colors, Colors::Of("https://plex.tv/a.jpg".to_owned()));
    }

    #[test]
    fn reads_the_official_colors_back() {
        let colors = GqlCornerColors {
            top_left: "#1a2c53".to_owned(),
            top_right: "#81502b".to_owned(),
            bottom_right: "#58421c".to_owned(),
            bottom_left: "#8f461d".to_owned(),
        };
        let read = from_graphql(&colors).unwrap();
        assert_eq!(plex_hex(read.top_left), "1a2c53");
        assert_eq!(plex_hex(read.bottom_left), "8f461d");
    }
}
