//! `GET /photo/:/transcode`: an image at the size and in the format a client asks for.
//!
//! Plex Web uses it for images hosted elsewhere, its account avatars first. The source is
//! named by a URL the client chooses, which on the reference reaches anything the server can
//! reach; here only Plex's own hosts are fetched ([`source`]). Library artwork is not served
//! yet, so a relative source answers `501`.
//!
//! Sizes and encodings are measured on the reference; `docs/spec-gaps.md` has the rules.

mod codec;
mod source;

use std::collections::HashMap;

use axum::extract::{RawQuery, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::sync::{OnceCell, Semaphore};

use self::codec::Encoding;
use crate::auth::Caller;
use crate::error::ApiError;
use crate::PlexState;

/// Registered apart from the merged routers: the path holds a segment starting with a colon,
/// which only passes once `without_v07_checks` has been applied.
pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    router.route("/photo/:/transcode", get(transcode))
}

/// Images being fetched and encoded at once. Each one holds a blocking thread and up to
/// [`source::MAX_SOURCE_BYTES`] of memory, and a client controls how many it asks for.
const MAX_CONCURRENT: usize = 8;

/// The reference's value. Without it a browser would ask again for every avatar on a page.
const CACHE_CONTROL: &str = "max-age=259200";

/// Beyond this the reference fails too (13344×5000 answers `500`), at a threshold not worth
/// locating: nothing displays such an image.
const MAX_OUTPUT_SIDE: u32 = 8192;

/// The size the reference substitutes when either requested dimension is zero.
const SIZE_WHEN_UNSPECIFIED: i32 = 256;

/// Parameters that alter the picture, with the value that leaves it alone. Plex Web sends
/// them for blurred backgrounds, which are library artwork and not served yet.
const EFFECTS: [(&str, &str); 6] = [
    ("blur", "0"),
    ("saturation", "100"),
    ("opacity", "100"),
    ("rotate", "1"),
    ("background", ""),
    ("blendColor", ""),
];

#[derive(Debug)]
pub struct Photos {
    slots: Semaphore,
    client: OnceCell<reqwest::Client>,
}

