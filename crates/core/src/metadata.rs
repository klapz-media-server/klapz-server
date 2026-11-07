//! Library contents: items, their media, and the queries bindings run against them.

use crate::ids::{MediaId, PartId, RatingKey, SectionId, StreamId, TagId};
use crate::page::{CursorKey, Direction, Page, Window};

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("no item with id {0}")]
    ItemNotFound(RatingKey),
    /// A cursor that cannot locate a position under the requested ordering — a client
    /// mistake, typically reusing a cursor after changing the sort field.
    #[error("the cursor does not match the requested ordering")]
    CursorMismatch,
    /// Storage failed. Boxed so the cause chain survives for the operator's logs without
    /// `core` knowing which engine is in use.
    #[error("metadata storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// What an item is.
///
/// The four kinds share one storage table and one hierarchy, but they are not
/// interchangeable: a show has children and no media, a movie the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Movie,
    Show,
    Season,
    Episode,
}

impl ItemKind {
    /// Whether items of this kind carry media.
    ///
    /// Measured on the reference library: every movie and episode has media, no show or
    /// season does.
    pub const fn is_playable(self) -> bool {
        matches!(self, Self::Movie | Self::Episode)
    }
}

/// An item in a library.
///
/// One struct for all four kinds, because they share a table and the bindings are what
/// present them as distinct types. Fields that only apply to some kinds are optional rather
/// than split out — `studio` is set on 61% of movies and no episodes.
#[derive(Debug, Clone)]
pub struct MetadataItem {
    pub id: RatingKey,
    pub section_id: SectionId,
    /// The season for an episode, the show for a season, `None` for a movie or a show.
    pub parent_id: Option<RatingKey>,
    pub kind: ItemKind,
    /// Identity of the work, shared by the same film in several libraries.
    pub guid: String,
    /// Absent for almost every season: the source stores none, and a display name is
    /// localized presentation that belongs to a binding.
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub original_title: Option<String>,
    pub summary: Option<String>,
    pub tagline: Option<String>,
    pub studio: Option<String>,
    pub content_rating: Option<String>,
    pub content_rating_age: Option<i32>,
    /// Episode or season number within its parent.
    pub position: Option<i32>,
    pub year: Option<i32>,
    /// Calendar date, `YYYY-MM-DD`.
    pub released_on: Option<String>,
    pub duration_ms: Option<i64>,
    pub rating: Option<f64>,
    pub audience_rating: Option<f64>,
    pub added_at: i64,
    pub updated_at: i64,
}

/// One encoding of an item.
#[derive(Debug, Clone)]
pub struct MediaVersion {
    pub id: MediaId,
    pub item_id: RatingKey,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Measured display aspect ratio, e.g. 2.3762. Not snapped to a standard ratio.
    pub aspect_ratio: Option<f64>,
    pub duration_ms: Option<i64>,
    /// Bits per second, as the source measured it — not kilobits.
    pub bitrate: Option<i32>,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i32>,
    /// Frames per second as measured, e.g. 23.976. Turning that into a label like `24p` is a
    /// wire concern.
    pub frame_rate: Option<f64>,
    pub interlaced: bool,
    pub optimized_for_streaming: bool,
}

/// One file on disk.
#[derive(Debug, Clone)]
pub struct MediaFile {
    pub id: PartId,
    pub version_id: MediaId,
    pub position: i32,
    /// Absolute path on the server's filesystem.
    ///
    /// Published by both surfaces, as Plex does. What that does *not* change: playback still
    /// resolves a file from its id and never from a client-supplied path. A path the server
    /// hands out is reference data; a path a client hands back is an attack surface.
    pub path: String,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
}

/// One track inside a file, or a sidecar subtitle next to it.
#[derive(Debug, Clone)]
pub struct MediaStream {
    pub id: StreamId,
    pub file_id: PartId,
    pub kind: StreamKind,
    /// The index the transcoder addresses this track by.
    pub position: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<i32>,
    /// Bits per second, as the source measured it — not kilobits.
    pub bitrate: Option<i32>,
    pub is_default: bool,
    pub is_forced: bool,
    /// Set when the track lives in its own file.
    pub external_path: Option<String>,
}

