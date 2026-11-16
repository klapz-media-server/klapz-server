//! In-memory repositories, for tests that must not depend on storage.
//!
//! Lives behind the `testing` feature so it never ships in a release build. Bindings use
//! these to exercise resolvers without a database, which keeps the layering honest: a binding
//! test that needed `open-plex-db` would prove the dependency rule is not holding.
//!
//! They apply windows, filters and ordering the way storage does rather than simplifying,
//! because a stand-in that behaves differently hides exactly the mismatches worth catching.

use std::collections::BTreeMap;

use crate::backdrop::CornerColors;
use crate::collection::{
    Collection, CollectionCondition, CollectionEdit, CollectionEntry, CollectionError,
    CollectionQuery, CollectionRef, CollectionRepository, CollectionSort, CollectionTotals,
    Membership, NewCollection,
};
use crate::common::{CommonFields, Shared, SharedTag};
use crate::edit::{ItemEdit, ItemField};
use crate::ids::{
    CollectionId, MarkerId, MediaId, PartId, PersonId, PlaylistEntryId, PlaylistId, RatingKey,
    SectionId, StreamId, TagId,
};
use crate::library::{LibraryError, LibraryRepository, LibrarySection, SectionRoot};
use crate::marker::{Marker, NewMarker};
use crate::metadata::{
    self, CreditRef, EpisodeCredit, ExternalId, FieldValue, FirstCharacter, ItemKind, ItemQuery,
    ItemSort, ItemTag, Listed, MediaFile, MediaStream, MediaVersion, MetadataError, MetadataItem,
    MetadataRepository, Person, PersonCredit, Rating, SearchOrder, SearchQuery, Tag, TagCondition,
    TagKind, TagRef, TagScope, TagSearch, TagUse, ValueField,
};
use crate::page::{Cursor, Page, Window};
use crate::playlist::{
    Destination, NewPlaylist, Playlist, PlaylistEdit, PlaylistEntry, PlaylistError, PlaylistQuery,
    PlaylistRepository, PlaylistSort, PlaylistTotals, SmartRule,
};

#[derive(Debug, Clone, Default)]
pub struct InMemoryLibrary {
    sections: Vec<LibrarySection>,
    roots: Vec<SectionRoot>,
}

impl InMemoryLibrary {
    pub const fn new(sections: Vec<LibrarySection>) -> Self {
        Self {
            sections,
            roots: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_roots(mut self, roots: Vec<SectionRoot>) -> Self {
        self.roots = roots;
        self
    }
}

#[async_trait::async_trait]
impl LibraryRepository for InMemoryLibrary {
    /// Applies the window the way storage does, so tests exercise the same semantics.
    ///
    /// Sections are ordered by id, so the cursor's sort key is unused here and only its
    /// tiebreaker applies.
    async fn sections(
        &self,
        only: Option<&[SectionId]>,
        window: Window,
    ) -> Result<Page<LibrarySection>, LibraryError> {
        let after = window.after.as_ref().map(|cursor| cursor.id);
        let before = window.before.as_ref().map(|cursor| cursor.id);

        let mut rows: Vec<LibrarySection> = self
            .sections
            .iter()
            .filter(|section| only.is_none_or(|only| only.contains(&section.id)))
            .filter(|section| after.is_none_or(|id| section.id.get() > id))
            .filter(|section| before.is_none_or(|id| section.id.get() < id))
            .cloned()
            .collect();
        rows.sort_by_key(|section| section.id.get());
        if window.is_reversed() {
            rows.reverse();
        }
        rows.truncate(page_limit(&window));

        let mut page = Page::from_overfetch(rows, &window);
        // Storage reads a backward page in reverse; callers always want ascending order.
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn section(&self, id: SectionId) -> Result<LibrarySection, LibraryError> {
        self.sections
            .iter()
            .find(|section| section.id == id)
            .cloned()
            .ok_or(LibraryError::SectionNotFound(id))
    }

    async fn roots_for(&self, sections: &[SectionId]) -> Result<Vec<SectionRoot>, LibraryError> {
        Ok(self
            .roots
            .iter()
            .filter(|root| sections.contains(&root.section_id))
            .cloned()
            .collect())
    }
}

/// An in-memory [`MetadataRepository`].
///
/// Items and locks sit behind locks of their own because an edit writes them; everything else
/// is fixed at construction.
#[derive(Debug, Default)]
pub struct InMemoryMetadata {
    items: std::sync::Mutex<Vec<MetadataItem>>,
    locked_fields: std::sync::Mutex<Vec<(RatingKey, ItemField)>>,
    markers: std::sync::Mutex<Vec<Marker>>,
    versions: Vec<MediaVersion>,
    files: Vec<MediaFile>,
    streams: Vec<MediaStream>,
    tags: Vec<ItemTag>,
    external_ids: Vec<ExternalId>,
    ratings: Vec<Rating>,
    corner_colors: Vec<(RatingKey, CornerColors)>,
    /// What a collection filter reads, shared with the collections' own store.
    collections: Option<std::sync::Arc<std::sync::Mutex<CollectionStore>>>,
}

impl InMemoryMetadata {
    pub fn new(items: Vec<MetadataItem>) -> Self {
        Self {
            items: std::sync::Mutex::new(items),
            locked_fields: std::sync::Mutex::default(),
            markers: std::sync::Mutex::default(),
            versions: Vec::new(),
            files: Vec::new(),
            streams: Vec::new(),
            tags: Vec::new(),
            external_ids: Vec::new(),
            ratings: Vec::new(),
            corner_colors: Vec::new(),
            collections: None,
        }
    }

    /// Gives items their backdrop colors, as an import would.
    #[must_use]
    pub fn with_corner_colors(mut self, corner_colors: Vec<(RatingKey, CornerColors)>) -> Self {
        self.corner_colors = corner_colors;
        self
    }

    /// Reads collection filters from these collections, as storage reads its own tables.
    #[must_use]
    pub fn with_collections(mut self, collections: &InMemoryCollections) -> Self {
        self.collections = Some(std::sync::Arc::clone(&collections.state));
        self
    }

    /// Whether an item satisfies one collection condition, by the rule storage applies.
    fn in_collection(&self, item: RatingKey, condition: &CollectionCondition) -> bool {
        let Some(store) = &self.collections else {
            return false;
        };
        let store = locked(store);
        store
            .entries
            .iter()
            .filter(|entry| entry.item == item)
            .filter_map(|entry| store.collection(entry.collection))
            .any(|collection| {
                condition.any_of.iter().any(|wanted| match wanted {
                    CollectionRef::Id(id) => collection.id == *id,
                    CollectionRef::TagNumber(number) => collection.tag_number == *number,
                    CollectionRef::Title(title) => {
                        title.to_lowercase() == collection.title.to_lowercase()
                    }
                })
            })
    }

    /// Attaches external identifiers and ratings, kept in the order given.
    #[must_use]
    pub fn with_references(mut self, external_ids: Vec<ExternalId>, ratings: Vec<Rating>) -> Self {
        self.external_ids = external_ids;
        self.ratings = ratings;
        self
    }

    /// The items it holds, for a store that has to see the same library.
    pub fn items(&self) -> Vec<MetadataItem> {
        self.rows()
    }

    /// A snapshot: edits replace items, and no guard may be held across a resolver's await.
    fn rows(&self) -> Vec<MetadataItem> {
        locked(&self.items).clone()
    }

    /// Locks fields on items, as an import would.
    #[must_use]
    pub fn with_locked_fields(self, locked_fields: Vec<(RatingKey, ItemField)>) -> Self {
        *locked(&self.locked_fields) = locked_fields;
        self
    }

    /// Places markers, as an import would; they are listed in the order given.
    #[must_use]
    pub fn with_markers(self, markers: Vec<Marker>) -> Self {
        *locked(&self.markers) = markers;
        self
    }

    /// Attaches tags, which are kept in the order given: that order is the billing order.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<ItemTag>) -> Self {
        self.tags = tags;
        self
    }

    /// Whether an item satisfies one tag condition, by the rule storage applies.
    fn carries(&self, item: RatingKey, condition: &TagCondition) -> bool {
        self.tags
            .iter()
            .filter(|attached| attached.item_id == item && attached.tag.kind == condition.kind)
            .any(|attached| {
                condition.any_of.iter().any(|wanted| match wanted {
                    TagRef::Id(id) => attached.tag.id == *id,
                    TagRef::Name(name) => name.to_lowercase() == attached.tag.name.to_lowercase(),
                })
            })
    }

    /// Whether an item, or an item below it, carries the credit, by the rule storage applies.
    fn credits(&self, items: &[MetadataItem], item: RatingKey, credited: CreditRef) -> bool {
        let names = |attached: &ItemTag| match credited {
            CreditRef::Tag(id) => attached.tag.id == id,
            CreditRef::Person(id) => attached.tag.person.as_ref().is_some_and(|p| p.id == id),
        };
        let parent_of = |id: RatingKey| {
            items
                .iter()
                .find(|row| row.id == id)
                .and_then(|row| row.parent_id)
        };
        self.tags
            .iter()
            .filter(|attached| names(attached))
            .any(|attached| {
                let parent = parent_of(attached.item_id);
                attached.item_id == item
                    || parent == Some(item)
                    || parent.and_then(parent_of) == Some(item)
            })
    }

    /// Every track of every version of an item.
    fn tracks_of(&self, item: RatingKey) -> impl Iterator<Item = &MediaStream> {
        self.versions
            .iter()
            .filter(move |version| version.item_id == item)
            .flat_map(|version| {
                self.files
                    .iter()
                    .filter(|file| file.version_id == version.id)
            })
            .flat_map(|file| self.streams.iter().filter(|track| track.file_id == file.id))
    }

    #[must_use]
    pub fn with_media(
        mut self,
        versions: Vec<MediaVersion>,
        files: Vec<MediaFile>,
        streams: Vec<MediaStream>,
    ) -> Self {
        self.versions = versions;
        self.files = files;
        self.streams = streams;
        self
    }

    /// Everything the query keeps, already ordered.
    fn matching(&self, query: &ItemQuery) -> Vec<MetadataItem> {
        let items = self.rows();
        let filter = &query.filter;
        let mut matched: Vec<MetadataItem> = items
            .iter()
            .filter(|item| query.sections.contains(&item.section_id))
            .filter(|_| !filter.kind_not_held)
            .filter(|item| filter.kind.is_none_or(|kind| item.kind == kind))
            .filter(|item| !filter.top_level_only() || item.parent_id.is_none())
            .filter(|item| {
                filter
                    .year_from
                    .is_none_or(|year| item.year.is_some_and(|value| value >= year))
            })
            .filter(|item| {
                filter
                    .year_to
                    .is_none_or(|year| item.year.is_some_and(|value| value <= year))
            })
            .filter(|item| {
                filter
                    .content_rating
                    .as_ref()
                    .is_none_or(|rating| item.content_rating.as_ref() == Some(rating))
            })
            .filter(|item| {
                // The same case-insensitive substring the SQL adapter applies.
                filter.studio_contains.as_ref().is_none_or(|needle| {
                    item.studio.as_ref().is_some_and(|studio| {
                        studio.to_lowercase().contains(&needle.to_lowercase())
                    })
                })
            })
            .filter(|item| {
                // The same word-prefix match storage applies, through the same tokenizer:
                // two implementations of "what counts as a word" would make a search that
                // passes here and misses items in production.
                filter
                    .title_word_prefix
                    .as_ref()
                    .is_none_or(|term| title_matches(item, term))
            })
            .filter(|item| title_completed(item, filter.title_completion.as_deref()))
            .filter(|item| filter.excluded != Some(item.id))
            .filter(|item| {
                filter
                    .released_from
                    .is_none_or(|from| item.released_on.is_some_and(|released| released >= from))
            })
            .filter(|item| {
                filter
                    .audience_rating_from
                    .is_none_or(|from| item.audience_rating.is_some_and(|rating| rating >= from))
            })
            // This fake holds no watch state, so it answers as storage does a caller with
            // none: nothing watched, nothing in progress, no episode begun.
            .filter(|_| {
                filter.watched != Some(true)
                    && filter.in_progress != Some(true)
                    && filter.episodes_started != Some(true)
            })
            .filter(|item| {
                // Every item with an episode below it has one unwatched.
                filter
                    .episodes_watched
                    .is_none_or(|watched| watched != has_episode(&items, item.id))
            })
            .filter(|item| {
                filter.frame.is_none_or(|range| {
                    self.versions.iter().any(|version| {
                        version.item_id == item.id && range.contains(version.width, version.height)
                    })
                })
            })
            .filter(|item| {
                (filter.ids.is_empty() || filter.ids.contains(&item.id))
                    && (filter.tracks.iter())
                        .all(|condition| condition.holds(self.tracks_of(item.id)))
            })
            .filter(|item| {
                filter
                    .tags
                    .iter()
                    .all(|condition| self.carries(item.id, condition))
            })
            .filter(|item| {
                filter
                    .credited
                    .is_none_or(|credited| self.credits(&items, item.id, credited))
            })
            .filter(|item| {
                filter
                    .collections
                    .iter()
                    .all(|condition| self.in_collection(item.id, condition))
            })
            .cloned()
            .collect();

        sort_matched(query, &mut matched);
        matched
    }
}

/// Orders by title, date added, release date or filmography only: the other orderings read
/// versions or watch state, and storage's tests are where they are checked.
fn sort_matched(query: &ItemQuery, matched: &mut [MetadataItem]) {
    match query.sort {
        ItemSort::AddedAt | ItemSort::Arrival => matched.sort_by(|left, right| {
            left.added_at
                .cmp(&right.added_at)
                .then(left.id.get().cmp(&right.id.get()))
        }),
        // Absent first, ties by title, as storage orders it.
        ItemSort::ReleasedOn => matched.sort_by(|left, right| {
            left.released_on
                .cmp(&right.released_on)
                .then_with(|| sort_key(left).cmp(&sort_key(right)))
                .then(left.id.get().cmp(&right.id.get()))
        }),
        ItemSort::Filmography => matched.sort_by_key(|item| {
            let library = query.sections.iter().position(|id| *id == item.section_id);
            let title = if item.kind == ItemKind::Show {
                String::new()
            } else {
                sort_key(item)
            };
            (library, title, item.id.get())
        }),
        _ => matched.sort_by(|left, right| {
            sort_key(left)
                .cmp(&sort_key(right))
                .then(left.id.get().cmp(&right.id.get()))
        }),
    }
    if query.direction.is_descending() {
        matched.reverse();
    }
}

/// The text completes the item's title, by the rule storage applies; no text, no constraint.
/// Whether an episode lies below this item, directly or through a season.
fn has_episode(items: &[MetadataItem], id: RatingKey) -> bool {
    items.iter().any(|episode| {
        episode.kind == ItemKind::Episode
            && (episode.parent_id == Some(id)
                || episode.parent_id.is_some_and(|parent| {
                    items
                        .iter()
                        .any(|season| season.id == parent && season.parent_id == Some(id))
                }))
    })
}

fn title_completed(item: &MetadataItem, text: Option<&str>) -> bool {
    text.is_none_or(|text| {
        item.title
            .as_deref()
            .is_some_and(|title| crate::completion::completes(title, text))
    })
}

/// Every word of the term begins a word of the item's titles, as storage applies it.
fn title_matches(item: &MetadataItem, term: &str) -> bool {
    let words = crate::search::title_words(
        item.title.as_deref(),
        item.title_sort.as_deref(),
        item.original_title.as_deref(),
    );
    words_match(&words, term)
}

/// Whether every word of the term begins one of the words, as storage matches a search.
fn words_match(words: &[String], term: &str) -> bool {
    crate::search::words(term).all(|prefix| {
        crate::search::prefix_range(&prefix).is_some_and(|(lower, upper)| {
            words
                .iter()
                .any(|word| word >= &lower && upper.as_ref().is_none_or(|bound| word < bound))
        })
    })
}

#[async_trait::async_trait]
impl MetadataRepository for InMemoryMetadata {
    async fn items(
        &self,
        query: &ItemQuery,
        window: Window,
    ) -> Result<Page<Listed>, MetadataError> {
        let ordered = self.matching(query);
        let position = |cursor: &Cursor| ordered.iter().position(|item| item.id.get() == cursor.id);

        let start = window
            .after
            .as_ref()
            .and_then(position)
            .map_or(0, |at| at + 1);
        let end = window
            .before
            .as_ref()
            .and_then(position)
            .unwrap_or(ordered.len());
        let mut slice: Vec<MetadataItem> = ordered[start..end.max(start)].to_vec();

        if window.is_reversed() {
            slice.reverse();
        }
        // Mirrors SQL `OFFSET`, which applies after ordering and before the limit.
        let skip = usize::try_from(window.skip).unwrap_or(usize::MAX);
        slice.drain(..skip.min(slice.len()));
        slice.truncate(page_limit(&window));

        let mut page = Page::from_overfetch(slice, &window);
        if window.is_reversed() {
            page.items.reverse();
        }
        // Positions are found by id here, so the cursor needs nothing else.
        Ok(page.map(|item| Listed {
            cursor: Cursor::by_id(item.id.get()),
            item,
        }))
    }

    async fn item_count(&self, query: &ItemQuery) -> Result<u64, MetadataError> {
        Ok(self.matching(query).len() as u64)
    }

    async fn search(
        &self,
        query: &SearchQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        if query.is_blank() {
            return Ok(Page {
                items: Vec::new(),
                has_previous: false,
                has_next: false,
            });
        }

        let rows = self.rows();
        let parent = |item: &MetadataItem| {
            item.parent_id
                .and_then(|parent| rows.iter().find(|row| row.id == parent))
        };
        // An episode is also found through its series' titles, as storage finds it.
        let series_matches = |item: &MetadataItem| {
            query.episodes_by_series
                && item.kind == ItemKind::Episode
                && parent(item)
                    .and_then(parent)
                    .is_some_and(|series| title_matches(series, &query.word_prefix))
        };
        let mut matched: Vec<MetadataItem> = rows
            .iter()
            .filter(|item| query.kinds.is_empty() || query.kinds.contains(&item.kind))
            .filter(|item| {
                query
                    .sections
                    .as_ref()
                    .is_none_or(|sections| sections.contains(&item.section_id))
            })
            .filter(|item| title_matches(item, &query.word_prefix) || series_matches(item))
            .cloned()
            .collect();
        let rank = |item: &MetadataItem| match query.order {
            SearchOrder::Title => (0, 0.0),
            SearchOrder::Relevance => (
                crate::search::hundredths(item.kind, query.rung(item)),
                item.audience_rating.unwrap_or(0.0),
            ),
        };
        matched.sort_by(|left, right| {
            let ((left_points, left_rating), (right_points, right_rating)) =
                (rank(left), rank(right));
            right_points
                .cmp(&left_points)
                .then_with(|| right_rating.total_cmp(&left_rating))
                .then_with(|| sort_key(left).cmp(&sort_key(right)))
                .then_with(|| left.id.get().cmp(&right.id.get()))
        });
        Ok(windowed(matched, &window, |item| query.cursor(item)))
    }

    async fn search_tags(
        &self,
        query: &TagSearch,
        window: Window,
    ) -> Result<Page<TagUse>, MetadataError> {
        if query.is_blank() {
            return Ok(Page {
                items: Vec::new(),
                has_previous: false,
                has_next: false,
            });
        }
        let items = self.rows();
        let mut carried: BTreeMap<(TagId, SectionId), (Tag, Vec<RatingKey>)> = BTreeMap::new();
        for attachment in &self.tags {
            let tag = &attachment.tag;
            if !(query.kinds.is_empty() || query.kinds.contains(&tag.kind))
                || !words_match(&crate::search::name_words(&tag.name), &query.word_prefix)
            {
                continue;
            }
            let Some(item) = items.iter().find(|item| item.id == attachment.item_id) else {
                continue;
            };
            if query
                .sections
                .as_ref()
                .is_some_and(|sections| !sections.contains(&item.section_id))
            {
                continue;
            }
            let (_, holders) = carried
                .entry((tag.id, item.section_id))
                .or_insert_with(|| (tag.clone(), Vec::new()));
            if !holders.contains(&item.id) {
                holders.push(item.id);
            }
        }
        let mut uses: Vec<TagUse> = carried
            .into_iter()
            .filter(|(_, (_, holders))| {
                holders.iter().any(|holder| {
                    items
                        .iter()
                        .any(|item| item.id == *holder && item.parent_id.is_none())
                })
            })
            .map(|((_, section), (tag, holders))| TagUse {
                tag,
                section,
                item_count: holders.len() as u64,
            })
            .collect();
        uses.sort_by(|left, right| {
            right
                .item_count
                .cmp(&left.item_count)
                .then_with(|| left.tag.id.get().cmp(&right.tag.id.get()))
                .then_with(|| left.section.get().cmp(&right.section.get()))
        });
        Ok(windowed(uses, &window, TagUse::cursor))
    }

    async fn first_characters(
        &self,
        query: &ItemQuery,
    ) -> Result<Vec<FirstCharacter>, MetadataError> {
        let mut merged: BTreeMap<String, u64> = BTreeMap::new();
        for item in self.matching(query) {
            *merged
                .entry(metadata::first_character_bucket(&sort_key(&item)))
                .or_default() += 1;
        }

        let mut buckets: Vec<FirstCharacter> = merged
            .into_iter()
            .map(|(character, count)| FirstCharacter { character, count })
            .collect();
        // `#` first, then the letters, as storage orders them.
        buckets.sort_by(|left, right| {
            let rank = |bucket: &FirstCharacter| u8::from(bucket.character != "#");
            rank(left)
                .cmp(&rank(right))
                .then_with(|| left.character.cmp(&right.character))
        });
        Ok(buckets)
    }

    async fn item(&self, id: RatingKey) -> Result<MetadataItem, MetadataError> {
        self.rows()
            .iter()
            .find(|item| item.id == id)
            .cloned()
            .ok_or(MetadataError::ItemNotFound(id))
    }

    async fn items_by_id(&self, ids: &[RatingKey]) -> Result<Vec<MetadataItem>, MetadataError> {
        Ok(self
            .rows()
            .iter()
            .filter(|item| ids.contains(&item.id))
            .cloned()
            .collect())
    }

    async fn children(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        let mut rows: Vec<MetadataItem> = self
            .rows()
            .iter()
            .filter(|item| item.parent_id == Some(parent))
            .cloned()
            .collect();
        rows.sort_by_key(|item| (item.position.unwrap_or(0), item.id.get()));
        Ok(Self::page_of(rows, &window))
    }

    async fn grandchildren(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        // Position of each child, so the grandchildren can be ordered season by season.
        let children: BTreeMap<RatingKey, i32> = self
            .rows()
            .iter()
            .filter(|item| item.parent_id == Some(parent))
            .map(|item| (item.id, item.position.unwrap_or(0)))
            .collect();
        let mut rows: Vec<(i32, MetadataItem)> = self
            .rows()
            .iter()
            .filter_map(|item| {
                let child = item.parent_id?;
                Some((*children.get(&child)?, item.clone()))
            })
            .collect();
        rows.sort_by_key(|(child_position, item)| {
            (*child_position, item.position.unwrap_or(0), item.id.get())
        });
        Ok(Self::page_of(
            rows.into_iter().map(|(_, item)| item).collect(),
            &window,
        ))
    }

    async fn child_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        Ok(parents
            .iter()
            .map(|parent| (*parent, self.count_children(*parent)))
            .collect())
    }

    async fn leaf_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        Ok(parents
            .iter()
            .map(|parent| {
                let children: Vec<RatingKey> = self
                    .rows()
                    .iter()
                    .filter(|item| item.parent_id == Some(*parent))
                    .map(|item| item.id)
                    .collect();
                let leaves = self
                    .rows()
                    .iter()
                    .filter(|item| {
                        item.parent_id
                            .is_some_and(|parent| children.contains(&parent))
                    })
                    .count();
                (*parent, leaves as u64)
            })
            .collect())
    }

