//! The four colors of an item's blurred backdrop.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Object, Result};
use klapz_core::backdrop::{CornerColors, Rgb};
use klapz_core::metadata::{MetadataError, MetadataRepository};
use klapz_core::RatingKey;

/// Loads the backdrop colors of several items in one call.
#[derive(Debug, Clone)]
pub struct CornerColorsLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl CornerColorsLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for CornerColorsLoader {
    type Value = CornerColors;
    type Error = Arc<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        Ok(self
            .repository
            .corner_colors(keys)
            .await
            .map_err(Arc::new)?
            .into_iter()
            .collect())
    }
}

#[derive(Debug)]
pub struct CornerColorsObject(pub CornerColors);

fn css(color: Rgb) -> String {
    let [red, green, blue] = color.channels();
    format!("#{red:02x}{green:02x}{blue:02x}")
}

/// The four colors a client blends into the blurred backdrop it draws behind an item, drawn
/// from the item's background art: its season's or its series' when it has none, its poster
/// when none of them has any.
#[Object(name = "CornerColors")]
impl CornerColorsObject {
    /// The top-left corner's color, sRGB as `#rrggbb`. Darker than the others, for the text
    /// usually laid over it.
    async fn top_left(&self) -> String {
        css(self.0.top_left)
    }

    /// The top-right corner's color, sRGB as `#rrggbb`.
    async fn top_right(&self) -> String {
        css(self.0.top_right)
    }

    /// The bottom-right corner's color, sRGB as `#rrggbb`.
    async fn bottom_right(&self) -> String {
        css(self.0.bottom_right)
    }

    /// The bottom-left corner's color, sRGB as `#rrggbb`.
    async fn bottom_left(&self) -> String {
        css(self.0.bottom_left)
    }
}

pub(crate) async fn corner_colors_of(
    context: &Context<'_>,
    item: RatingKey,
) -> Result<Option<CornerColorsObject>> {
    Ok(context
        .data::<DataLoader<CornerColorsLoader>>()?
        .load_one(item)
        .await?
        .map(CornerColorsObject))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_color_as_css_does() {
        assert_eq!(css(Rgb::new(0x1a, 0x2c, 0x05)), "#1a2c05");
    }
}
