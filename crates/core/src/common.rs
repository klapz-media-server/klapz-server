//! What a set of items has in common: the starting point for editing several at once.

use crate::edit::ItemField;
use crate::metadata::Tag;
use crate::{CollectionId, RatingKey};

/// A field across a set of items: the value they all hold, or the sign that two differ.
#[derive(Debug, Clone, PartialEq)]
pub enum Shared<T> {
    /// Two items differ, one holding a value another lacks included.
    Mixed,
    /// Every item holds this value; `None` when none holds any, or the set is empty.
    Same(Option<T>),
}

impl<T> Shared<T> {
    /// Folds one more item's value in.
    #[must_use]
    pub fn with(self, value: Option<&T>) -> Self
    where
        T: PartialEq,
    {
        match self {
            Self::Same(held) if held.as_ref() == value => Self::Same(held),
            _ => Self::Mixed,
        }
    }

    pub const fn is_mixed(&self) -> bool {
        matches!(self, Self::Mixed)
    }

    /// The common value, when there is one.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Same(value) => value.as_ref(),
            Self::Mixed => None,
        }
    }
}

/// The fields, tags and locks a set of items shares.
#[derive(Debug, Clone, PartialEq)]
pub struct CommonFields {
    pub items: u64,
    pub id: Shared<RatingKey>,
    pub parent: Shared<RatingKey>,
    pub parent_title: Shared<String>,
    pub grandparent: Shared<RatingKey>,
    pub grandparent_title: Shared<String>,
    pub title: Shared<String>,
    pub title_sort: Shared<String>,
    pub original_title: Shared<String>,
    pub tagline: Shared<String>,
    pub summary: Shared<String>,
    pub studio: Shared<String>,
    pub content_rating: Shared<String>,
    pub year: Shared<i32>,
    /// Seconds since the Unix epoch.
    pub released_on: Shared<i64>,
    pub rating: Shared<f64>,
    pub position: Shared<i32>,
    /// The tags every item carries, by tag id.
    pub tags: Vec<SharedTag>,
    /// The collections holding every item, by the number their items carry them under.
    pub collections: Vec<CollectionId>,
    /// The fields every item has locked, in [`ItemField::ALL`] order.
    pub locked: Vec<ItemField>,
}

impl CommonFields {
    /// What an empty set has in common: nothing differs, and nothing is held.
    pub const fn empty() -> Self {
        Self {
            items: 0,
            id: Shared::Same(None),
            parent: Shared::Same(None),
            parent_title: Shared::Same(None),
            grandparent: Shared::Same(None),
            grandparent_title: Shared::Same(None),
            title: Shared::Same(None),
            title_sort: Shared::Same(None),
            original_title: Shared::Same(None),
            tagline: Shared::Same(None),
            summary: Shared::Same(None),
            studio: Shared::Same(None),
            content_rating: Shared::Same(None),
            year: Shared::Same(None),
            released_on: Shared::Same(None),
            rating: Shared::Same(None),
            position: Shared::Same(None),
            tags: Vec::new(),
            collections: Vec::new(),
            locked: Vec::new(),
        }
    }
}

/// A tag every item of a set carries.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedTag {
    pub tag: Tag,
    /// The character played, when the set is one item: several items' characters differ.
    pub role: Option<String>,
    /// Where the tag is billed, when the set is one item.
    pub position: Option<i32>,
}

/// Orders the tags of a set: a single item's in billing order, as its own page lists them;
/// several items', whose billing differs from one to the next, by tag id.
pub fn order_shared_tags(tags: &mut [SharedTag]) {
    tags.sort_by_key(|shared| (shared.position, shared.tag.id.get()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_value_differs_from_a_present_one() {
        let shared = Shared::Same(None).with(Some(&2009)).with(None);
        assert!(shared.is_mixed());
        let shared = Shared::Same(Some(2009)).with(Some(&2009));
        assert_eq!(shared.value(), Some(&2009));
        assert_eq!(Shared::<i32>::Same(None).with(None), Shared::Same(None));
    }
}
