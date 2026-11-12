//! GraphQL types for library items.
//!
//! One storage table maps onto one interface and four concrete types. Shows and seasons carry
//! no `versions` field because, measured on the reference library, none of 4,002 shows and
//! none of 9,736 seasons has media — while every movie and episode does. Giving them an
//! always-empty field would be worse than omitting it: it invites the client to ask.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Interface, Object, Result, ID};
use open_plex_core::metadata::{ItemKind, MetadataItem};
use open_plex_core::page::{Cursor, MAX_ITEMS};
use open_plex_core::RatingKey;

use crate::collection::MembershipConnection;
use crate::cursor::OpaqueCursor;
use crate::edit::ItemFieldType;
use crate::library::Library;
use crate::loader::{ChildCountLoader, ItemLoader, LeafCountLoader, LibraryLoader, VersionsLoader};
use crate::marker::MarkerConnection;
use crate::media::Version;
use crate::node::GlobalId;
use crate::tag::{external_ids_of, ratings_of, ExternalIdConnection, ItemTags, RatingConnection};
use crate::watch::WatchStateObject;
use crate::DateTime;

/// The type tag all four item kinds share in their global id.
///
/// One tag rather than four because a caller holding only a number does not know the kind;
/// `node` reads it from storage and returns the matching concrete type.
pub const ITEM_TYPE: &str = "Item";

/// Default page size for a child connection when the client requests neither end.
const DEFAULT_PAGE_SIZE: u32 = 50;

/// Anything that appears in a library listing.
// The derive takes one `field(...)` entry per interface field, and each carries its own `ty`
// key; clippy reads those repeats as one duplicated attribute.
#[allow(clippy::duplicated_attributes)]
#[derive(Debug, Clone, Interface)]
#[graphql(
    field(
        name = "id",
        ty = "ID",
        desc = "Globally unique, opaque identifier. Pass it to `node` to refetch this item."
    ),
    field(
        name = "title",
        ty = "Option<String>",
        desc = "Display title, in the library's metadata language. Null for almost every season, which the source stores without one — use `number` there."
    ),
    field(
        name = "title_sort",
        ty = "Option<String>",
        desc = "Title to sort by when it differs from `title`. Null when `title` already sorts correctly."
    ),
    field(
        name = "guid",
        ty = "String",
        desc = "Identity of the work this item represents, shared by the same film or series wherever it appears. Two libraries holding the same film give two items with different `id`s and the same `guid`."
    ),
    field(
        name = "slug",
        ty = "String",
        desc = "The item's name for a URL: lowercase, accents folded, words joined by hyphens. A film or a series by its title, a season by its series and its number (`bull-s01`), an episode by its series and its own title (`bull-good-news-about-hell`). A name, not an identity: two items can share one. `unknown` when nothing in its name survives."
    ),
    field(
        name = "summary",
        ty = "Option<String>",
        desc = "Synopsis, in the library's metadata language."
    ),
    field(
        name = "added_at",
        ty = "DateTime",
        desc = "When the item was added to the library."
    ),
    field(
        name = "updated_at",
        ty = "DateTime",
        desc = "When the item's metadata last changed."
    ),
    field(
        name = "library",
        ty = "Option<Library>",
        desc = "The library this item belongs to. The same work present in two libraries is two distinct items with two ids."
    ),
    field(
        name = "watch_state",
        ty = "Option<WatchStateObject>",
        desc = "What the caller has watched of the work, shared by every copy of it. Null without a caller."
    ),
    field(
        name = "locked_fields",
        ty = "Vec<ItemFieldType>",
        desc = "The fields an editor locked, which a metadata refresh leaves as they are."
    )
)]
pub enum Item {
    /// A film.
    Movie(Movie),
    /// A television series.
    Show(Show),
    /// A season of a series.
    Season(Season),
    /// An episode of a series.
    Episode(Episode),
}

impl Item {
    /// Wraps a stored row as the concrete type its kind calls for.
    pub fn from_domain(item: MetadataItem) -> Self {
        match item.kind {
            ItemKind::Movie => Self::Movie(Movie(item)),
            ItemKind::Show => Self::Show(Show(item)),
            ItemKind::Season => Self::Season(Season(item)),
            ItemKind::Episode => Self::Episode(Episode(item)),
        }
    }
}