/// What a tag classifies or credits.
///
/// The six the reference server publishes on an item and filters by. Collections are not
/// among them: a collection is an item of its own, with its own endpoints, and treating it
/// as a label would leave nowhere to put its artwork and summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagKind {
    Genre,
    Country,
    Director,
    Writer,
    Actor,
    Producer,
}

impl TagKind {
    /// Whether tags of this kind name a person.
    ///
    /// Only people carry a provider key and a portrait, and two of them may share a name:
    /// the reference library holds 11,879 actor names belonging to more than one person.
    pub const fn is_person(self) -> bool {
        !matches!(self, Self::Genre | Self::Country)
    }
}

/// A genre, a country, or a person in one credit role.
///
/// A person credited in two roles is two tags — Luc Besson the writer and Luc Besson the
/// producer are distinct rows with distinct ids, as they are in the source — because what a
/// filter asks for is "films he wrote", not "films he touched".
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub id: TagId,
    pub kind: TagKind,
    pub name: String,
    /// The metadata provider's identifier for a person, which tells homonyms apart.
    pub provider_key: Option<String>,
    /// Portrait of a person, hosted by the metadata provider.
    pub thumb_url: Option<String>,
}

/// A tag as attached to one item.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemTag {
    pub item_id: RatingKey,
    pub tag: Tag,
    /// The character an actor plays. Never set for any other kind.
    pub role: Option<String>,
}

/// Which tag a filter names: by identifier, or by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagRef {
    Id(TagId),
    /// Matched exactly, ignoring ASCII case only — see [`tag_name_key`].
    Name(String),
}

/// Keeps items carrying at least one of the listed tags of one kind.
///
/// Several conditions combine with `AND`, the values inside one with `OR`: "a comedy *and* a
/// drama" is two conditions, "a comedy *or* a drama" is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCondition {
    pub kind: TagKind,
    pub any_of: Vec<TagRef>,
}

impl TagCondition {
    /// How many tags one condition may name.
    ///
    /// The list comes from the client and each value becomes a bound parameter, so it needs
    /// a ceiling like any other client-sized input. Fifty is far past what a filter UI
    /// builds and far below what an engine's parameter limit tolerates.
    pub const MAX_VALUES: usize = 50;
}

/// The form a tag name is matched in.
///
/// ASCII case folded and nothing else, which is the reference server's rule: `genre=ACTION`
/// finds Action, `genre=COMÉDIE` does not find Comédie, and neither accents nor surrounding
/// spaces are forgiven. It is `SQLite`'s `NOCASE`, made explicit so that an engine whose
/// `LOWER` understands Unicode does not quietly match more.
pub fn tag_name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// How a listing is ordered.
///
/// Deliberately short. Every variant here is backed by a `NOT NULL` column, which is what
/// lets a cursor locate a position unambiguously. Ordering by a nullable column — year,
/// release date, rating — additionally requires encoding "the value is absent" in the cursor
/// and pinning where nulls sort; that is worth doing, and is not done yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemSort {
    /// By sort title, falling back to the title when there is none.
    #[default]
    Title,
    /// By when the item was added to the library.
    AddedAt,
}

/// Narrows a listing to a subset of a library.
///
/// Every field is an `AND`: a filter with a kind and a year range matches items satisfying
/// both. Absent fields place no constraint.
///
/// Filtering on a nullable column is fine — an item with no year simply fails a year
/// predicate. That is unlike *ordering* by one, which additionally needs the cursor to encode
/// absence, and is why [`ItemSort`] is shorter than this struct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemFilter {
    /// Restricts to one kind. `None` lists every kind the library holds.
    pub kind: Option<ItemKind>,
    /// How deep into the library's hierarchy to look.
    pub depth: ItemDepth,
    /// Earliest release year, inclusive.
    pub year_from: Option<i32>,
    /// Latest release year, inclusive.
    pub year_to: Option<i32>,
    /// Exact age-rating label, as the metadata source expresses it.
    pub content_rating: Option<String>,
    /// Keeps only items whose studio *contains* this text, case-insensitively.
    ///
    /// A substring and not an exact name, because that is what the reference server does:
    /// `studio=Films` matches 992 of 9,667 films, which is every studio with "Films"
    /// anywhere in it, not the 14 whose name starts with it. Folding is ASCII-only.
    ///
    /// The one filter besides `min_width` that cannot use an index: a leading wildcard has
    /// nothing to seek to. It is applied after the indexed section predicate, so the scan is
    /// bounded by the section rather than by the table.
    pub studio_contains: Option<String>,
    /// Keeps only items having a word that starts with this text, in their title, sort
    /// title or original title. Accents are folded.
    ///
    /// A word prefix and not a column prefix, because that is what the reference server
    /// does: `le` finds "Le Nouveau Jouet" (whose sort title opens with `[CAM]`) and
    /// "Sans Laisser de Traces" alike. Served by a word index, not by the title column —
    /// see `search` for the tokenizer the index and this filter must share.
    pub title_word_prefix: Option<String>,
    /// Keeps only items whose media is at least this wide, in pixels.
    ///
    /// A resolution filter is a property of the file rather than the item, so it is a join
    /// rather than a column — which is why it is the one filter that can slow a listing.
    pub min_width: Option<i32>,
    /// Keeps only items carrying the named tags. Each condition must hold.
    pub tags: Vec<TagCondition>,
}

