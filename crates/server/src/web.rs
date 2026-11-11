//! The bundled web client, served as static files under `/web`.
//!
//! The reference server ships Plex Web and serves it from the same port as its API, which is
//! what lets a browser pointed at the server open a client with no installation. This is
//! static file serving and nothing else: the client talks to the API like any other client.
//!
//! Measured on the reference: `/web` and `/web/` redirect to `/web/index.html`, nothing
//! under `/web` needs a token — the client has to load before it can ask for one — and a
//! directory is never listed.

use std::path::{Path, PathBuf};

use axum::http::{header, StatusCode};
use axum::routing::get;
use axum::Router;
use tower_http::services::ServeDir;

/// Where the client is mounted. Clients and bookmarks expect exactly this prefix.
pub const PREFIX: &str = "/web";

/// The web client's routes, or `None` when `dir` holds no client.
///
/// Absent rather than failing: the server is fully usable without the web client, and an
/// installation without one should not have to configure it away.
pub fn router(dir: &Path) -> Option<Router> {
    if !dir.join("index.html").is_file() {
        return None;
    }
    // A 302, as the reference answers: axum's `Redirect` offers 303 and 307, not this one.
    // Nested, the bare prefix arrives as `/`; the prefix with a slash does not reach the
    // nested router's root, so it gets a route of its own.
    let to_index = || async { (StatusCode::FOUND, [(header::LOCATION, "/web/index.html")]) };
    let client = Router::new().route("/", get(to_index)).fallback_service(
        // `ServeDir` resolves a request against `dir` and refuses any path that would leave
        // it — `..`, absolute components, Windows prefixes — which is the whole of the
        // traversal defense; nothing here builds a path by hand.
        ServeDir::new(dir).append_index_html_on_directories(false),
    );
    Some(
        Router::new()
            .route("/web/", get(to_index))
            .nest(PREFIX, client),
    )
}

/// The directory the client is served from: `OPENPLEX_WEB_DIR`, or `web` beside the working
/// directory.
pub fn directory() -> PathBuf {
    std::env::var_os("OPENPLEX_WEB_DIR").map_or_else(|| PathBuf::from("web"), PathBuf::from)
}

/// Whether a request path belongs to the web client, which is served without a token.
pub fn is_client_path(path: &str) -> bool {
    path == PREFIX || path.starts_with("/web/")
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    /// A client directory with an index and one asset, unique per test.
    fn client(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("open-plex-web-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("js")).unwrap();
        std::fs::write(dir.join("index.html"), "<html>plex</html>").unwrap();
        std::fs::write(dir.join("js").join("main.js"), "console.log(1)").unwrap();
        dir
    }

    async fn get(router: Router, uri: &str) -> (StatusCode, Option<String>, String) {
        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let location = response
            .headers()
            .get(header::LOCATION)
            .map(|value| value.to_str().unwrap().to_owned());
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        (
            status,
            location,
            String::from_utf8_lossy(&body).into_owned(),
        )
    }

    #[tokio::test]
    async fn the_bare_prefix_redirects_to_the_index() {
        let router = router(&client("redirect")).unwrap();
        for uri in ["/web", "/web/"] {
            let (status, location, _) = get(router.clone(), uri).await;
            assert_eq!(status, StatusCode::FOUND, "{uri}");
            assert_eq!(location.as_deref(), Some("/web/index.html"), "{uri}");
        }
    }

    #[tokio::test]
    async fn serves_the_files_of_the_client() {
        let router = router(&client("files")).unwrap();
        let (status, _, body) = get(router.clone(), "/web/index.html").await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, "<html>plex</html>")
        );
        let (status, _, body) = get(router, "/web/js/main.js").await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "console.log(1)"));
    }

    #[tokio::test]
    async fn never_lists_a_directory() {
        let router = router(&client("listing")).unwrap();
        assert_eq!(get(router, "/web/js/").await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn never_leaves_the_client_directory() {
        // A file beside the client directory, reachable by `..` if traversal were possible.
        let dir = client("traversal");
        std::fs::write(dir.parent().unwrap().join("open-plex-secret.txt"), "secret").unwrap();
        let router = router(&dir).unwrap();
        for uri in [
            "/web/../open-plex-secret.txt",
            "/web/%2e%2e/open-plex-secret.txt",
            "/web/js/..%2f..%2fopen-plex-secret.txt",
        ] {
            let (status, _, body) = get(router.clone(), uri).await;
            assert_ne!(status, StatusCode::OK, "{uri}");
            assert!(!body.contains("secret"), "{uri}");
        }
    }

    #[test]
    fn mounts_nothing_without_a_client() {
        let empty = std::env::temp_dir().join(format!("open-plex-web-none-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert!(router(&empty).is_none());
    }

    #[test]
    fn only_the_client_prefix_is_public() {
        assert!(is_client_path("/web"));
        assert!(is_client_path("/web/index.html"));
        assert!(!is_client_path("/webhooks"));
        assert!(!is_client_path("/library/sections"));
    }
}
