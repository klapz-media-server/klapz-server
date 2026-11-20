//! The web client, served as static files under `/web` from a directory on disk: nothing of it
//! is compiled into the server.
//!
//! The reference server ships Plex Web and serves it from the same port as its API, which is
//! what lets a browser pointed at the server open a client with no installation. This is
//! static file serving and nothing else: the client talks to the API like any other client.
//! Nothing under `/web` needs a token — the client has to load before it can ask for one — and
//! a directory is never listed.
//!
//! Unlike the reference, which redirects `/web` and `/web/` to `/web/index.html`, every page
//! carries `<base href="/web/">` and a path naming no file answers the index: a client that
//! routes in the browser then works below `/web/` whatever its own paths are, and resolves its
//! assets there from any of them. Plex Web is unaffected: its index names no base of its own
//! and loads the same assets.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

/// Where the client is mounted. Clients and bookmarks expect exactly this prefix.
pub const PREFIX: &str = "/web";

/// What every page is given, so that relative addresses resolve below the prefix.
const BASE: &[u8] = br#"<base href="/web/">"#;

/// The largest page given a base: a client's HTML is kilobytes, its scripts are files apart.
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;

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
            .layer(axum::middleware::map_response(with_base)),
    )
}

/// Gives a page served whole its base; anything else goes through untouched.
async fn with_base(response: Response) -> Response {
    let is_page = response.status() == StatusCode::OK
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"));
    if !is_page {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(page) = axum::body::to_bytes(body, MAX_PAGE_BYTES).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let page = based(&page);
    parts
        .headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from(page.len()));
    Response::from_parts(parts, Body::from(page))
}

/// `page` with [`BASE`] right after its `<head>`, else after its `<html>`, else first. A page
/// that names a base already keeps its own.
fn based(page: &[u8]) -> Vec<u8> {
    let lower = page.to_ascii_lowercase();
    if find(&lower, b"<base").is_some() {
        return page.to_vec();
    }
    let at = [b"<head".as_slice(), b"<html"]
        .into_iter()
        .find_map(|tag| {
            let start = find(&lower, tag)?;
            Some(start + find(&lower[start..], b">")? + 1)
        })
        .unwrap_or(0);
    [&page[..at], BASE, &page[at..]].concat()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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

    /// The test client's index as served.
    const INDEX: &str = r#"<html><head><base href="/web/"></head>plex</html>"#;

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
        std::fs::write(dir.join("index.html"), "<html><head></head>plex</html>").unwrap();
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
    async fn the_prefix_serves_the_index_with_its_base() {
        let router = router(&client("prefix")).unwrap();
        for uri in ["/web", "/web/", "/web/index.html"] {
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

    #[test]
    fn a_page_gets_one_base_after_its_head() {
        let based = |page: &str| String::from_utf8(based(page.as_bytes())).unwrap();
        assert_eq!(
            based(r#"<!doctype html><HTML lang="fr"><Head data-x="1"><title>t</title>"#),
            r#"<!doctype html><HTML lang="fr"><Head data-x="1"><base href="/web/"><title>t</title>"#
        );
        assert_eq!(
            based("<html><body>x</body></html>"),
            r#"<html><base href="/web/"><body>x</body></html>"#
        );
        assert_eq!(based("plain"), r#"<base href="/web/">plain"#);
        let own = r#"<html><head><BASE href="/app/"></head></html>"#;
        assert_eq!(based(own), own);
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

    #[test]
    fn only_the_client_prefix_is_public() {
        assert!(is_client_path("/web"));
        assert!(is_client_path("/web/index.html"));
        assert!(!is_client_path("/webhooks"));
        assert!(!is_client_path("/library/sections"));
    }
}
