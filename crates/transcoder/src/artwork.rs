//! Artwork kept on this server's disk: the images metadata providers offer, downloaded once,
//! kept as the provider has them, and served from here even when the provider no longer is.
//!
//! A file is named after the SHA-256 of the address it came from, below a directory of its
//! first two hex digits: `provider/ab/abcd….jpg`. The name is known before anything is fetched,
//! so an image already on disk is never fetched again, and two items showing one poster share
//! one file. Its extension is the format its bytes are, not the one its address claims.
//!
//! What clients draw from an image — its `BlurHash`, a backdrop's four corner colors — is worked
//! out once, while it is decoded to be checked, and stored with it.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::imageops::FilterType;
use image::{GenericImageView, ImageFormat};
use reqwest::{redirect, Client, Url};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::backdrop::{self, CornerColors};
use crate::TranscodeError;

/// The hosts providers serve their images from, and the only ones fetched.
const PROVIDER_HOSTS: &[&str] = &["image.tmdb.org", "artworks.thetvdb.com"];

/// Originals: a 4K backdrop as PNG runs to tens of megabytes.
const MAX_ARTWORK_BYTES: usize = 64 * 1024 * 1024;

const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// What Plex scales an image to before hashing it: 100 pixels on its longer side.
const BLUR_HASH_SIDE: u32 = 100;

/// An image stored on disk, with what was worked out from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtwork {
    /// Below the store's root: `provider/ab/abcd….jpg`.
    pub file: String,
    pub content_type: &'static str,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub blur_hash: String,
    /// `None` for an image too small or too white to have any.
    pub corner_colors: Option<CornerColors>,
}

#[derive(Debug)]
pub struct ArtworkStore {
    root: PathBuf,
    client: OnceCell<Client>,
}

