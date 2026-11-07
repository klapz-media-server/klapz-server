//! Content negotiation and response wrapping.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::error::ApiError;
use crate::xml;

/// Which representation the client asked for.
///
/// Measured against a real server: JSON is returned if and only if `Accept` contains
/// `application/json`. `*/*` yields XML, `Content-Type` on the request is ignored, and there
/// is no `?format=` override — implementing one would be behavior Plex does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFormat {
    Xml,
    Json,
}

impl ResponseFormat {
    fn from_accept(accept: Option<&str>) -> Self {
        match accept {
            Some(value) if value.contains("application/json") => Self::Json,
            _ => Self::Xml,
        }
    }

    fn content_type(self) -> HeaderValue {
        match self {
            // The exact string a real server sends; clients were written against it, so
            // `application/xml` is not an acceptable substitute.
            Self::Xml => HeaderValue::from_static("text/xml;charset=utf-8"),
            Self::Json => HeaderValue::from_static("application/json"),
        }
    }
}

impl<S: Sync> FromRequestParts<S> for ResponseFormat {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let accept = parts
            .headers
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok());
        Ok(Self::from_accept(accept))
    }
}

/// A `MediaContainer` response, serialized according to the negotiated format.
///
/// Handlers never choose a serializer or set a content type themselves; keeping that here is
/// what guarantees both formats stay derived from one model.
#[derive(Debug)]
pub struct PlexResponse<T> {
    body: T,
    format: ResponseFormat,
    headers: Vec<(HeaderName, HeaderValue)>,
}

/// Where a paged listing starts, and how many items match in total.
///
/// A real server reports both in the body *and* in these two headers, and sends neither
/// unless the client asked for a window. Note which header is absent: there is no
/// `X-Plex-Container-Size` in a response, only in a request.
pub const CONTAINER_START: &str = "X-Plex-Container-Start";
pub const CONTAINER_TOTAL_SIZE: &str = "X-Plex-Container-Total-Size";

impl<T: Serialize> PlexResponse<T> {
    pub const fn new(body: T, format: ResponseFormat) -> Self {
        Self {
            body,
            format,
            headers: Vec::new(),
        }
    }

    /// Adds the paging headers a windowed listing carries alongside its body.
    #[must_use]
    pub fn paged(mut self, start: u64, total: u64) -> Self {
        for (name, value) in [(CONTAINER_START, start), (CONTAINER_TOTAL_SIZE, total)] {
            // A decimal integer is always a valid header value; the fallible API is the only
            // one available, so an unparseable value simply goes unreported.
            if let (Ok(name), Ok(value)) = (
                HeaderName::try_from(name),
                HeaderValue::try_from(value.to_string()),
            ) {
                self.headers.push((name, value));
            }
        }
        self
    }
}

impl<T: Serialize> IntoResponse for PlexResponse<T> {
    fn into_response(self) -> Response {
        let rendered = match self.format {
            ResponseFormat::Json => serde_json::to_string(&MediaContainerEnvelope {
                media_container: &self.body,
            })
            .map_err(|error| ApiError::Internal(Box::new(error))),
            ResponseFormat::Xml => xml::to_plex_xml(&self.body, "MediaContainer")
                .map_err(|error| ApiError::Internal(Box::new(error))),
        };

        match rendered {
            Ok(payload) => {
                let mut response = (
                    [(header::CONTENT_TYPE, self.format.content_type())],
                    payload,
                )
                    .into_response();
                let map = response.headers_mut();
                for (name, value) in self.headers {
                    map.insert(name, value);
                }
                response
            }
            Err(error) => error.into_response(),
        }
    }
}

/// JSON nests the container under a `MediaContainer` key; XML uses it as the root element
/// name instead, which is why only the JSON path needs this wrapper.
#[derive(Debug, Serialize)]
struct MediaContainerEnvelope<'a, T> {
    #[serde(rename = "MediaContainer")]
    media_container: &'a T,
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::ResponseFormat;

    #[test]
    fn negotiates_exactly_as_the_reference_server_does() {
        // Each row was measured against 127.0.0.1:32400.
        let cases = [
            (None, ResponseFormat::Xml),
            (Some("application/json"), ResponseFormat::Json),
            // What the bundled Plex Web App actually sends.
            (
                Some("application/json, text/plain, */*"),
                ResponseFormat::Json,
            ),
            (Some("text/xml"), ResponseFormat::Xml),
            (Some("*/*"), ResponseFormat::Xml),
            (Some("text/html,*/*;q=0.8"), ResponseFormat::Xml),
        ];

        for (accept, expected) in cases {
            assert_eq!(
                ResponseFormat::from_accept(accept),
                expected,
                "Accept: {accept:?}"
            );
        }
    }
}
