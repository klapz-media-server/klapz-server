//! What a set of items has in common: `Library.common`, for editing several items at once.

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, Object, Result};
use klapz_core::common::{CommonFields, Shared};
use klapz_core::metadata::TagKind;
use klapz_core::RatingKey;

use crate::collection::MembershipConnection;
use crate::edit::ItemFieldType;
use crate::item::Item;
use crate::loader::ItemLoader;
use crate::tag::{page_in_memory, Attachment, TagConnection, TagObject, TagType};
use crate::DateTime;

/// A field of the items compared, which `CommonFields.mixed` names when two of them differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum CommonField {
    /// The items themselves: mixed whenever there are two.
    Item,
    /// `title`.
    Title,
    /// `parent`.
    Parent,
    /// `parentTitle`.
    ParentTitle,
    /// `grandparent`.
    Grandparent,
    /// `grandparentTitle`.
    GrandparentTitle,
    /// `titleSort`.
    TitleSort,
    /// `originalTitle`.
    OriginalTitle,
    /// `tagline`.
    Tagline,
    /// `summary`.
    Summary,
    /// `studio`.
    Studio,
    /// `contentRating`.
    ContentRating,
    /// `year`.
    Year,
    /// `releasedOn`.
    ReleasedOn,
    /// `rating`.
    Rating,
    /// `number`.
    Number,
}

#[derive(Debug, Clone)]
pub struct CommonFieldsObject(pub CommonFields);

/// What every item of a set shares. A field two items differ on is null and listed in
/// `mixed`; one no item holds is null and not listed.
#[allow(clippy::missing_errors_doc)]
#[Object(name = "CommonFields")]
impl CommonFieldsObject {
    /// How many items were compared.
    async fn item_count(&self) -> u64 {
        self.0.items
    }

    /// The fields on which two items differ, one holding a value and another none included.
    async fn mixed(&self) -> Vec<CommonField> {
        let fields = &self.0;
        [
            (CommonField::Item, fields.id.is_mixed()),
            (CommonField::Title, fields.title.is_mixed()),
            (CommonField::Parent, fields.parent.is_mixed()),
            (CommonField::ParentTitle, fields.parent_title.is_mixed()),
            (CommonField::Grandparent, fields.grandparent.is_mixed()),
            (
                CommonField::GrandparentTitle,
                fields.grandparent_title.is_mixed(),
            ),
            (CommonField::TitleSort, fields.title_sort.is_mixed()),
            (CommonField::OriginalTitle, fields.original_title.is_mixed()),
            (CommonField::Tagline, fields.tagline.is_mixed()),
            (CommonField::Summary, fields.summary.is_mixed()),
            (CommonField::Studio, fields.studio.is_mixed()),
            (CommonField::ContentRating, fields.content_rating.is_mixed()),
            (CommonField::Year, fields.year.is_mixed()),
            (CommonField::ReleasedOn, fields.released_on.is_mixed()),
            (CommonField::Rating, fields.rating.is_mixed()),
            (CommonField::Number, fields.position.is_mixed()),
        ]
        .into_iter()
        .filter_map(|(field, mixed)| mixed.then_some(field))
        .collect()
    }

    /// The item, when exactly one was compared.
    async fn item(&self, context: &Context<'_>) -> Result<Option<Item>> {
        load(context, &self.0.id).await
    }

    /// The parent all items share: the season of episodes, the series of seasons.
    async fn parent(&self, context: &Context<'_>) -> Result<Option<Item>> {
        load(context, &self.0.parent).await
    }

    /// The title of the parent, when every parent has the same one.
    async fn parent_title(&self) -> Option<&str> {
        self.0.parent_title.value().map(String::as_str)
    }

    /// The parent's parent all items share: the series of episodes.
    async fn grandparent(&self, context: &Context<'_>) -> Result<Option<Item>> {
        load(context, &self.0.grandparent).await
    }

    /// The title of the parent's parent, when every one has the same.
    async fn grandparent_title(&self) -> Option<&str> {
        self.0.grandparent_title.value().map(String::as_str)
    }

    /// The common title.
    async fn title(&self) -> Option<&str> {
        self.0.title.value().map(String::as_str)
    }

    /// The common title sorted by, as stored: an item whose sort title was never set has none.
    async fn title_sort(&self) -> Option<&str> {
        self.0.title_sort.value().map(String::as_str)
    }

    /// The common original-language title.
    async fn original_title(&self) -> Option<&str> {
        self.0.original_title.value().map(String::as_str)
    }

    /// The common tagline.
    async fn tagline(&self) -> Option<&str> {
        self.0.tagline.value().map(String::as_str)
    }

    /// The common synopsis.
    async fn summary(&self) -> Option<&str> {
        self.0.summary.value().map(String::as_str)
    }

    /// The common studio.
    async fn studio(&self) -> Option<&str> {
        self.0.studio.value().map(String::as_str)
    }

    /// The common age-rating label.
    async fn content_rating(&self) -> Option<&str> {
        self.0.content_rating.value().map(String::as_str)
    }

    /// The common release year.
    async fn year(&self) -> Option<i32> {
        self.0.year.value().copied()
    }

    /// The common release date, as its day's midnight UTC.
    async fn released_on(&self) -> Option<DateTime> {
        self.0
            .released_on
            .value()
            .copied()
            .map(DateTime::from_unix_seconds)
    }

    /// The common critics' rating, from 0 to 10.
    async fn rating(&self) -> Option<f64> {
        self.0.rating.value().copied()
    }

    /// The common position among siblings: an episode's or a season's number.
    async fn number(&self) -> Option<i32> {
        self.0.position.value().copied()
    }

    /// The tags of one kind every item carries. For one item, in billing order and with the
    /// character played; for several, whose billing and characters differ, by tag id.
    async fn tags(
        &self,
        #[graphql(desc = "Which kind of tag to list.")] kind: TagType,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<TagConnection> {
        let kind = TagKind::from(kind);
        let rows = self
            .0
            .tags
            .iter()
            .filter(|shared| shared.tag.kind == kind)
            .map(|shared| {
                (
                    TagObject(shared.tag.clone()),
                    Attachment {
                        character: shared.role.clone(),
                        episode_count: None,
                    },
                )
            })
            .collect();
        page_in_memory(rows, after, before, first, last).await
    }

    /// The collections holding every item, by the number their items carry them under.
    async fn collections(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return collections after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return collections before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many collections from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many collections from the end of the range.")]
        last: Option<i32>,
    ) -> Result<MembershipConnection> {
        crate::collection::collections_by_id(
            context,
            &self.0.collections,
            (after, before, first, last),
        )
        .await
    }

    /// The fields every item has locked.
    async fn locked_fields(&self) -> Vec<ItemFieldType> {
        self.0.locked.iter().copied().map(Into::into).collect()
    }
}

async fn load(context: &Context<'_>, key: &Shared<RatingKey>) -> Result<Option<Item>> {
    let Some(key) = key.value() else {
        return Ok(None);
    };
    Ok(context
        .data::<DataLoader<ItemLoader>>()?
        .load_one(*key)
        .await?
        .map(Item::from_domain))
}
