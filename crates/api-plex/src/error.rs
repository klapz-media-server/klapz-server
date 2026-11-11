//! The binding's single error type.
//!
//! This is the only place in the repo that knows about HTTP status codes: `core` reports
//! what went wrong in domain terms, and translating that into a status is a protocol
//! decision.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use open_plex_core::LibraryError;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request names an operation that makes no sense on its target — the children of
    /// a film, say. A reference server answers `400` here rather than an empty container.
    #[error("bad request")]
    BadRequest,
    /// Refused by policy rather than for want of the feature.
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("not implemented")]
    NotImplemented,
    #[error("internal error")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::NotImplemented => StatusCode::NOT_IMPLEMENTED,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<crate::listing::ListingError> for ApiError {
    fn from(error: crate::listing::ListingError) -> Self {
        if let crate::listing::ListingError::TooManyValues { .. } = error {
            // A ceiling on client-sized input, not a missing feature.
            tracing::warn!(%error, "refused an oversized listing filter");
            return Self::BadRequest;
        }
        // `501`, not `400`: the request is well-formed and a real server honors it, we simply
        // have not built it. Refusing visibly is the point — honoring the rest of the query
        // and dropping this filter would answer with the whole library and let the client
        // present it as the filtered result.
        tracing::warn!(
            %error,
            supported = %crate::listing::supported_filters(),
            "refused a listing filter"
        );
        Self::NotImplemented
    }
}

impl From<LibraryError> for ApiError {
    fn from(error: LibraryError) -> Self {
        // Matched exhaustively on purpose: a new domain variant should force a decision
        // about its status rather than silently becoming a 500.
        match error {
            LibraryError::SectionNotFound(_) => Self::NotFound,
            LibraryError::Storage(_) => Self::Internal(Box::new(error)),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let Self::Internal(source) = &self {
            // The only place internal detail is allowed to surface, and it goes to the
            // server's log rather than the client.
            tracing::error!(error = %source, "request failed");
        }

        let status = self.status();
        (
            status,
            [(header::CONTENT_TYPE, HeaderValue::from_static("text/html"))],
            error_html(status),
        )
            .into_response()
    }
}

/// Builds the error body a real server returns.
///
/// Plex answers errors with a minimal HTML document and **ignores content negotiation**: a
/// request asking for JSON still gets HTML. Routing errors through the normal response path
/// would therefore diverge on every failure.
fn error_html(status: StatusCode) -> String {
    let reason = status.canonical_reason().unwrap_or("Error");
    format!(
        "<html><head><title>{reason}</title></head><body><h1>{} {reason}</h1></body></html>",
        status.as_u16()
    )
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn matches_the_reference_server_byte_for_byte() {
        // Captured from 1.43.5: Content-Length was 91 for this exact body.
        let body = error_html(StatusCode::UNAUTHORIZED);
        assert_eq!(
            body,
            "<html><head><title>Unauthorized</title></head><body><h1>401 Unauthorized</h1></body></html>"
        );
        assert_eq!(body.len(), 91);
    }

    #[test]
    fn not_found_body_length_matches_capture() {
        // Captured Content-Length was 85.
        assert_eq!(error_html(StatusCode::NOT_FOUND).len(), 85);
    }

    #[test]
    fn missing_section_becomes_not_found_not_forbidden() {
        // 403 would confirm the object exists, leaking other users' library contents.
        let error = ApiError::from(LibraryError::SectionNotFound(
            open_plex_core::SectionId::new(99),
        ));
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
    }
}
