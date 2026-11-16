//! Extras on the official API: `Movie.extras`, `Show.extras`, `Season.extras`,
//! `Episode.extras`, and the trailer `primaryExtra` names.
//!
//! An extra is read by whoever reads its item, and is nothing to anyone else. Its versions are
//! `MediaVersion`s, with files and tracks, refetchable like an item's. Nothing adds one: an
//! extra comes with the library, and an endpoint that took a file or an address to make one
//! would take a path from a client.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::connection::{Connection, EmptyFields};
use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, Object, Result, ID};
use open_plex_core::extra::{self, Extra, ExtraKind};
use open_plex_core::metadata::{MediaVersion, MetadataError, MetadataRepository};
use open_plex_core::{ExtraId, RatingKey};

use crate::cursor::OpaqueCursor;
use crate::date_time::DateTime;
use crate::item::Item;
use crate::media::Version;
use crate::node::GlobalId;

pub const EXTRA_TYPE: &str = "Extra";

/// What an extra is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "ExtraKind")]
pub enum ExtraKindType {
    /// A trailer or a teaser. The first one is the item's `primaryExtra`.
    Trailer,
    /// A scene cut from the final edit.
    DeletedScene,
    /// An interview with the cast or the crew.
    Interview,
    /// A music video.
    MusicVideo,
    /// A making-of.
    BehindTheScenes,
    /// A scene or a sample of the work itself.
    Scene,
    /// A music video recorded live.
    LiveMusicVideo,
    /// A music video showing the lyrics.
    LyricMusicVideo,
    /// A concert.
    Concert,
    /// A short documentary about the work.
    Featurette,
    /// A short film.
    Short,
    /// Anything else.
    Other,
}

impl From<ExtraKind> for ExtraKindType {
    fn from(kind: ExtraKind) -> Self {
        match kind {
            ExtraKind::Trailer => Self::Trailer,
            ExtraKind::DeletedScene => Self::DeletedScene,
            ExtraKind::Interview => Self::Interview,
            ExtraKind::MusicVideo => Self::MusicVideo,
            ExtraKind::BehindTheScenes => Self::BehindTheScenes,
            ExtraKind::Scene => Self::Scene,
            ExtraKind::LiveMusicVideo => Self::LiveMusicVideo,
            ExtraKind::LyricMusicVideo => Self::LyricMusicVideo,
            ExtraKind::Concert => Self::Concert,
            ExtraKind::Featurette => Self::Featurette,
            ExtraKind::Short => Self::Short,
            ExtraKind::Other => Self::Other,
        }
    }
}

/// A trailer, a deleted scene, an interview: something that comes with an item without being
/// one of its library's.
#[derive(Debug, Clone)]
pub struct ExtraObject(pub Extra);

#[allow(clippy::missing_errors_doc)]
#[Object(name = "Extra")]
impl ExtraObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this extra.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(EXTRA_TYPE, self.0.id.get()))
    }

    /// The film, series, season or episode it comes with.
    async fn item(&self, context: &Context<'_>) -> Result<Option<Item>> {
        Ok(crate::schema::readable_item(context, self.0.item)
            .await?
            .map(Item::from_domain))
    }

    /// What it is.
    async fn kind(&self) -> ExtraKindType {
        self.0.kind.into()
    }

    /// Its place among its item's extras, which are listed by it.
    async fn position(&self) -> Option<i32> {
        self.0.position
    }

    /// Its identity at its source: a metadata provider's asset (`iva://…`), or the URL of a
    /// file beside its item (`file://…`).
    async fn guid(&self) -> &str {
        &self.0.guid
    }

    /// Display title.
    async fn title(&self) -> Option<&str> {
        self.0.title.as_deref()
    }

    /// The title it sorts by, when the source gives one.
    async fn title_sort(&self) -> Option<&str> {
        self.0.title_sort.as_deref()
    }

    /// Synopsis.
    async fn summary(&self) -> Option<&str> {
        self.0.summary.as_deref()
    }

    /// Release year.
    async fn year(&self) -> Option<i32> {
        self.0.year
    }

    /// Release date, as its midnight UTC.
    async fn released_on(&self) -> Option<DateTime> {
        self.0.released_on.map(DateTime::from)
    }

    /// Runtime in milliseconds.
    async fn duration_ms(&self) -> Option<i64> {
        self.0.duration_ms
    }

    /// When the library gained it. Null when the source does not say.
    async fn added_at(&self) -> Option<DateTime> {
        self.0.added_at.map(DateTime::from)
    }

    /// When its metadata last changed. Null when it never did, as for every extra a provider
    /// supplied to the reference library.
    async fn updated_at(&self) -> Option<DateTime> {
        self.0.updated_at.map(DateTime::from)
    }

    /// Its encodings: one per bitrate for a provider's stream, one for a file.
    async fn versions(&self, context: &Context<'_>) -> Result<Vec<Version>> {
        Ok(context
            .data::<DataLoader<ExtraVersionsLoader>>()?
            .load_one(self.0.id)
            .await?
            .unwrap_or_default()
            .into_iter()
            .map(Version)
            .collect())
    }
}