impl ArtworkStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: OnceCell::new(),
        }
    }

    /// The image at `url`, from disk when it is there, else downloaded, checked and written.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::ForbiddenSource`] for a host that is no provider's,
    /// [`TranscodeError::UnavailableSource`] when it cannot be fetched or written,
    /// [`TranscodeError::Undecodable`] for what is not an image this server reads.
    pub async fn fetch(&self, url: &str) -> Result<StoredArtwork, TranscodeError> {
        let parsed = provider_url(url)?;
        let hash = hex_sha256(url.as_bytes());
        if let Some(file) = self.existing(&hash).await {
            let bytes = tokio::fs::read(self.root.join(&file))
                .await
                .map_err(|_| TranscodeError::UnavailableSource)?;
            return analyzed(file, bytes).await;
        }
        let client = self.client.get_or_try_init(|| async { client() }).await?;
        let bytes = download(client, parsed).await?;
        let format = image::guess_format(&bytes).map_err(|_| TranscodeError::Undecodable)?;
        let extension = extension(format).ok_or(TranscodeError::Undecodable)?;
        let file = format!("provider/{}/{hash}.{extension}", &hash[..2]);
        let stored = analyzed(file.clone(), bytes.clone()).await?;
        self.write(&file, &hash, &bytes).await?;
        Ok(stored)
    }

    /// A stored file's bytes and type. Only a name this store gives out is read: anything
    /// else is [`TranscodeError::MissingFile`], whatever it points at.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::MissingFile`] for a name of another shape or a file gone from disk.
    pub async fn read(&self, file: &str) -> Result<(Vec<u8>, &'static str), TranscodeError> {
        let content_type = stored_name_type(file).ok_or(TranscodeError::MissingFile)?;
        let bytes = tokio::fs::read(self.root.join(file))
            .await
            .map_err(|_| TranscodeError::MissingFile)?;
        Ok((bytes, content_type))
    }

    async fn existing(&self, hash: &str) -> Option<String> {
        for extension in ["jpg", "png", "webp"] {
            let file = format!("provider/{}/{hash}.{extension}", &hash[..2]);
            if tokio::fs::try_exists(self.root.join(&file))
                .await
                .unwrap_or(false)
            {
                return Some(file);
            }
        }
        None
    }

    /// Written beside, then renamed into place: a file under `provider/` is always whole.
    async fn write(&self, file: &str, hash: &str, bytes: &[u8]) -> Result<(), TranscodeError> {
        let unavailable = |_| TranscodeError::UnavailableSource;
        let temporary_directory = self.root.join(".tmp");
        tokio::fs::create_dir_all(&temporary_directory)
            .await
            .map_err(unavailable)?;
        let target = self.root.join(file);
        if let Some(directory) = target.parent() {
            tokio::fs::create_dir_all(directory)
                .await
                .map_err(unavailable)?;
        }
        let temporary = temporary_directory.join(format!("{hash}.part"));
        tokio::fs::write(&temporary, bytes)
            .await
            .map_err(unavailable)?;
        tokio::fs::rename(&temporary, &target)
            .await
            .map_err(unavailable)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn provider_url(requested: &str) -> Result<Url, TranscodeError> {
    let url = Url::parse(requested).map_err(|_| TranscodeError::InvalidSource)?;
    let allowed = url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url
            .host_str()
            .is_some_and(|host| PROVIDER_HOSTS.contains(&host));
    if !allowed {
        tracing::warn!("refused an artwork source outside the providers' hosts");
        return Err(TranscodeError::ForbiddenSource);
    }
    Ok(url)
}

fn client() -> Result<Client, TranscodeError> {
    Client::builder()
        .user_agent("klapz")
        .timeout(FETCH_TIMEOUT)
        // Providers answer from the address they publish; a redirect elsewhere is refused.
        .redirect(redirect::Policy::none())
        .build()
        .map_err(TranscodeError::Client)
}

async fn download(client: &Client, url: Url) -> Result<Vec<u8>, TranscodeError> {
    let unavailable = |_| TranscodeError::UnavailableSource;
    let mut response = client.get(url).send().await.map_err(unavailable)?;
    if !response.status().is_success() {
        return Err(TranscodeError::UnavailableSource);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARTWORK_BYTES as u64)
    {
        return Err(TranscodeError::SourceTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unavailable)? {
        if bytes.len() + chunk.len() > MAX_ARTWORK_BYTES {
            return Err(TranscodeError::SourceTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn analyzed(file: String, bytes: Vec<u8>) -> Result<StoredArtwork, TranscodeError> {
    tokio::task::spawn_blocking(move || analyze(file, &bytes))
        .await
        .map_err(TranscodeError::Worker)?
}

fn analyze(file: String, bytes: &[u8]) -> Result<StoredArtwork, TranscodeError> {
    let content_type = stored_name_type(&file).ok_or(TranscodeError::Undecodable)?;
    let image = crate::convert::decode(bytes)?.pixels;
    let (width, height) = image.dimensions();
    let small = image.resize(BLUR_HASH_SIDE, BLUR_HASH_SIDE, FilterType::Triangle);
    Ok(StoredArtwork {
        file,
        content_type,
        width,
        height,
        size_bytes: bytes.len() as u64,
        blur_hash: blur_hash(&small.to_rgb8()),
        corner_colors: backdrop::corner_colors(bytes).ok(),
    })
}

fn extension(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Jpeg => Some("jpg"),
        ImageFormat::Png => Some("png"),
        ImageFormat::WebP => Some("webp"),
        _ => None,
    }
}

/// The type of a name this store gives out — `provider/`, two hex digits, the 64 of a SHA-256,
/// a known extension — and `None` for any other string.
fn stored_name_type(file: &str) -> Option<&'static str> {
    let rest = file.strip_prefix("provider/")?;
    let (shard, name) = rest.split_once('/')?;
    let (hash, extension) = name.split_once('.')?;
    let hex = |text: &str| {
        text.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if shard.len() != 2 || hash.len() != 64 || !hex(shard) || !hex(hash) || !hash.starts_with(shard)
    {
        return None;
    }
    match extension {
        "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            // Writing to a `String` cannot fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

const BASE83: &[u8] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~";

/// The `BlurHash` of an image, with Plex's grid: four by three components for a landscape image,
/// three by four for a portrait one, so that a poster's hash starts with `T` and a backdrop's
/// with `L`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "each value cast is clamped just above to a small non-negative range"
)]
fn blur_hash(image: &image::RgbImage) -> String {
    let (width, height) = image.dimensions();
    let (columns, rows): (u32, u32) = if width >= height { (4, 3) } else { (3, 4) };
    let linear: Vec<[f64; 3]> = image
        .pixels()
        .map(|pixel| pixel.0.map(srgb_to_linear))
        .collect();
    let mut factors = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let mut sum = [0.0; 3];
            for (index, pixel) in linear.iter().enumerate() {
                let x = index as u32 % width;
                let y = index as u32 / width;
                let basis = (PI * f64::from(column) * f64::from(x) / f64::from(width)).cos()
                    * (PI * f64::from(row) * f64::from(y) / f64::from(height)).cos();
                for channel in 0..3 {
                    sum[channel] += basis * pixel[channel];
                }
            }
            let normalization = if column == 0 && row == 0 { 1.0 } else { 2.0 };
            let scale = normalization / (f64::from(width) * f64::from(height));
            factors.push(sum.map(|value| value * scale));
        }
    }

    let mut hash = String::new();
    push_base83(&mut hash, columns - 1 + (rows - 1) * 9, 1);
    let (dc, ac) = factors
        .split_first()
        .map_or(([0.0; 3], &[][..]), |(dc, ac)| (*dc, ac));
    let maximum = if ac.is_empty() {
        push_base83(&mut hash, 0, 1);
        1.0
    } else {
        let actual = ac
            .iter()
            .flat_map(|factor| factor.iter().map(|value| value.abs()))
            .fold(0.0, f64::max);
        let quantised = (actual * 166.0 - 0.5).floor().clamp(0.0, 82.0);
        push_base83(&mut hash, quantised as u32, 1);
        (quantised + 1.0) / 166.0
    };
    let [r, g, b] = dc.map(linear_to_srgb);
    push_base83(&mut hash, (r << 16) | (g << 8) | b, 4);
    for factor in ac {
        let [r, g, b] = factor.map(|value| {
            let scaled = (value / maximum).signum() * (value / maximum).abs().sqrt();
            (scaled * 9.0 + 9.5).floor().clamp(0.0, 18.0) as u32
        });
        push_base83(&mut hash, r * 19 * 19 + g * 19 + b, 2);
    }
    hash
}

fn push_base83(hash: &mut String, value: u32, length: u32) {
    for place in (0..length).rev() {
        let digit = (value / 83u32.pow(place)) % 83;
        hash.push(char::from(BASE83[digit as usize]));
    }
}

fn srgb_to_linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a clamped value of 0 to 1 gives 0.5 to 255.5"
)]
fn linear_to_srgb(value: f64) -> u32 {
    let value = value.clamp(0.0, 1.0);
    let srgb = if value <= 0.003_130_8 {
        value * 12.92 * 255.0 + 0.5
    } else {
        (1.055 * value.powf(1.0 / 2.4) - 0.055) * 255.0 + 0.5
    };
    srgb as u32
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn png(image: &image::RgbImage) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn a_poster_hashes_on_three_by_four_and_a_backdrop_on_four_by_three() {
        let poster = image::RgbImage::from_pixel(67, 100, image::Rgb([200, 30, 30]));
        let backdrop = image::RgbImage::from_pixel(100, 56, image::Rgb([30, 30, 200]));
        let poster_hash = blur_hash(&poster);
        let backdrop_hash = blur_hash(&backdrop);
        assert!(poster_hash.starts_with('T'), "{poster_hash}");
        assert!(backdrop_hash.starts_with('L'), "{backdrop_hash}");
        assert_eq!(poster_hash.len(), 2 + 4 + 2 * 11);
    }

    #[test]
    fn a_flat_color_hashes_to_its_own_color() {
        let gray = image::RgbImage::from_pixel(100, 56, image::Rgb([128, 128, 128]));
        let hash = blur_hash(&gray);
        // The DC term, after the size and the AC maximum, is the color itself: 0x808080.
        let mut expected = String::new();
        push_base83(&mut expected, 0x0080_8080, 4);
        assert_eq!(&hash[2..6], expected);
    }

    #[test]
    fn only_provider_hosts_over_https_are_fetched() {
        assert!(provider_url("https://image.tmdb.org/t/p/original/a.jpg").is_ok());
        assert!(provider_url("https://artworks.thetvdb.com/banners/p.jpg").is_ok());
        for refused in [
            "http://image.tmdb.org/t/p/original/a.jpg",
            "https://image.tmdb.org:8443/a.jpg",
            "https://image.tmdb.org.evil.example/a.jpg",
            "https://user@image.tmdb.org/a.jpg",
            "https://127.0.0.1/a.jpg",
        ] {
            assert!(provider_url(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn reads_only_names_the_store_gives_out() {
        let hash = hex_sha256(b"https://image.tmdb.org/t/p/original/a.jpg");
        let good = format!("provider/{}/{hash}.jpg", &hash[..2]);
        assert_eq!(stored_name_type(&good), Some("image/jpeg"));
        for bad in [
            "provider/../secrets.jpg".to_owned(),
            format!("provider/{}/{hash}.exe", &hash[..2]),
            format!("provider/zz/{hash}.jpg"),
            format!("uploads/{}/{hash}.jpg", &hash[..2]),
            format!("provider/{}/{hash}.jpg/..", &hash[..2]),
        ] {
            assert_eq!(stored_name_type(&bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn an_image_on_disk_is_read_back_with_what_was_worked_out() {
        let root = std::env::temp_dir().join(format!("klapz-artwork-{}", std::process::id()));
        let store = ArtworkStore::new(root.clone());
        let url = "https://image.tmdb.org/t/p/original/poster.png";
        let hash = hex_sha256(url.as_bytes());
        let file = format!("provider/{}/{hash}.png", &hash[..2]);
        let poster = image::RgbImage::from_pixel(200, 300, image::Rgb([10, 120, 60]));
        store.write(&file, &hash, &png(&poster)).await.unwrap();

        let stored = store.fetch(url).await.unwrap();

        assert_eq!(stored.file, file);
        assert_eq!((stored.width, stored.height), (200, 300));
        assert!(stored.blur_hash.starts_with('T'));
        let (bytes, content_type) = store.read(&file).await.unwrap();
        assert_eq!(content_type, "image/png");
        assert_eq!(bytes.len() as u64, stored.size_bytes);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
