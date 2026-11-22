//! The images an item shows: its poster, background, logo, banner or still, kept on this
//! server and served at an address of their own.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, Object, Result};
use klapz_core::source_metadata::{
    ImageKind, ShownImage, SourceMetadataError, SourceMetadataRepository,
};
use klapz_core::RatingKey;

use crate::DateTime;

/// Where stored images are served: the store's own name for a file follows.
pub const ARTWORK_PREFIX: &str = "/artwork/";

/// Loads the images of several items in one call.
#[derive(Debug, Clone)]
pub struct ImagesLoader {
    repository: Arc<dyn SourceMetadataRepository>,
}

impl ImagesLoader {
    pub fn new(repository: Arc<dyn SourceMetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for ImagesLoader {
    type Value = Vec<ShownImage>;
    type Error = Arc<SourceMetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let mut found: HashMap<RatingKey, Vec<ShownImage>> = HashMap::new();
        for image in self.repository.images_of(keys).await.map_err(Arc::new)? {
            found.entry(image.item).or_default().push(image);
        }
        Ok(found)
    }
}

/// What an image is to the item it belongs to.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
#[graphql(name = "ImageKind")]
pub enum ImageKindType {
    /// The cover, portrait, in the library's language where the provider has one.
    Poster,
    /// The art drawn behind the item, without text where the provider has such.
    Background,
    /// The title drawn on a transparent ground.
    ClearLogo,
    /// A wide strip, for series.
    Banner,
    /// An episode's still.
    Thumb,
}

impl From<ImageKind> for ImageKindType {
    fn from(kind: ImageKind) -> Self {
        match kind {
            ImageKind::Poster => Self::Poster,
            ImageKind::Background => Self::Background,
            ImageKind::ClearLogo => Self::ClearLogo,
            ImageKind::Banner => Self::Banner,
            ImageKind::Thumb => Self::Thumb,
        }
    }
}

#[derive(Debug)]
pub struct ItemImageObject(pub ShownImage);

/// An image an item shows, kept on this server: it stays served when its provider is not.
#[Object(name = "ItemImage")]
impl ItemImageObject {
    /// What it is to the item: its poster, its background…
    async fn kind(&self) -> ImageKindType {
        self.0.kind.into()
    }

    /// Where this server serves it, below `/artwork/`. A new image has a new address, so a
    /// client may cache one for good.
    async fn url(&self) -> String {
        format!("{ARTWORK_PREFIX}{}", self.0.file)
    }

    /// Its media type: `image/jpeg`, `image/png` or `image/webp`.
    async fn content_type(&self) -> &str {
        &self.0.content_type
    }

    /// In pixels, as the provider has it: images are kept at their original size.
    async fn width(&self) -> u32 {
        self.0.width
    }

    /// In pixels.
    async fn height(&self) -> u32 {
        self.0.height
    }

    /// A `BlurHash` of it, to draw while it loads: three by four components for a portrait
    /// image, four by three for a landscape one.
    async fn blur_hash(&self) -> &str {
        &self.0.blur_hash
    }

    /// When the item took it.
    async fn changed_at(&self) -> DateTime {
        self.0.chosen_at.into()
    }
}

pub(crate) async fn images_of(
    context: &Context<'_>,
    item: RatingKey,
) -> Result<Vec<ItemImageObject>> {
    let Ok(loader) = context.data::<DataLoader<ImagesLoader>>() else {
        // A server without source metadata keeps no images.
        return Ok(Vec::new());
    };
    let mut images = loader.load_one(item).await?.unwrap_or_default();
    images.sort_by_key(|image| image.kind);
    Ok(images.into_iter().map(ItemImageObject).collect())
}
