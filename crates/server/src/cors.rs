//! Letting the hosted Plex Web read this server's answers.
//!
//! `app.plex.tv` calls the server from another origin, so without these headers a browser hides
//! every response from it. Only Plex's own domains are allowed, as on the reference. Any other
//! origin must stay refused: a network the operator trusts is admitted without a token, so a
//! page allowed to read answers from such a peer could read everything the operator can.

use std::time::Duration;

use axum::http::{header, request::Parts, HeaderValue, Method};
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

/// What the reference sends, in seconds: two weeks.
const PREFLIGHT_CACHED_FOR: Duration = Duration::from_secs(1_209_600);
const PLEX_DOMAIN: &str = "plex.tv";

fn is_plex_origin(origin: &str) -> bool {
    let Some(host) = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
    else {
        return false;
    };
    host == PLEX_DOMAIN
        || host.strip_suffix(PLEX_DOMAIN).is_some_and(|subdomain| {
            subdomain.ends_with('.')
                && subdomain
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        })
}

/// The operator's surfaces are not Plex's, and no Plex page has any business reading them.
fn is_plex_surface(path: &str) -> bool {
    !path.starts_with("/graphql") && path != crate::admin::PATH
}

fn allowed(origin: &HeaderValue, request: &Parts) -> bool {
    is_plex_surface(request.uri.path()) && origin.to_str().is_ok_and(is_plex_origin)
}

/// Goes outside authentication: a preflight carries no token, and a browser needs these headers
/// on a `401` to let the client see it.
pub fn layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(allowed))
        .allow_methods([
            Method::POST,
            Method::GET,
            Method::OPTIONS,
            Method::DELETE,
            Method::PUT,
            Method::HEAD,
        ])
        .allow_headers(AllowHeaders::mirror_request())
        .expose_headers([header::LOCATION, header::DATE])
        .max_age(PREFLIGHT_CACHED_FOR)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use axum::body::Body;
    use axum::http::{Request, Response, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    use super::*;

    async fn answer(method: Method, path: &str, origin: &str) -> Response<Body> {
        Router::new()
            .route(
                "/media/providers",
                get(|| async { StatusCode::UNAUTHORIZED }),
            )
            .route("/graphql", get(|| async { StatusCode::OK }))
            .route("/admin", get(|| async { StatusCode::OK }))
            .layer(layer())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::ORIGIN, origin)
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "x-plex-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    fn allowed_origin(response: &Response<Body>) -> Option<&str> {
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .map(|value| value.to_str().unwrap())
    }

    #[test]
    fn only_plex_domains_are_plex_origins() {
        for good in [
            "https://app.plex.tv",
            "https://plex.tv",
            "http://app.plex.tv",
            "https://a.b.plex.tv",
        ] {
            assert!(is_plex_origin(good), "{good}");
        }
        for bad in [
            "https://evil.example",
            "https://notplex.tv",
            "https://plex.tv.evil.example",
            "https://app.plex.tv:8443",
            "https://evil.example/.plex.tv",
            "null",
            "app.plex.tv",
        ] {
            assert!(!is_plex_origin(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_refusal_stays_readable_by_plex_web() {
        let response = answer(Method::GET, "/media/providers", "https://app.plex.tv").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(allowed_origin(&response), Some("https://app.plex.tv"));
    }

    #[tokio::test]
    async fn a_preflight_is_answered_with_what_the_reference_sends() {
        let response = answer(Method::OPTIONS, "/media/providers", "https://app.plex.tv").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(allowed_origin(&response), Some("https://app.plex.tv"));
        let headers = response.headers();
        assert_eq!(headers[header::ACCESS_CONTROL_MAX_AGE], "1209600");
        assert_eq!(
            headers[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "x-plex-token"
        );
    }

    #[tokio::test]
    async fn another_site_cannot_read_the_answers() {
        let response = answer(Method::GET, "/media/providers", "https://evil.example").await;
        assert_eq!(allowed_origin(&response), None);
    }

    #[tokio::test]
    async fn the_operators_surfaces_stay_closed_even_to_plex() {
        for path in ["/graphql", "/admin"] {
            let response = answer(Method::GET, path, "https://app.plex.tv").await;
            assert_eq!(allowed_origin(&response), None, "{path}");
        }
    }
}
