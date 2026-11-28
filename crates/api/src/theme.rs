//! A series' theme music, kept on this server and served beside its images.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, Object, Result};
use klapz_core::source_metadata::theme::{ShownTheme, ThemeSource};
use klapz_core::source_metadata::{SourceMetadataError, SourceMetadataRepository};
use klapz_core::RatingKey;

use crate::artwork::ARTWORK_PREFIX;
use crate::{DateTime, Int64};

/// Loads the themes of several series in one call.
#[derive(Debug, Clone)]
pub struct ThemesLoader {
    repository: Arc<dyn SourceMetadataRepository>,
}

impl ThemesLoader {
    pub fn new(repository: Arc<dyn SourceMetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for ThemesLoader {
    type Value = ShownTheme;
    type Error = Arc<SourceMetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        Ok(self
            .repository
            .themes_of(keys)
            .await
            .map_err(Arc::new)?
            .into_iter()
            .map(|theme| (theme.item, theme))
            .collect())
    }
}

/// Where a theme came from.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
#[graphql(name = "ThemeSource")]
pub enum ThemeSourceType {
    /// A `theme.*` file beside the series' episodes, copied when the series was matched.
    Local,
    /// Plex's theme service, by the series' TVDB id.
    Provider,
}

impl From<ThemeSource> for ThemeSourceType {
    fn from(source: ThemeSource) -> Self {
        match source {
            ThemeSource::Local => Self::Local,
            ThemeSource::Provider => Self::Provider,
        }
    }
}

#[derive(Debug)]
pub struct ItemThemeObject(pub ShownTheme);

/// A series' theme music, kept on this server: a local `theme.*` file wins over the theme
/// service's, as on the reference.
#[Object(name = "ItemTheme")]
impl ItemThemeObject {
    /// Where this server serves it, below `/artwork/`, with `Range`. A new theme has a new
    /// address, so a client may cache one for good.
    async fn url(&self) -> String {
        format!("{ARTWORK_PREFIX}{}", self.0.file)
    }

    /// Its media type: `audio/mpeg` for the service's; a local one's by its extension.
    async fn content_type(&self) -> &str {
        &self.0.content_type
    }

    /// Its size in bytes.
    async fn size_bytes(&self) -> Int64 {
        Int64(i64::try_from(self.0.size_bytes).unwrap_or(i64::MAX))
    }

    /// Where it came from.
    async fn source(&self) -> ThemeSourceType {
        self.0.source.into()
    }

    /// When the series took it.
    async fn changed_at(&self) -> DateTime {
        self.0.chosen_at.into()
    }
}

pub(crate) async fn theme_of(
    context: &Context<'_>,
    item: RatingKey,
) -> Result<Option<ItemThemeObject>> {
    let Ok(loader) = context.data::<DataLoader<ThemesLoader>>() else {
        // A server without source metadata keeps no themes.
        return Ok(None);
    };
    Ok(loader.load_one(item).await?.map(ItemThemeObject))
}
