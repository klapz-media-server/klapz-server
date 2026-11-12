//! Collections: items of one library grouped under a name, either chosen one by one or found
//! by a saved listing.
//!
//! A collection belongs to its library, not to a viewer: whoever may read the library sees it
//! and everything in it, and the library's admins and the server's owners change it. It holds items
//! of one kind, all from its library, each once.

use crate::ids::{CollectionId, RatingKey, SectionId};
use crate::metadata::ItemKind;
use crate::page::{Cursor, CursorKey, Direction, Page, Window};

/// How a library page treats a collection and the items in it. Clients read it; nothing on
/// the server lists differently for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CollectionMode {
    /// Whatever the library's own setting says.
    #[default]
    LibraryDefault,
    /// The collection is not shown, its items are.
    Hidden,
    /// The collection is shown in place of its items.
    InsteadOfItems,
    /// The collection is shown, and its items too.
    WithItems,
}

/// How a collection of chosen items orders them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CollectionOrder {
    /// By release date, items without one first.
    #[default]
    ReleaseDate,
    /// By sort title.
    Title,
    /// In the order an editor arranged them.
    Custom,
}

/// A field an editor can lock. Nothing refreshes a collection's metadata here; the lock is
/// kept because clients show and set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CollectionField {
    Title,
    TitleSort,
    Summary,
    ContentRating,
    Poster,
    Background,
}

/// A collection of a library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub id: CollectionId,
    pub section: SectionId,
    /// What it holds: every item in it is of this kind.
    pub kind: ItemKind,
    /// Stable and unique across servers, unlike the id.
    pub guid: String,
    /// The number items carry it under, which a listing filters on.
    ///
    /// Not unique: the reference numbers collections of the same name in two libraries
    /// alike, because to it a collection is a tag its items carry, and one tag spans
    /// libraries. Unique within one library.
    pub tag_number: i64,
    pub title: String,
    pub title_sort: Option<String>,
    /// Empty when it has none.
    pub summary: String,
    pub content_rating: Option<String>,
    /// A smart collection's listing of its library, opaque here as a smart playlist's is:
    /// the official API stores its own listing input and reads it back. `None` for a
    /// collection of chosen items.
    pub rule: Option<String>,
    /// Where a smart collection's content came from, as the client that created it named it.
    pub source_uri: Option<String>,
    pub mode: CollectionMode,
    pub order: CollectionOrder,
    /// Ascending.
    pub locked: Vec<CollectionField>,
    /// A poster, hosted by the metadata provider.
    pub thumb_url: Option<String>,
    /// A background, hosted by the metadata provider.
    pub art_url: Option<String>,
    pub added_at: i64,
    /// When an item last joined or left it, in seconds since the Unix epoch. An edit of its
    /// fields or its order leaves it, as on the reference.
    pub updated_at: i64,
}

impl Collection {
    pub const fn is_smart(&self) -> bool {
        self.rule.is_some()
    }

    /// What a listing by title orders it by.
    pub fn sort_key(&self) -> String {
        crate::metadata::sort_key(self.title_sort.as_deref(), Some(&self.title))
    }
}

/// A new collection's guid, which the reference writes `collection://` and a random UUID.
pub fn new_guid() -> String {
    format!("collection://{}", uuid::Uuid::new_v4())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCollection {
    pub section: SectionId,
    pub kind: ItemKind,
    pub title: String,
    /// The first items, in order. Repeats are dropped. Empty for a smart collection.
    pub items: Vec<RatingKey>,
    pub rule: Option<String>,
    pub source_uri: Option<String>,
}

/// Changes to a collection's own fields. `None` leaves a field alone; `Some(None)` clears an
/// optional one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionEdit {
    pub title: Option<String>,
    pub title_sort: Option<Option<String>>,
    pub summary: Option<String>,
    pub content_rating: Option<Option<String>>,
    pub mode: Option<CollectionMode>,
    pub order: Option<CollectionOrder>,
    pub lock: Vec<CollectionField>,
    pub unlock: Vec<CollectionField>,
}

