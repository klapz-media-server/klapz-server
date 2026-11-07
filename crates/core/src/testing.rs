//! In-memory repositories, for tests that must not depend on storage.
//!
//! Lives behind the `testing` feature so it never ships in a release build. Bindings use
//! these to exercise resolvers without a database, which keeps the layering honest: a binding
//! test that needed `open-plex-db` would prove the dependency rule is not holding.
//!
//! They apply windows, filters and ordering the way storage does rather than simplifying,
//! because a stand-in that behaves differently hides exactly the mismatches worth catching.

use std::collections::BTreeMap;

use crate::ids::{MediaId, PartId, PersonId, RatingKey, SectionId, StreamId, TagId};
use crate::library::{LibraryError, LibraryRepository, LibrarySection, SectionRoot};
use crate::metadata::{
    self, EpisodeCredit, ExternalId, FirstCharacter, ItemKind, ItemQuery, ItemSort, ItemTag,
    MediaFile, MediaStream, MediaVersion, MetadataError, MetadataItem, MetadataRepository, Person,
    Rating, SearchQuery, Tag, TagCondition, TagKind, TagRef,
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
    tags: Vec<ItemTag>,
    external_ids: Vec<ExternalId>,
    ratings: Vec<Rating>,
}

impl InMemoryMetadata {
    pub const fn new(items: Vec<MetadataItem>) -> Self {
        Self {
            items,
            versions: Vec::new(),
            files: Vec::new(),
            streams: Vec::new(),
            tags: Vec::new(),
            external_ids: Vec::new(),
            ratings: Vec::new(),
        }
    }

    /// Attaches external identifiers and ratings, kept in the order given.
    #[must_use]
    pub fn with_references(mut self, external_ids: Vec<ExternalId>, ratings: Vec<Rating>) -> Self {
        self.external_ids = external_ids;
        self.ratings = ratings;
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
            .filter(|item| {
                filter
                    .tags
                    .iter()
                    .all(|condition| self.carries(item.id, condition))
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
        Ok(Self::page_of(rows, &window))
    }

    async fn grandchildren(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        // Position of each child, so the grandchildren can be ordered season by season.
        let children: BTreeMap<RatingKey, i32> = self
            .items
            .iter()
            .filter(|item| item.parent_id == Some(parent))
            .map(|item| (item.id, item.position.unwrap_or(0)))
            .collect();
        let mut rows: Vec<(i32, MetadataItem)> = self
            .items
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
        let mut credits: Vec<EpisodeCredit> = Vec::new();
        for &parent in parents {
            let episodes: Vec<RatingKey> = self
                .items
                .iter()
                .filter(|item| item.kind == ItemKind::Episode)
                .filter(|episode| {
                    episode.parent_id == Some(parent)
                        || episode.parent_id.is_some_and(|season| {
                            self.items
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

    async fn tag(&self, id: TagId) -> Result<Option<Tag>, MetadataError> {
        Ok(self
            .tags
            .iter()
            .find(|attached| attached.tag.id == id)
            .map(|attached| attached.tag.clone()))
    }
}

impl InMemoryMetadata {
    fn count_children(&self, parent: RatingKey) -> u64 {
        self.items
            .iter()
            .filter(|item| item.parent_id == Some(parent))
            .count() as u64
    }

    /// Cuts one page out of rows already in their final order.
    ///
    /// Children are ordered by position, where the cursor is the id alone, so only the
    /// offset and the limit apply — mirroring SQL `OFFSET` after `ORDER BY`.
    fn page_of(mut rows: Vec<MetadataItem>, window: &Window) -> Page<MetadataItem> {
        let skip = usize::try_from(window.skip).unwrap_or(usize::MAX);
        rows.drain(..skip.min(rows.len()));
        rows.truncate(page_limit(window));
        Page::from_overfetch(rows, window)
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