/// A page of library items, with cursors for walking the rest.
pub type ItemConnection = Connection<OpaqueCursor, Item, EmptyFields, EmptyFields>;
/// A page of seasons.
pub type SeasonConnection = Connection<OpaqueCursor, Season, EmptyFields, EmptyFields>;
/// A page of episodes.
pub type EpisodeConnection = Connection<OpaqueCursor, Episode, EmptyFields, EmptyFields>;

/// A film's or a series' slug: its title alone.
fn slug(title: Option<&str>) -> String {
    open_plex_core::slug::slug(title)
}

// The interface fields are written out on each concrete type rather than generated.
// `#[Object]` is an attribute macro, so it is applied to the impl block *before* a
// `macro_rules!` call inside it expands — it would see an opaque token and silently emit a
// type with none of these fields. Verified against the generated SDL, which is the only
// place that mistake is visible.

/// A film: a single playable item with no children.
#[derive(Debug, Clone)]
pub struct Movie(pub MetadataItem);

#[allow(clippy::missing_errors_doc)]
#[Object]
impl Movie {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this item.
    pub async fn id(&self, _context: &Context<'_>) -> Result<ID> {
        Ok(ID(GlobalId::encode(ITEM_TYPE, self.0.id.get())))
    }

    /// Display title, in the library's metadata language. Null for almost every season,
    /// which the source stores without one — use `number` there.
    pub async fn title(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.title.clone())
    }

    /// The item's name for a URL: its title, lowercase, accents folded, words joined by
    /// hyphens. `unknown` when it has no title.
    pub async fn slug(&self, _context: &Context<'_>) -> Result<String> {
        Ok(slug(self.0.title.as_deref()))
    }

    /// Title to sort by when it differs from `title`. Null when `title` already sorts
    /// correctly.
    pub async fn title_sort(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.title_sort.clone())
    }

    /// Identity of the work this item represents, shared by the same film or series
    /// wherever it appears.
    ///
    /// Two libraries holding the same film give two items with different `id`s and the
    /// same `guid` — the case for 78% of the films here, since libraries can share a
    /// scanned folder.
    pub async fn guid(&self, _context: &Context<'_>) -> Result<String> {
        Ok(self.0.guid.clone())
    }

    /// Synopsis, in the library's metadata language.
    pub async fn summary(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.summary.clone())
    }

    /// When the item was added to the library.
    pub async fn added_at(&self, _context: &Context<'_>) -> Result<DateTime> {
        Ok(self.0.added_at.into())
    }

    /// What the caller has watched of the work, shared by every copy of it. Null without a
    /// caller.
    pub async fn watch_state(&self, context: &Context<'_>) -> Result<Option<WatchStateObject>> {
        crate::watch::state_of(context, &self.0.guid).await
    }

    /// The fields an editor locked, which a metadata refresh leaves as they are.
    pub async fn locked_fields(&self, context: &Context<'_>) -> Result<Vec<ItemFieldType>> {
        crate::edit::locked_fields_of(context, self.0.id).await
    }

    /// Where its intro, credits and commercials are, and the places the caller bookmarked in
    /// it, in the order they were added.
    async fn markers(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return markers after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return markers before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many markers from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many markers from the end of the range.")]
        last: Option<i32>,
    ) -> Result<MarkerConnection> {
        crate::marker::markers_of(context, self.0.id, after, before, first, last).await
    }

    /// When the item's metadata last changed.
    pub async fn updated_at(&self, _context: &Context<'_>) -> Result<DateTime> {
        Ok(self.0.updated_at.into())
    }

    /// The library this item belongs to.
    ///
    /// The same work present in two libraries is two distinct items with two ids — 78% of
    /// movies here have such a twin, because libraries can share a scanned folder.
    pub async fn library(&self, context: &Context<'_>) -> Result<Option<Library>> {
        let loader = context.data::<DataLoader<LibraryLoader>>()?;
        Ok(loader.load_one(self.0.section_id).await?.map(Library::new))
    }

    /// Original-language title, when it differs from `title`.
    async fn original_title(&self) -> Option<String> {
        self.0.original_title.clone()
    }

    /// Release year.
    async fn year(&self) -> Option<i32> {
        self.0.year
    }

    /// Release date, as its midnight UTC.
    async fn released_on(&self) -> Option<DateTime> {
        self.0.released_on.map(DateTime::from)
    }

    /// Production studio.
    async fn studio(&self) -> Option<String> {
        self.0.studio.clone()
    }

    /// Marketing tagline.
    async fn tagline(&self) -> Option<String> {
        self.0.tagline.clone()
    }

    /// Age-rating label, as the metadata source expresses it — for example
    /// `fr/Tous publics`. Not comparable across countries.
    async fn content_rating(&self) -> Option<String> {
        self.0.content_rating.clone()
    }

    /// Minimum age the rating implies, when the source states one.
    ///
    /// The comparable form of `contentRating`: the label is country-specific, this is not.
    async fn content_rating_age(&self) -> Option<i32> {
        self.0.content_rating_age
    }

    /// Critic rating out of 10.
    async fn rating(&self) -> Option<f64> {
        self.0.rating
    }

    /// Audience rating out of 10.
    async fn audience_rating(&self) -> Option<f64> {
        self.0.audience_rating
    }

    /// Runtime in milliseconds.
    async fn duration_ms(&self) -> Option<i64> {
        self.0.duration_ms
    }

    /// The available encodings of this film.
    ///
    /// Several when the library holds more than one cut, which is rare: 8 of 18,332 films in
    /// the reference library.
    async fn versions(&self, context: &Context<'_>) -> Result<Vec<Version>> {
        versions_of(context, self.0.id).await
    }

    /// Genres, countries and credits.
    async fn tags(&self) -> ItemTags {
        ItemTags {
            item: self.0.id,
            classified_by: self.0.id,
            with_episodes: false,
        }
    }

    /// Films like this one in its library, best first: shared genres, actors and directors.
    async fn similar(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<ItemConnection> {
        similar_connection(context, &self.0, after, before, first, last).await
    }

    /// The collections it is in, in the order it joined them.
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
        crate::collection::collections_of(context, self.0.id, (after, before, first, last)).await
    }

    /// The film to play once this one ends: the one after it by release date in the first
    /// of its collections where one follows it, watched or not. Null when none does.
    async fn up_next(
        &self,
        context: &Context<'_>,
        #[graphql(
            desc = "Count a collection only when this film is among its first this many, as a shelf showing that many does; what follows it may be past them. Every collection when absent."
        )]
        within: Option<u32>,
    ) -> Result<Option<crate::collection::UpNextInCollection>> {
        crate::collection::next_film(context, &self.0, within).await
    }

    /// Identifiers this item has at external metadata providers.
    async fn external_ids(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
    ) -> Result<ExternalIdConnection> {
        external_ids_of(context, self.0.id, after, before, first, last).await
    }

    /// Scores external providers give this item.
    async fn ratings(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
    ) -> Result<RatingConnection> {
        ratings_of(context, self.0.id, after, before, first, last).await
    }
}