impl ItemFilter {
    /// Whether anything here reaches past the item table.
    pub const fn needs_media(&self) -> bool {
        self.min_width.is_some()
    }

    /// Whether to restrict the listing to items with no parent.
    ///
    /// Naming a kind overrides the depth, because it is already a statement about depth: a
    /// caller asking for episodes wants episodes, and answering nothing because episodes
    /// have parents would be obtuse.
    pub const fn top_level_only(&self) -> bool {
        matches!(self.depth, ItemDepth::TopLevel) && self.kind.is_none()
    }
}

/// How deep into a library's hierarchy a listing reaches.
///
/// A library of series holds three generations in one table — 1,980 shows, 5,157 seasons and
/// 79,518 episodes in the reference library — and listing all of them flat is almost never
/// what a caller wants. The top level is what a client puts on a library's front page, and
/// the rest is reached by navigating into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemDepth {
    /// Items with no parent: the films of a film library, the series of a series library.
    #[default]
    TopLevel,
    /// Every item in the library, at any depth.
    All,
}

/// One letter of a library's A–Z jump bar, with how many items fall under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstCharacter {
    /// The bucket: an uppercase letter, or `#` for everything that does not start with one.
    pub character: String,
    pub count: u64,
}

/// Which bucket a sort key falls into.
///
/// Folded first, so `Çélèbre` lands under `C` — matching the reference server, which reports
/// 594 films under `C` where a raw first character gives 593. Ligatures are not folded and so
/// get their own bucket, which is also what the reference does: `Œ` is a letter there, with
/// two films under it.
///
/// Everything that does not start with a letter — digits, `#`, `[` — collapses into `#`, the
/// single bucket the reference opens its list with.
pub fn first_character_bucket(sort_key: &str) -> String {
    let folded = crate::search::fold(sort_key);
    match folded.chars().next() {
        Some(first) if first.is_alphabetic() => first.to_uppercase().collect(),
        _ => "#".to_owned(),
    }
}

/// Which items a listing covers, and in what order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemQuery {
    pub section_id: SectionId,
    pub filter: ItemFilter,
    pub sort: ItemSort,
    pub direction: Direction,
}

impl ItemQuery {
    pub fn new(section_id: SectionId) -> Self {
        Self {
            section_id,
            filter: ItemFilter::default(),
            sort: ItemSort::Title,
            direction: Direction::Ascending,
        }
    }

    #[must_use]
    pub fn of_kind(mut self, kind: ItemKind) -> Self {
        self.filter.kind = Some(kind);
        self
    }

    #[must_use]
    pub const fn sorted_by(mut self, sort: ItemSort, direction: Direction) -> Self {
        self.sort = sort;
        self.direction = direction;
        self
    }

    /// Sets the title prefix, treating blank input as no filter.
    ///
    /// An empty search box must not become a constraint that matches everything.
    #[must_use]
    pub fn matching(mut self, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        self.filter.title_word_prefix = (!prefix.trim().is_empty()).then_some(prefix);
        self
    }

    #[must_use]
    pub fn filtered(mut self, filter: ItemFilter) -> Self {
        self.filter = filter;
        self
    }

