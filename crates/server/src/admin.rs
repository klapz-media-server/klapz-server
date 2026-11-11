//! A small page for the server's owners: users, their roles, their libraries and their Plex
//! connection.
//!
//! The page itself holds no data and is served without a token, like the web client: it has to
//! load before anyone signs in. It signs in through `signIn`, whose session cookie then rides on
//! every call to `/graphql`, and everything it shows comes from there, where managing users is
//! for owners alone.

use axum::http::{header, HeaderName, HeaderValue};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;

pub const PATH: &str = "/admin";

const PAGE: &str = include_str!("admin.html");

/// Nothing but this page's own inline code and calls back to this server, and no framing: the
/// page acts with an owner's rights.
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; \
     style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'none'; \
     frame-ancestors 'none'";

pub fn router() -> Router {
    Router::new().route(PATH, get(page))
}

async fn page() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(CONTENT_SECURITY_POLICY),
            ),
            (
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ),
        ],
        Html(PAGE),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn serves_the_page_with_its_protections() {
        let response = router()
            .oneshot(Request::builder().uri(PATH).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(policy.contains("frame-ancestors 'none'"));
        assert!(policy.contains("connect-src 'self'"));
    }
}
