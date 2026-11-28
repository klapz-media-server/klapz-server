//! Where provider images are kept, the artwork store as matching keeps them through it, and
//! `/artwork/…`, where the official API's `ItemImage.url` points.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use klapz_core::source_metadata::artwork::{
    ArtworkError, ArtworkFetcher, ArtworkReader, StoredImage,
};
use klapz_transcoder::artwork::ArtworkStore;
use klapz_transcoder::Transcoder;

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
        .with_state(store)
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