    /// The cursor key locating `item` under this query's ordering.
    ///
    /// Lives here rather than in storage because the sort field and the cursor shape have to
    /// agree, and keeping both in one place is what makes that checkable.
    pub fn cursor_key(&self, item: &MetadataItem) -> CursorKey {
        match self.sort {
            // Lowercased to match the stored ordering column: the listing sorts
            // case-insensitively, so a cursor built from the raw title would not line up.
            ItemSort::Title => CursorKey::Text(
                item.title_sort
                    .as_ref()
                    .or(item.title.as_ref())
                    .map(|text| text.to_lowercase())
                    .unwrap_or_default(),
            ),
            ItemSort::AddedAt => CursorKey::Int(item.added_at),
        }
    }
}

/// A search across every library.
///
/// Separate from [`ItemQuery`], which is always rooted in one library: a global search is a
/// different question and would otherwise mean an optional section id that every caller has
/// to think about.
///
/// Ordered by sort title. The reference server orders by a relevance score it computes from
/// its own index; we have no score, and a title ordering at least groups the four films named
/// *Avatar* together and gives the cursor something stable to resume from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Matches items having a word beginning with this text. See [`crate::search`].
    pub word_prefix: String,
    /// Restricts to one kind. `None` searches every kind, which is what the reference does —
    /// its results mix films, series and episodes.
    pub kind: Option<ItemKind>,
}

impl SearchQuery {
    pub fn new(word_prefix: impl Into<String>) -> Self {
        Self {
            word_prefix: word_prefix.into(),
            kind: None,
        }
    }

