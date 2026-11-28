//! Where provider images and series' themes are kept, the artwork store as matching keeps them
//! through it, and `/artwork/…`, where the official API's `ItemImage.url` and `ItemTheme.url`
//! point.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use klapz_core::source_metadata::artwork::{
    ArtworkError, ArtworkFetcher, ArtworkReader, StoredImage,
};
use klapz_core::source_metadata::theme::{StoredTheme, ThemeStore};
use klapz_transcoder::artwork::{ArtworkStore, StoredThemeFile};
use klapz_transcoder::Transcoder;
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

/// A stored image never changes under its name, which is the hash of its source.
const CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

/// The largest box a stored image is scaled into: a 4K screen's.
const MAX_SIDE: u32 = 3840;

/// Stored images read back for clients, scaled down by the transcoder when asked: what
/// `/artwork/…` serves and what the Jellyfin surface reads through.
#[derive(Debug, Clone)]
pub struct StoreReader {
    pub store: Arc<ArtworkStore>,
    pub transcoder: Arc<Transcoder>,
}

#[async_trait::async_trait]
impl ArtworkReader for StoreReader {
    async fn read(
        &self,
        file: &str,
        scale: Option<(u32, u32)>,
    ) -> Result<(Vec<u8>, String), ArtworkError> {
        let read = match scale {
            None => self.store.read(file).await,
            Some(scale) => {
                self.store
                    .scaled(
                        &self.transcoder,
                        file,
                        (scale.0.min(MAX_SIDE), scale.1.min(MAX_SIDE)),
                        false,
                    )
                    .await
            }
        };
        let (bytes, content_type) = read.map_err(|error| ArtworkError(Box::new(error)))?;
        Ok((bytes, content_type.to_owned()))
    }
}

/// `/artwork/provider/{shard}/{name}`: a stored file as it is, to any caller. Scaled copies are
/// `/photo/:/transcode`'s, with the file's address as its `url`. The token middleware applies,
/// and the store reads only names of its own shape.
pub fn router(store: Arc<ArtworkStore>) -> Router {
    Router::new()
        .route("/artwork/provider/{shard}/{name}", get(serve))
        .route("/artwork/themes/{shard}/{name}", get(serve_theme))
        .with_state(store)
}

/// `/artwork/themes/{shard}/{name}`: a stored theme, with `Range`, which a player seeks with.
async fn serve_theme(
    State(store): State<Arc<ArtworkStore>>,
    Path((shard, name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let Some((path, content_type)) = store.located(&format!("themes/{shard}/{name}")) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };
    if response.status().is_success() {
        let headers = response.headers_mut();
        // The store's type, not one guessed from the extension.
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(CACHE_CONTROL),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    response
}

async fn serve(
    State(store): State<Arc<ArtworkStore>>,
    Path((shard, name)): Path<(String, String)>,
) -> Response {
    match store.read(&format!("provider/{shard}/{name}")).await {
        Ok((body, content_type)) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, CACHE_CONTROL),
            ],
            body,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Debug, Clone)]
pub struct StoreFetcher(pub Arc<ArtworkStore>);

#[async_trait::async_trait]
impl ArtworkFetcher for StoreFetcher {
    async fn fetch(&self, url: &str) -> Result<StoredImage, ArtworkError> {
        let stored = self
            .0
            .fetch(url)
            .await
            .map_err(|error| ArtworkError(Box::new(error)))?;
        let color = |[r, g, b]: [u8; 3]| (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        Ok(StoredImage {
            file: stored.file,
            content_type: stored.content_type.to_owned(),
            width: stored.width,
            height: stored.height,
            size_bytes: stored.size_bytes,
            blur_hash: stored.blur_hash,
            corner_colors: stored.corner_colors.map(|corners| {
                [
                    color(corners.top_left),
                    color(corners.top_right),
                    color(corners.bottom_right),
                    color(corners.bottom_left),
                ]
            }),
        })
    }
}

#[async_trait::async_trait]
impl ThemeStore for StoreFetcher {
    async fn fetch(&self, url: &str) -> Result<Option<StoredTheme>, ArtworkError> {
        let stored = self
            .0
            .fetch_theme(url)
            .await
            .map_err(|error| ArtworkError(Box::new(error)))?;
        Ok(stored.map(stored_theme))
    }

    async fn keep(&self, bytes: Vec<u8>, extension: &str) -> Result<StoredTheme, ArtworkError> {
        self.0
            .keep_theme(&bytes, extension)
            .await
            .map(stored_theme)
            .map_err(|error| ArtworkError(Box::new(error)))
    }
}

fn stored_theme(stored: StoredThemeFile) -> StoredTheme {
    StoredTheme {
        file: stored.file,
        content_type: stored.content_type.to_owned(),
        size_bytes: stored.size_bytes,
    }
}