impl CollectionEdit {
    /// Applies the edit to a collection's fields, as storage writes them.
    ///
    /// A new title derives the sort title, unless the sort title is locked or given: the
    /// rule an item's title follows, measured on the reference's collections too.
    pub fn apply(&self, collection: &mut Collection) {
        let locked_after = |field: CollectionField, current: bool| {
            self.lock.contains(&field) || (current && !self.unlock.contains(&field))
        };
        let sort_locked = locked_after(
            CollectionField::TitleSort,
            collection.locked.contains(&CollectionField::TitleSort),
        );
        if let Some(title) = &self.title {
            if !sort_locked && self.title_sort.is_none() {
                collection.title_sort = crate::edit::derived_title_sort(title);
            }
            collection.title.clone_from(title);
        }
        if let Some(sort) = &self.title_sort {
            collection.title_sort.clone_from(sort);
        }
        if let Some(summary) = &self.summary {
            collection.summary.clone_from(summary);
        }
        if let Some(rating) = &self.content_rating {
            collection.content_rating.clone_from(rating);
        }
        if let Some(mode) = self.mode {
            collection.mode = mode;
        }
        if let Some(order) = self.order {
            collection.order = order;
        }
        let mut locked: Vec<CollectionField> = [
            CollectionField::Title,
            CollectionField::TitleSort,
            CollectionField::Summary,
            CollectionField::ContentRating,
            CollectionField::Poster,
            CollectionField::Background,
        ]
        .into_iter()
        .filter(|field| locked_after(*field, collection.locked.contains(field)))
        .collect();
        locked.sort();
        collection.locked = locked;
    }
}

/// One item at its place in a collection of chosen items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectionEntry {
    pub collection: CollectionId,
    pub item: RatingKey,
    /// Orders the entries in a custom order; neither dense nor stable across moves.
    pub position: i64,
}

impl CollectionEntry {
    /// Where a page in custom order resumes after this entry.
    pub fn cursor(&self) -> Cursor {
        Cursor::new(CursorKey::Int(self.position), self.item.get())
    }
}

/// A collection an item is in, as an item's page lists them: in the order the item joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub item: RatingKey,
    pub collection: Collection,
}

/// What a collection of chosen items holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollectionTotals {
    pub items: u64,
    /// The earliest and latest release years among its items; `None` when no item has one.
    pub min_year: Option<i32>,
    pub max_year: Option<i32>,
}

/// How a list of collections is ordered. Equal values follow by sort title, ascending.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CollectionSort {
    /// By sort title: the reference's order.
    #[default]
    Title,
    AddedAt,
    UpdatedAt,
}

/// Which collections to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionQuery {
    /// The libraries listed. None lists nothing.
    pub sections: Vec<SectionId>,
    pub kind: Option<ItemKind>,
    pub smart: Option<bool>,
    /// Keeps the collections the items carry under this number.
    pub tag_number: Option<i64>,
    /// Keeps the collections holding a chosen item: what a filter on collections can find.
    pub with_entries: bool,
    /// Keeps the collections having a word that starts with each word of this text, in their
    /// title or sort title, accents folded: the rule of an item search.
    pub title_word_prefix: Option<String>,
    pub sort: CollectionSort,
    pub direction: Direction,
}

impl CollectionQuery {
    pub fn across(sections: Vec<SectionId>) -> Self {
        Self {
            sections,
            kind: None,
            smart: None,
            tag_number: None,
            with_entries: false,
            title_word_prefix: None,
            sort: CollectionSort::Title,
            direction: Direction::Ascending,
        }
    }

    /// Where a page resumes after this collection: its leading value, its sort title, its id.
    pub fn cursor(&self, collection: &Collection) -> Cursor {
        let mut keys = match self.sort {
            CollectionSort::Title => Vec::new(),
            CollectionSort::AddedAt => vec![CursorKey::Int(collection.added_at)],
            CollectionSort::UpdatedAt => vec![CursorKey::Int(collection.updated_at)],
        };
        keys.push(CursorKey::Text(collection.sort_key()));
        Cursor {
            keys,
            id: collection.id.get(),
        }
    }
}

/// Which collections an item filter keeps the items of: one by id, the ones carried under a
/// number, or the ones of a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionRef {
    Id(CollectionId),
    TagNumber(i64),
    /// Matched exactly, ignoring case.
    Title(String),
}