    async fn versions_for(&self, items: &[RatingKey]) -> Result<Vec<MediaVersion>, MetadataError> {
        Ok(self
            .versions
            .iter()
            .filter(|version| items.contains(&version.item_id))
            .cloned()
            .collect())
    }

    async fn files_for(&self, versions: &[MediaId]) -> Result<Vec<MediaFile>, MetadataError> {
        Ok(self
            .files
            .iter()
            .filter(|file| versions.contains(&file.version_id))
            .cloned()
            .collect())
    }

    async fn streams_for(&self, files: &[PartId]) -> Result<Vec<MediaStream>, MetadataError> {
        Ok(self
            .streams
            .iter()
            .filter(|stream| files.contains(&stream.file_id))
            .cloned()
            .collect())
    }

    async fn version(&self, id: MediaId) -> Result<Option<MediaVersion>, MetadataError> {
        Ok(self.versions.iter().find(|item| item.id == id).cloned())
    }

    async fn file(&self, id: PartId) -> Result<Option<MediaFile>, MetadataError> {
        Ok(self.files.iter().find(|item| item.id == id).cloned())
    }

    async fn stream(&self, id: StreamId) -> Result<Option<MediaStream>, MetadataError> {
        Ok(self.streams.iter().find(|item| item.id == id).cloned())
    }

    async fn items_sharing_tags(
        &self,
        section: SectionId,
        kind: ItemKind,
        tags: &[TagId],
    ) -> Result<Vec<crate::similar::TagOverlap>, MetadataError> {
        Ok(self
            .rows()
            .iter()
            .filter(|item| item.section_id == section && item.kind == kind)
            .filter_map(|item| {
                let shared = self
                    .tags
                    .iter()
                    .filter(|attached| {
                        attached.item_id == item.id && tags.contains(&attached.tag.id)
                    })
                    .count();
                (shared > 0).then(|| crate::similar::TagOverlap {
                    item: item.id,
                    shared: u32::try_from(shared).unwrap_or(u32::MAX),
                    year: item.year,
                })
            })
            .collect())
    }

    async fn tags_for(&self, items: &[RatingKey]) -> Result<Vec<ItemTag>, MetadataError> {
        Ok(self
            .tags
            .iter()
            .filter(|attached| items.contains(&attached.item_id))
            .cloned()
            .collect())
    }

