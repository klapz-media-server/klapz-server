//! Letting the hosted Plex Web, and klapz's own web client, read this server's answers.
//!
//! `app.plex.tv` calls the server from another origin, so without these headers a browser hides
//! every response from it. Plex's own domains are allowed on the Plex surface, as on the
//! reference, without credentials: Plex Web carries its token in every URL. klapz's client
//! (`app.klapz.io`, the origins `KLAPZ_CORS_ORIGINS` adds, and `localhost` in a debug build)
//! reads every surface with its user's session, cookie included. Any other origin must stay
//! refused: a browser here carries its user's session, so a page allowed to read the answers
//! could read whatever that user can.

use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, request::Parts, HeaderValue, Method};
use tower_http::cors::{AllowCredentials, AllowHeaders, AllowOrigin, CorsLayer};

/// What the reference sends, in seconds: two weeks.
const PREFLIGHT_CACHED_FOR: Duration = Duration::from_secs(1_209_600);
const PLEX_DOMAIN: &str = "plex.tv";
const HOSTED_CLIENT: &str = "https://app.klapz.io";
const LOCAL_HOSTS: [&str; 2] = ["localhost", "127.0.0.1"];

fn host_of(origin: &str) -> Option<&str> {
    origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
}

fn is_plex_origin(origin: &str) -> bool {
    let Some(host) = host_of(origin) else {
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

/// A development server of the client, on any port.
fn is_local_origin(origin: &str) -> bool {
    let Some(authority) = host_of(origin) else {
        return false;
    };
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    };
    LOCAL_HOSTS.contains(&host)
}

#[derive(Debug, thiserror::Error)]
#[error("KLAPZ_CORS_ORIGINS: {0:?} is not an origin such as https://klapz.example or http://192.168.1.2:5173")]
pub struct InvalidOrigin(String);

/// The origins klapz's web client is served from.
#[derive(Debug, Clone)]
pub struct ClientOrigins {
    configured: Arc<[String]>,
    local: bool,
}

impl ClientOrigins {
    /// The hosted client, `localhost` in a debug build, and `setting`'s origins, separated by
    /// commas. Compared exactly, as a browser writes them: lower case, no path.
    pub fn from_setting(setting: Option<&str>) -> Result<Self, InvalidOrigin> {
        let configured = setting
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(|origin| {
                let origin = origin.trim_end_matches('/').to_ascii_lowercase();
                match host_of(&origin) {
                    Some(host) if !host.is_empty() && !host.contains(['/', '?', '#', '@']) => {
                        Ok(origin)
                    }
                    _ => Err(InvalidOrigin(origin)),
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            configured,
            local: cfg!(debug_assertions),
        })
    }

    pub fn configured(&self) -> &[String] {
        &self.configured
    }

    fn admits(&self, origin: &HeaderValue) -> bool {
        origin.to_str().is_ok_and(|origin| {
            origin == HOSTED_CLIENT
                || self.configured.iter().any(|known| known == origin)
                || (self.local && is_local_origin(origin))
        })
    }
}

/// The official API and the owners' page are not Plex's, and no Plex page has any business
/// reading them.
fn is_plex_surface(path: &str) -> bool {
    !path.starts_with("/graphql") && path != crate::admin::PATH
}

fn plex_may_read(origin: &HeaderValue, request: &Parts) -> bool {
    is_plex_surface(request.uri.path()) && origin.to_str().is_ok_and(is_plex_origin)
}

/// Goes outside authentication: a preflight carries no token, and a browser needs these headers
/// on a `401` to let the client see it.
pub fn layer(clients: ClientOrigins) -> CorsLayer {
    let with_credentials = clients.clone();
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, request| {
            clients.admits(origin) || plex_may_read(origin, request)
        }))
        .allow_credentials(AllowCredentials::predicate(move |origin, _| {
            with_credentials.admits(origin)
        }))
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
            .layer(layer(clients()))
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

    fn clients() -> ClientOrigins {
        ClientOrigins::from_setting(Some(" https://Klapz.Example/ ,http://192.168.1.2:5173"))
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
    async fn the_official_surfaces_stay_closed_even_to_plex() {
        for path in ["/graphql", "/admin"] {
            let response = answer(Method::GET, path, "https://app.plex.tv").await;
            assert_eq!(allowed_origin(&response), None, "{path}");
        }
    }

    fn allows_credentials(response: &Response<Body>) -> bool {
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .is_some_and(|value| value == "true")
    }

    #[tokio::test]
    async fn the_klapz_client_reads_every_surface_with_its_session() {
        for origin in [
            "https://app.klapz.io",
            "https://klapz.example",
            "http://192.168.1.2:5173",
            "http://localhost:5173",
            "http://127.0.0.1:3000",
            "http://localhost",
        ] {
            for path in ["/graphql", "/media/providers"] {
                for method in [Method::GET, Method::OPTIONS] {
                    let response = answer(method.clone(), path, origin).await;
                    assert_eq!(allowed_origin(&response), Some(origin), "{origin} {path}");
                    assert!(allows_credentials(&response), "{origin} {path} {method}");
                }
            }
        }
    }

    #[tokio::test]
    async fn plex_web_is_not_given_credentials() {
        let response = answer(Method::GET, "/media/providers", "https://app.plex.tv").await;
        assert!(!allows_credentials(&response));
    }

    #[tokio::test]
    async fn lookalikes_of_the_klapz_origins_are_refused() {
        for origin in [
            "http://app.klapz.io",
            "https://app.klapz.io.evil.example",
            "https://evil.app.klapz.io",
            "http://localhost.evil.example",
            "http://localhost:",
            "http://127.0.0.1.evil.example:80",
            "http://192.168.1.2:5174",
        ] {
            let response = answer(Method::GET, "/graphql", origin).await;
            assert_eq!(allowed_origin(&response), None, "{origin}");
            assert!(!allows_credentials(&response), "{origin}");
        }
    }

    #[test]
    fn localhost_is_admitted_only_in_a_debug_build() {
        let release = ClientOrigins {
            local: false,
            ..clients()
        };
        let local = HeaderValue::from_static("http://localhost:5173");
        assert!(!release.admits(&local));
        assert!(release.admits(&HeaderValue::from_static("https://app.klapz.io")));
    }

    #[test]
    fn a_configured_value_that_is_not_an_origin_is_refused() {
        for bad in [
            "klapz.example",
            "https://",
            "https://klapz.example/web",
            "ftp://x",
        ] {
            assert!(ClientOrigins::from_setting(Some(bad)).is_err(), "{bad}");
        }
        assert!(ClientOrigins::from_setting(Some(" , "))
            .unwrap()
            .configured()
            .is_empty());
    }
}