impl Default for Photos {
    fn default() -> Self {
        Self {
            slots: Semaphore::new(MAX_CONCURRENT),
            client: OnceCell::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PhotoError {
    #[error("could not build the HTTP client: {0}")]
    Client(reqwest::Error),
    #[error("the source image is larger than the server accepts")]
    SourceTooLarge,
    #[error("the source is not an image this server reads")]
    Undecodable,
    #[error("could not encode the image: {0}")]
    Encode(image::ImageError),
    #[error("the worker encoding the image died: {0}")]
    Worker(tokio::task::JoinError),
    #[error("the requested size yields no usable image")]
    UnusableSize,
}

impl From<PhotoError> for ApiError {
    fn from(error: PhotoError) -> Self {
        Self::Internal(Box::new(error))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Jpeg,
    Png,
    Ppm,
}

#[derive(Debug)]
struct Request {
    url: String,
    width: i32,
    height: i32,
    fill: bool,
    upscale: bool,
    format: Option<Format>,
    quality: Option<u8>,
}

impl Request {
    fn parse(query: &str) -> Result<Self, ApiError> {
        // Collected into a map, so a repeated parameter takes its last value.
        let params: HashMap<_, _> = form_urlencoded::parse(query.as_bytes()).collect();
        let param = |name: &str| params.get(name).map(AsRef::as_ref);
        let dimension = |name: &str| {
            param(name)
                .and_then(|value: &str| value.parse().ok())
                .ok_or(ApiError::BadRequest)
        };

        let url = param("url").ok_or(ApiError::BadRequest)?.to_owned();
        let width = dimension("width")?;
        let height = dimension("height")?;
        // Every value of it, valid ones included, answers `400` on the reference.
        if param("chromaSubsampling").is_some() {
            return Err(ApiError::BadRequest);
        }
        for (name, neutral) in EFFECTS {
            if param(name).is_some_and(|value| value != neutral) {
                tracing::warn!(parameter = name, "refused an image effect");
                return Err(ApiError::NotImplemented);
            }
        }

        Ok(Self {
            url,
            width,
            height,
            fill: param("minSize") == Some("1"),
            upscale: param("upscale") == Some("1"),
            format: match param("format") {
                Some("jpg" | "jpeg") => Some(Format::Jpeg),
                Some("png") => Some(Format::Png),
                Some("ppm") => Some(Format::Ppm),
                _ => None,
            },
            // 0 and -1 are the default quality, and so is anything past 127.
            quality: param("quality")
                .and_then(|value: &str| value.parse::<u8>().ok())
                .filter(|quality| (1..=127).contains(quality)),
        })
    }

    /// The output size, `None` when the request rounds to nothing, is negative in the
    /// dimension that decides, or is absurdly large — all `500` on the reference.
    fn target_size(&self, source: (u32, u32)) -> Option<(u32, u32)> {
        let (width, height) = if self.width == 0 || self.height == 0 {
            (SIZE_WHEN_UNSPECIFIED, SIZE_WHEN_UNSPECIFIED)
        } else {
            (self.width, self.height)
        };
        let (source_width, source_height) = (f64::from(source.0), f64::from(source.1));
        let by_width = f64::from(width) / source_width;
        let by_height = f64::from(height) / source_height;
        let scale = if self.fill {
            by_width.max(by_height)
        } else {
            by_width.min(by_height)
        };
        let scale = if self.upscale { scale } else { scale.min(1.0) };
        Some((
            scaled_side(source_width, scale)?,
            scaled_side(source_height, scale)?,
        ))
    }

    /// The encoding, and the content type to announce. They disagree on purpose: asked for
    /// nothing or for JPEG, the reference may answer a PNG and still label it `image/jpeg`.
    fn encoding(&self, is_png: bool, has_alpha: bool) -> (Encoding, &'static str) {
        const JPEG: &str = "image/jpeg";
        match self.format {
            Some(Format::Png) => (Encoding::Png, "image/png"),
            Some(Format::Ppm) => (Encoding::Ppm, "image/x-portable-pixmap"),
            // JPEG cannot hold transparency, which wins over the request.
            _ if has_alpha => (Encoding::Png, JPEG),
            None if is_png => (Encoding::Png, JPEG),
            _ => (Encoding::Jpeg(self.quality), JPEG),
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the range is checked before the cast"
)]
fn scaled_side(side: f64, scale: f64) -> Option<u32> {
    let scaled = (side * scale).round();
    (scaled >= 1.0 && scaled <= f64::from(MAX_OUTPUT_SIDE)).then_some(scaled as u32)
}

fn convert(bytes: &[u8], request: &Request) -> Result<(Vec<u8>, &'static str), PhotoError> {
    let source = codec::decode(bytes)?;
    let size = request
        .target_size(source.dimensions())
        .ok_or(PhotoError::UnusableSize)?;
    let (encoding, content_type) = request.encoding(source.is_png, source.has_alpha());
    Ok((codec::encode(&source, size, encoding)?, content_type))
}

async fn transcode(
    _caller: Caller,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let request = Request::parse(query.as_deref().unwrap_or_default())?;
    let url = source::remote_url(&request.url)?;

    let photos = &state.photos;
    let _slot = photos
        .slots
        .acquire()
        .await
        .map_err(|error| ApiError::Internal(Box::new(error)))?;
    let client = photos
        .client
        .get_or_try_init(|| async { source::client() })
        .await?;

    let bytes = source::fetch(client, url).await?;
    let (output, content_type) = tokio::task::spawn_blocking(move || convert(&bytes, &request))
        .await
        .map_err(PhotoError::Worker)??;

    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, CACHE_CONTROL),
        ],
        output,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A 1708×640 JPEG the reference ships in Plex Web; every expected size below is what
    /// the reference answered for it.
    const SOURCE: (u32, u32) = (1708, 640);

    fn request(width: i32, height: i32, fill: bool, upscale: bool) -> Request {
        Request {
            url: String::new(),
            width,
            height,
            fill,
            upscale,
            format: None,
            quality: None,
        }
    }

    fn size(width: i32, height: i32, fill: bool, upscale: bool) -> Option<(u32, u32)> {
        request(width, height, fill, upscale).target_size(SOURCE)
    }

    fn encoding_of(query: &str, is_png: bool, has_alpha: bool) -> (Encoding, &'static str) {
        Request::parse(&format!("url=x&width=1&height=1&{query}"))
            .unwrap()
            .encoding(is_png, has_alpha)
    }

    #[test]
    fn fits_inside_the_box_by_default() {
        assert_eq!(size(300, 300, false, false), Some((300, 112)));
        assert_eq!(size(100, 300, false, false), Some((100, 37)));
        assert_eq!(size(300, 100, false, false), Some((267, 100)));
        assert_eq!(size(333, 777, false, false), Some((333, 125)));
        assert_eq!(size(7, 100, false, false), Some((7, 3)));
    }

    #[test]
    fn min_size_covers_the_box() {
        assert_eq!(size(300, 300, true, false), Some((801, 300)));
        assert_eq!(size(100, 300, true, false), Some((801, 300)));
        assert_eq!(size(300, 100, true, false), Some((300, 112)));
    }

    #[test]
    fn stays_at_the_source_size_unless_told_to_upscale() {
        assert_eq!(size(5000, 5000, false, false), Some(SOURCE));
        assert_eq!(size(5000, 5000, true, false), Some(SOURCE));
        assert_eq!(size(333, 777, true, false), Some(SOURCE));
        assert_eq!(size(5000, 5000, false, true), Some((5000, 1874)));
    }

    #[test]
    fn a_zero_dimension_means_256() {
        assert_eq!(size(0, 0, false, false), Some((256, 96)));
        assert_eq!(size(150, 0, false, false), Some((256, 96)));
        assert_eq!(size(0, 150, false, false), Some((256, 96)));
        assert_eq!(size(150, 0, true, true), Some((683, 256)));
    }

    #[test]
    fn an_impossible_size_yields_nothing() {
        assert_eq!(size(1, 1, false, false), None);
        assert_eq!(size(-1, 150, false, false), None);
        assert_eq!(size(5000, 5000, true, true), None);
    }

    #[test]
    fn a_negative_dimension_is_harmless_when_the_other_decides() {
        assert_eq!(
            request(-5, 60, true, true).target_size((256, 256)),
            Some((60, 60))
        );
    }

    #[test]
    fn keeps_the_source_format_when_none_is_asked() {
        assert_eq!(encoding_of("", true, false), (Encoding::Png, "image/jpeg"));
        assert_eq!(
            encoding_of("", false, false),
            (Encoding::Jpeg(None), "image/jpeg")
        );
    }

    #[test]
    fn honors_a_requested_format() {
        assert_eq!(
            encoding_of("format=jpg", true, false),
            (Encoding::Jpeg(None), "image/jpeg")
        );
        assert_eq!(
            encoding_of("format=png", false, false),
            (Encoding::Png, "image/png")
        );
        assert_eq!(
            encoding_of("format=ppm", false, false),
            (Encoding::Ppm, "image/x-portable-pixmap")
        );
    }

    #[test]
    fn transparency_stays_png_even_when_jpeg_is_asked() {
        assert_eq!(
            encoding_of("format=jpg", true, true),
            (Encoding::Png, "image/jpeg")
        );
    }

    #[test]
    fn quality_outside_its_range_is_the_default() {
        for quality in ["-1", "0", "200", "abc"] {
            assert_eq!(
                encoding_of(&format!("quality={quality}"), false, false).0,
                Encoding::Jpeg(None),
                "{quality}"
            );
        }
        assert_eq!(
            encoding_of("quality=90", false, false).0,
            Encoding::Jpeg(Some(90))
        );
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        for query in [
            "width=1&height=1",
            "url=x&height=1",
            "url=x&width=1",
            "url=x&width=abc&height=1",
            "url=x&width=1&height=1&chromaSubsampling=3",
        ] {
            assert!(
                matches!(Request::parse(query), Err(ApiError::BadRequest)),
                "{query}"
            );
        }
    }

    #[test]
    fn refuses_an_effect_but_not_its_neutral_value() {
        for effect in [
            "blur=20",
            "opacity=50",
            "saturation=10",
            "rotate=0",
            "background=000000",
        ] {
            let query = format!("url=x&width=1&height=1&{effect}");
            assert!(
                matches!(Request::parse(&query), Err(ApiError::NotImplemented)),
                "{effect}"
            );
        }
        assert!(Request::parse("url=x&width=1&height=1&blur=0&opacity=100&rotate=1").is_ok());
    }
}