/// A television series. Not playable itself: its episodes are.
#[derive(Debug, Clone)]
pub struct Show(pub MetadataItem);

#[allow(clippy::missing_errors_doc)]
#[Object]
impl Show {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this item.
    pub async fn id(&self, _context: &Context<'_>) -> Result<ID> {
        Ok(ID(GlobalId::encode(ITEM_TYPE, self.0.id.get())))
    }

    /// Display title, in the library's metadata language. Null for almost every season,
    /// which the source stores without one — use `number` there.
    pub async fn title(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.title.clone())
    }

    /// The item's name for a URL: its title, lowercase, accents folded, words joined by
    /// hyphens. `unknown` when it has no title.
    pub async fn slug(&self, _context: &Context<'_>) -> Result<String> {
        Ok(slug(self.0.title.as_deref()))
    }

    /// Title to sort by when it differs from `title`. Null when `title` already sorts
    /// correctly.
    pub async fn title_sort(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.title_sort.clone())
    }

    /// Identity of the work this item represents, shared by the same film or series
    /// wherever it appears.
    ///
    /// Two libraries holding the same film give two items with different `id`s and the
    /// same `guid` — the case for 78% of the films here, since libraries can share a
    /// scanned folder.
    pub async fn guid(&self, _context: &Context<'_>) -> Result<String> {
        Ok(self.0.guid.clone())
    }

    /// Synopsis, in the library's metadata language.
    pub async fn summary(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.summary.clone())
    }

    /// When the item was added to the library.
    pub async fn added_at(&self, _context: &Context<'_>) -> Result<DateTime> {
        Ok(self.0.added_at.into())
    }

    /// What the caller has watched of the work, shared by every copy of it. Null without a
    /// caller.
    pub async fn watch_state(&self, context: &Context<'_>) -> Result<Option<WatchStateObject>> {
        crate::watch::state_of(context, &self.0.guid).await
    }

    /// The fields an editor locked, which a metadata refresh leaves as they are.
    pub async fn locked_fields(&self, context: &Context<'_>) -> Result<Vec<ItemFieldType>> {
        crate::edit::locked_fields_of(context, self.0.id).await
    }

    /// When the item's metadata last changed.
    pub async fn updated_at(&self, _context: &Context<'_>) -> Result<DateTime> {
        Ok(self.0.updated_at.into())
    }

    /// The library this item belongs to.
    ///
    /// The same work present in two libraries is two distinct items with two ids — 78% of
    /// movies here have such a twin, because libraries can share a scanned folder.
    pub async fn library(&self, context: &Context<'_>) -> Result<Option<Library>> {
        let loader = context.data::<DataLoader<LibraryLoader>>()?;
        Ok(loader.load_one(self.0.section_id).await?.map(Library::new))
    }

    /// Year the series first aired.
    async fn year(&self) -> Option<i32> {
        self.0.year
    }

    /// Position among sibling series, as the source records it.
    ///
    /// One for every series in the reference library — nothing orders series the way a
    /// season orders within a show. Published because it is a stored value and clients read
    /// it, not because it discriminates.
    async fn number(&self) -> Option<i32> {
        self.0.position
    }

    /// Production studio or network.
    async fn studio(&self) -> Option<String> {
        self.0.studio.clone()
    }

    /// Age-rating label, as the metadata source expresses it.
    async fn content_rating(&self) -> Option<String> {
        self.0.content_rating.clone()
    }

    /// Minimum age the rating implies, when the source states one.
    async fn content_rating_age(&self) -> Option<i32> {
        self.0.content_rating_age
    }

    /// Original-language title, when it differs from `title`.
    async fn original_title(&self) -> Option<String> {
        self.0.original_title.clone()
    }

    /// Marketing tagline.
    async fn tagline(&self) -> Option<String> {
        self.0.tagline.clone()
    }

    /// Audience rating out of 10.
    ///
    /// No critic rating: the metadata sources publish none for series, and 0 of 4,002 shows
    /// here carries one.
    async fn audience_rating(&self) -> Option<f64> {
        self.0.audience_rating
    }

    /// First-air date, as its midnight UTC.
    async fn released_on(&self) -> Option<DateTime> {
        self.0.released_on.map(DateTime::from)
    }

    /// Typical episode runtime in milliseconds, not the sum over the series.
    async fn duration_ms(&self) -> Option<i64> {
        self.0.duration_ms
    }

    /// The seasons of this series, ordered by season number.
    async fn seasons(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(
            desc = "Discard this many items before the page begins. Prefer cursors: an offset costs the database a traversal of every discarded row, and a concurrent insert shifts it. Provided because a client that renders a scrollbar needs to jump to an absolute position, which no cursor can express."
        )]
        offset: Option<u32>,
    ) -> Result<SeasonConnection> {
        let page = Descent::Children.of(self.0.id, offset);
        children_connection(context, page, after, before, first, last, Season).await
    }

    /// Every episode of this series across its seasons, ordered by season then episode
    /// number.
    ///
    /// One list rather than a walk through `seasons`: a client opening a series shows its
    /// episodes as a single scrollable sequence, and assembling that from one page per
    /// season would leave every page boundary to the client.
    async fn episodes(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(
            desc = "Discard this many items before the page begins. Prefer cursors: an offset costs the database a traversal of every discarded row, and a concurrent insert shifts it. Provided because a client that renders a scrollbar needs to jump to an absolute position, which no cursor can express."
        )]
        offset: Option<u32>,
    ) -> Result<EpisodeConnection> {
        let page = Descent::Grandchildren.of(self.0.id, offset);
        children_connection(context, page, after, before, first, last, Episode).await
    }

    /// Series like this one in its library, best first: shared genres and actors.
    async fn similar(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<ItemConnection> {
        similar_connection(context, &self.0, after, before, first, last).await
    }

    /// Genres, countries and credits — the series' own, and by default those of its
    /// episodes after them.
    async fn tags(
        &self,
        #[graphql(
            desc = "Also list the credits of the episodes, after the series' own.",
            default = true
        )]
        include_episodes: bool,
    ) -> ItemTags {
        ItemTags {
            item: self.0.id,
            classified_by: self.0.id,
            with_episodes: include_episodes,
        }
    }
    /// The collections it is in, in the order it joined them.
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
        crate::collection::collections_of(context, self.0.id, (after, before, first, last)).await
    }

    /// Identifiers this item has at external metadata providers.
    async fn external_ids(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
    ) -> Result<ExternalIdConnection> {
        external_ids_of(context, self.0.id, after, before, first, last).await
    }

    /// Scores external providers give this item.
    async fn ratings(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
    ) -> Result<RatingConnection> {
        ratings_of(context, self.0.id, after, before, first, last).await
    }

    /// Aggregates over the contents of this series.
    async fn stats(&self) -> ShowStats {
        ShowStats(self.0.id)
    }
}

