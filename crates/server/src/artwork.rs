//! Where provider images are kept, the artwork store as matching keeps them through it, and
//! `/artwork/…`, where the official API's `ItemImage.url` points.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use klapz_core::source_metadata::artwork::{
    ArtworkError, ArtworkFetcher, ArtworkReader, StoredImage,
};
use klapz_transcoder::artwork::ArtworkStore;
use klapz_transcoder::{Fit, ImageRequest, Transcoder};
use serde::Deserialize;

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
        let (bytes, content_type) = self
            .store
            .read(file)
            .await
            .map_err(|error| ArtworkError(Box::new(error)))?;
        let Some((width, height)) = scale else {
            return Ok((bytes, content_type.to_owned()));
        };
        let request = ImageRequest {
            source: String::new(),
            width: i32::try_from(width.min(MAX_SIDE)).unwrap_or(i32::MAX),
            height: i32::try_from(height.min(MAX_SIDE)).unwrap_or(i32::MAX),
            fit: Fit::Contain,
            upscale: false,
            format: None,
            quality: None,
        };
        let image = self
            .transcoder
            .convert_image_bytes(bytes, request)
            .await
            .map_err(|error| ArtworkError(Box::new(error)))?;
        Ok((image.bytes, image_type(image.format).to_owned()))
    }
}

/// `/artwork/provider/{shard}/{name}`: a stored file, to any caller, whole or scaled down
/// into `width` × `height` (both given, or neither) — a grid of posters need not fetch
/// originals of a few megabytes each. The token middleware applies, and the store reads only
/// names of its own shape.
pub fn router(reader: StoreReader) -> Router {
    Router::new()
        .route("/artwork/provider/{shard}/{name}", get(serve))
        .with_state(reader)
}

#[derive(Debug, Deserialize)]
struct Scale {
    width: Option<u32>,
    height: Option<u32>,
}

async fn serve(
    State(reader): State<StoreReader>,
    Path((shard, name)): Path<(String, String)>,
    Query(scale): Query<Scale>,
) -> Response {
    let scale = match (scale.width, scale.height) {
        (None, None) => None,
        (Some(width), Some(height))
            if (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height) =>
        {
            Some((width, height))
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    match reader
        .read(&format!("provider/{shard}/{name}"), scale)
        .await
    {
        Ok((body, content_type)) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, CACHE_CONTROL.to_owned()),
            ],
            body,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn image_type(format: klapz_transcoder::OutputFormat) -> &'static str {
    match format {
        klapz_transcoder::OutputFormat::Png => "image/png",
        klapz_transcoder::OutputFormat::Ppm => "image/x-portable-pixmap",
        klapz_transcoder::OutputFormat::Jpeg => "image/jpeg",
    }
}

/// The images directory: `KLAPZ_IMAGES_DIR`, else `images` beside an `SQLite` database's file,
/// else beside the executable. Beside the database rather than the binary, so that a rebuild or
/// a `cargo clean` does not take the images with it.
pub fn images_directory(database_url: &str) -> PathBuf {
    if let Some(configured) = std::env::var_os("KLAPZ_IMAGES_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(configured);
    }
    let database_file = database_url
        .strip_prefix("sqlite:")
        .map(|rest| rest.trim_start_matches("//"))
        .map(|rest| rest.split_once('?').map_or(rest, |(path, _)| path))
        .filter(|path| !path.is_empty() && !path.starts_with(':'));
    if let Some(directory) =
        database_file.and_then(|file| PathBuf::from(file).parent().map(PathBuf::from))
    {
        return directory.join("images");
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|directory| directory.join("images")))
        .unwrap_or_else(|| PathBuf::from("images"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_sit_beside_an_sqlite_database() {
        if std::env::var_os("KLAPZ_IMAGES_DIR").is_some() {
            return;
        }
        assert_eq!(
            images_directory("sqlite://C:/data/klapz.db?mode=rwc"),
            PathBuf::from("C:/data/images")
        );
        assert_eq!(
            images_directory("sqlite:///srv/klapz/klapz.db"),
            PathBuf::from("/srv/klapz/images")
        );
        assert_ne!(
            images_directory("sqlite::memory:"),
            PathBuf::from("images/images")
        );
    }
}