    async fn credits_below(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<EpisodeCredit>, MetadataError> {
        let items = self.rows();
        let mut credits: Vec<EpisodeCredit> = Vec::new();
        for &parent in parents {
            let episodes: Vec<RatingKey> = items
                .iter()
                .filter(|item| item.kind == ItemKind::Episode)
                .filter(|episode| {
                    episode.parent_id == Some(parent)
                        || episode.parent_id.is_some_and(|season| {
                            items
                                .iter()
                                .any(|item| item.id == season && item.parent_id == Some(parent))
                        })
                })
                .map(|episode| episode.id)
                .collect();
            let mut tallied: Vec<EpisodeCredit> = Vec::new();
            for attached in self.tags.iter().filter(|attached| {
                episodes.contains(&attached.item_id)
                    && !matches!(attached.tag.kind, TagKind::Genre | TagKind::Country)
            }) {
                match tallied
                    .iter_mut()
                    .find(|credit| credit.tag.id == attached.tag.id)
                {
                    Some(credit) => credit.episodes += 1,
                    None => tallied.push(EpisodeCredit {
                        parent,
                        tag: attached.tag.clone(),
                        episodes: 1,
                    }),
                }
            }
            // Most episodes first, then by name: the order storage promises.
            tallied.sort_by(|left, right| {
                right
                    .episodes
                    .cmp(&left.episodes)
                    .then_with(|| left.tag.name.cmp(&right.tag.name))
            });
            credits.extend(tallied);
        }
        Ok(credits)
    }

    async fn external_ids_for(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<ExternalId>, MetadataError> {
        Ok(self
            .external_ids
            .iter()
            .filter(|id| items.contains(&id.item_id))
            .cloned()
            .collect())
    }

    async fn ratings_for(&self, items: &[RatingKey]) -> Result<Vec<Rating>, MetadataError> {
        Ok(self
            .ratings
            .iter()
            .filter(|rating| items.contains(&rating.item_id))
            .cloned()
            .collect())
    }

    async fn person(&self, id: PersonId) -> Result<Option<Person>, MetadataError> {
        Ok(self
            .tags
            .iter()
            .filter_map(|attached| attached.tag.person.as_ref())
            .find(|person| person.id == id)
            .cloned())
    }

    async fn credits_of(&self, people: &[PersonId]) -> Result<Vec<PersonCredit>, MetadataError> {
        let items = self.rows();
        let mut credits: Vec<PersonCredit> = Vec::new();
        for attached in &self.tags {
            if !attached
                .tag
                .person
                .as_ref()
                .is_some_and(|person| people.contains(&person.id))
            {
                continue;
            }
            let section = items
                .iter()
                .find(|item| item.id == attached.item_id)
                .map(|item| item.section_id);
            if !credits.iter().any(|known| known.tag.id == attached.tag.id) {
                credits.push(PersonCredit {
                    tag: attached.tag.clone(),
                    sections: Vec::new(),
                });
            }
            let credit = credits
                .iter_mut()
                .find(|known| known.tag.id == attached.tag.id);
            if let (Some(credit), Some(section)) = (credit, section) {
                if !credit.sections.contains(&section) {
                    credit.sections.push(section);
                }
            }
        }
        credits.sort_by_key(|credit| credit.tag.id);
        Ok(credits)
    }

    async fn episode_additions(
        &self,
        section: SectionId,
        after: Option<(i64, RatingKey)>,
        limit: u32,
    ) -> Result<Vec<crate::recent::EpisodeAddition>, MetadataError> {
        let items = self.rows();
        let mut additions: Vec<crate::recent::EpisodeAddition> = items
            .iter()
            .filter(|item| item.section_id == section && item.kind == ItemKind::Episode)
            .filter_map(|episode| {
                let season = items
                    .iter()
                    .find(|item| Some(item.id) == episode.parent_id)?;
                Some(crate::recent::EpisodeAddition {
                    episode: episode.id,
                    season: season.id,
                    show: season.parent_id?,
                    added_at: episode.added_at,
                })
            })
            .collect();
        // Newest first, the id breaking ties downward, as storage orders them.
        additions.sort_by(|left, right| {
            right
                .added_at
                .cmp(&left.added_at)
                .then(right.episode.get().cmp(&left.episode.get()))
        });
        let start = after.map_or(0, |(added_at, episode)| {
            additions
                .iter()
                .position(|addition| {
                    (addition.added_at, addition.episode.get()) < (added_at, episode.get())
                })
                .unwrap_or(additions.len())
        });
        Ok(additions
            .into_iter()
            .skip(start)
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect())
    }

    async fn tags_in(
        &self,
        query: &ItemQuery,
        scope: TagScope,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError> {
        Ok(windowed(
            self.distinct_tags(query, scope),
            &window,
            Tag::name_cursor,
        ))
    }

    async fn tag_count_in(&self, query: &ItemQuery, scope: TagScope) -> Result<u64, MetadataError> {
        Ok(self.distinct_tags(query, scope).len() as u64)
    }

    async fn tags_completing(
        &self,
        query: &ItemQuery,
        scope: TagScope,
        text: &str,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError> {
        let mut tags = self.distinct_tags(query, scope);
        tags.retain(|tag| crate::completion::completes(&tag.name, text));
        Ok(windowed(tags, &window, Tag::name_cursor))
    }

    async fn closest_titles(
        &self,
        query: &crate::closest::ClosestTitles,
        window: Window,
    ) -> Result<Page<crate::closest::TitleMatch>, MetadataError> {
        let distance = crate::closest::Distance::new(&crate::closest::term_key(&query.term));
        let mut matches: Vec<crate::closest::TitleMatch> = self
            .rows()
            .iter()
            .filter(|item| query.kinds.contains(&item.kind))
            .filter(|item| {
                query
                    .sections
                    .as_ref()
                    .is_none_or(|sections| sections.contains(&item.section_id))
            })
            .map(|item| {
                let key =
                    crate::closest::title_key(item.title_sort.as_deref(), item.title.as_deref());
                crate::closest::TitleMatch {
                    distance: distance.to(&key),
                    key,
                    item: item.clone(),
                }
            })
            .collect();
        matches.sort_by(|left, right| {
            (left.distance, &left.key, left.item.id.get()).cmp(&(
                right.distance,
                &right.key,
                right.item.id.get(),
            ))
        });
        Ok(windowed(
            matches,
            &window,
            crate::closest::TitleMatch::cursor,
        ))
    }

    async fn values_in(
        &self,
        query: &ItemQuery,
        field: ValueField,
        window: Window,
    ) -> Result<Page<FieldValue>, MetadataError> {
        Ok(windowed(
            self.distinct_values(query, field),
            &window,
            FieldValue::cursor,
        ))
    }

    async fn value_count_in(
        &self,
        query: &ItemQuery,
        field: ValueField,
    ) -> Result<u64, MetadataError> {
        Ok(self.distinct_values(query, field).len() as u64)
    }

    async fn descendants(&self, item: RatingKey) -> Result<Vec<MetadataItem>, MetadataError> {
        let items = self.rows();
        let children: Vec<&MetadataItem> = items
            .iter()
            .filter(|child| child.parent_id == Some(item))
            .collect();
        let grandchildren = items.iter().filter(|grandchild| {
            children
                .iter()
                .any(|child| grandchild.parent_id == Some(child.id))
        });
        Ok(children
            .iter()
            .copied()
            .chain(grandchildren)
            .cloned()
            .collect())
    }

    async fn items_by_guid(
        &self,
        guids: &[String],
        sections: Option<&[SectionId]>,
    ) -> Result<Vec<MetadataItem>, MetadataError> {
        Ok(self
            .rows()
            .iter()
            .filter(|item| guids.contains(&item.guid))
            .filter(|item| sections.is_none_or(|sections| sections.contains(&item.section_id)))
            .cloned()
            .collect())
    }

    async fn tag(&self, id: TagId) -> Result<Option<Tag>, MetadataError> {
        Ok(self
            .tags
            .iter()
            .find(|attached| attached.tag.id == id)
            .map(|attached| attached.tag.clone()))
    }

    async fn locked_fields(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<(RatingKey, ItemField)>, MetadataError> {
        Ok(locked(&self.locked_fields)
            .iter()
            .filter(|(item, _)| items.contains(item))
            .copied()
            .collect())
    }

    async fn corner_colors(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<(RatingKey, CornerColors)>, MetadataError> {
        Ok(self
            .corner_colors
            .iter()
            .filter(|(item, _)| items.contains(item))
            .copied()
            .collect())
    }

    async fn edit_item(
        &self,
        id: RatingKey,
        edit: &ItemEdit,
    ) -> Result<metadata::Edited, MetadataError> {
        let mut items = locked(&self.items);
        let item = items
            .iter_mut()
            .find(|item| item.id == id && item.kind == edit.kind())
            .ok_or(MetadataError::ItemNotFound(id))?;
        let mut all_locks = locked(&self.locked_fields);
        let mut fields: std::collections::BTreeSet<ItemField> = all_locks
            .iter()
            .filter(|(locked_item, _)| *locked_item == id)
            .map(|(_, field)| *field)
            .collect();
        edit.apply(item, &mut fields);
        all_locks.retain(|(locked_item, _)| *locked_item != id);
        all_locks.extend(fields.iter().map(|field| (id, *field)));
        Ok(metadata::Edited {
            item: item.clone(),
            locked: fields.into_iter().collect(),
        })
    }

    async fn markers(
        &self,
        items: &[RatingKey],
        viewer: Option<crate::ids::UserId>,
    ) -> Result<Vec<Marker>, MetadataError> {
        Ok(locked(&self.markers)
            .iter()
            .filter(|marker| items.contains(&marker.item) && marker.is_visible_to(viewer))
            .cloned()
            .collect())
    }

    async fn marker(&self, id: MarkerId) -> Result<Option<Marker>, MetadataError> {
        Ok(locked(&self.markers)
            .iter()
            .find(|marker| marker.id == id)
            .cloned())
    }

    async fn add_marker(&self, marker: &NewMarker) -> Result<(Marker, bool), MetadataError> {
        let mut markers = locked(&self.markers);
        if let Some(present) = markers.iter().find(|present| marker.duplicates(present)) {
            return Ok((present.clone(), false));
        }
        let id = markers
            .iter()
            .map(|present| present.id.get())
            .max()
            .unwrap_or(0)
            + 1;
        let values = marker.values();
        let added = Marker {
            id: MarkerId::new(id),
            item: marker.item(),
            kind: marker.kind(),
            owner: marker.owner(),
            start_ms: values.start_ms,
            end_ms: values.end_ms,
            is_final: values.is_final,
            version: None,
            title: values.title.clone(),
            color: values.color.clone(),
        };
        markers.push(added.clone());
        Ok((added, true))
    }

    async fn update_marker(&self, marker: &Marker) -> Result<Option<Marker>, MetadataError> {
        let mut markers = locked(&self.markers);
        let Some(present) = markers.iter_mut().find(|present| present.id == marker.id) else {
            return Ok(None);
        };
        *present = Marker {
            id: present.id,
            item: present.item,
            kind: present.kind,
            owner: present.owner,
            ..marker.clone()
        };
        Ok(Some(present.clone()))
    }

    async fn delete_marker(&self, id: MarkerId) -> Result<bool, MetadataError> {
        let mut markers = locked(&self.markers);
        let before = markers.len();
        markers.retain(|marker| marker.id != id);
        Ok(markers.len() < before)
    }

    async fn common_fields(&self, query: &ItemQuery) -> Result<CommonFields, MetadataError> {
        let matched = self.matching(query);
        if matched.is_empty() {
            return Ok(CommonFields::empty());
        }
        let rows = self.rows();
        let parent = |item: &MetadataItem| {
            item.parent_id
                .and_then(|id| rows.iter().find(|row| row.id == id))
        };
        let grandparent = |item: &MetadataItem| parent(item).and_then(parent);
        let ids: Vec<RatingKey> = matched.iter().map(|item| item.id).collect();
        Ok(CommonFields {
            items: ids.len() as u64,
            id: shared_by(&matched, |item| Some(item.id)),
            parent: shared_by(&matched, |item| item.parent_id),
            parent_title: shared_by(&matched, |item| parent(item)?.title.clone()),
            grandparent: shared_by(&matched, |item| grandparent(item).map(|row| row.id)),
            grandparent_title: shared_by(&matched, |item| grandparent(item)?.title.clone()),
            title: shared_by(&matched, |item| item.title.clone()),
            title_sort: shared_by(&matched, |item| item.title_sort.clone()),
            original_title: shared_by(&matched, |item| item.original_title.clone()),
            tagline: shared_by(&matched, |item| item.tagline.clone()),
            summary: shared_by(&matched, |item| item.summary.clone()),
            studio: shared_by(&matched, |item| item.studio.clone()),
            content_rating: shared_by(&matched, |item| item.content_rating.clone()),
            year: shared_by(&matched, |item| item.year),
            released_on: shared_by(&matched, |item| item.released_on),
            rating: shared_by(&matched, |item| item.rating),
            position: shared_by(&matched, |item| item.position),
            tags: self.shared_tags(&ids),
            collections: self.shared_collections(&ids),
            locked: self.shared_locks(&ids),
        })
    }
}

/// A field across a non-empty set of items, as storage aggregates it.
fn shared_by<T: PartialEq>(
    items: &[MetadataItem],
    value: impl Fn(&MetadataItem) -> Option<T>,
) -> Shared<T> {
    let mut values = items.iter().map(value);
    let first = values.next().flatten();
    values.fold(Shared::Same(first), |shared, value| {
        shared.with(value.as_ref())
    })
}

impl InMemoryMetadata {
    /// The tags every item carries, as storage orders them.
    fn shared_tags(&self, ids: &[RatingKey]) -> Vec<SharedTag> {
        let single = ids.len() == 1;
        let carriers = |tag: TagId| {
            ids.iter()
                .filter(|id| {
                    self.tags
                        .iter()
                        .any(|other| other.item_id == **id && other.tag.id == tag)
                })
                .count()
        };
        let mut tags: Vec<SharedTag> = Vec::new();
        let attachments = self
            .tags
            .iter()
            .filter(|attached| ids.contains(&attached.item_id));
        for (index, attached) in attachments.enumerate() {
            let known = tags.iter().any(|shared| shared.tag.id == attached.tag.id);
            if !known && carriers(attached.tag.id) == ids.len() {
                tags.push(SharedTag {
                    tag: attached.tag.clone(),
                    role: attached.role.clone().filter(|_| single),
                    position: single.then(|| i32::try_from(index).unwrap_or(i32::MAX)),
                });
            }
        }
        crate::common::order_shared_tags(&mut tags);
        tags
    }

    fn shared_locks(&self, ids: &[RatingKey]) -> Vec<ItemField> {
        let locks = locked(&self.locked_fields);
        ItemField::ALL
            .into_iter()
            .filter(|field| {
                locks
                    .iter()
                    .filter(|(item, locked)| locked == field && ids.contains(item))
                    .count()
                    == ids.len()
            })
            .collect()
    }

    fn shared_collections(&self, ids: &[RatingKey]) -> Vec<CollectionId> {
        let Some(store) = &self.collections else {
            return Vec::new();
        };
        let store = locked(store);
        let members = |collection: CollectionId| {
            store
                .entries
                .iter()
                .filter(|entry| entry.collection == collection && ids.contains(&entry.item))
                .count()
        };
        let mut held: Vec<CollectionId> = store
            .entries
            .iter()
            .filter(|entry| Some(&entry.item) == ids.first())
            .map(|entry| entry.collection)
            .filter(|collection| members(*collection) == ids.len())
            .collect();
        held.sort_by_key(|id| {
            store
                .collection(*id)
                .map(|collection| collection.tag_number)
        });
        held
    }
}

impl InMemoryMetadata {
    fn count_children(&self, parent: RatingKey) -> u64 {
        self.rows()
            .iter()
            .filter(|item| item.parent_id == Some(parent))
            .count() as u64
    }

    /// Cuts one page out of rows already in their final order.
    ///
    /// Children are ordered by position, where the cursor is the id alone, so only the
    /// offset and the limit apply — mirroring SQL `OFFSET` after `ORDER BY`.
    fn distinct_tags(&self, query: &ItemQuery, scope: TagScope) -> Vec<Tag> {
        let items: Vec<RatingKey> = self.matching(query).iter().map(|item| item.id).collect();
        let mut tags: Vec<Tag> = Vec::new();
        // Billing is the order of an item's attachments of one kind, as storage numbers it.
        let mut billed: BTreeMap<RatingKey, u32> = BTreeMap::new();
        for attached in &self.tags {
            if attached.tag.kind != scope.kind || !items.contains(&attached.item_id) {
                continue;
            }
            let position = billed.entry(attached.item_id).or_default();
            let within = scope.billed_within.is_none_or(|limit| *position < limit);
            *position += 1;
            if within && !tags.iter().any(|tag| tag.id == attached.tag.id) {
                tags.push(attached.tag.clone());
            }
        }
        tags.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        tags
    }

    fn distinct_values(&self, query: &ItemQuery, field: ValueField) -> Vec<FieldValue> {
        let matched = self.matching(query);
        let mut values: Vec<FieldValue> = matched
            .iter()
            .flat_map(|item| -> Vec<FieldValue> {
                match field {
                    ValueField::Year => item.year.map(FieldValue::Year).into_iter().collect(),
                    ValueField::ContentRating => item
                        .content_rating
                        .clone()
                        .map(FieldValue::Text)
                        .into_iter()
                        .collect(),
                    ValueField::Studio => item
                        .studio
                        .clone()
                        .map(FieldValue::Text)
                        .into_iter()
                        .collect(),
                    ValueField::Track(track_field) => self
                        .tracks_of(item.id)
                        .filter_map(|track| track_field.of(track))
                        .map(|value| FieldValue::Text(value.to_owned()))
                        .collect(),
                }
            })
            .collect();
        values.sort_by(|a, b| match (a, b) {
            (FieldValue::Year(a), FieldValue::Year(b)) => b.cmp(a),
            (FieldValue::Text(a), FieldValue::Text(b)) => a.cmp(b),
            (FieldValue::Year(_), FieldValue::Text(_)) => std::cmp::Ordering::Less,
            (FieldValue::Text(_), FieldValue::Year(_)) => std::cmp::Ordering::Greater,
        });
        values.dedup();
        values
    }

    fn page_of(mut rows: Vec<MetadataItem>, window: &Window) -> Page<MetadataItem> {
        let skip = usize::try_from(window.skip).unwrap_or(usize::MAX);
        rows.drain(..skip.min(rows.len()));
        rows.truncate(page_limit(window));
        Page::from_overfetch(rows, window)
    }
}

/// The ordering key a title-sorted listing uses, matching storage's folded column.
fn sort_key(item: &MetadataItem) -> String {
    metadata::sort_key(item.title_sort.as_deref(), item.title.as_deref())
}

/// Windows an already ordered list, locating cursors by equality as storage locates them
/// by comparison.
fn windowed<T>(ordered: Vec<T>, window: &Window, cursor_of: impl Fn(&T) -> Cursor) -> Page<T> {
    let position = |cursor: &Cursor| ordered.iter().position(|row| cursor_of(row) == *cursor);
    let start = window
        .after
        .as_ref()
        .and_then(position)
        .map_or(0, |at| at + 1);
    let end = window
        .before
        .as_ref()
        .and_then(position)
        .unwrap_or(ordered.len());
    let mut slice: Vec<T> = ordered
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();

    if window.is_reversed() {
        slice.reverse();
    }
    let skip = usize::try_from(window.skip).unwrap_or(usize::MAX);
    slice.drain(..skip.min(slice.len()));
    slice.truncate(page_limit(window));

    let mut page = Page::from_overfetch(slice, window);
    if window.is_reversed() {
        page.items.reverse();
    }
    page
}

fn page_limit(window: &Window) -> usize {
    usize::try_from(window.fetch_limit()).unwrap_or(usize::MAX)
}

/// Sessions held in memory, with the storage rules: expiry, and one row per key.
#[derive(Debug, Default)]
pub struct InMemoryPlayback {
    sessions: std::sync::Mutex<Vec<crate::playback::PlaybackSession>>,
}

impl InMemoryPlayback {
    fn rows(&self) -> std::sync::MutexGuard<'_, Vec<crate::playback::PlaybackSession>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl crate::playback::PlaybackRepository for InMemoryPlayback {
    async fn start(
        &self,
        session: crate::playback::NewPlaybackSession,
        now: i64,
    ) -> Result<crate::playback::PlaybackSession, crate::playback::PlaybackError> {
        let mut rows = self.rows();
        let previous = rows.iter().find(|row| row.key == session.key);
        let id = previous.map_or_else(
            || rows.iter().map(|row| row.id).max().unwrap_or(0) + 1,
            |row| row.id,
        );
        let termination = previous.and_then(|row| row.termination.clone());
        rows.retain(|row| row.key != session.key);
        let stored = crate::playback::PlaybackSession {
            id,
            key: session.key,
            user: session.user,
            playback_id: session.playback_id,
            item: session.item,
            part: session.part,
            client: session.client,
            product: session.product,
            state: crate::playback::PlaybackState::Playing,
            video: session.video,
            audio: session.audio,
            position_ms: session.position_ms,
            output: session.output,
            started_at: now,
            updated_at: now,
            expires_at: now + crate::playback::SESSION_LIFETIME_SECONDS,
            termination,
        };
        rows.push(stored.clone());
        Ok(stored)
    }

    async fn touch(&self, key: &str, now: i64) -> Result<bool, crate::playback::PlaybackError> {
        let mut rows = self.rows();
        let found = rows
            .iter_mut()
            .find(|row| row.key == key && row.expires_at > now && row.termination.is_none());
        Ok(found
            .map(|row| {
                row.updated_at = now;
                row.expires_at = now + crate::playback::SESSION_LIFETIME_SECONDS;
            })
            .is_some())
    }

    async fn report(
        &self,
        playback_id: &str,
        state: crate::playback::PlaybackState,
        position_ms: Option<i64>,
        now: i64,
    ) -> Result<Vec<String>, crate::playback::PlaybackError> {
        let mut reached = Vec::new();
        for row in self.rows().iter_mut().filter(|row| {
            row.playback_id.as_deref() == Some(playback_id)
                && row.expires_at > now
                && row.termination.is_none()
        }) {
            reached.push(row.key.clone());
            row.state = state;
            row.position_ms = position_ms.unwrap_or(row.position_ms);
            row.updated_at = now;
            row.expires_at = now + crate::playback::SESSION_LIFETIME_SECONDS;
        }
        Ok(reached)
    }

    async fn keys_of_playback(
        &self,
        playback_id: &str,
    ) -> Result<Vec<String>, crate::playback::PlaybackError> {
        Ok(self
            .rows()
            .iter()
            .filter(|row| row.playback_id.as_deref() == Some(playback_id))
            .map(|row| row.key.clone())
            .collect())
    }

    async fn end(&self, key: &str) -> Result<bool, crate::playback::PlaybackError> {
        let mut rows = self.rows();
        let before = rows.len();
        rows.retain(|row| row.key != key);
        Ok(rows.len() < before)
    }

    async fn end_if_expired(
        &self,
        key: &str,
        now: i64,
    ) -> Result<bool, crate::playback::PlaybackError> {
        let mut rows = self.rows();
        let before = rows.len();
        rows.retain(|row| row.key != key || row.expires_at > now);
        Ok(rows.len() < before)
    }

    async fn terminate(
        &self,
        key: &str,
        reason: Option<String>,
        now: i64,
    ) -> Result<Option<crate::playback::PlaybackSession>, crate::playback::PlaybackError> {
        let mut rows = self.rows();
        let found = rows
            .iter_mut()
            .find(|row| row.key == key && row.expires_at > now && row.termination.is_none());
        Ok(found.map(|row| {
            row.termination = Some(crate::playback::Termination { at: now, reason });
            row.updated_at = now;
            row.expires_at = now + crate::playback::SESSION_LIFETIME_SECONDS;
            row.clone()
        }))
    }

    async fn sessions(
        &self,
        only: Option<crate::ids::UserId>,
        status: crate::playback::SessionStatus,
        now: i64,
        _window: Window,
    ) -> Result<Page<crate::playback::PlaybackSession>, crate::playback::PlaybackError> {
        Ok(Page {
            items: self
                .rows()
                .iter()
                .filter(|row| {
                    row.expires_at > now
                        && row.termination.is_some()
                            == (status == crate::playback::SessionStatus::Terminated)
                })
                .filter(|row| only.is_none_or(|user| row.user == user))
                .cloned()
                .collect(),
            has_previous: false,
            has_next: false,
        })
    }

    async fn session(
        &self,
        id: i64,
        now: i64,
    ) -> Result<Option<crate::playback::PlaybackSession>, crate::playback::PlaybackError> {
        Ok(self
            .rows()
            .iter()
            .find(|row| row.id == id && row.expires_at > now)
            .cloned())
    }

    async fn expired(&self, now: i64) -> Result<Vec<String>, crate::playback::PlaybackError> {
        Ok(self
            .rows()
            .iter()
            .filter(|row| row.expires_at <= now)
            .map(|row| row.key.clone())
            .collect())
    }

    async fn delete_all(&self) -> Result<u64, crate::playback::PlaybackError> {
        let mut rows = self.rows();
        let before = rows.len();
        rows.clear();
        Ok(u64::try_from(before).unwrap_or(u64::MAX))
    }
}

/// Users held in memory, for tests that exercise the schema's access rules.
#[derive(Debug, Default)]
pub struct InMemoryAccess {
    users: std::sync::Mutex<Vec<crate::access::User>>,
    permissions: std::sync::Mutex<Vec<crate::access::LibraryPermission>>,
    extensions: std::sync::Mutex<Vec<(crate::access::UserExtension, String)>>,
    accounts: std::sync::Mutex<Vec<crate::access::AccountLink>>,
    passwords: std::sync::Mutex<Vec<(crate::ids::UserId, String)>>,
}

impl InMemoryAccess {
    /// An active user with no password, as an owner creates one.
    ///
    /// # Errors
    ///
    /// As [`crate::access::AccessRepository::create_user`].
    pub fn add_user(
        &self,
        username: &str,
        role: crate::access::ServerRole,
    ) -> Result<crate::access::User, crate::access::AccessError> {
        self.insert_user(
            crate::access::NewUser {
                username,
                email: None,
                role,
                active: true,
                password_hash: None,
            },
            0,
        )
    }

    fn insert_user(
        &self,
        new: crate::access::NewUser<'_>,
        now: i64,
    ) -> Result<crate::access::User, crate::access::AccessError> {
        let mut users = locked(&self.users);
        if users.iter().any(|user| user.username == new.username) {
            return Err(crate::access::AccessError::UsernameTaken);
        }
        if new.email.is_some() && users.iter().any(|user| user.email.as_deref() == new.email) {
            return Err(crate::access::AccessError::EmailTaken);
        }
        let user = crate::access::User {
            id: crate::ids::UserId::new(i64::try_from(users.len()).unwrap_or(0) + 1),
            username: new.username.to_owned(),
            email: new.email.map(str::to_owned),
            role: new.role,
            active: new.active,
            created_at: now,
            updated_at: now,
        };
        users.push(user.clone());
        if let Some(hash) = new.password_hash {
            locked(&self.passwords).push((user.id, hash.to_owned()));
        }
        Ok(user)
    }

    /// The user behind a grant, when it is active.
    fn active_user(&self, id: crate::ids::UserId) -> Option<crate::access::User> {
        locked(&self.users)
            .iter()
            .find(|user| user.id == id && user.active)
            .cloned()
    }
}

/// An account service that answers from the code or token it is given.
#[derive(Debug, Default)]
pub struct StubDirectory {
    /// The addresses announced so far.
    pub published: std::sync::Mutex<Vec<String>>,
    /// How many tokens it was asked about.
    pub lookups: std::sync::atomic::AtomicUsize,
    /// Answer every publication and lookup as an outage.
    pub offline: std::sync::atomic::AtomicBool,
}

impl StubDirectory {
    /// The token of every account this directory knows.
    pub const ACCOUNT_TOKEN: &'static str = "remote-account-token";

    fn is_offline(&self) -> bool {
        self.offline.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn outage() -> crate::access::AccessError {
        crate::access::AccessError::Unreachable("offline".into())
    }
}

#[async_trait::async_trait]
impl crate::access::AccountDirectory for StubDirectory {
    async fn redeem(
        &self,
        _server_id: &str,
        code: &str,
    ) -> Result<crate::access::RemoteAccount, crate::access::AccessError> {
        let id = code
            .strip_prefix("claim-")
            .ok_or(crate::access::AccessError::Refused)?;
        Ok(crate::access::RemoteAccount {
            id: id.to_owned(),
            name: format!("account {id}"),
            token: Self::ACCOUNT_TOKEN.to_owned(),
        })
    }

    async fn publish(
        &self,
        link: &crate::access::AccountLink,
    ) -> Result<(), crate::access::AccessError> {
        if self.is_offline() {
            return Err(Self::outage());
        }
        locked(&self.published).push(link.connection_uri.clone());
        Ok(())
    }

    async fn account_of(
        &self,
        token: &str,
    ) -> Result<Option<crate::access::RemoteAccount>, crate::access::AccessError> {
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.is_offline() {
            return Err(Self::outage());
        }
        Ok(
            (token == Self::ACCOUNT_TOKEN).then(|| crate::access::RemoteAccount {
                id: "linked".to_owned(),
                name: "account linked".to_owned(),
                token: token.to_owned(),
            }),
        )
    }
}

fn locked<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[async_trait::async_trait]
impl crate::access::AccessRepository for InMemoryAccess {
    async fn create_user(
        &self,
        new: crate::access::NewUser<'_>,
        now: i64,
    ) -> Result<crate::access::User, crate::access::AccessError> {
        self.insert_user(new, now)
    }

    async fn credentials(
        &self,
        login: &crate::access::Login,
    ) -> Result<Option<(crate::access::User, String)>, crate::access::AccessError> {
        let Some(user) = locked(&self.users)
            .iter()
            .find(|user| match login {
                crate::access::Login::Username(name) => &user.username == name,
                crate::access::Login::Email(email) => user.email.as_ref() == Some(email),
            })
            .cloned()
        else {
            return Ok(None);
        };
        Ok(locked(&self.passwords)
            .iter()
            .find(|(owner, _)| *owner == user.id)
            .map(|(_, hash)| (user, hash.clone())))
    }

    async fn user_named(
        &self,
        username: &str,
    ) -> Result<Option<crate::access::User>, crate::access::AccessError> {
        Ok(locked(&self.users)
            .iter()
            .find(|user| user.username == username)
            .cloned())
    }

    async fn has_password(
        &self,
        user: crate::ids::UserId,
    ) -> Result<bool, crate::access::AccessError> {
        Ok(locked(&self.passwords)
            .iter()
            .any(|(owner, _)| *owner == user))
    }

    async fn set_password(
        &self,
        user: crate::ids::UserId,
        password_hash: &str,
        _now: i64,
    ) -> Result<(), crate::access::AccessError> {
        if !locked(&self.users).iter().any(|held| held.id == user) {
            return Err(crate::access::AccessError::UnknownUser);
        }
        let mut passwords = locked(&self.passwords);
        passwords.retain(|(owner, _)| *owner != user);
        passwords.push((user, password_hash.to_owned()));
        Ok(())
    }

    async fn set_role(
        &self,
        user: crate::ids::UserId,
        role: crate::access::ServerRole,
        now: i64,
    ) -> Result<Option<crate::access::User>, crate::access::AccessError> {
        let mut users = locked(&self.users);
        Ok(users.iter_mut().find(|held| held.id == user).map(|held| {
            held.role = role;
            held.updated_at = now;
            held.clone()
        }))
    }

    async fn set_active(
        &self,
        user: crate::ids::UserId,
        active: bool,
        now: i64,
    ) -> Result<Option<crate::access::User>, crate::access::AccessError> {
        let mut users = locked(&self.users);
        Ok(users.iter_mut().find(|held| held.id == user).map(|held| {
            held.active = active;
            held.updated_at = now;
            held.clone()
        }))
    }

    async fn users(
        &self,
        _window: Window,
    ) -> Result<Page<crate::access::User>, crate::access::AccessError> {
        Ok(Page {
            items: locked(&self.users).clone(),
            has_previous: false,
            has_next: false,
        })
    }

    async fn user(
        &self,
        id: crate::ids::UserId,
    ) -> Result<Option<crate::access::User>, crate::access::AccessError> {
        Ok(locked(&self.users)
            .iter()
            .find(|user| user.id == id)
            .cloned())
    }

    async fn delete_user(
        &self,
        id: crate::ids::UserId,
    ) -> Result<bool, crate::access::AccessError> {
        locked(&self.permissions).retain(|permission| permission.user != id);
        locked(&self.extensions).retain(|(extension, _)| extension.user != id);
        locked(&self.accounts).retain(|link| link.user != id);
        locked(&self.passwords).retain(|(owner, _)| *owner != id);
        let mut users = locked(&self.users);
        let before = users.len();
        users.retain(|user| user.id != id);
        Ok(users.len() < before)
    }

    async fn set_permission(
        &self,
        permission: crate::access::LibraryPermission,
        _now: i64,
    ) -> Result<(), crate::access::AccessError> {
        if !locked(&self.users)
            .iter()
            .any(|user| user.id == permission.user)
        {
            return Err(crate::access::AccessError::UnknownUser);
        }
        let mut permissions = locked(&self.permissions);
        permissions
            .retain(|held| (held.user, held.section) != (permission.user, permission.section));
        permissions.push(permission);
        Ok(())
    }

    async fn remove_permission(
        &self,
        user: crate::ids::UserId,
        section: SectionId,
    ) -> Result<bool, crate::access::AccessError> {
        let mut permissions = locked(&self.permissions);
        let before = permissions.len();
        permissions.retain(|held| (held.user, held.section) != (user, section));
        Ok(permissions.len() < before)
    }

    async fn permissions(
        &self,
        user: crate::ids::UserId,
    ) -> Result<Vec<crate::access::LibraryPermission>, crate::access::AccessError> {
        Ok(locked(&self.permissions)
            .iter()
            .filter(|held| held.user == user)
            .copied()
            .collect())
    }

    async fn connect_extension(
        &self,
        user: crate::ids::UserId,
        extension: crate::access::Extension,
        external_id: &str,
        token_digest: &str,
        _now: i64,
    ) -> Result<crate::access::UserExtension, crate::access::AccessError> {
        let mut extensions = locked(&self.extensions);
        if let Some((held, digest)) = extensions
            .iter_mut()
            .find(|(held, _)| held.user == user && held.extension == extension)
        {
            token_digest.clone_into(digest);
            held.enabled = true;
            return Ok(held.clone());
        }
        let connected = crate::access::UserExtension {
            user,
            extension,
            external_id: external_id.to_owned(),
            enabled: true,
        };
        extensions.push((connected.clone(), token_digest.to_owned()));
        Ok(connected)
    }

    async fn disable_extension(
        &self,
        user: crate::ids::UserId,
        extension: crate::access::Extension,
        _now: i64,
    ) -> Result<bool, crate::access::AccessError> {
        let mut extensions = locked(&self.extensions);
        let found = extensions
            .iter_mut()
            .find(|(held, _)| held.user == user && held.extension == extension && held.enabled);
        Ok(found.map(|(held, _)| held.enabled = false).is_some())
    }

    async fn extensions(
        &self,
        user: crate::ids::UserId,
    ) -> Result<Vec<crate::access::UserExtension>, crate::access::AccessError> {
        Ok(locked(&self.extensions)
            .iter()
            .filter(|(held, _)| held.user == user)
            .map(|(held, _)| held.clone())
            .collect())
    }

    async fn grant(
        &self,
        extension: crate::access::Extension,
        token_digest: &str,
    ) -> Result<Option<crate::access::Grant>, crate::access::AccessError> {
        let found = locked(&self.extensions)
            .iter()
            .find(|(held, digest)| {
                held.extension == extension && held.enabled && digest == token_digest
            })
            .map(|(held, _)| held.clone());
        let Some(extension) = found else {
            return Ok(None);
        };
        let Some(user) = self.active_user(extension.user) else {
            return Ok(None);
        };
        let permissions = self.permissions(user.id).await?;
        Ok(Some(crate::access::Grant {
            user,
            extension,
            permissions,
        }))
    }

    async fn link_account(
        &self,
        user: crate::ids::UserId,
        extension: crate::access::Extension,
        account: &crate::access::RemoteAccount,
        connection_uri: &str,
        _now: i64,
    ) -> Result<crate::access::AccountLink, crate::access::AccessError> {
        let connection = locked(&self.extensions)
            .iter()
            .find(|(held, _)| held.user == user && held.extension == extension)
            .map(|(held, _)| held.clone())
            .ok_or(crate::access::AccessError::NotConnected)?;
        let mut accounts = locked(&self.accounts);
        if accounts
            .iter()
            .any(|link| link.account.id == account.id && link.user != user)
        {
            return Err(crate::access::AccessError::AccountTaken);
        }
        accounts.retain(|link| (link.user, link.extension) != (user, extension));
        let link = crate::access::AccountLink {
            user,
            extension,
            server_id: connection.external_id,
            enabled: connection.enabled,
            account: account.clone(),
            connection_uri: connection_uri.to_owned(),
            published_at: None,
        };
        accounts.push(link.clone());
        Ok(link)
    }

    async fn account_link(
        &self,
        user: crate::ids::UserId,
        extension: crate::access::Extension,
    ) -> Result<Option<crate::access::AccountLink>, crate::access::AccessError> {
        let enabled = locked(&self.extensions)
            .iter()
            .any(|(held, _)| held.user == user && held.extension == extension && held.enabled);
        Ok(locked(&self.accounts)
            .iter()
            .find(|link| (link.user, link.extension) == (user, extension))
            .cloned()
            .map(|link| crate::access::AccountLink { enabled, ..link }))
    }

    async fn connection_at(
        &self,
        extension: crate::access::Extension,
        connection_uris: &[String],
    ) -> Result<Option<crate::access::UserExtension>, crate::access::AccessError> {
        let owners: Vec<crate::ids::UserId> = locked(&self.accounts)
            .iter()
            .filter(|link| {
                link.extension == extension && connection_uris.contains(&link.connection_uri)
            })
            .map(|link| link.user)
            .collect();
        let extensions = locked(&self.extensions);
        let mut enabled = extensions.iter().filter(|(held, _)| {
            held.extension == extension && held.enabled && owners.contains(&held.user)
        });
        Ok(match (enabled.next(), enabled.next()) {
            (Some((held, _)), None) => Some(held.clone()),
            _ => None,
        })
    }

    async fn unlink_account(
        &self,
        user: crate::ids::UserId,
        extension: crate::access::Extension,
    ) -> Result<bool, crate::access::AccessError> {
        let mut accounts = locked(&self.accounts);
        let before = accounts.len();
        accounts.retain(|link| (link.user, link.extension) != (user, extension));
        Ok(accounts.len() < before)
    }

    async fn move_account(
        &self,
        user: crate::ids::UserId,
        extension: crate::access::Extension,
        connection_uri: &str,
        _now: i64,
    ) -> Result<Option<crate::access::AccountLink>, crate::access::AccessError> {
        if let Some(link) = locked(&self.accounts)
            .iter_mut()
            .find(|link| (link.user, link.extension) == (user, extension))
        {
            connection_uri.clone_into(&mut link.connection_uri);
            link.published_at = None;
        }
        self.account_link(user, extension).await
    }

    async fn mark_published(
        &self,
        user: crate::ids::UserId,
        extension: crate::access::Extension,
        at: i64,
    ) -> Result<(), crate::access::AccessError> {
        if let Some(link) = locked(&self.accounts)
            .iter_mut()
            .find(|link| (link.user, link.extension) == (user, extension))
        {
            link.published_at = Some(at);
        }
        Ok(())
    }

    async fn grant_for_account(
        &self,
        extension: crate::access::Extension,
        account_id: &str,
    ) -> Result<Option<crate::access::Grant>, crate::access::AccessError> {
        let found = locked(&self.accounts)
            .iter()
            .find(|link| link.extension == extension && link.account.id == account_id)
            .map(|link| link.user);
        let Some(owner) = found else {
            return Ok(None);
        };
        let connection = locked(&self.extensions)
            .iter()
            .find(|(held, _)| held.user == owner && held.extension == extension && held.enabled)
            .map(|(held, _)| held.clone());
        let (Some(extension), Some(user)) = (connection, self.active_user(owner)) else {
            return Ok(None);
        };
        let permissions = self.permissions(user.id).await?;
        Ok(Some(crate::access::Grant {
            user,
            extension,
            permissions,
        }))
    }
}

/// Play queues held in memory, edited by the same rules as stored ones.
#[derive(Debug, Default)]
pub struct InMemoryPlayQueues {
    state: std::sync::Mutex<InMemoryQueueState>,
}

#[derive(Debug, Default)]
struct InMemoryQueueState {
    queues: Vec<InMemoryQueue>,
    next_entry: i64,
    next_queue: i64,
}

#[derive(Debug)]
struct InMemoryQueue {
    queue: crate::play_queue::PlayQueue,
    contents: crate::play_queue::PlayQueueContents,
    source: Vec<RatingKey>,
    created: i64,
}

impl InMemoryQueueState {
    /// Gives the new entries an id, and restates the queue from its contents.
    fn settle(&mut self, index: usize) -> crate::play_queue::PlayQueue {
        let next = &mut self.next_entry;
        let stored = &mut self.queues[index];
        for slot in &mut stored.contents.slots {
            if slot.entry.is_none() {
                *next += 1;
                slot.entry = Some(*next);
            }
        }
        let id = stored.queue.id;
        let entry_at = |at: Option<usize>| {
            at.and_then(|at| {
                let slot = stored.contents.slots.get(at)?;
                Some(crate::play_queue::PlayQueueEntry {
                    id: slot.entry?,
                    queue: id,
                    position: i64::try_from(at).ok()?,
                    item: slot.item,
                })
            })
        };
        stored.queue.selected = entry_at(stored.contents.selected);
        stored.queue.last_added = entry_at(stored.contents.last_added);
        stored.queue.shuffled = stored.contents.shuffled;
        stored.queue.total = u64::try_from(stored.contents.slots.len()).unwrap_or(u64::MAX);
        stored.queue.clone()
    }

    fn entries(&self, queue: i64) -> Vec<crate::play_queue::PlayQueueEntry> {
        self.queues
            .iter()
            .filter(|stored| stored.queue.id == queue)
            .flat_map(|stored| {
                stored
                    .contents
                    .slots
                    .iter()
                    .zip(0_i64..)
                    .filter_map(move |(slot, position)| {
                        Some(crate::play_queue::PlayQueueEntry {
                            id: slot.entry?,
                            queue,
                            position,
                            item: slot.item,
                        })
                    })
            })
            .collect()
    }
}

impl InMemoryPlayQueues {
    fn state(&self) -> std::sync::MutexGuard<'_, InMemoryQueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl crate::play_queue::PlayQueueRepository for InMemoryPlayQueues {
    async fn create(
        &self,
        queue: crate::play_queue::NewPlayQueue,
        now: i64,
    ) -> Result<crate::play_queue::PlayQueue, crate::play_queue::PlayQueueError> {
        let mut state = self.state();
        if queue.client.is_some() {
            state.queues.retain(|stored| {
                (stored.queue.user, &stored.queue.client, stored.queue.kind)
                    != (queue.user, &queue.client, queue.kind)
            });
        }
        state.next_queue += 1;
        let contents = queue.initial_contents();
        let stored = InMemoryQueue {
            queue: crate::play_queue::PlayQueue {
                id: state.next_queue,
                user: queue.user,
                client: queue.client,
                kind: queue.kind,
                source_uri: queue.source_uri,
                shuffled: contents.shuffled,
                version: 1,
                total: 0,
                selected: None,
                last_added: None,
                playlist: queue.playlist,
            },
            contents,
            source: queue.source,
            created: now,
        };
        state.queues.push(stored);
        let index = state.queues.len() - 1;
        Ok(state.settle(index))
    }

    async fn queue(
        &self,
        id: i64,
    ) -> Result<Option<crate::play_queue::PlayQueue>, crate::play_queue::PlayQueueError> {
        Ok(self
            .state()
            .queues
            .iter()
            .find(|stored| stored.queue.id == id)
            .map(|stored| stored.queue.clone()))
    }

    async fn entry(
        &self,
        id: i64,
    ) -> Result<Option<crate::play_queue::PlayQueueEntry>, crate::play_queue::PlayQueueError> {
        let state = self.state();
        Ok(state
            .queues
            .iter()
            .flat_map(|stored| state.entries(stored.queue.id))
            .find(|entry| entry.id == id))
    }

    async fn entries(
        &self,
        queue: i64,
        window: Window,
    ) -> Result<Page<crate::play_queue::PlayQueueEntry>, crate::play_queue::PlayQueueError> {
        let items: Vec<_> = self
            .state()
            .entries(queue)
            .into_iter()
            .filter(|entry| {
                window
                    .after
                    .as_ref()
                    .is_none_or(|after| entry.position > after.id)
            })
            .filter(|entry| {
                window
                    .before
                    .as_ref()
                    .is_none_or(|before| entry.position < before.id)
            })
            .skip(usize::try_from(window.skip).unwrap_or(0))
            .take(usize::try_from(window.fetch_limit()).unwrap_or(usize::MAX))
            .collect();
        Ok(Page::from_overfetch(items, &window))
    }

    async fn change(
        &self,
        queue: i64,
        change: &crate::play_queue::PlayQueueChange,
    ) -> Result<crate::play_queue::ChangeOutcome, crate::play_queue::PlayQueueError> {
        let mut state = self.state();
        let Some(index) = state
            .queues
            .iter()
            .position(|stored| stored.queue.id == queue)
        else {
            return Ok(None);
        };
        let stored = &mut state.queues[index];
        let mut contents = stored.contents.clone();
        match change.apply(&mut contents, &stored.source) {
            Ok(raised) => {
                stored.contents = contents;
                if raised {
                    stored.queue.version += 1;
                }
                Ok(Some(Ok(state.settle(index))))
            }
            Err(refusal) => Ok(Some(Err(refusal))),
        }
    }

    async fn delete_created_before(
        &self,
        before: i64,
    ) -> Result<u64, crate::play_queue::PlayQueueError> {
        let mut state = self.state();
        let count = state.queues.len();
        state.queues.retain(|stored| stored.created >= before);
        Ok(u64::try_from(count - state.queues.len()).unwrap_or(u64::MAX))
    }
}

/// Watch state held in memory, over the items whose episodes it counts.
#[derive(Debug, Default)]
pub struct InMemoryWatch {
    /// The library's items, for counting the episodes below a series or a season.
    items: Vec<MetadataItem>,
    states: std::sync::Mutex<Vec<(crate::ids::UserId, String, crate::watch::WatchState)>>,
    viewings: std::sync::Mutex<Vec<(crate::ids::UserId, String, crate::watch::Viewing, i64)>>,
    choices: std::sync::Mutex<Vec<(crate::ids::UserId, PartId, crate::tracks::TrackChoice)>>,
    history: std::sync::Mutex<Vec<crate::history::HistoryEntry>>,
}

impl InMemoryWatch {
    pub const fn over(items: Vec<MetadataItem>) -> Self {
        Self {
            items,
            states: std::sync::Mutex::new(Vec::new()),
            viewings: std::sync::Mutex::new(Vec::new()),
            choices: std::sync::Mutex::new(Vec::new()),
            history: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The entries a query reads, ordered, before any window.
    fn read_history(
        &self,
        query: &crate::history::HistoryQuery,
    ) -> Vec<crate::history::HistoryEntry> {
        let parent_of = |id: RatingKey| self.items.iter().find(|item| item.id == id)?.parent_id;
        let mut entries: Vec<crate::history::HistoryEntry> = Self::lock(&self.history)
            .iter()
            .filter(|entry| {
                let parents: Vec<RatingKey> =
                    std::iter::successors(parent_of(entry.item), |id| parent_of(*id)).collect();
                query.admits(entry, &parents)
            })
            .copied()
            .collect();
        entries.sort_by(|a, b| {
            let key = |entry: &crate::history::HistoryEntry| match query.order {
                crate::history::HistoryOrder::Recorded => 0,
                crate::history::HistoryOrder::ViewedAt => entry.viewed_at,
                crate::history::HistoryOrder::Library => entry.section.get(),
            };
            key(a).cmp(&key(b)).then(a.id.cmp(&b.id))
        });
        if query.direction.is_descending() {
            entries.reverse();
        }
        entries
    }

    fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        // A test that panicked while holding the lock has already failed; the data is still
        // what it wrote.
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl crate::watch::WatchRepository for InMemoryWatch {
    async fn states(
        &self,
        viewer: crate::ids::UserId,
        guids: &[String],
    ) -> Result<Vec<(String, crate::watch::WatchState)>, crate::watch::WatchError> {
        Ok(Self::lock(&self.states)
            .iter()
            .filter(|(who, guid, _)| *who == viewer && guids.contains(guid))
            .map(|(_, guid, state)| (guid.clone(), *state))
            .collect())
    }

    async fn save(
        &self,
        viewer: crate::ids::UserId,
        states: &[(String, crate::watch::WatchState)],
    ) -> Result<(), crate::watch::WatchError> {
        let mut stored = Self::lock(&self.states);
        for (guid, state) in states {
            stored.retain(|(who, kept, _)| !(*who == viewer && kept == guid));
            if !state.is_empty() {
                stored.push((viewer, guid.clone(), *state));
            }
        }
        Ok(())
    }

    async fn record_viewings(
        &self,
        viewer: crate::ids::UserId,
        states: &[(String, crate::watch::WatchState)],
        viewings: &[crate::history::NewViewing],
    ) -> Result<(), crate::watch::WatchError> {
        self.save(viewer, states).await?;
        let mut history = Self::lock(&self.history);
        for viewing in viewings {
            let Some(item) = self.items.iter().find(|item| item.id == viewing.item) else {
                continue;
            };
            let id = history.last().map_or(1, |last| last.id.get() + 1);
            history.push(crate::history::HistoryEntry {
                id: crate::HistoryId::new(id),
                viewer,
                item: viewing.item,
                section: item.section_id,
                kind: viewing.kind,
                viewed_at: viewing.viewed_at,
            });
        }
        Ok(())
    }

    /// Windows by position only, after any cursor's entry: enough for the tests, which page
    /// forward.
    async fn history(
        &self,
        query: &crate::history::HistoryQuery,
        window: Window,
    ) -> Result<Page<crate::history::HistoryEntry>, crate::watch::WatchError> {
        let entries = self.read_history(query);
        let start = window.after.as_ref().map_or(0, |after| {
            entries
                .iter()
                .position(|entry| entry.id.get() == after.id)
                .map_or(entries.len(), |position| position + 1)
        });
        let rows: Vec<crate::history::HistoryEntry> = entries
            .into_iter()
            .skip(start)
            .skip(usize::try_from(window.skip).unwrap_or(usize::MAX))
            .take(usize::try_from(window.fetch_limit()).unwrap_or(usize::MAX))
            .collect();
        Ok(Page::from_overfetch(rows, &window))
    }

    async fn history_count(
        &self,
        query: &crate::history::HistoryQuery,
    ) -> Result<u64, crate::watch::WatchError> {
        Ok(self.read_history(query).len() as u64)
    }

    async fn history_entry(
        &self,
        id: crate::HistoryId,
    ) -> Result<Option<crate::history::HistoryEntry>, crate::watch::WatchError> {
        Ok(Self::lock(&self.history)
            .iter()
            .find(|entry| entry.id == id)
            .copied())
    }

    async fn delete_history_entry(
        &self,
        id: crate::HistoryId,
    ) -> Result<bool, crate::watch::WatchError> {
        let mut history = Self::lock(&self.history);
        let count = history.len();
        history.retain(|entry| entry.id != id);
        Ok(history.len() < count)
    }

    async fn watched_leaves(
        &self,
        viewer: crate::ids::UserId,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, crate::watch::WatchError> {
        let stored = Self::lock(&self.states);
        let watched = |guid: &str| {
            stored
                .iter()
                .any(|(who, kept, state)| *who == viewer && kept == guid && state.view_count > 0)
        };
        let parent_of = |id: RatingKey| self.items.iter().find(|item| item.id == id)?.parent_id;
        Ok(parents
            .iter()
            .map(|parent| {
                let count = self
                    .items
                    .iter()
                    .filter(|item| item.kind == ItemKind::Episode && watched(&item.guid))
                    .filter(|item| {
                        item.parent_id == Some(*parent)
                            || item.parent_id.and_then(parent_of) == Some(*parent)
                    })
                    .count() as u64;
                (*parent, count)
            })
            .filter(|(_, count)| *count > 0)
            .collect())
    }

    async fn recent(
        &self,
        viewer: crate::ids::UserId,
        since: i64,
        limit: u32,
    ) -> Result<Vec<(String, crate::watch::WatchState)>, crate::watch::WatchError> {
        let mut recent: Vec<(String, crate::watch::WatchState)> = Self::lock(&self.states)
            .iter()
            .filter(|(who, _, state)| {
                *who == viewer && state.last_viewed_at.is_some_and(|at| at >= since)
            })
            .map(|(_, guid, state)| (guid.clone(), *state))
            .collect();
        recent.sort_by(|a, b| b.1.last_viewed_at.cmp(&a.1.last_viewed_at));
        recent.truncate(limit as usize);
        Ok(recent)
    }

    async fn viewing(
        &self,
        viewer: crate::ids::UserId,
        playback: &str,
        now: i64,
    ) -> Result<Option<crate::watch::Viewing>, crate::watch::WatchError> {
        Ok(Self::lock(&self.viewings)
            .iter()
            .find(|(who, key, _, expires)| *who == viewer && key == playback && *expires > now)
            .map(|(_, _, viewing, _)| *viewing))
    }

    async fn remember(
        &self,
        viewer: crate::ids::UserId,
        playback: &str,
        viewing: crate::watch::Viewing,
        now: i64,
    ) -> Result<(), crate::watch::WatchError> {
        let mut viewings = Self::lock(&self.viewings);
        viewings.retain(|(who, key, _, expires)| {
            !(*who == viewer && (key == playback || *expires <= now))
        });
        viewings.push((
            viewer,
            playback.to_owned(),
            viewing,
            now + crate::watch::VIEWING_LIFETIME_SECONDS,
        ));
        Ok(())
    }

    async fn track_choices(
        &self,
        viewer: crate::ids::UserId,
        files: &[PartId],
    ) -> Result<Vec<(PartId, crate::tracks::TrackChoice)>, crate::watch::WatchError> {
        Ok(Self::lock(&self.choices)
            .iter()
            .filter(|(who, file, _)| *who == viewer && files.contains(file))
            .map(|(_, file, choice)| (*file, *choice))
            .collect())
    }

    async fn choose_tracks(
        &self,
        viewer: crate::ids::UserId,
        file: PartId,
        choice: crate::tracks::TrackChoice,
    ) -> Result<(), crate::watch::WatchError> {
        let mut choices = Self::lock(&self.choices);
        choices.retain(|(who, held, _)| !(*who == viewer && *held == file));
        if !choice.is_empty() {
            choices.push((viewer, file, choice));
        }
        Ok(())
    }
}

/// A stream engine that runs nothing: it records what it was asked and answers as an engine
/// copying both tracks would.
#[derive(Debug, Default)]
pub struct StubStreams {
    pub started: std::sync::Mutex<Vec<crate::stream::StreamOrder>>,
    /// The keys it was asked to stop, in order.
    pub stopped: std::sync::Mutex<Vec<String>>,
}

impl StubStreams {
    fn lock_stopped(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.stopped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl crate::stream::StreamEngine for StubStreams {
    async fn start(
        &self,
        order: crate::stream::StreamOrder,
    ) -> Result<crate::stream::StartedStream, crate::stream::StreamError> {
        let url = match order.delivery {
            crate::stream::Delivery::Original => "/streams/stub/file?token=stub",
            crate::stream::Delivery::Converted(crate::stream::StreamProtocol::Dash) => {
                "/streams/stub/manifest.mpd?token=stub"
            }
            crate::stream::Delivery::Converted(crate::stream::StreamProtocol::Hls) => {
                "/streams/stub/master.m3u8?token=stub"
            }
        };
        let started = crate::stream::StartedStream {
            url: url.to_owned(),
            subtitles_url: None,
            video: crate::playback::StreamDecision::Copy,
            audio: crate::playback::StreamDecision::Copy,
            output: crate::playback::PlaybackOutput {
                video_bitrate: order.source.bitrate,
                audio_bitrate: order.source.audio.bitrate,
                width: order.source.width,
                height: order.source.height,
            },
            replaced: Vec::new(),
        };
        self.started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(order);
        Ok(started)
    }

    fn stop(&self, key: &str) -> bool {
        let mut stopped = self.lock_stopped();
        stopped.push(key.to_owned());
        false
    }

    fn touch(&self, _key: &str) -> bool {
        false
    }

    fn idle(&self) -> Vec<String> {
        Vec::new()
    }

    fn stop_if_idle(&self, _key: &str) -> bool {
        false
    }

    async fn stop_all(&self) {}
}

/// Playlists held in memory, over the items whose libraries and durations they read.
#[derive(Debug, Default)]
pub struct InMemoryPlaylists {
    items: Vec<MetadataItem>,
    state: std::sync::Mutex<PlaylistStore>,
}

#[derive(Debug, Default)]
struct PlaylistStore {
    playlists: Vec<Playlist>,
    entries: Vec<PlaylistEntry>,
    next_id: i64,
}

impl PlaylistStore {
    fn next(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn find(&mut self, id: PlaylistId) -> Result<&mut Playlist, PlaylistError> {
        self.playlists
            .iter_mut()
            .find(|playlist| playlist.id == id)
            .ok_or(PlaylistError::NotFound(id))
    }

    fn ordered(&self, playlist: PlaylistId) -> Vec<PlaylistEntry> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.playlist == playlist)
            .copied()
            .collect();
        entries.sort_by_key(|entry| (entry.position, entry.id));
        entries
    }

    fn entry(&self, id: PlaylistEntryId) -> Result<PlaylistEntry, PlaylistError> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .copied()
            .ok_or(PlaylistError::EntryNotFound(id))
    }

    fn append(&mut self, playlist: PlaylistId, items: &[RatingKey], now: i64) -> u64 {
        let ordered = self.ordered(playlist);
        let present: Vec<RatingKey> = ordered.iter().map(|entry| entry.item).collect();
        let mut last = ordered.last().map(|entry| entry.position);
        let added = crate::playlist::new_entries(items, &present);
        for item in &added {
            let position = crate::playlist::position_between(last, None).unwrap_or(0);
            let id = PlaylistEntryId::new(self.next());
            self.entries.push(PlaylistEntry {
                id,
                playlist,
                item: *item,
                position,
                added_at: now,
            });
            last = Some(position);
        }
        u64::try_from(added.len()).unwrap_or(u64::MAX)
    }
}

impl InMemoryPlaylists {
    pub fn new(items: Vec<MetadataItem>) -> Self {
        Self {
            items,
            state: std::sync::Mutex::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PlaylistStore> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn visible(&self, item: RatingKey, sections: Option<&[SectionId]>) -> Option<&MetadataItem> {
        self.items
            .iter()
            .find(|candidate| candidate.id == item)
            .filter(|found| sections.is_none_or(|sections| sections.contains(&found.section_id)))
    }
}

fn playlist_matches(playlist: &Playlist, query: &PlaylistQuery) -> bool {
    let filter = &query.filter;
    playlist.owner == query.owner
        && filter.kind.is_none_or(|kind| playlist.kind == kind)
        && filter
            .smart
            .is_none_or(|smart| playlist.is_smart() == smart)
        && filter.library.is_none_or(|library| {
            playlist
                .rule
                .as_ref()
                .is_none_or(|rule| rule.section == library)
        })
}

fn playlist_order(query: &PlaylistQuery, a: &Playlist, b: &Playlist) -> std::cmp::Ordering {
    let by_title = a.sort_key().cmp(&b.sort_key()).then(a.id.cmp(&b.id));
    let descending = query.direction.is_descending();
    let value = |playlist: &Playlist| match query.sort {
        PlaylistSort::Title => 0,
        PlaylistSort::AddedAt => playlist.added_at,
        PlaylistSort::UpdatedAt => playlist.updated_at,
        PlaylistSort::LastViewedAt => playlist.last_viewed_at,
    };
    if query.sort == PlaylistSort::Title {
        let title = if descending {
            by_title.reverse()
        } else {
            by_title
        };
        b.is_smart().cmp(&a.is_smart()).then(title)
    } else {
        let leading = value(a).cmp(&value(b));
        let leading = if descending {
            leading.reverse()
        } else {
            leading
        };
        leading.then(by_title)
    }
}

#[async_trait::async_trait]
impl PlaylistRepository for InMemoryPlaylists {
    async fn create(&self, new: NewPlaylist, now: i64) -> Result<Playlist, PlaylistError> {
        let mut store = self.lock();
        let id = PlaylistId::new(store.next());
        let playlist = Playlist {
            id,
            owner: new.owner,
            guid: format!("guid-{id}"),
            title: new.title,
            summary: new.summary,
            kind: new.kind,
            rule: new.rule,
            source_uri: new.source_uri,
            locked: Vec::new(),
            view_count: 1,
            last_viewed_at: now,
            added_at: now,
            updated_at: now,
        };
        store.playlists.push(playlist.clone());
        store.append(id, &new.items, now);
        Ok(playlist)
    }

    async fn playlist(&self, id: PlaylistId) -> Result<Option<Playlist>, PlaylistError> {
        Ok(self
            .lock()
            .playlists
            .iter()
            .find(|playlist| playlist.id == id)
            .cloned())
    }

    async fn playlists(
        &self,
        query: &PlaylistQuery,
        window: Window,
    ) -> Result<Page<Playlist>, PlaylistError> {
        let mut found: Vec<_> = self
            .lock()
            .playlists
            .iter()
            .filter(|playlist| playlist_matches(playlist, query))
            .cloned()
            .collect();
        found.sort_by(|a, b| playlist_order(query, a, b));
        Ok(windowed(found, &window, |playlist| query.cursor(playlist)))
    }

    async fn count(&self, query: &PlaylistQuery) -> Result<u64, PlaylistError> {
        let count = self
            .lock()
            .playlists
            .iter()
            .filter(|playlist| playlist_matches(playlist, query))
            .count();
        Ok(u64::try_from(count).unwrap_or(u64::MAX))
    }

    async fn edit(
        &self,
        id: PlaylistId,
        edit: &PlaylistEdit,
        now: i64,
    ) -> Result<Playlist, PlaylistError> {
        let mut store = self.lock();
        let playlist = store.find(id)?;
        if let Some(title) = &edit.title {
            playlist.title.clone_from(title);
        }
        if let Some(summary) = &edit.summary {
            playlist.summary.clone_from(summary);
        }
        playlist.locked.retain(|field| !edit.unlock.contains(field));
        playlist.locked.extend(edit.lock.iter().copied());
        playlist.locked.sort();
        playlist.locked.dedup();
        playlist.updated_at = now;
        Ok(playlist.clone())
    }

    async fn delete(&self, id: PlaylistId) -> Result<bool, PlaylistError> {
        let mut store = self.lock();
        let before = store.playlists.len();
        store.playlists.retain(|playlist| playlist.id != id);
        store.entries.retain(|entry| entry.playlist != id);
        Ok(store.playlists.len() != before)
    }

    async fn entry(&self, id: PlaylistEntryId) -> Result<Option<PlaylistEntry>, PlaylistError> {
        Ok(self.lock().entry(id).ok())
    }

    async fn entries(
        &self,
        playlist: PlaylistId,
        sections: Option<&[SectionId]>,
        window: Window,
    ) -> Result<Page<PlaylistEntry>, PlaylistError> {
        let ordered = self.lock().ordered(playlist);
        let visible: Vec<PlaylistEntry> = ordered
            .into_iter()
            .filter(|entry| self.visible(entry.item, sections).is_some())
            .collect();
        Ok(windowed(visible, &window, PlaylistEntry::cursor))
    }

    async fn totals(
        &self,
        playlists: &[PlaylistId],
        sections: Option<&[SectionId]>,
    ) -> Result<Vec<(PlaylistId, PlaylistTotals)>, PlaylistError> {
        let store = self.lock();
        let mut totals = Vec::new();
        for playlist in playlists {
            let mut total = PlaylistTotals::default();
            let entries = store
                .entries
                .iter()
                .filter(|entry| entry.playlist == *playlist);
            for item in entries.filter_map(|entry| self.visible(entry.item, sections)) {
                total.entries += 1;
                total.duration_ms += item.duration_ms.unwrap_or(0);
            }
            if total.entries > 0 {
                totals.push((*playlist, total));
            }
        }
        Ok(totals)
    }

    async fn append(
        &self,
        playlist: PlaylistId,
        items: &[RatingKey],
        now: i64,
    ) -> Result<u64, PlaylistError> {
        let mut store = self.lock();
        store.find(playlist)?.updated_at = now;
        Ok(store.append(playlist, items, now))
    }

    async fn remove(&self, entry: PlaylistEntryId, now: i64) -> Result<(), PlaylistError> {
        let mut store = self.lock();
        let found = store.entry(entry)?;
        store.entries.retain(|candidate| candidate.id != entry);
        store.find(found.playlist)?.updated_at = now;
        Ok(())
    }

    async fn move_entry(
        &self,
        entry: PlaylistEntryId,
        destination: Destination,
        now: i64,
    ) -> Result<(), PlaylistError> {
        let mut store = self.lock();
        let moved = store.entry(entry)?;
        let mut order: Vec<PlaylistEntryId> = store
            .ordered(moved.playlist)
            .into_iter()
            .map(|candidate| candidate.id)
            .filter(|candidate| *candidate != entry)
            .collect();
        let at = match destination {
            Destination::First => 0,
            Destination::After(after) => {
                order
                    .iter()
                    .position(|candidate| *candidate == after)
                    .ok_or(PlaylistError::EntryNotFound(after))?
                    + 1
            }
        };
        order.insert(at, entry);
        for (id, rank) in order.iter().zip(1_i64..) {
            if let Some(stored) = store.entries.iter_mut().find(|stored| stored.id == *id) {
                stored.position = rank * crate::playlist::POSITION_GAP;
            }
        }
        store.find(moved.playlist)?.updated_at = now;
        Ok(())
    }

    async fn clear(&self, playlist: PlaylistId, now: i64) -> Result<(), PlaylistError> {
        let mut store = self.lock();
        store.find(playlist)?.updated_at = now;
        store.entries.retain(|entry| entry.playlist != playlist);
        Ok(())
    }

    async fn replace_rule(
        &self,
        playlist: PlaylistId,
        rule: SmartRule,
        source_uri: Option<String>,
        now: i64,
    ) -> Result<Playlist, PlaylistError> {
        let mut store = self.lock();
        let found = store.find(playlist)?;
        found.rule = Some(rule);
        found.source_uri = source_uri;
        found.updated_at = now;
        Ok(found.clone())
    }

    async fn record_play(&self, playlist: PlaylistId, now: i64) -> Result<(), PlaylistError> {
        let mut store = self.lock();
        let found = store.find(playlist)?;
        found.view_count += 1;
        found.last_viewed_at = now;
        Ok(())
    }
}

/// Collections held in memory, over the items whose years they read.
///
/// Its store is shared with an [`InMemoryMetadata`] built `with_collections`, so a listing
/// filtered on a collection sees the same entries.
#[derive(Debug, Default)]
pub struct InMemoryCollections {
    items: Vec<MetadataItem>,
    state: std::sync::Arc<std::sync::Mutex<CollectionStore>>,
}

#[derive(Debug, Default)]
pub struct CollectionStore {
    collections: Vec<Collection>,
    /// In the order items joined, which is what an item's list of collections follows.
    entries: Vec<CollectionEntry>,
    next_id: i64,
}

impl CollectionStore {
    fn collection(&self, id: CollectionId) -> Option<&Collection> {
        self.collections
            .iter()
            .find(|collection| collection.id == id)
    }

    fn find(&mut self, id: CollectionId) -> Result<&mut Collection, CollectionError> {
        self.collections
            .iter_mut()
            .find(|collection| collection.id == id)
            .ok_or(CollectionError::NotFound(id))
    }

    fn ordered(&self, collection: CollectionId) -> Vec<CollectionEntry> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.collection == collection)
            .copied()
            .collect();
        entries.sort_by_key(|entry| (entry.position, entry.item));
        entries
    }

    fn append(&mut self, collection: CollectionId, items: &[RatingKey]) -> u64 {
        let ordered = self.ordered(collection);
        let present: Vec<RatingKey> = ordered.iter().map(|entry| entry.item).collect();
        let mut last = ordered.iter().map(|entry| entry.position).max();
        let added = crate::playlist::new_entries(items, &present);
        for item in &added {
            let position = crate::playlist::position_between(last, None).unwrap_or(0);
            self.entries.push(CollectionEntry {
                collection,
                item: *item,
                position,
            });
            last = Some(position);
        }
        u64::try_from(added.len()).unwrap_or(u64::MAX)
    }
}

impl InMemoryCollections {
    /// Ids are drawn past the items', as storage draws them from the same numbering.
    pub fn new(items: Vec<MetadataItem>) -> Self {
        let next_id = items.iter().map(|item| item.id.get()).max().unwrap_or(0);
        Self {
            items,
            state: std::sync::Arc::new(std::sync::Mutex::new(CollectionStore {
                next_id,
                ..CollectionStore::default()
            })),
        }
    }

    /// Adds a collection as an import would, with its entries in the order given.
    #[must_use]
    pub fn with(self, collection: Collection, items: &[RatingKey]) -> Self {
        {
            let mut store = locked(&self.state);
            store.next_id = store.next_id.max(collection.id.get());
            let id = collection.id;
            store.collections.push(collection);
            store.append(id, items);
        }
        self
    }

    fn item(&self, id: RatingKey) -> Option<&MetadataItem> {
        self.items.iter().find(|item| item.id == id)
    }
}

fn collection_matches(
    store: &CollectionStore,
    collection: &Collection,
    query: &CollectionQuery,
) -> bool {
    query.sections.contains(&collection.section)
        && (!query.with_entries
            || store
                .entries
                .iter()
                .any(|entry| entry.collection == collection.id))
        && query.kind.is_none_or(|kind| collection.kind == kind)
        && query
            .smart
            .is_none_or(|smart| collection.is_smart() == smart)
        && query
            .tag_number
            .is_none_or(|number| collection.tag_number == number)
        && query.title_word_prefix.as_ref().is_none_or(|term| {
            let words = crate::search::title_words(
                Some(&collection.title),
                collection.title_sort.as_deref(),
                None,
            );
            words_match(&words, term)
        })
}

/// The leading value in the query's direction, then the sort title and id ascending; by
/// title alone, the title and id in the query's direction.
fn collection_order(query: &CollectionQuery, a: &Collection, b: &Collection) -> std::cmp::Ordering {
    let by_title = a.sort_key().cmp(&b.sort_key()).then(a.id.cmp(&b.id));
    let directed = |ordering: std::cmp::Ordering| {
        if query.direction.is_descending() {
            ordering.reverse()
        } else {
            ordering
        }
    };
    match query.sort {
        CollectionSort::Title => directed(by_title),
        CollectionSort::AddedAt => directed(a.added_at.cmp(&b.added_at)).then(by_title),
        CollectionSort::UpdatedAt => directed(a.updated_at.cmp(&b.updated_at)).then(by_title),
    }
}

#[async_trait::async_trait]
impl CollectionRepository for InMemoryCollections {
    async fn create(&self, new: NewCollection, now: i64) -> Result<Collection, CollectionError> {
        let mut store = locked(&self.state);
        store.next_id += 1;
        let id = CollectionId::new(store.next_id);
        let tag_number = store
            .collections
            .iter()
            .map(|collection| collection.tag_number)
            .max()
            .unwrap_or(0)
            + 1;
        let collection = Collection {
            id,
            section: new.section,
            kind: new.kind,
            guid: format!("collection://{id}"),
            tag_number,
            title_sort: crate::edit::derived_title_sort(&new.title),
            title: new.title,
            summary: String::new(),
            content_rating: None,
            rule: new.rule,
            source_uri: new.source_uri,
            mode: crate::collection::CollectionMode::LibraryDefault,
            order: crate::collection::CollectionOrder::ReleaseDate,
            locked: Vec::new(),
            thumb_url: None,
            art_url: None,
            added_at: now,
            updated_at: now,
        };
        store.collections.push(collection.clone());
        store.append(id, &new.items);
        Ok(collection)
    }

    async fn collection(&self, id: CollectionId) -> Result<Option<Collection>, CollectionError> {
        Ok(locked(&self.state).collection(id).cloned())
    }

    async fn collections(
        &self,
        query: &CollectionQuery,
        window: Window,
    ) -> Result<Page<Collection>, CollectionError> {
        let store = locked(&self.state);
        let mut found: Vec<_> = store
            .collections
            .iter()
            .filter(|collection| collection_matches(&store, collection, query))
            .cloned()
            .collect();
        drop(store);
        found.sort_by(|a, b| collection_order(query, a, b));
        Ok(windowed(found, &window, |collection| {
            query.cursor(collection)
        }))
    }

    async fn count(&self, query: &CollectionQuery) -> Result<u64, CollectionError> {
        let store = locked(&self.state);
        let count = store
            .collections
            .iter()
            .filter(|collection| collection_matches(&store, collection, query))
            .count();
        Ok(u64::try_from(count).unwrap_or(u64::MAX))
    }

    async fn edit(
        &self,
        id: CollectionId,
        edit: &CollectionEdit,
    ) -> Result<Collection, CollectionError> {
        let mut store = locked(&self.state);
        let collection = store.find(id)?;
        edit.apply(collection);
        Ok(collection.clone())
    }

    async fn delete(&self, id: CollectionId) -> Result<bool, CollectionError> {
        let mut store = locked(&self.state);
        let before = store.collections.len();
        store.collections.retain(|collection| collection.id != id);
        store.entries.retain(|entry| entry.collection != id);
        Ok(store.collections.len() != before)
    }

    async fn entries(
        &self,
        collection: CollectionId,
        window: Window,
    ) -> Result<Page<CollectionEntry>, CollectionError> {
        let ordered = locked(&self.state).ordered(collection);
        Ok(windowed(ordered, &window, CollectionEntry::cursor))
    }

    async fn totals(
        &self,
        collections: &[CollectionId],
    ) -> Result<Vec<(CollectionId, CollectionTotals)>, CollectionError> {
        let store = locked(&self.state);
        let mut totals = Vec::new();
        for collection in collections {
            let mut total = CollectionTotals::default();
            for entry in store.entries.iter().filter(|e| e.collection == *collection) {
                total.items += 1;
                if let Some(year) = self.item(entry.item).and_then(|item| item.year) {
                    total.min_year = Some(total.min_year.map_or(year, |min| min.min(year)));
                    total.max_year = Some(total.max_year.map_or(year, |max| max.max(year)));
                }
            }
            if total.items > 0 {
                totals.push((*collection, total));
            }
        }
        Ok(totals)
    }

    async fn memberships(&self, items: &[RatingKey]) -> Result<Vec<Membership>, CollectionError> {
        let store = locked(&self.state);
        Ok(store
            .entries
            .iter()
            .filter(|entry| items.contains(&entry.item))
            .filter_map(|entry| {
                store
                    .collection(entry.collection)
                    .map(|collection| Membership {
                        item: entry.item,
                        collection: collection.clone(),
                    })
            })
            .collect())
    }

    async fn add(
        &self,
        collection: CollectionId,
        items: &[RatingKey],
        now: i64,
    ) -> Result<u64, CollectionError> {
        let mut store = locked(&self.state);
        store.find(collection)?;
        let added = store.append(collection, items);
        if added > 0 {
            store.find(collection)?.updated_at = now;
        }
        Ok(added)
    }

    async fn remove(
        &self,
        collection: CollectionId,
        item: RatingKey,
        now: i64,
    ) -> Result<bool, CollectionError> {
        let mut store = locked(&self.state);
        store.find(collection)?;
        let before = store.entries.len();
        store
            .entries
            .retain(|entry| !(entry.collection == collection && entry.item == item));
        let removed = store.entries.len() != before;
        if removed {
            store.find(collection)?.updated_at = now;
        }
        Ok(removed)
    }

    async fn move_item(
        &self,
        collection: CollectionId,
        item: RatingKey,
        destination: crate::collection::Destination,
    ) -> Result<(), CollectionError> {
        let mut store = locked(&self.state);
        store.find(collection)?;
        let mut order: Vec<RatingKey> = store
            .ordered(collection)
            .into_iter()
            .map(|entry| entry.item)
            .collect();
        if !order.contains(&item) {
            return Err(CollectionError::NotInCollection(item));
        }
        order.retain(|candidate| *candidate != item);
        let at = match destination {
            crate::collection::Destination::First => 0,
            crate::collection::Destination::After(after) => {
                order
                    .iter()
                    .position(|candidate| *candidate == after)
                    .ok_or(CollectionError::NotInCollection(after))?
                    + 1
            }
        };
        order.insert(at, item);
        for (moved, rank) in order.iter().zip(1_i64..) {
            if let Some(entry) = store
                .entries
                .iter_mut()
                .find(|entry| entry.collection == collection && entry.item == *moved)
            {
                entry.position = rank * crate::playlist::POSITION_GAP;
            }
        }
        Ok(())
    }

    async fn clear(&self, collection: CollectionId) -> Result<(), CollectionError> {
        let mut store = locked(&self.state);
        store.find(collection)?;
        store.entries.retain(|entry| entry.collection != collection);
        Ok(())
    }
}

/// An owner of the server, as the transport attaches one: every library, everyone's sessions.
/// Its id is past any a test's [`InMemoryAccess`] hands out, so that it is nobody else.
#[must_use]
pub fn owner() -> crate::Principal {
    crate::Principal {
        user: crate::ids::UserId::new(1_000),
        name: "owner".to_owned(),
        role: crate::access::ServerRole::Owner,
        libraries: crate::access::LibraryAccess::All,
        server_identifier: None,
    }
}
