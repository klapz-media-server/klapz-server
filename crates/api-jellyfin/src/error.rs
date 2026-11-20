//! The binding's error type, answered as the reference answers each case.
//!
//! A refused credential is a bare `401`; a missing thing and a malformed request are
//! ASP.NET's problem details (RFC 9457), JSON; a server fault is a bare `500`; an exception
//! the reference's own code throws is its middleware's line of plain text.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum JellyfinError {
    /// No caller, a credential that opens nothing, or a wrong password.
    #[error("unauthorized")]
    Unauthorized,
    /// Asked of another user than the caller, or of something the caller may not read: as if
    /// it did not exist, which is how the official API answers it.
    #[error("not found")]
    NotFound,
    /// A request the reference refuses: a parameter that is not what its type says.
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    /// Well-formed, honored by the reference, and not built here. Refused visibly rather than
    /// answered without the part we cannot honor.
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),
    /// What the reference answers when its own code throws rather than refusing the request
    /// up front: the status its middleware maps the exception to, over one line of text.
    #[error("refused as the reference's exception: {0}")]
    Exception(StatusCode),
    #[error("internal error")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl From<open_plex_api::in_process::InProcessError> for JellyfinError {
    fn from(error: open_plex_api::in_process::InProcessError) -> Self {
        Self::Internal(Box::new(error))
    }
}

impl From<crate::id::IdError> for JellyfinError {
    fn from(error: crate::id::IdError) -> Self {
        match error {
            crate::id::IdError::Malformed => Self::BadRequest("an id is not a GUID"),
            crate::id::IdError::Unknown => Self::NotFound,
        }
    }
}

/// RFC 9110's section for each status the reference describes.
fn problem(status: StatusCode, title: &str, section: &str) -> Response {
    let body = json!({
        "type": format!("https://tools.ietf.org/html/rfc9110#section-{section}"),
        "title": title,
        "status": status.as_u16(),
    });
    (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(crate::response::JSON),
        )],
        body.to_string(),
    )
        .into_response()
}

impl IntoResponse for JellyfinError {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED.into_response(),
            Self::NotFound => problem(StatusCode::NOT_FOUND, "Not Found", "15.5.5"),
            Self::BadRequest(reason) => {
                tracing::debug!(reason, "refused a Jellyfin request");
                problem(
                    StatusCode::BAD_REQUEST,
                    "One or more validation errors occurred.",
                    "15.5.1",
                )
            }
            Self::NotImplemented(what) => {
                tracing::warn!(what, "refused a Jellyfin request we cannot honor");
                StatusCode::NOT_IMPLEMENTED.into_response()
            }
            Self::Exception(status) => (
                status,
                [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"))],
                "Error processing request.",
            )
                .into_response(),
            Self::Internal(error) => {
                tracing::error!(%error, "a Jellyfin request failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    async fn body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn a_refused_credential_is_a_bare_401() {
        let response = JellyfinError::Unauthorized.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get(header::CONTENT_TYPE).is_none());
        assert_eq!(body(response).await, "");
    }

    #[tokio::test]
    async fn a_missing_thing_is_a_problem_document() {
        let response = JellyfinError::NotFound.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        let problem: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
        assert_eq!(
            problem,
            json!({
                "type": "https://tools.ietf.org/html/rfc9110#section-15.5.5",
                "title": "Not Found",
                "status": 404,
            })
        );
    }

    #[tokio::test]
    async fn an_exception_is_one_line_of_text() {
        let response = JellyfinError::Exception(StatusCode::BAD_REQUEST).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/plain");
        assert_eq!(body(response).await, "Error processing request.");
    }

    #[tokio::test]
    async fn a_malformed_id_is_a_400_and_an_unknown_one_a_404() {
        assert_eq!(
            JellyfinError::from(crate::id::IdError::Malformed)
                .into_response()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            JellyfinError::from(crate::id::IdError::Unknown)
                .into_response()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}