/// Keeps items in at least one of the listed collections. Several conditions combine with
/// `AND`, as tag conditions do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionCondition {
    pub any_of: Vec<CollectionRef>,
}

/// Where a moved item goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    First,
    After(RatingKey),
}

/// What to offer once `item` ends, from the collections it is in, each listed by release
/// date and given in the order the item joined them: the item after it in the first where
/// one follows it. Answers which collection, by its index, and the item.
///
/// With `within`, a collection counts only when the item is among its first `within`, as
/// on a shelf showing that many; what follows it may be past them. Measured on the
/// reference's post-play shelf, which shows six: the seventh James Bond is offered nothing
/// from the saga that lists it seventh, but the next one from a smaller collection, and the
/// sixth Jurassic Park offers the seventh. A film last of its first collection is followed in
/// its second, and one watched already is offered all the same.
pub fn next_in_collections(
    item: RatingKey,
    collections: &[Vec<RatingKey>],
    within: Option<usize>,
) -> Option<(usize, RatingKey)> {
    collections.iter().enumerate().find_map(|(index, members)| {
        let at = members.iter().position(|member| *member == item)?;
        if within.is_some_and(|within| at >= within) {
            return None;
        }
        Some((index, *members.get(at + 1)?))
    })
}

#[derive(Debug, thiserror::Error)]
pub enum CollectionError {
    #[error("no collection with id {0}")]
    NotFound(CollectionId),
    #[error("item {0} is not in this collection")]
    NotInCollection(RatingKey),
    #[error("collection storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait CollectionRepository: Send + Sync + std::fmt::Debug {
    async fn create(
        &self,
        collection: NewCollection,
        now: i64,
    ) -> Result<Collection, CollectionError>;

    async fn collection(&self, id: CollectionId) -> Result<Option<Collection>, CollectionError>;

    async fn collections(
        &self,
        query: &CollectionQuery,
        window: Window,
    ) -> Result<Page<Collection>, CollectionError>;

    async fn count(&self, query: &CollectionQuery) -> Result<u64, CollectionError>;

    async fn edit(
        &self,
        id: CollectionId,
        edit: &CollectionEdit,
    ) -> Result<Collection, CollectionError>;

    /// Says whether there was one to delete.
    async fn delete(&self, id: CollectionId) -> Result<bool, CollectionError>;

    /// A collection's entries in its custom order.
    async fn entries(
        &self,
        collection: CollectionId,
        window: Window,
    ) -> Result<Page<CollectionEntry>, CollectionError>;

    /// Totals per collection of chosen items. A collection with no item has none in the
    /// answer.
    async fn totals(
        &self,
        collections: &[CollectionId],
    ) -> Result<Vec<(CollectionId, CollectionTotals)>, CollectionError>;

    /// The collections several items are in, in the order each item joined them.
    ///
    /// Not windowed, as an item's tags are not: it serves a loader resolving many items at
    /// once, and an item is in a handful of collections — three at most in the reference
    /// library. The connection a client reads is windowed above it.
    async fn memberships(&self, items: &[RatingKey]) -> Result<Vec<Membership>, CollectionError>;

    /// Appends the items not in it already, and says how many that was. Changes `updated_at`
    /// when it adds one.
    async fn add(
        &self,
        collection: CollectionId,
        items: &[RatingKey],
        now: i64,
    ) -> Result<u64, CollectionError>;

    /// Takes an item out, and says whether it was in.
    async fn remove(
        &self,
        collection: CollectionId,
        item: RatingKey,
        now: i64,
    ) -> Result<bool, CollectionError>;

    async fn move_item(
        &self,
        collection: CollectionId,
        item: RatingKey,
        destination: Destination,
    ) -> Result<(), CollectionError>;

    /// Takes every item out, leaving `updated_at` as it was: the reference's way of emptying
    /// a collection through its tag.
    async fn clear(&self, collection: CollectionId) -> Result<(), CollectionError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection() -> Collection {
        Collection {
            id: CollectionId::new(578_395),
            section: SectionId::new(27),
            kind: ItemKind::Movie,
            guid: "collection://6b7e55c5-3503-4efc-990b-ba8d133298cc".to_owned(),
            tag_number: 987_806,
            title: "OP Coll A".to_owned(),
            title_sort: Some("OP Coll A".to_owned()),
            summary: String::new(),
            content_rating: None,
            rule: None,
            source_uri: None,
            mode: CollectionMode::LibraryDefault,
            order: CollectionOrder::ReleaseDate,
            locked: Vec::new(),
            thumb_url: None,
            art_url: None,
            added_at: 1_794_406_080,
            updated_at: 1_794_406_080,
        }
    }

