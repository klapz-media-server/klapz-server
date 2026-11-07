//! HTTP token authentication, shared by every surface.
//!
//! Lives here rather than in a binding because the official GraphQL API and the Plex adapter
//! both need it, and neither should depend on the other. The middleware resolves the token
//! to a [`Principal`] and attaches it to the request; each surface then enforces scope in
//! its own idiom.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use open_plex_core::Accounts;

/// The header and query parameter carrying the token.
///
/// Both forms are required for compatibility: the artwork and stream URLs a server hands out
/// embed the token in the query string, and the bundled Plex web client passes every
/// `X-Plex-*` value that way rather than as headers.
const TOKEN_KEY: &str = "X-Plex-Token";

/// Paths served without authentication.
///
/// Kept deliberately short. `/` and `/identity` must be reachable with no credential at all:
/// clients call them to decide whether a host is a Plex server, and what it can do, before
/// they hold any token. Requiring one would make the server undiscoverable.
const PUBLIC_PATHS: &[&str] = &["/", "/identity"];

pub async fn require_token(
    State(accounts): State<Arc<Accounts>>,
    mut request: Request,
    next: Next,
) -> Response {
    if PUBLIC_PATHS.contains(&request.uri().path()) {
        return next.run(request).await;
    }

    let Some(presented) = token_from(&request) else {
        return unauthorized();
    };
    let Some(principal) = accounts.authenticate(&presented) else {
        return unauthorized();
    };
    let principal = principal.clone();

    // Logged without the URI: the token travels in the query string, so logging the full
    // target would write credentials to disk.
    tracing::debug!(principal = %principal.name, path = request.uri().path(), "authenticated");

    request.extensions_mut().insert(principal);
    next.run(request).await
}

/// Reads the token from the header, falling back to the query string.
///
/// Header first because it is the form that does not end up in logs or `Referer`.
fn token_from(request: &Request) -> Option<String> {
    if let Some(value) = request.headers().get(TOKEN_KEY) {
        if let Ok(token) = value.to_str() {
            return Some(token.to_owned());
        }
    }

    let query = request.uri().query()?;
    form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == TOKEN_KEY)
        .map(|(_, value)| value.into_owned())
}

/// The minimal HTML body a real Plex server returns, content negotiation ignored.
///
/// Duplicating the shape here rather than reusing the adapter's would risk the two drifting;
/// this is the one response the middleware produces, and the adapter's tests pin the bytes.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/html"))],
        "<html><head><title>Unauthorized</title></head><body>\
         <h1>401 Unauthorized</h1></body></html>",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::{Extension, Router};
    use open_plex_core::{Credential, Scope};
    use tower::ServiceExt;

    use super::{require_token, Accounts, Arc, StatusCode};

    const TOKEN: &str = "owner-token";

    /// Echoes the authenticated principal's name, or `anonymous` on a public route.
    async fn whoami(principal: Option<Extension<open_plex_core::Principal>>) -> String {
        principal.map_or_else(
            || "anonymous".to_owned(),
            |Extension(principal)| principal.name,
        )
    }

    fn app() -> Router {
        let accounts = Arc::new(Accounts::new(vec![Credential::new(
            TOKEN,
            "owner",
            Scope::Admin,
        )]));
        Router::new()
            .route("/library/sections", get(whoami))
            .route("/identity", get(whoami))
            .layer(axum::middleware::from_fn_with_state(
                accounts,
                require_token,
            ))
    }

    async fn call(uri: &str, header: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::builder().uri(uri);
        if let Some(token) = header {
            request = request.header("X-Plex-Token", token);
        }
        let response = app()
            .oneshot(request.body(Body::empty()).expect("request builds"))
            .await
            .expect("router responds");
        let status = response.status();
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .expect("body")
            .to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn accepts_the_token_in_a_header() {
        let (status, body) = call("/library/sections", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "owner");
    }

    #[tokio::test]
    async fn accepts_the_token_in_the_query_string() {
        // Artwork and stream URLs a server hands out carry it this way, and the bundled web
        // client passes every X-Plex-* value as a query parameter.
        let (status, body) = call(&format!("/library/sections?X-Plex-Token={TOKEN}"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "owner");
    }

    #[tokio::test]
    async fn prefers_the_header_over_the_query_string() {
        // The header is the form that does not leak into logs or `Referer`.
        let (status, body) = call("/library/sections?X-Plex-Token=wrong", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "owner");
    }

    #[tokio::test]
    async fn refuses_a_missing_or_unknown_token() {
        assert_eq!(
            call("/library/sections", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call("/library/sections", Some("nope")).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn refuses_a_truncated_token() {
        // The case a non-constant-time comparison would expose one byte at a time.
        let truncated = &TOKEN[..TOKEN.len() - 1];
        assert_eq!(
            call("/library/sections", Some(truncated)).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn answers_rejections_with_the_reference_html_body() {
        let (status, body) = call("/library/sections", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            body,
            "<html><head><title>Unauthorized</title></head><body><h1>401 Unauthorized</h1></body></html>"
        );
        assert_eq!(body.len(), 91);
    }

    #[tokio::test]
    async fn lets_identity_through_unauthenticated() {
        // Clients call it to decide whether a host is a Plex server at all, before they hold
        // any token.
        let (status, body) = call("/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "anonymous");
    }
}