/// An item's extras, by position then id.
pub type ExtraConnection = Connection<OpaqueCursor, ExtraObject, EmptyFields, EmptyFields>;

/// Loads the extras of several items in one call.
#[derive(Debug, Clone)]
pub struct ExtrasLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl ExtrasLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for ExtrasLoader {
    type Value = Vec<Extra>;
    type Error = Arc<MetadataError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let mut found: HashMap<RatingKey, Self::Value> = HashMap::new();
        for extra in self.repository.extras(keys).await.map_err(Arc::new)? {
            found.entry(extra.item).or_default().push(extra);
        }
        Ok(found)
    }
}

/// Loads the versions of several extras in one call.
#[derive(Debug, Clone)]
pub struct ExtraVersionsLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl ExtraVersionsLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<ExtraId> for ExtraVersionsLoader {
    type Value = Vec<MediaVersion>;
    type Error = Arc<MetadataError>;

    async fn load(&self, keys: &[ExtraId]) -> Result<HashMap<ExtraId, Self::Value>, Self::Error> {
        let mut found: HashMap<ExtraId, Self::Value> = HashMap::new();
        for (extra, version) in self
            .repository
            .extra_versions(keys)
            .await
            .map_err(Arc::new)?
        {
            found.entry(extra).or_default().push(version);
        }
        Ok(found)
    }
}

async fn loaded(context: &Context<'_>, item: RatingKey) -> Result<Vec<Extra>> {
    Ok(context
        .data::<DataLoader<ExtrasLoader>>()?
        .load_one(item)
        .await?
        .unwrap_or_default())
}

/// The extras of an item the caller reads, by position then id.
pub(crate) async fn extras_of(
    context: &Context<'_>,
    item: RatingKey,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<ExtraConnection> {
    let rows = loaded(context, item)
        .await?
        .into_iter()
        .map(|extra| (ExtraObject(extra), EmptyFields))
        .collect();
    crate::tag::page_in_memory(rows, after, before, first, last).await
}

/// The trailer an item's player plays as its own.
pub(crate) async fn primary_extra_of(
    context: &Context<'_>,
    item: RatingKey,
) -> Result<Option<ExtraObject>> {
    let extras = loaded(context, item).await?;
    Ok(extra::primary(&extras).cloned().map(ExtraObject))
}

/// An extra as `node` refetches it: one whose item the caller reads.
pub(crate) async fn extra_node(context: &Context<'_>, id: i64) -> Result<Option<ExtraObject>> {
    let Some(extra) = crate::schema::metadata(context)?
        .extra(ExtraId::new(id))
        .await?
    else {
        return Ok(None);
    };
    Ok(crate::schema::readable_item(context, extra.item)
        .await?
        .map(|_| ExtraObject(extra)))
}
