//! Batched loaders.
//!
//! Resolving a field per parent is the N+1 problem: a page of fifty episodes asking for its
//! series, its files and their tracks would issue two hundred queries. A `DataLoader`
//! collects the keys requested within one resolution pass and asks storage once, so the cost
//! is one query per level regardless of how many parents are in the result.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::Loader;
use open_plex_core::library::{LibraryError, LibraryRepository, LibrarySection, SectionRoot};
use open_plex_core::metadata::{
    EpisodeCredit, ExternalId, ItemTag, MediaFile, MediaStream, MediaVersion, MetadataError,
    MetadataItem, MetadataRepository, Rating,
};
use open_plex_core::{MediaId, PartId, RatingKey, SectionId};

/// `Arc` because a `DataLoader` hands the same error to every caller that shared the batch,
/// so it has to be cloneable.
type Shared<E> = Arc<E>;

/// Loads the roots of several libraries in one storage call.
#[derive(Debug, Clone)]
pub struct RootsLoader {
    repository: Arc<dyn LibraryRepository>,
}

impl RootsLoader {
    pub const fn new(repository: Arc<dyn LibraryRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<SectionId> for RootsLoader {
    type Value = Vec<SectionRoot>;
    type Error = Shared<LibraryError>;

    async fn load(
        &self,
        keys: &[SectionId],
    ) -> Result<HashMap<SectionId, Self::Value>, Self::Error> {
        let roots = self.repository.roots_for(keys).await.map_err(Arc::new)?;

        let mut grouped: HashMap<SectionId, Self::Value> = HashMap::new();
        for root in roots {
            grouped.entry(root.section_id).or_default().push(root);
        }
        Ok(grouped)
    }
}

/// Loads libraries by id, for the `library` field every item carries.
#[derive(Debug, Clone)]
pub struct LibraryLoader {
    repository: Arc<dyn LibraryRepository>,
}

impl LibraryLoader {
    pub const fn new(repository: Arc<dyn LibraryRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<SectionId> for LibraryLoader {
    type Value = LibrarySection;
    type Error = Shared<LibraryError>;

    async fn load(
        &self,
        keys: &[SectionId],
    ) -> Result<HashMap<SectionId, Self::Value>, Self::Error> {
        // A library listing is a handful of rows, so fetching one page of them and indexing
        // is cheaper than a lookup per key.
        let window = open_plex_core::page::Window::by_id(None, None, Some(500), None)
            .map_err(|error| Arc::new(LibraryError::Storage(Box::new(error))))?;
        let sections = self.repository.sections(window).await.map_err(Arc::new)?;

        Ok(sections
            .items
            .into_iter()
            .filter(|section| keys.contains(&section.id))
            .map(|section| (section.id, section))
            .collect())
    }
}

/// Loads items by id, for the parents a child points at.
#[derive(Debug, Clone)]
pub struct ItemLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl ItemLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for ItemLoader {
    type Value = MetadataItem;
    type Error = Shared<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let items = self.repository.items_by_id(keys).await.map_err(Arc::new)?;
        Ok(items.into_iter().map(|item| (item.id, item)).collect())
    }
}

/// Loads the versions of several items in one call.
#[derive(Debug, Clone)]
pub struct VersionsLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl VersionsLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for VersionsLoader {
    type Value = Vec<MediaVersion>;
    type Error = Shared<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let versions = self.repository.versions_for(keys).await.map_err(Arc::new)?;

        let mut grouped: HashMap<RatingKey, Self::Value> = HashMap::new();
        for version in versions {
            grouped.entry(version.item_id).or_default().push(version);
        }
        Ok(grouped)
    }
}

/// Loads the files of several versions in one call.
#[derive(Debug, Clone)]
pub struct FilesLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl FilesLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<MediaId> for FilesLoader {
    type Value = Vec<MediaFile>;
    type Error = Shared<MetadataError>;

    async fn load(&self, keys: &[MediaId]) -> Result<HashMap<MediaId, Self::Value>, Self::Error> {
        let files = self.repository.files_for(keys).await.map_err(Arc::new)?;

        let mut grouped: HashMap<MediaId, Self::Value> = HashMap::new();
        for file in files {
            grouped.entry(file.version_id).or_default().push(file);
        }
        Ok(grouped)
    }
}

/// Loads the tracks of several files in one call.
#[derive(Debug, Clone)]
pub struct StreamsLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl StreamsLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<PartId> for StreamsLoader {
    type Value = Vec<MediaStream>;
    type Error = Shared<MetadataError>;

    async fn load(&self, keys: &[PartId]) -> Result<HashMap<PartId, Self::Value>, Self::Error> {
        let streams = self.repository.streams_for(keys).await.map_err(Arc::new)?;

        let mut grouped: HashMap<PartId, Self::Value> = HashMap::new();
        for stream in streams {
            grouped.entry(stream.file_id).or_default().push(stream);
        }
        Ok(grouped)
    }
}

/// Counts the direct children of several parents in one call.
#[derive(Debug, Clone)]
pub struct ChildCountLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl ChildCountLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for ChildCountLoader {
    type Value = u64;
    type Error = Shared<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let counts = self.repository.child_counts(keys).await.map_err(Arc::new)?;
        Ok(counts.into_iter().collect())
    }
}

/// Counts the grandchildren of several parents in one call — a series' episodes.
#[derive(Debug, Clone)]
pub struct LeafCountLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl LeafCountLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for LeafCountLoader {
    type Value = u64;
    type Error = Shared<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let counts = self.repository.leaf_counts(keys).await.map_err(Arc::new)?;
        Ok(counts.into_iter().collect())
    }
}