/// Counts over a series, grouped so a client opts into the cost of computing them.
#[derive(Debug, Clone, Copy)]
pub struct ShowStats(RatingKey);

#[allow(clippy::missing_errors_doc)]
#[Object]
impl ShowStats {
    /// How many seasons the series has, without paging through them.
    async fn season_count(&self, context: &Context<'_>) -> Result<u64> {
        let loader = context.data::<DataLoader<ChildCountLoader>>()?;
        Ok(loader.load_one(self.0).await?.unwrap_or(0))
    }

    /// How many episodes the series has across all seasons.
    async fn episode_count(&self, context: &Context<'_>) -> Result<u64> {
        let loader = context.data::<DataLoader<LeafCountLoader>>()?;
        Ok(loader.load_one(self.0).await?.unwrap_or(0))
    }

    /// How many of its episodes the caller has watched to the end. Zero without a caller.
    async fn watched_episode_count(&self, context: &Context<'_>) -> Result<u64> {
        crate::watch::watched_below(context, self.0).await
    }
}

/// A season of a series. Not playable itself: its episodes are.
#[derive(Debug, Clone)]
pub struct Season(pub MetadataItem);

#[allow(clippy::missing_errors_doc)]
#[Object]
impl Season {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this item.
    pub async fn id(&self, _context: &Context<'_>) -> Result<ID> {
        Ok(ID(GlobalId::encode(ITEM_TYPE, self.0.id.get())))
    }

