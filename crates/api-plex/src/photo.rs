//! `GET /photo/:/transcode`: Plex's route onto the transcoder.
//!
//! Translation only, as for the GraphQL schema: this reads Plex's parameters, runs the
//! transcoder and writes Plex's answer. Plex Web uses it for images hosted elsewhere, its
//! account avatars first. Library artwork is not served yet, so a relative source answers
//! `501`.
//!
//! Measured on the reference; `docs/spec-gaps.md` has the rules.

use std::collections::HashMap;

use axum::extract::{RawQuery, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use klapz_transcoder::{Fit, ImageRequest, OutputFormat, TranscodeError};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::PlexState;

/// Registered apart from the merged routers: the path holds a segment starting with a colon,
/// which only passes once `without_v07_checks` has been applied.
pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    router.route("/photo/:/transcode", get(transcode))
}

/// The reference's value. Without it a browser would ask again for every avatar on a page.
const CACHE_CONTROL: &str = "max-age=259200";

/// The box the reference substitutes when either requested dimension is zero.
const SIDE_WHEN_UNSPECIFIED: i32 = 256;

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

impl From<TranscodeError> for ApiError {
    fn from(error: TranscodeError) -> Self {
        // Matched exhaustively on purpose, so a new failure has to choose its status.
        match error {
            // The reference answers `400` too when it will not start another stream.
            TranscodeError::InvalidSource
            | TranscodeError::TooManySessions
            | TranscodeError::Superseded => Self::BadRequest,
            TranscodeError::ForbiddenSource => Self::Forbidden,
            TranscodeError::UnavailableSource
            | TranscodeError::MissingFile
            | TranscodeError::UnknownSession
            | TranscodeError::UnknownSegment => Self::NotFound,
            TranscodeError::VideoNeedsTranscoding | TranscodeError::SubtitleCannotBeBurned => {
                Self::NotImplemented
            }
            TranscodeError::SourceTooLarge
            | TranscodeError::Undecodable
            | TranscodeError::UnusableSize
            | TranscodeError::Encode(_)
            | TranscodeError::Client(_)
            | TranscodeError::Worker(_)
            | TranscodeError::ShuttingDown(_)
            | TranscodeError::OutsideLibrary
            | TranscodeError::SidecarTooLarge
            | TranscodeError::SubtitleExtraction
            | TranscodeError::Spawn(_)
            | TranscodeError::Io(_) => Self::Internal(Box::new(error)),
        }
    }
}

fn parse(query: &str) -> Result<ImageRequest, ApiError> {
    // Collected into a map, so a repeated parameter takes its last value.
    let params: HashMap<_, _> = form_urlencoded::parse(query.as_bytes()).collect();
    let param = |name: &str| params.get(name).map(AsRef::as_ref);
    let dimension = |name: &str| -> Result<i32, ApiError> {
        param(name)
            .and_then(|value: &str| value.parse().ok())
            .ok_or(ApiError::BadRequest)
    };

    let source = param("url").ok_or(ApiError::BadRequest)?.to_owned();
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

    let unspecified = width == 0 || height == 0;
    Ok(ImageRequest {
        source,
        width: if unspecified {
            SIDE_WHEN_UNSPECIFIED
        } else {
            width
        },
        height: if unspecified {
            SIDE_WHEN_UNSPECIFIED
        } else {
            height
        },
        fit: if param("minSize") == Some("1") {
            Fit::Cover
        } else {
            Fit::Contain
        },
        upscale: param("upscale") == Some("1"),
        format: match param("format") {
            Some("jpg" | "jpeg") => Some(OutputFormat::Jpeg),
            Some("png") => Some(OutputFormat::Png),
            Some("ppm") => Some(OutputFormat::Ppm),
            _ => None,
        },
        // 0 and -1 are the default quality, and so is anything past 127; the reference reads
        // 100 to 127 as 100.
        quality: param("quality")
            .and_then(|value: &str| value.parse::<u8>().ok())
            .filter(|quality| (1..=127).contains(quality))
            .map(|quality| quality.min(100)),
    })
}