/// Loads the tags of several items in one call: genres, countries and credits together.
#[derive(Debug, Clone)]
pub struct TagsLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl TagsLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for TagsLoader {
    type Value = Vec<ItemTag>;
    type Error = Shared<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let tags = self.repository.tags_for(keys).await.map_err(Arc::new)?;

        let mut grouped: HashMap<RatingKey, Self::Value> = HashMap::new();
        for tag in tags {
            grouped.entry(tag.item_id).or_default().push(tag);
        }
        Ok(grouped)
    }
}

/// Loads the credits of the episodes below several series or seasons in one call.
#[derive(Debug, Clone)]
pub struct CreditsBelowLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl CreditsBelowLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for CreditsBelowLoader {
    type Value = Vec<EpisodeCredit>;
    type Error = Shared<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let rows = self
            .repository
            .credits_below(keys)
            .await
            .map_err(Arc::new)?;

        let mut grouped: HashMap<RatingKey, Self::Value> = HashMap::new();
        for row in rows {
            grouped.entry(row.parent).or_default().push(row);
        }
        Ok(grouped)
    }
}

/// Loads the external identifiers of several items in one call.
#[derive(Debug, Clone)]
pub struct ExternalIdsLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl ExternalIdsLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for ExternalIdsLoader {
    type Value = Vec<ExternalId>;
    type Error = Shared<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let rows = self
            .repository
            .external_ids_for(keys)
            .await
            .map_err(Arc::new)?;

        let mut grouped: HashMap<RatingKey, Self::Value> = HashMap::new();
        for row in rows {
            grouped.entry(row.item_id).or_default().push(row);
        }
        Ok(grouped)
    }
}

/// Loads the ratings of several items in one call.
#[derive(Debug, Clone)]
pub struct RatingsLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl RatingsLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for RatingsLoader {
    type Value = Vec<Rating>;
    type Error = Shared<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let rows = self.repository.ratings_for(keys).await.map_err(Arc::new)?;

        let mut grouped: HashMap<RatingKey, Self::Value> = HashMap::new();
        for row in rows {
            grouped.entry(row.item_id).or_default().push(row);
        }
        Ok(grouped)
    }
}
