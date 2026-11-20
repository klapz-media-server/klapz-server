//! Chapters on the official API: `Movie.chapters`, `Episode.chapters` and where they came from.
//! Read-only: they come from the file or from the import.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::connection::{Connection, EmptyFields};
use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, Object, Result, ID};
use open_plex_core::chapter::{Chapter, ChapterSource};
use open_plex_core::metadata::{MetadataError, MetadataRepository};
use open_plex_core::{ChapterId, RatingKey};

use crate::cursor::OpaqueCursor;
use crate::item::Item;
use crate::loader::VersionsLoader;
use crate::media::Version;
use crate::node::GlobalId;

pub const CHAPTER_TYPE: &str = "Chapter";

/// Where an item's chapters came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "ChapterSource")]
pub enum ChapterSourceType {
    /// Read from the file.
    Media,
    /// Supplied by a metadata agent, in whole seconds.
    Agent,
    /// A metadata agent's, over the file's.
    Mixed,
}

impl From<ChapterSource> for ChapterSourceType {
    fn from(source: ChapterSource) -> Self {
        match source {
            ChapterSource::Media => Self::Media,
            ChapterSource::Agent => Self::Agent,
            ChapterSource::Mixed => Self::Mixed,
        }
    }
}

/// A named section of a film or an episode, which a player lists and jumps to.
#[derive(Debug, Clone)]
pub struct ChapterObject(pub Chapter);

#[Object(name = "Chapter")]
impl ChapterObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this chapter.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(CHAPTER_TYPE, self.0.id.get()))
    }

    /// The film or episode it divides.
    async fn item(&self, context: &Context<'_>) -> Result<Option<Item>> {
        Ok(crate::schema::readable_item(context, self.0.item)
            .await?
            .map(Item::from_domain))
    }

    /// Its place among the item's chapters, from 1.
    async fn index(&self) -> u32 {
        self.0.index
    }

    /// Where it starts, in milliseconds from the start of the media.
    async fn start_ms(&self) -> i64 {
        self.0.start_ms
    }

    /// Where it ends, in milliseconds from the start of the media.
    async fn end_ms(&self) -> i64 {
        self.0.end_ms
    }

    /// Its name. Null when the media names none, or only numbers them (`Chapter 3`,
    /// `00:07:13.224`): a player then names it itself.
    async fn title(&self) -> Option<String> {
        self.0.title.clone()
    }

    /// The version a frame of which was drawn as the chapter's image. Null when none was drawn.
    async fn image_version(&self, context: &Context<'_>) -> Result<Option<Version>> {
        let Some(id) = self.0.image_version else {
            return Ok(None);
        };
        Ok(context
            .data::<DataLoader<VersionsLoader>>()?
            .load_one(self.0.item)
            .await?
            .unwrap_or_default()
            .into_iter()
            .find(|version| version.id == id)
            .map(Version))
    }
}

/// An item's chapters, in order.
pub type ChapterConnection = Connection<OpaqueCursor, ChapterObject, EmptyFields, EmptyFields>;

/// Loads the chapters of several items in one call.
#[derive(Debug, Clone)]
pub struct ChaptersLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl ChaptersLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for ChaptersLoader {
    type Value = Vec<Chapter>;
    type Error = Arc<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let mut found: HashMap<RatingKey, Self::Value> = HashMap::new();
        for chapter in self.repository.chapters(keys).await.map_err(Arc::new)? {
            found.entry(chapter.item).or_default().push(chapter);
        }
        Ok(found)
    }
}

/// The chapters of an item the caller reads, in order.
pub(crate) async fn chapters_of(
    context: &Context<'_>,
    item: RatingKey,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<ChapterConnection> {
    let rows = context
        .data::<DataLoader<ChaptersLoader>>()?
        .load_one(item)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| (ChapterObject(chapter), EmptyFields))
        .collect();
    crate::tag::page_in_memory(rows, after, before, first, last).await
}

/// A chapter as `node` refetches it: one of an item the caller reads.
pub(crate) async fn chapter_node(context: &Context<'_>, id: i64) -> Result<Option<ChapterObject>> {
    let Some(chapter) = crate::schema::metadata(context)?
        .chapter(ChapterId::new(id))
        .await?
    else {
        return Ok(None);
    };
    Ok(crate::schema::readable_item(context, chapter.item)
        .await?
        .map(|_| ChapterObject(chapter)))
}