/// Follows the request, not the result: asked for nothing or for JPEG, the reference may
/// answer a PNG and still label it `image/jpeg`.
const fn content_type(requested: Option<OutputFormat>) -> &'static str {
    match requested {
        Some(OutputFormat::Png) => "image/png",
        Some(OutputFormat::Ppm) => "image/x-portable-pixmap",
        Some(OutputFormat::Jpeg) | None => "image/jpeg",
    }
}

async fn transcode(
    _caller: Caller,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let request = parse(query.as_deref().unwrap_or_default())?;
    let content_type = content_type(request.format);
    let image = if request.source.starts_with('/') {
        // An item's own artwork, at the address its `thumb` or `art` publishes.
        let (item, element) =
            crate::artwork::parse_element_path(&request.source).ok_or(ApiError::NotImplemented)?;
        let (bytes, _) = crate::artwork::element_bytes(&state, item, element).await?;
        state.transcoder.convert_image_bytes(bytes, request).await?
    } else {
        state.transcoder.convert_image(request).await?
    };

    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, CACHE_CONTROL),
        ],
        image.bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parsed(extra: &str) -> ImageRequest {
        parse(&format!("url=x&width=60&height=60&{extra}")).unwrap()
    }

    #[test]
    fn a_zero_dimension_makes_the_box_256() {
        for query in [
            "url=x&width=0&height=0",
            "url=x&width=150&height=0",
            "url=x&width=0&height=150",
        ] {
            let request = parse(query).unwrap();
            assert_eq!((request.width, request.height), (256, 256), "{query}");
        }
        assert_eq!(parsed("").width, 60);
    }

    #[test]
    fn reads_the_fit_and_the_upscale() {
        assert_eq!(parsed("").fit, Fit::Contain);
        assert_eq!(parsed("minSize=1").fit, Fit::Cover);
        assert!(!parsed("").upscale);
        assert!(parsed("upscale=1").upscale);
    }

    #[test]
    fn an_unknown_format_is_none() {
        assert_eq!(parsed("format=jpg").format, Some(OutputFormat::Jpeg));
        assert_eq!(parsed("format=jpeg").format, Some(OutputFormat::Jpeg));
        assert_eq!(parsed("format=png").format, Some(OutputFormat::Png));
        assert_eq!(parsed("format=ppm").format, Some(OutputFormat::Ppm));
        assert_eq!(parsed("format=gif").format, None);
    }

    #[test]
    fn quality_outside_its_range_is_the_default() {
        for quality in ["-1", "0", "200", "abc"] {
            assert_eq!(
                parsed(&format!("quality={quality}")).quality,
                None,
                "{quality}"
            );
        }
        assert_eq!(parsed("quality=90").quality, Some(90));
        assert_eq!(parsed("quality=127").quality, Some(100));
    }

    #[test]
    fn labels_by_what_was_asked_not_by_what_came_out() {
        assert_eq!(content_type(None), "image/jpeg");
        assert_eq!(content_type(Some(OutputFormat::Jpeg)), "image/jpeg");
        assert_eq!(content_type(Some(OutputFormat::Png)), "image/png");
        assert_eq!(
            content_type(Some(OutputFormat::Ppm)),
            "image/x-portable-pixmap"
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
            assert!(matches!(parse(query), Err(ApiError::BadRequest)), "{query}");
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
                matches!(parse(&query), Err(ApiError::NotImplemented)),
                "{effect}"
            );
        }
        assert!(parse("url=x&width=1&height=1&blur=0&opacity=100&rotate=1").is_ok());
    }

    #[test]
    fn each_failure_of_the_transcoder_has_its_status() {
        let into = |error| ApiError::from(error);
        assert!(matches!(
            into(TranscodeError::InvalidSource),
            ApiError::BadRequest
        ));
        assert!(matches!(
            into(TranscodeError::ForbiddenSource),
            ApiError::Forbidden
        ));
        assert!(matches!(
            into(TranscodeError::UnavailableSource),
            ApiError::NotFound
        ));
        assert!(matches!(
            into(TranscodeError::Undecodable),
            ApiError::Internal(_)
        ));
    }
}
