//! In-memory repositories, for tests that must not depend on storage.
//!
//! Lives behind the `testing` feature so it never ships in a release build. Bindings use
//! these to exercise resolvers without a database, which keeps the layering honest: a binding
//! test that needed `open-plex-db` would prove the dependency rule is not holding.
//!
//! They apply windows, filters and ordering the way storage does rather than simplifying,
//! because a stand-in that behaves differently hides exactly the mismatches worth catching.

use std::collections::BTreeMap;

use crate::ids::{MediaId, PartId, RatingKey, SectionId, StreamId};
use crate::library::{LibraryError, LibraryRepository, LibrarySection, SectionRoot};
use crate::metadata::{
    self, FirstCharacter, ItemQuery, ItemSort, MediaFile, MediaStream, MediaVersion, MetadataError,
    MetadataItem, MetadataRepository, SearchQuery,
};
use crate::page::{Cursor, Page, Window};

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
    async fn sections(&self, window: Window) -> Result<Page<LibrarySection>, LibraryError> {
        let after = window.after.as_ref().map(|cursor| cursor.id);
        let before = window.before.as_ref().map(|cursor| cursor.id);

        let mut rows: Vec<LibrarySection> = self
            .sections
            .iter()
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
#[derive(Debug, Clone, Default)]
pub struct InMemoryMetadata {
    items: Vec<MetadataItem>,
    versions: Vec<MediaVersion>,
    files: Vec<MediaFile>,
    streams: Vec<MediaStream>,
}

impl InMemoryMetadata {
    pub const fn new(items: Vec<MetadataItem>) -> Self {
        Self {
            items,
            versions: Vec::new(),
            files: Vec::new(),
            streams: Vec::new(),
        }
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
        let filter = &query.filter;
        let mut matched: Vec<MetadataItem> = self
            .items
            .iter()
            .filter(|item| item.section_id == query.section_id)
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
                filter.title_word_prefix.as_ref().is_none_or(|prefix| {
                    let Some((lower, upper)) = crate::search::prefix_range(prefix) else {
                        return true;
                    };
                    crate::search::title_words(
                        item.title.as_deref(),
                        item.title_sort.as_deref(),
                        item.original_title.as_deref(),
                    )
                    .iter()
                    .any(|word| word >= &lower && upper.as_ref().is_none_or(|bound| word < bound))
                })
            })
            .filter(|item| {
                filter.min_width.is_none_or(|width| {
                    self.versions.iter().any(|version| {
                        version.item_id == item.id
                            && version.width.is_some_and(|value| value >= width)
                    })
                })
            })
            .cloned()
            .collect();

        match query.sort {
            ItemSort::Title => matched.sort_by(|left, right| {
                sort_key(left)
                    .cmp(&sort_key(right))
                    .then(left.id.get().cmp(&right.id.get()))
            }),
            ItemSort::AddedAt => matched.sort_by(|left, right| {
                left.added_at
                    .cmp(&right.added_at)
                    .then(left.id.get().cmp(&right.id.get()))
            }),
        }
        if query.direction.is_descending() {
            matched.reverse();
        }
        matched
    }
}

#[async_trait::async_trait]
impl MetadataRepository for InMemoryMetadata {
    async fn items(
        &self,
        query: &ItemQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
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
        Ok(page)
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

        let Some((lower, upper)) = crate::search::prefix_range(&query.word_prefix) else {
            return Ok(Page {
                items: Vec::new(),
                has_previous: false,
                has_next: false,
            });
        };

        let mut matched: Vec<MetadataItem> = self
            .items
            .iter()
            .filter(|item| query.kind.is_none_or(|kind| item.kind == kind))
            .filter(|item| {
                crate::search::title_words(
                    item.title.as_deref(),
                    item.title_sort.as_deref(),
                    item.original_title.as_deref(),
                )
                .iter()
                .any(|word| word >= &lower && upper.as_ref().is_none_or(|bound| word < bound))
            })
            .cloned()
            .collect();
        // Sort title then id, as storage orders a search.
        matched.sort_by(|left, right| {
            sort_key(left)
                .cmp(&sort_key(right))
                .then_with(|| left.id.get().cmp(&right.id.get()))
        });

        let position = |cursor: &Cursor| matched.iter().position(|item| item.id.get() == cursor.id);
        let start = window
            .after
            .as_ref()
            .and_then(position)
            .map_or(0, |at| at + 1);
        let end = window
            .before
            .as_ref()
            .and_then(position)
            .unwrap_or(matched.len());
        let mut slice: Vec<MetadataItem> = matched[start..end.max(start)].to_vec();

        if window.is_reversed() {
            slice.reverse();
        }
        let skip = usize::try_from(window.skip).unwrap_or(usize::MAX);
        slice.drain(..skip.min(slice.len()));
        slice.truncate(page_limit(&window));

        let mut page = Page::from_overfetch(slice, &window);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
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
        self.items
            .iter()
            .find(|item| item.id == id)
            .cloned()
            .ok_or(MetadataError::ItemNotFound(id))
    }

    async fn items_by_id(&self, ids: &[RatingKey]) -> Result<Vec<MetadataItem>, MetadataError> {
        Ok(self
            .items
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
            .items
            .iter()
            .filter(|item| item.parent_id == Some(parent))
            .cloned()
            .collect();
        rows.sort_by_key(|item| (item.position.unwrap_or(0), item.id.get()));
        rows.truncate(page_limit(&window));
        Ok(Page::from_overfetch(rows, &window))
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
                    .items
                    .iter()
                    .filter(|item| item.parent_id == Some(*parent))
                    .map(|item| item.id)
                    .collect();
                let leaves = self
                    .items
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
}

impl InMemoryMetadata {
    fn count_children(&self, parent: RatingKey) -> u64 {
        self.items
            .iter()
            .filter(|item| item.parent_id == Some(parent))
            .count() as u64
    }
}

/// The ordering key a title-sorted listing uses, matching storage's folded column.
fn sort_key(item: &MetadataItem) -> String {
    item.title_sort
        .as_ref()
        .or(item.title.as_ref())
        .map(|text| text.to_lowercase())
        .unwrap_or_default()
}

fn page_limit(window: &Window) -> usize {
    usize::try_from(window.fetch_limit()).unwrap_or(usize::MAX)
}