    #[must_use]
    pub const fn of_kind(mut self, kind: ItemKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Whether the search has anything to match.
    ///
    /// A blank term is not an error and not a match-everything: a client clearing its search
    /// box must get nothing back rather than the whole server.
    pub fn is_blank(&self) -> bool {
        crate::search::fold(&self.word_prefix).trim().is_empty()
    }

    /// The cursor position of an item under this ordering.
    pub fn cursor_key(&self, item: &MetadataItem) -> CursorKey {
        CursorKey::Text(
            item.title_sort
                .as_ref()
                .or(item.title.as_ref())
                .map(|text| text.to_lowercase())
                .unwrap_or_default(),
        )
    }
}

/// Storage port for library contents.
///
/// Split from `LibraryRepository` because they are different aggregates: one is a handful of
/// rows describing structure, the other is the hundreds of thousands of rows inside it.
#[async_trait::async_trait]
pub trait MetadataRepository: Send + Sync + std::fmt::Debug {
    /// One page of a library's items.
    async fn items(
        &self,
        query: &ItemQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError>;

    /// How many items the query matches.
    async fn item_count(&self, query: &ItemQuery) -> Result<u64, MetadataError>;

    /// One page of a search across every library.
    ///
    /// Ordered by sort title, so the cursor machinery is the one the listing already uses.
    async fn search(
        &self,
        query: &SearchQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError>;

    /// The first-letter buckets of a query's items, with their counts.
    ///
    /// One row per letter rather than per item: the A–Z bar a client draws beside a long list
    /// needs 28 numbers, and paging 9,667 items to count them would be absurd.
    async fn first_characters(
        &self,
        query: &ItemQuery,
    ) -> Result<Vec<FirstCharacter>, MetadataError>;

    async fn item(&self, id: RatingKey) -> Result<MetadataItem, MetadataError>;

    /// Several items at once, for the loaders that resolve a parent from a child.
    ///
    /// Items that do not exist are absent from the result rather than an error: a loader asks
    /// about ids it read from other rows, and a dangling reference is a data problem to
    /// report, not a request to reject.
    async fn items_by_id(&self, ids: &[RatingKey]) -> Result<Vec<MetadataItem>, MetadataError>;

    /// One page of a parent's children, ordered by their position.
    ///
    /// Not batched across parents, unlike the loaders below: each parent carries its own
    /// window, and a window cannot be shared. Listing fifty shows and each one's seasons is
    /// therefore fifty queries — an unusual shape, since a client browsing a series fetches
    /// one.
    async fn children(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError>;

    /// One page of a parent's grandchildren — every episode of a series, across its
    /// seasons — ordered by the child's position, then their own.
    ///
    /// A separate port rather than `children` applied twice: a client opening a series
    /// wants its episodes as one list, and walking each season in turn would cost a query
    /// per season and leave the page boundary to the caller.
    async fn grandchildren(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError>;

    /// How many direct children each parent has.
    async fn child_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError>;

    /// How many descendants two levels down each parent has — a show's episodes.
    async fn leaf_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError>;

    async fn versions_for(&self, items: &[RatingKey]) -> Result<Vec<MediaVersion>, MetadataError>;

    async fn files_for(&self, versions: &[MediaId]) -> Result<Vec<MediaFile>, MetadataError>;

    async fn streams_for(&self, files: &[PartId]) -> Result<Vec<MediaStream>, MetadataError>;

    /// One encoding by id, for a Relay refetch.
    async fn version(&self, id: MediaId) -> Result<Option<MediaVersion>, MetadataError>;

    /// One file by id, for a Relay refetch and for resolving a path before playback.
    async fn file(&self, id: PartId) -> Result<Option<MediaFile>, MetadataError>;

    /// One track by id, for a Relay refetch.
    async fn stream(&self, id: StreamId) -> Result<Option<MediaStream>, MetadataError>;

    /// Every tag of several items, in billing order within each item and kind.
    ///
    /// The order *across* kinds is unspecified: a consumer groups by kind, and promising one
    /// would constrain storage for nothing.
    ///
    /// All kinds at once: an item page shows genres, cast and crew together, and one query
    /// for the six is cheaper than six for one each.
    async fn tags_for(&self, items: &[RatingKey]) -> Result<Vec<ItemTag>, MetadataError>;

    /// One tag by id, for a Relay refetch.
    async fn tag(&self, id: TagId) -> Result<Option<Tag>, MetadataError>;
}

/// Lets an `Arc<dyn MetadataRepository>` be used wherever a repository is expected, so the
/// bindings can hold a trait object instead of being generic over the storage type.
#[async_trait::async_trait]
impl<T: MetadataRepository + ?Sized> MetadataRepository for std::sync::Arc<T> {
    async fn items(
        &self,
        query: &ItemQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        (**self).items(query, window).await
    }

    async fn item_count(&self, query: &ItemQuery) -> Result<u64, MetadataError> {
        (**self).item_count(query).await
    }

    async fn search(
        &self,
        query: &SearchQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        (**self).search(query, window).await
    }

    async fn first_characters(
        &self,
        query: &ItemQuery,
    ) -> Result<Vec<FirstCharacter>, MetadataError> {
        (**self).first_characters(query).await
    }

    async fn item(&self, id: RatingKey) -> Result<MetadataItem, MetadataError> {
        (**self).item(id).await
    }

    async fn items_by_id(&self, ids: &[RatingKey]) -> Result<Vec<MetadataItem>, MetadataError> {
        (**self).items_by_id(ids).await
    }

    async fn children(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        (**self).children(parent, window).await
    }

    async fn grandchildren(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        (**self).grandchildren(parent, window).await
    }

    async fn child_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        (**self).child_counts(parents).await
    }

    async fn leaf_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        (**self).leaf_counts(parents).await
    }

    async fn versions_for(&self, items: &[RatingKey]) -> Result<Vec<MediaVersion>, MetadataError> {
        (**self).versions_for(items).await
    }

    async fn files_for(&self, versions: &[MediaId]) -> Result<Vec<MediaFile>, MetadataError> {
        (**self).files_for(versions).await
    }

    async fn streams_for(&self, files: &[PartId]) -> Result<Vec<MediaStream>, MetadataError> {
        (**self).streams_for(files).await
    }

    async fn version(&self, id: MediaId) -> Result<Option<MediaVersion>, MetadataError> {
        (**self).version(id).await
    }

    async fn file(&self, id: PartId) -> Result<Option<MediaFile>, MetadataError> {
        (**self).file(id).await
    }

    async fn stream(&self, id: StreamId) -> Result<Option<MediaStream>, MetadataError> {
        (**self).stream(id).await
    }

    async fn tags_for(&self, items: &[RatingKey]) -> Result<Vec<ItemTag>, MetadataError> {
        (**self).tags_for(items).await
    }

    async fn tag(&self, id: TagId) -> Result<Option<Tag>, MetadataError> {
        (**self).tag(id).await
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn item(title: Option<&str>, title_sort: Option<&str>, added_at: i64) -> MetadataItem {
        MetadataItem {
            id: RatingKey::new(1),
            section_id: SectionId::new(4),
            parent_id: None,
            kind: ItemKind::Movie,
            guid: "plex://movie/abc".to_owned(),
            title: title.map(ToOwned::to_owned),
            title_sort: title_sort.map(ToOwned::to_owned),
            original_title: None,
            summary: None,
            tagline: None,
            studio: None,
            content_rating: None,
            content_rating_age: None,
            position: None,
            year: None,
            released_on: None,
            duration_ms: None,
            rating: None,
            audience_rating: None,
            added_at,
            updated_at: 0,
        }
    }

    #[test]
    fn only_movies_and_episodes_are_playable() {
        // The schema relies on this: shows and seasons expose no media field at all.
        assert!(ItemKind::Movie.is_playable());
        assert!(ItemKind::Episode.is_playable());
        assert!(!ItemKind::Show.is_playable());
        assert!(!ItemKind::Season.is_playable());
    }

    #[test]
    fn a_title_cursor_prefers_the_sort_title() {
        let query = ItemQuery::new(SectionId::new(4));
        let key = query.cursor_key(&item(Some("The Matrix"), Some("Matrix, The"), 0));
        // Folded, because the stored ordering column is folded. A cursor carrying the raw
        // case would not line up with it, and pagination would skip rows.
        assert_eq!(key, CursorKey::Text("matrix, the".to_owned()));
    }

    #[test]
    fn a_title_cursor_falls_back_to_the_title() {
        let query = ItemQuery::new(SectionId::new(4));
        let key = query.cursor_key(&item(Some("Avatar"), None, 0));
        assert_eq!(key, CursorKey::Text("avatar".to_owned()));
    }

    #[test]
    fn a_title_cursor_folds_case_so_it_matches_the_stored_ordering() {
        // Two spellings of the same title must produce the same cursor, or paging past one
        // would skip the other.
        let query = ItemQuery::new(SectionId::new(4));
        assert_eq!(
            query.cursor_key(&item(Some("ALIEN"), None, 0)),
            query.cursor_key(&item(Some("alien"), None, 0))
        );
    }

    #[test]
    fn a_title_cursor_on_a_titleless_item_is_empty_rather_than_absent() {
        // Seasons have no title, and they are ordered by position rather than by title — but
        // the cursor must still be constructible.
        let query = ItemQuery::new(SectionId::new(4));
        assert_eq!(
            query.cursor_key(&item(None, None, 0)),
            CursorKey::Text(String::new())
        );
    }

    #[test]
    fn an_added_at_cursor_carries_the_timestamp() {
        let query =
            ItemQuery::new(SectionId::new(4)).sorted_by(ItemSort::AddedAt, Direction::Descending);
        assert_eq!(
            query.cursor_key(&item(Some("Avatar"), None, 1_655_072_133)),
            CursorKey::Int(1_655_072_133)
        );
    }

    #[test]
    fn a_tag_name_folds_ascii_case_and_nothing_else() {
        // Each half measured on the reference: `genre=ACTION` finds Action, while
        // `genre=COMÉDIE` and `genre=Comedie` both miss Comédie.
        assert_eq!(tag_name_key("ACTION"), tag_name_key("Action"));
        assert_ne!(tag_name_key("COMÉDIE"), tag_name_key("Comédie"));
        assert_ne!(tag_name_key("Comedie"), tag_name_key("Comédie"));
        assert_ne!(tag_name_key(" Action"), tag_name_key("Action"));
    }

    #[test]
    fn only_people_are_people() {
        assert!(TagKind::Actor.is_person());
        assert!(TagKind::Producer.is_person());
        assert!(!TagKind::Genre.is_person());
        assert!(!TagKind::Country.is_person());
    }

    #[test]
    fn a_blank_search_is_no_search() {
        // An empty box in a UI must not become a constraint that matches everything.
        assert_eq!(
            ItemQuery::new(SectionId::new(4))
                .matching("   ")
                .filter
                .title_word_prefix,
            None
        );
        assert_eq!(
            ItemQuery::new(SectionId::new(4))
                .matching("jura")
                .filter
                .title_word_prefix,
            Some("jura".to_owned())
        );
    }
}
