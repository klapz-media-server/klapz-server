//! The web client, served as static files under `/web` from a directory on disk: nothing of it
//! is compiled into the server.
//!
//! The reference server ships Plex Web and serves it from the same port as its API, which is
//! what lets a browser pointed at the server open a client with no installation. This is
//! static file serving and nothing else: the client talks to the API like any other client.
//! Nothing under `/web` needs a token — the client has to load before it can ask for one — and
//! a directory is never listed.
//!
//! Unlike the reference, which redirects `/web` and `/web/` to `/web/index.html`, `/web`
//! redirects to `/web/`, which serves the index, and a path naming no file answers the index:
//! a client that routes in the browser then works at any of its paths below `/web/`. Plex Web
//! is unaffected: it loads the same assets.
//!
//! A browser opening the server's root is sent to the client with a `307`, where the reference
//! answers it `401`: see [`send_navigation_to_client`].

use std::path::{Path, PathBuf};

use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

/// Where the client is mounted. Clients and bookmarks expect exactly this prefix.
pub const PREFIX: &str = "/web";

/// The web client's routes, or `None` when `dir` holds no client.
///
/// Absent rather than failing: the server is fully usable without the web client, and an
/// installation without one should not have to configure it away.
pub fn router(dir: &Path) -> Option<Router> {
    let index = dir.join("index.html");
    if !index.is_file() {
        return None;
    }
    // `ServeDir` resolves a request against `dir` and refuses any path that would leave it —
    // `..`, absolute components, Windows prefixes — which is the whole of the traversal
    // defense; nothing here builds a path by hand. What it does not serve, a directory
    // included, is the index.
    let files = ServeDir::new(dir)
        .append_index_html_on_directories(false)
        .fallback(ServeFile::new(&index));
    Some(
        Router::new()
            .nest_service(PREFIX, files)
            .layer(axum::middleware::from_fn(with_trailing_slash)),
    )
}

/// Sends the bare prefix to the prefix with a slash, so that the index's relative addresses
/// resolve below it. A 302, as the reference answers: axum's `Redirect` offers 303 and 307.
async fn with_trailing_slash(request: Request, next: Next) -> Response {
    if request.uri().path() == PREFIX {
        return (StatusCode::FOUND, [(header::LOCATION, "/web/")]).into_response();
    }
    next.run(request).await
}

/// Sends a browser opening `/` to the client: a `307` to `/web/`, before any token is asked
/// for. Only a navigation says `Sec-Fetch-Mode: navigate`, which a browser sets itself and a
/// page's scripts cannot; Plex's apps send no such header, and Plex Web's own calls to `/` say
/// `cors`, so every API client keeps the root's capabilities.
pub async fn send_navigation_to_client(request: Request, next: Next) -> Response {
    let navigates = request
        .headers()
        .get("sec-fetch-mode")
        .is_some_and(|mode| mode.as_bytes().eq_ignore_ascii_case(b"navigate"));
    let reads = matches!(*request.method(), Method::GET | Method::HEAD);
    if navigates && reads && request.uri().path() == "/" {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, "/web/")],
        )
            .into_response();
    }
    next.run(request).await
}

