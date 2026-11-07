//! Genres, countries and credits.
//!
//! An item's tags are plain lists rather than connections, like its versions: they are a
//! bounded property of one item, loaded for a whole page of items in one batched query, and a
//! cursor per item would cost a query per item to honor. `first` covers the one real need,
//! which is showing the top of a long cast.

use async_graphql::{Enum, InputObject, Object, ID};
use open_plex_core::metadata::{ItemTag, Tag, TagCondition, TagKind, TagRef};
use open_plex_core::TagId;

use crate::node::GlobalId;

pub const TAG_TYPE: &str = "Tag";

/// What a tag classifies or credits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TagType {
    /// A genre, in the library's metadata language: `Comédie` in a French library.
    Genre,
    /// A country of production.
    Country,
    /// A person credited as director.
    Director,
    /// A person credited as writer.
    Writer,
    /// A person credited in the cast.
    Actor,
    /// A person credited as producer.
    Producer,
}

impl From<TagKind> for TagType {
    fn from(kind: TagKind) -> Self {
        match kind {
            TagKind::Genre => Self::Genre,
            TagKind::Country => Self::Country,
            TagKind::Director => Self::Director,
            TagKind::Writer => Self::Writer,
            TagKind::Actor => Self::Actor,
            TagKind::Producer => Self::Producer,
        }
    }
}

impl From<TagType> for TagKind {
    fn from(kind: TagType) -> Self {
        match kind {
            TagType::Genre => Self::Genre,
            TagType::Country => Self::Country,
            TagType::Director => Self::Director,
            TagType::Writer => Self::Writer,
            TagType::Actor => Self::Actor,
            TagType::Producer => Self::Producer,
        }
    }
}

/// A genre, a country, or a person in one credit role.
#[derive(Debug, Clone)]
pub struct TagObject(pub Tag);

#[Object(name = "Tag")]
impl TagObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this tag, or to an
    /// item filter to list what carries it.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(TAG_TYPE, self.0.id.get()))
    }

    /// What the tag classifies or credits.
    ///
    /// A person credited in two roles is two tags: the writer and the producer are filtered
    /// on separately, because "films she wrote" is not "films she produced".
    async fn kind(&self) -> TagType {
        self.0.kind.into()
    }

    /// The genre, the country, or the person's name.
    ///
    /// Not unique among people: two people can share a name, and `providerKey` tells them
    /// apart.
    async fn name(&self) -> &str {
        &self.0.name
    }

    /// The metadata provider's identifier for a person. Null for genres and countries, and
    /// for the people the provider does not know.
    async fn provider_key(&self) -> Option<&str> {
        self.0.provider_key.as_deref()
    }

    /// A portrait of the person, hosted by the metadata provider. Null for genres and
    /// countries.
    async fn thumb_url(&self) -> Option<&str> {
        self.0.thumb_url.as_deref()
    }
}

/// One credit in a cast.
#[derive(Debug, Clone)]
pub struct CastMember(pub ItemTag);

#[Object]
impl CastMember {
    /// Who plays.
    async fn person(&self) -> TagObject {
        TagObject(self.0.tag.clone())
    }

    /// The character played, as the metadata source credits it — sometimes several, as in
    /// `Homer Simpson / Barney Gumble`. Null when the source does not say.
    async fn character(&self) -> Option<&str> {
        self.0.role.as_deref()
    }
}

/// Keeps items carrying at least one of the named tags of one kind.
///
/// `ids` and `names` combine with `OR`, so a condition can mix both. Several conditions in
/// one filter combine with `AND`: "a comedy and a drama" is two conditions, "a comedy or a
/// drama" is one.
#[derive(Debug, Clone, InputObject)]
pub struct TagFilterInput {
    /// The kind of tag to match.
    pub kind: TagType,
    /// Tags by identifier, as returned in `Tag.id`.
    pub ids: Option<Vec<ID>>,
    /// Tags by name, matched exactly and ignoring ASCII case only: `ACTION` matches
    /// `Action`, but `COMÉDIE` does not match `Comédie`, and spaces are not trimmed. A name
    /// shared by two people matches both.
    pub names: Option<Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum TagFilterError {
    #[error("a tag condition must name at least one tag")]
    Empty,
    #[error("a tag condition names {0} tags, past the maximum of {max}", max = TagCondition::MAX_VALUES)]
    TooMany(usize),
    #[error("`{0}` is not a tag identifier")]
    NotATag(String),
}

impl TryFrom<TagFilterInput> for TagCondition {
    type Error = TagFilterError;

    fn try_from(input: TagFilterInput) -> Result<Self, Self::Error> {
        let mut any_of = Vec::new();
        for id in input.ids.unwrap_or_default() {
            let global =
                GlobalId::decode(&id.0).map_err(|_| TagFilterError::NotATag(id.0.clone()))?;
            if global.type_name != TAG_TYPE {
                return Err(TagFilterError::NotATag(id.0));
            }
            any_of.push(TagRef::Id(TagId::new(global.key)));
        }
        any_of.extend(
            input
                .names
                .unwrap_or_default()
                .into_iter()
                .map(TagRef::Name),
        );

        if any_of.is_empty() {
            return Err(TagFilterError::Empty);
        }
        if any_of.len() > TagCondition::MAX_VALUES {
            return Err(TagFilterError::TooMany(any_of.len()));
        }
        Ok(Self {
            kind: input.kind.into(),
            any_of,
        })
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn input(ids: Vec<ID>, names: Vec<&str>) -> TagFilterInput {
        TagFilterInput {
            kind: TagType::Genre,
            ids: Some(ids),
            names: Some(names.into_iter().map(ToOwned::to_owned).collect()),
        }
    }

    #[test]
    fn mixes_ids_and_names_in_one_condition() {
        let condition = TagCondition::try_from(input(
            vec![ID(GlobalId::encode(TAG_TYPE, 38))],
            vec!["Crime"],
        ))
        .unwrap();
        assert_eq!(condition.kind, TagKind::Genre);
        assert_eq!(
            condition.any_of,
            vec![TagRef::Id(TagId::new(38)), TagRef::Name("Crime".to_owned())]
        );
    }

    #[test]
    fn refuses_an_identifier_of_another_type() {
        // An item id passed as a tag would otherwise filter on whatever tag shares its number.
        let error = TagCondition::try_from(input(vec![ID(GlobalId::encode("Item", 38))], vec![]))
            .unwrap_err();
        assert!(matches!(error, TagFilterError::NotATag(_)));
    }

    #[test]
    fn refuses_a_condition_naming_nothing() {
        // Matching nothing and matching everything are both wrong answers to an empty list.
        let error = TagCondition::try_from(input(vec![], vec![])).unwrap_err();
        assert!(matches!(error, TagFilterError::Empty));
    }

    #[test]
    fn refuses_more_values_than_the_cap() {
        let names = vec!["Drama"; TagCondition::MAX_VALUES + 1];
        let error = TagCondition::try_from(input(vec![], names)).unwrap_err();
        assert!(matches!(error, TagFilterError::TooMany(_)));
    }

    #[test]
    fn tag_kinds_round_trip() {
        for kind in [
            TagKind::Genre,
            TagKind::Country,
            TagKind::Director,
            TagKind::Writer,
            TagKind::Actor,
            TagKind::Producer,
        ] {
            assert_eq!(TagKind::from(TagType::from(kind)), kind);
        }
    }
}