    #[test]
    fn a_new_title_derives_the_sort_title_unless_one_is_given() {
        let mut edited = collection();
        CollectionEdit {
            title: Some("The Saga".to_owned()),
            ..CollectionEdit::default()
        }
        .apply(&mut edited);
        assert_eq!(edited.title_sort.as_deref(), Some("Saga"));

        let mut locked = collection();
        CollectionEdit {
            title: Some("OP Lock2".to_owned()),
            title_sort: Some(Some("aaa".to_owned())),
            lock: vec![CollectionField::Title, CollectionField::TitleSort],
            ..CollectionEdit::default()
        }
        .apply(&mut locked);
        assert_eq!(locked.title_sort.as_deref(), Some("aaa"));
        assert_eq!(
            locked.locked,
            vec![CollectionField::Title, CollectionField::TitleSort]
        );
    }

    fn keys(ids: &[i64]) -> Vec<RatingKey> {
        ids.iter().copied().map(RatingKey::new).collect()
    }

    #[test]
    fn the_next_film_is_found_in_the_first_collection_that_continues() {
        // Measured on the reference: Terminator 2 offers Terminator 3 of "Terminator - Saga",
        // and a film last of its first collection is followed in the second.
        let item = RatingKey::new(2);
        assert_eq!(
            next_in_collections(item, &[keys(&[1, 2, 3]), keys(&[2, 4])], None),
            Some((0, RatingKey::new(3)))
        );
        assert_eq!(
            next_in_collections(item, &[keys(&[1, 2]), keys(&[2, 4])], None),
            Some((1, RatingKey::new(4)))
        );
        assert_eq!(
            next_in_collections(item, &[keys(&[1, 2]), keys(&[2])], None),
            None
        );
        assert_eq!(next_in_collections(item, &[], None), None);
    }

    #[test]
    fn a_film_past_the_shelf_is_followed_in_another_collection() {
        let item = RatingKey::new(3);
        let collections = [keys(&[1, 2, 3, 4]), keys(&[3, 5])];
        assert_eq!(
            next_in_collections(item, &collections, Some(2)),
            Some((1, RatingKey::new(5)))
        );
        // The last one shown is followed by one that is not.
        assert_eq!(
            next_in_collections(item, &collections, Some(3)),
            Some((0, RatingKey::new(4)))
        );
        assert_eq!(
            next_in_collections(item, &[keys(&[1, 2, 3, 4])], Some(2)),
            None
        );
    }

    #[test]
    fn unlocking_leaves_the_other_locks() {
        let mut edited = collection();
        edited.locked = vec![CollectionField::Title, CollectionField::Summary];
        CollectionEdit {
            unlock: vec![CollectionField::Title],
            lock: vec![CollectionField::Poster],
            ..CollectionEdit::default()
        }
        .apply(&mut edited);
        assert_eq!(
            edited.locked,
            vec![CollectionField::Summary, CollectionField::Poster]
        );
    }

    #[test]
    fn a_guid_reads_as_the_references_do() {
        let guid = new_guid();
        assert!(guid.starts_with("collection://"), "{guid}");
        assert_eq!(guid.len(), "collection://".len() + 36);
    }

    #[test]
    fn a_title_cursor_carries_the_sort_key_and_a_dated_one_the_date_first() {
        let collection = collection();
        let by_title = CollectionQuery::across(vec![SectionId::new(27)]).cursor(&collection);
        assert_eq!(by_title.keys, vec![CursorKey::Text("op coll a".to_owned())]);
        let mut query = CollectionQuery::across(vec![SectionId::new(27)]);
        query.sort = CollectionSort::AddedAt;
        assert_eq!(
            query.cursor(&collection).keys,
            vec![
                CursorKey::Int(1_794_406_080),
                CursorKey::Text("op coll a".to_owned())
            ]
        );
    }
}