/// The directory the client is served from: `KLAPZ_WEB_DIR`, or `web` beside the server's
/// executable, whatever the working directory.
pub fn directory() -> Option<PathBuf> {
    std::env::var_os("KLAPZ_WEB_DIR")
        .map(PathBuf::from)
        .or_else(|| crate::beside_executable("web"))
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

    const INDEX: &str = "<html><head></head>plex</html>";

    #[test]
    fn the_client_is_looked_for_beside_the_executable_not_the_working_directory() {
        if std::env::var_os("KLAPZ_WEB_DIR").is_some() {
            return;
        }
        let executable = std::env::current_exe().unwrap();
        assert_eq!(directory(), Some(executable.parent().unwrap().join("web")));
    }

    /// A client directory with an index and one asset, unique per test.
    fn client(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("klapz-web-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("js")).unwrap();
        std::fs::write(dir.join("index.html"), INDEX).unwrap();
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
    async fn the_prefix_redirects_to_its_slash_which_serves_the_index() {
        let router = router(&client("prefix")).unwrap();
        let (status, location, _) = get(router.clone(), "/web").await;
        assert_eq!(
            (status, location.as_deref()),
            (StatusCode::FOUND, Some("/web/"))
        );
        for uri in ["/web/", "/web/index.html"] {
            let (status, location, body) = get(router.clone(), uri).await;
            assert_eq!(
                (status, location, body.as_str()),
                (StatusCode::OK, None, INDEX),
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn a_path_naming_no_file_is_the_index() {
        let router = router(&client("fallback")).unwrap();
        for uri in ["/web/movies/42", "/web/js/missing.js", "/web/js/"] {
            let (status, _, body) = get(router.clone(), uri).await;
            assert_eq!((status, body.as_str()), (StatusCode::OK, INDEX), "{uri}");
        }
    }

    #[tokio::test]
    async fn serves_the_other_files_untouched() {
        let router = router(&client("files")).unwrap();
        let (status, _, body) = get(router, "/web/js/main.js").await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "console.log(1)"));
    }

    #[tokio::test]
    async fn never_leaves_the_client_directory() {
        // A file beside the client directory, reachable by `..` if traversal were possible.
        let dir = client("traversal");
        std::fs::write(dir.parent().unwrap().join("klapz-secret.txt"), "secret").unwrap();
        let router = router(&dir).unwrap();
        for uri in [
            "/web/../klapz-secret.txt",
            "/web/%2e%2e/klapz-secret.txt",
            "/web/js/..%2f..%2fklapz-secret.txt",
        ] {
            let (_, _, body) = get(router.clone(), uri).await;
            assert!(!body.contains("secret"), "{uri}");
        }
    }

    #[test]
    fn mounts_nothing_without_a_client() {
        let empty = std::env::temp_dir().join(format!("klapz-web-none-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert!(router(&empty).is_none());
    }

    /// What `send_navigation_to_client` answers a request with this `Sec-Fetch-Mode`.
    async fn root(mode: Option<&str>, method: Method, uri: &str) -> (StatusCode, Option<String>) {
        let app = Router::new()
            .route("/", axum::routing::any(|| async { "capabilities" }))
            .route("/library", axum::routing::get(|| async { "library" }))
            .layer(axum::middleware::from_fn(send_navigation_to_client));
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(mode) = mode {
            request = request.header("Sec-Fetch-Mode", mode);
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let location = response
            .headers()
            .get(header::LOCATION)
            .map(|value| value.to_str().unwrap().to_owned());
        (response.status(), location)
    }

    #[tokio::test]
    async fn a_browser_opening_the_root_is_sent_to_the_client() {
        for method in [Method::GET, Method::HEAD] {
            assert_eq!(
                root(Some("navigate"), method, "/").await,
                (StatusCode::TEMPORARY_REDIRECT, Some("/web/".to_owned()))
            );
        }
    }

    #[tokio::test]
    async fn api_clients_keep_the_root() {
        let cases = [
            (None, Method::GET, "/"),
            (Some("cors"), Method::GET, "/"),
            (Some("navigate"), Method::POST, "/"),
            (Some("navigate"), Method::GET, "/library"),
        ];
        for (mode, method, uri) in cases {
            assert_eq!(
                root(mode, method.clone(), uri).await,
                (StatusCode::OK, None),
                "{method} {uri} {mode:?}"
            );
        }
    }

    #[test]
    fn only_the_client_prefix_is_public() {
        assert!(is_client_path("/web"));
        assert!(is_client_path("/web/index.html"));
        assert!(!is_client_path("/webhooks"));
        assert!(!is_client_path("/library/sections"));
    }
}