    /// Display title, in the library's metadata language. Null for almost every season,
    /// which the source stores without one — use `number` there.
    pub async fn title(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.title.clone())
    }

    /// The item's name for a URL: its series' title and its number, `bull-s01`.
    pub async fn slug(&self, context: &Context<'_>) -> Result<String> {
        let show = parent_of(context, self.0.parent_id).await?;
        let number = self.0.position.map(|number| format!("s{number:02}"));
        Ok(open_plex_core::slug::slug(
            [show.and_then(|show| show.title), number]
                .iter()
                .flatten()
                .map(String::as_str),
        ))
    }

    /// Title to sort by when it differs from `title`. Null when `title` already sorts
    /// correctly.
    pub async fn title_sort(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.title_sort.clone())
    }

    /// Identity of the work this item represents, shared by the same film or series
    /// wherever it appears.
    ///
    /// Two libraries holding the same film give two items with different `id`s and the
    /// same `guid` — the case for 78% of the films here, since libraries can share a
    /// scanned folder.
    pub async fn guid(&self, _context: &Context<'_>) -> Result<String> {
        Ok(self.0.guid.clone())
    }

    /// Synopsis, in the library's metadata language.
    pub async fn summary(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.summary.clone())
    }

    /// When the item was added to the library.
    pub async fn added_at(&self, _context: &Context<'_>) -> Result<DateTime> {
        Ok(self.0.added_at.into())
    }

    /// What the caller has watched of the work, shared by every copy of it. Null without a
    /// caller.
    pub async fn watch_state(&self, context: &Context<'_>) -> Result<Option<WatchStateObject>> {
        crate::watch::state_of(context, &self.0.guid).await
    }

    /// The fields an editor locked, which a metadata refresh leaves as they are.
    pub async fn locked_fields(&self, context: &Context<'_>) -> Result<Vec<ItemFieldType>> {
        crate::edit::locked_fields_of(context, self.0.id).await
    }

    /// When the item's metadata last changed.
    pub async fn updated_at(&self, _context: &Context<'_>) -> Result<DateTime> {
        Ok(self.0.updated_at.into())
    }

    /// The library this item belongs to.
    ///
    /// The same work present in two libraries is two distinct items with two ids — 78% of
    /// movies here have such a twin, because libraries can share a scanned folder.
    pub async fn library(&self, context: &Context<'_>) -> Result<Option<Library>> {
        let loader = context.data::<DataLoader<LibraryLoader>>()?;
        Ok(loader.load_one(self.0.section_id).await?.map(Library::new))
    }

    /// Season number within the series. Zero conventionally denotes specials.
    async fn number(&self) -> Option<i32> {
        self.0.position
    }

    /// Year the season first aired. Null for most: the sources date the series, not the
    /// season, and 36% of the 9,736 seasons here carry one.
    async fn year(&self) -> Option<i32> {
        self.0.year
    }

    /// The series this season belongs to.
    async fn show(&self, context: &Context<'_>) -> Result<Option<Show>> {
        Ok(parent_of(context, self.0.parent_id).await?.map(Show))
    }

    /// The episodes of this season, ordered by episode number.
    async fn episodes(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(
            desc = "Discard this many items before the page begins. Prefer cursors: an offset costs the database a traversal of every discarded row, and a concurrent insert shifts it. Provided because a client that renders a scrollbar needs to jump to an absolute position, which no cursor can express."
        )]
        offset: Option<u32>,
    ) -> Result<EpisodeConnection> {
        let page = Descent::Children.of(self.0.id, offset);
        children_connection(context, page, after, before, first, last, Episode).await
    }

    /// Genres and countries, which are the series', and the credits of the season's
    /// episodes.
    async fn tags(
        &self,
        #[graphql(
            desc = "Also list the credits of the episodes, after the season's own.",
            default = true
        )]
        include_episodes: bool,
    ) -> ItemTags {
        ItemTags {
            item: self.0.id,
            classified_by: self.0.parent_id.unwrap_or(self.0.id),
            with_episodes: include_episodes,
        }
    }
    /// The collections it is in, in the order it joined them.
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
        crate::collection::collections_of(context, self.0.id, (after, before, first, last)).await
    }

    /// Identifiers this item has at external metadata providers.
    async fn external_ids(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
    ) -> Result<ExternalIdConnection> {
        external_ids_of(context, self.0.id, after, before, first, last).await
    }

    /// Scores external providers give this item.
    async fn ratings(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
    ) -> Result<RatingConnection> {
        ratings_of(context, self.0.id, after, before, first, last).await
    }

    /// Aggregates over the contents of this season.
    async fn stats(&self) -> SeasonStats {
        SeasonStats(self.0.id)
    }
}

