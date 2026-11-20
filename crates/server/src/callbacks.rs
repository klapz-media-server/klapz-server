//! The transcoder process's way back in: it reads its source and reports its segments and its
//! progress over HTTP, at URLs naming its session by an unguessable id and the run it belongs
//! to, `/transcoder/{session}/{run}/…`.
//!
//! The process holds no token, so these routes take none. They answer only to a session's id
//! and only on loopback, where the process runs: anything else meets a `404`, as an unknown
//! session does.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use klapz_transcoder::Transcoder;
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

/// An announcement is a segment list or a manifest: a few kilobytes.
const MAX_ANNOUNCEMENT_BYTES: usize = 1 << 20;

pub fn router(transcoder: Arc<Transcoder>) -> Router {
    Router::new()
        .route("/transcoder/{session}/{run}/source", get(source))
        .route("/transcoder/{session}/{run}/manifest", post(manifest))
        .route("/transcoder/{session}/{run}/progress", put(progress))
        .route(
            "/transcoder/{session}/{run}/progress/{detail}",
            put(progress),
        )
        .route_layer(middleware::from_fn(loopback_only))
        .with_state(transcoder)
}

/// The process is always this machine's: a request from anywhere else is not it.
async fn loopback_only(request: Request, next: Next) -> Response {
    let local = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(peer)| peer.ip().to_canonical().is_loopback());
    if local {
        next.run(request).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// The file being converted, with `Range`, which the process seeks with.
async fn source(
    State(transcoder): State<Arc<Transcoder>>,
    Path((session, _run)): Path<(String, String)>,
    request: Request,
) -> Response {
    let Some(path) = transcoder.callback_source(&session) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    }
}

/// A segment list or a manifest, for the run named: one replaced since may still be heard from.
async fn manifest(
    State(transcoder): State<Arc<Transcoder>>,
    Path((session, run)): Path<(String, String)>,
    body: Body,
) -> StatusCode {
    if !transcoder.has_callback(&session) {
        return StatusCode::NOT_FOUND;
    }
    // Answered before the body is read: this build often sends a request's head and waits for
    // the answer before its body, as the reference answers at once. Waiting for the body first
    // would hold both sides, and every announcement after the first would be lost.
    tokio::spawn(async move {
        match axum::body::to_bytes(body, MAX_ANNOUNCEMENT_BYTES).await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                if !transcoder.announce_segments(&session, &run, &text) {
                    tracing::debug!("an announcement arrived for a session since stopped");
                }
            }
            Err(error) => tracing::debug!(%error, "a transcoder announcement did not arrive"),
        }
    });
    StatusCode::OK
}

/// Progress reports. Nothing reads them yet, and the process only needs an answer.
async fn progress(
    State(transcoder): State<Arc<Transcoder>>,
    Path(path): Path<HashMap<String, String>>,
) -> StatusCode {
    if path
        .get("session")
        .is_some_and(|session| transcoder.has_callback(session))
    {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn request(method: &str, path: &str, peer: Option<&str>) -> Request {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        if let Some(peer) = peer {
            request
                .extensions_mut()
                .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        }
        request
    }

    async fn status(router: &Router, request: Request) -> StatusCode {
        router.clone().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn only_the_machine_itself_is_answered() {
        let router = Router::new()
            .route("/here", get(|| async { StatusCode::OK }))
            .route_layer(middleware::from_fn(loopback_only));
        for peer in ["127.0.0.1:5000", "[::1]:5000", "[::ffff:127.0.0.1]:5000"] {
            assert_eq!(
                status(&router, request("GET", "/here", Some(peer))).await,
                StatusCode::OK,
                "{peer}"
            );
        }
        for peer in [Some("192.168.1.20:5000"), None] {
            assert_eq!(
                status(&router, request("GET", "/here", peer)).await,
                StatusCode::NOT_FOUND,
                "{peer:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_session_that_does_not_exist_is_not_found() {
        let router = router(Arc::new(Transcoder::new(
            klapz_transcoder::Config::default(),
        )));
        let local = Some("127.0.0.1:5000");
        for (method, path) in [
            ("GET", "/transcoder/unknown/0/source"),
            ("POST", "/transcoder/unknown/0/manifest"),
            ("PUT", "/transcoder/unknown/0/progress"),
            ("PUT", "/transcoder/unknown/0/progress/streamDetail"),
        ] {
            assert_eq!(
                status(&router, request(method, path, local)).await,
                StatusCode::NOT_FOUND,
                "{method} {path}"
            );
        }
    }
}