/// Counts over a season.
#[derive(Debug, Clone, Copy)]
pub struct SeasonStats(RatingKey);

#[allow(clippy::missing_errors_doc)]
#[Object]
impl SeasonStats {
    /// How many episodes the season has, without paging through them.
    async fn episode_count(&self, context: &Context<'_>) -> Result<u64> {
        let loader = context.data::<DataLoader<ChildCountLoader>>()?;
        Ok(loader.load_one(self.0).await?.unwrap_or(0))
    }

    /// How many of its episodes the caller has watched to the end. Zero without a caller.
    async fn watched_episode_count(&self, context: &Context<'_>) -> Result<u64> {
        crate::watch::watched_below(context, self.0).await
    }
}

/// An episode of a series.
#[derive(Debug, Clone)]
pub struct Episode(pub MetadataItem);

#[allow(clippy::missing_errors_doc)]
#[Object]
impl Episode {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this item.
    pub async fn id(&self, _context: &Context<'_>) -> Result<ID> {
        Ok(ID(GlobalId::encode(ITEM_TYPE, self.0.id.get())))
    }

    /// Display title, in the library's metadata language. Null for almost every season,
    /// which the source stores without one — use `number` there.
    pub async fn title(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.title.clone())
    }

    /// The item's name for a URL: its series' title then its own,
    /// `bull-good-news-about-hell`. An episode without a title is named by its numbers,
    /// `bull-s01e02`, and one with neither is `unknown`.
    pub async fn slug(&self, context: &Context<'_>) -> Result<String> {
        let season = parent_of(context, self.0.parent_id).await?;
        let show = match &season {
            Some(season) => parent_of(context, season.parent_id).await?,
            None => None,
        };
        let own = self.0.title.clone().or_else(|| {
            let season = season.as_ref()?.position?;
            Some(format!("s{season:02}e{:02}", self.0.position?))
        });
        Ok(open_plex_core::slug::slug(
            [show.and_then(|show| show.title), own]
                .iter()
                .flatten()
                .map(String::as_str),
        ))
    }

    /// Title to sort by when it differs from `title`. Null when `title` already sorts
    /// correctly.
    pub async fn title_sort(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.title_sort.clone())
    }

    /// Identity of the work this item represents, shared by the same film or series
    /// wherever it appears.
    ///
    /// Two libraries holding the same film give two items with different `id`s and the
    /// same `guid` — the case for 78% of the films here, since libraries can share a
    /// scanned folder.
    pub async fn guid(&self, _context: &Context<'_>) -> Result<String> {
        Ok(self.0.guid.clone())
    }

    /// Synopsis, in the library's metadata language.
    pub async fn summary(&self, _context: &Context<'_>) -> Result<Option<String>> {
        Ok(self.0.summary.clone())
    }

    /// When the item was added to the library.
    pub async fn added_at(&self, _context: &Context<'_>) -> Result<DateTime> {
        Ok(self.0.added_at.into())
    }

    /// What the caller has watched of the work, shared by every copy of it. Null without a
    /// caller.
    pub async fn watch_state(&self, context: &Context<'_>) -> Result<Option<WatchStateObject>> {
        crate::watch::state_of(context, &self.0.guid).await
    }

    /// The fields an editor locked, which a metadata refresh leaves as they are.
    pub async fn locked_fields(&self, context: &Context<'_>) -> Result<Vec<ItemFieldType>> {
        crate::edit::locked_fields_of(context, self.0.id).await
    }

    /// Where its intro, credits and commercials are, and the places the caller bookmarked in
    /// it, in the order they were added.
    async fn markers(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return markers after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return markers before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many markers from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many markers from the end of the range.")]
        last: Option<i32>,
    ) -> Result<MarkerConnection> {
        crate::marker::markers_of(context, self.0.id, after, before, first, last).await
    }

    /// When the item's metadata last changed.
    pub async fn updated_at(&self, _context: &Context<'_>) -> Result<DateTime> {
        Ok(self.0.updated_at.into())
    }

    /// The library this item belongs to.
    ///
    /// The same work present in two libraries is two distinct items with two ids — 78% of
    /// movies here have such a twin, because libraries can share a scanned folder.
    pub async fn library(&self, context: &Context<'_>) -> Result<Option<Library>> {
        let loader = context.data::<DataLoader<LibraryLoader>>()?;
        Ok(loader.load_one(self.0.section_id).await?.map(Library::new))
    }

    /// Episode number within its season.
    async fn number(&self) -> Option<i32> {
        self.0.position
    }

    /// The season this episode belongs to.
    async fn season(&self, context: &Context<'_>) -> Result<Option<Season>> {
        Ok(parent_of(context, self.0.parent_id).await?.map(Season))
    }

    /// The series this episode belongs to, reachable directly rather than through the season.
    ///
    /// Rendering an episode row needs the series title, and this costs no more than going via
    /// the season: storage holds only the direct parent either way, so both are two batched
    /// lookups.
    async fn show(&self, context: &Context<'_>) -> Result<Option<Show>> {
        let Some(season) = parent_of(context, self.0.parent_id).await? else {
            return Ok(None);
        };
        Ok(parent_of(context, season.parent_id).await?.map(Show))
    }

    /// Original air date, as its midnight UTC.
    async fn released_on(&self) -> Option<DateTime> {
        self.0.released_on.map(DateTime::from)
    }

    /// Age-rating label, as the metadata source expresses it — the series' when the episode
    /// has none of its own.
    ///
    /// Sources rate the series and rarely the episode; an episode that says nothing is rated
    /// as its series is. Measured: the reference reports the series' label for 14 of 15
    /// episodes stored without one.
    async fn content_rating(&self, context: &Context<'_>) -> Result<Option<String>> {
        if self.0.content_rating.is_some() {
            return Ok(self.0.content_rating.clone());
        }
        let Some(season) = parent_of(context, self.0.parent_id).await? else {
            return Ok(None);
        };
        Ok(parent_of(context, season.parent_id)
            .await?
            .and_then(|show| show.content_rating))
    }

    /// Original-language title, when it differs from `title`.
    async fn original_title(&self) -> Option<String> {
        self.0.original_title.clone()
    }

    /// Year the episode first aired.
    async fn year(&self) -> Option<i32> {
        self.0.year
    }

    /// Audience rating out of 10.
    ///
    /// No critic rating, no studio and no tagline: the sources populate none of the three
    /// for episodes, and exposing an always-null field only invites the client to ask.
    async fn audience_rating(&self) -> Option<f64> {
        self.0.audience_rating
    }

    /// Runtime in milliseconds.
    async fn duration_ms(&self) -> Option<i64> {
        self.0.duration_ms
    }

    /// The available encodings of this episode.
    async fn versions(&self, context: &Context<'_>) -> Result<Vec<Version>> {
        versions_of(context, self.0.id).await
    }

    /// Genres and countries, which are the series', and the episode's own credits.
    async fn tags(&self, context: &Context<'_>) -> Result<ItemTags> {
        let show = match parent_of(context, self.0.parent_id).await? {
            Some(season) => season.parent_id,
            None => None,
        };
        Ok(ItemTags {
            item: self.0.id,
            classified_by: show.unwrap_or(self.0.id),
            with_episodes: false,
        })
    }

    /// The collections it is in, in the order it joined them.
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
        crate::collection::collections_of(context, self.0.id, (after, before, first, last)).await
    }

    /// What to play once this episode ends: its series' entry in `continueWatching`, unless
    /// that entry is this episode — one still in progress, or the next of a series whose
    /// state has not moved yet. Not the episode after this one: every episode of a series
    /// watched up to its fourth offers its fifth. Null without a caller, and for a series
    /// not played in the last sixteen weeks or with nothing left to play.
    async fn up_next(&self, context: &Context<'_>) -> Result<Option<Self>> {
        Ok(crate::watch::after_episode(context, &self.0)
            .await?
            .map(Self))
    }

    /// Identifiers this item has at external metadata providers.
    async fn external_ids(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
    ) -> Result<ExternalIdConnection> {
        external_ids_of(context, self.0.id, after, before, first, last).await
    }

    /// Scores external providers give this item.
    async fn ratings(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
    ) -> Result<RatingConnection> {
        ratings_of(context, self.0.id, after, before, first, last).await
    }
}

async fn versions_of(context: &Context<'_>, item: RatingKey) -> Result<Vec<Version>> {
    let loader = context.data::<DataLoader<VersionsLoader>>()?;
    Ok(loader
        .load_one(item)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(Version)
        .collect())
}

async fn parent_of(
    context: &Context<'_>,
    parent: Option<RatingKey>,
) -> Result<Option<MetadataItem>> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    let loader = context.data::<DataLoader<ItemLoader>>()?;
    Ok(loader.load_one(parent).await?)
}

/// How far below a parent a connection reaches.
#[derive(Debug, Clone, Copy)]
enum Descent {
    Children,
    Grandchildren,
}

impl Descent {
    const fn of(self, parent: RatingKey, offset: Option<u32>) -> ChildPage {
        ChildPage {
            parent,
            descent: self,
            skip: offset,
        }
    }
}

/// Which rows a child connection pages over, before Relay's window is applied.
#[derive(Debug, Clone, Copy)]
struct ChildPage {
    parent: RatingKey,
    descent: Descent,
    skip: Option<u32>,
}

/// One page below a parent, wrapped as the concrete type the caller wants.
///
/// Not batched across parents, unlike the loaders: each parent carries its own window, and a
/// window cannot be shared. A client browsing a series fetches one, which is the shape this
/// is for.
async fn children_connection<T, F>(
    context: &Context<'_>,
    page: ChildPage,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
    wrap: F,
) -> Result<Connection<OpaqueCursor, T, EmptyFields, EmptyFields>>
where
    T: async_graphql::OutputType,
    F: Fn(MetadataItem) -> T + Send + Sync,
{
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = crate::schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                .skipping(u64::from(page.skip.unwrap_or(0)));
            let metadata = crate::schema::metadata(context)?;
            let page = match page.descent {
                Descent::Children => metadata.children(page.parent, window).await?,
                Descent::Grandchildren => metadata.grandchildren(page.parent, window).await?,
            };

            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection.edges.extend(page.items.into_iter().map(|item| {
                // Children are ordered by their number, so the id alone locates a position.
                Edge::new(OpaqueCursor(Cursor::by_id(item.id.get())), wrap(item))
            }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// Scores then pages in memory, the score needing every candidate anyway.
async fn similar_connection(
    context: &Context<'_>,
    item: &MetadataItem,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<ItemConnection> {
    let metadata = crate::schema::metadata(context)?;
    let ids = open_plex_core::similar::similar_items(metadata, item, MAX_ITEMS as usize).await?;
    let loaded = context
        .data::<DataLoader<ItemLoader>>()?
        .load_many(ids.iter().copied())
        .await?;
    let rows = ids
        .iter()
        .filter_map(|id| Some((Item::from_domain(loaded.get(id).cloned()?), EmptyFields)))
        .collect();
    crate::tag::page_in_memory(rows, after, before, first, last).await
}
