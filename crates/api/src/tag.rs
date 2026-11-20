//! Genres, countries, credits, and what external providers say about an item.
//!
//! Every list here is a cursor connection, paged in memory: an item's tags are loaded for a
//! whole page of items in one batched query, and then each item's list is windowed on its
//! own. Pushing the window down to storage instead would cost a query per item, since each
//! carries its own window; the lists are bounded — 200 credits at most on one item in the
//! reference library — so the trade is the right one.

use async_graphql::connection::{
    query, Connection, ConnectionNameType, Edge, EdgeNameType, EmptyFields,
};
use async_graphql::dataloader::DataLoader;
use async_graphql::{
    Context, Enum, InputObject, Object, ObjectType, OutputType, Result, SimpleObject, ID,
};
use klapz_core::library::LibrarySection;
use klapz_core::metadata::{
    credits_with_episodes, Credit, CreditRef, ExternalId, ItemDepth, ItemFilter, ItemQuery,
    ItemSort, Person, Rating, RatingKind, Tag, TagCondition, TagKind, TagRef, TagSearch, TagUse,
};
use klapz_core::page::{Cursor, Direction, Window, MAX_ITEMS};
use klapz_core::{PersonId, RatingKey, SectionId, TagId};

use crate::cursor::OpaqueCursor;
use crate::filter::ItemFilterInput;
use crate::item::{Item, ItemConnection};
use crate::loader::{
    CreditsBelowLoader, ExternalIdsLoader, PersonCreditsLoader, RatingsLoader, TagsLoader,
};
use crate::node::GlobalId;
use crate::schema;

pub const TAG_TYPE: &str = "Tag";
pub const PERSON_TYPE: &str = "Person";

/// Default page size of a tag list when the client asks for neither end.
const DEFAULT_PAGE_SIZE: usize = 50;

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
    /// A person credited in two roles has two tags, one per role: "films she wrote" and
    /// "films she produced" are different filters.
    async fn kind(&self) -> TagType {
        self.0.kind.into()
    }

    /// The genre, the country, or the name as credited — which can differ from the person's
    /// current name, as `Andy Garcia` does from `Andy García`.
    async fn name(&self) -> &str {
        &self.0.name
    }

    /// Who a credit names. Null for a genre or a country.
    async fn person(&self) -> Option<PersonObject> {
        self.0.person.clone().map(PersonObject)
    }

    /// What carries this tag, across the caller's libraries, hidden ones left out.
    ///
    /// A series is listed for a tag on it or on any of its episodes, once: an actor's guest
    /// appearances name the series, not each episode. The filter narrows that list further,
    /// and naming a `kind` reaches below the top level as it does on `Library.items`.
    ///
    /// Library by library, by the library's title; within one, films by title and series in
    /// the order they arrived.
    #[allow(clippy::too_many_arguments)]
    async fn items(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(desc = "Narrows the list. Every field is combined with `AND`.")] filter: Option<
            ItemFilterInput,
        >,
        #[graphql(
            desc = "Discard this many items before the page begins. Prefer cursors: an offset costs a traversal of every discarded row."
        )]
        offset: Option<u32>,
    ) -> Result<ItemConnection> {
        let works = works(context, CreditRef::Tag(self.0.id), filter).await?;
        page_of_works(context, works, (after, before, first, last), offset).await
    }

    /// How many items `items` lists under the same filter, without paging through them.
    async fn item_count(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Count only the items this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
    ) -> Result<u64> {
        let works = works(context, CreditRef::Tag(self.0.id), filter).await?;
        Ok(schema::metadata(context)?.item_count(&works).await?)
    }
}

/// Someone credited on an item, whatever the role.
#[derive(Debug, Clone)]
pub struct PersonObject(pub Person);

#[Object(name = "Person")]
impl PersonObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this person.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(PERSON_TYPE, self.0.id.get()))
    }

    /// The person's name. Not unique: two people can share one, and `providerKey` is what
    /// tells them apart.
    async fn name(&self) -> &str {
        &self.0.name
    }

    /// The metadata provider's identifier for this person. Null for the people the provider
    /// does not know.
    async fn provider_key(&self) -> Option<&str> {
        self.0.provider_key.as_deref()
    }

    /// A portrait, hosted by the metadata provider.
    async fn thumb_url(&self) -> Option<&str> {
        self.0.thumb_url.as_deref()
    }

    /// The person's tags, one per role, in the order they were created: what to filter a
    /// library by to find the films they directed, or those they played in. Only the tags
    /// carried in a library the caller can see.
    async fn credits(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<PersonCreditConnection> {
        let rows = credits_in_view(context, self.0.id)
            .await?
            .into_iter()
            .map(|tag| (TagObject(tag), EmptyFields))
            .collect();
        page_in_memory(rows, after, before, first, last).await
    }

    /// What credits this person in any role, across the caller's libraries, hidden ones left
    /// out: the items of all their `credits`, each item once, in the order of `Tag.items`.
    #[allow(clippy::too_many_arguments)]
    async fn items(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(desc = "Narrows the list. Every field is combined with `AND`.")] filter: Option<
            ItemFilterInput,
        >,
        #[graphql(
            desc = "Discard this many items before the page begins. Prefer cursors: an offset costs a traversal of every discarded row."
        )]
        offset: Option<u32>,
    ) -> Result<ItemConnection> {
        let works = works(context, CreditRef::Person(self.0.id), filter).await?;
        page_of_works(context, works, (after, before, first, last), offset).await
    }

    /// How many items `items` lists under the same filter, without paging through them.
    async fn item_count(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Count only the items this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
    ) -> Result<u64> {
        let works = works(context, CreditRef::Person(self.0.id), filter).await?;
        Ok(schema::metadata(context)?.item_count(&works).await?)
    }
}

/// Names the connection apart from `TagConnection`, whose edges describe an attachment to
/// an item: a person's list of credits has none.
#[derive(Debug)]
pub struct PersonCreditConnectionName;

impl ConnectionNameType for PersonCreditConnectionName {
    fn type_name<T: OutputType>() -> String {
        "PersonCreditConnection".to_owned()
    }
}

#[derive(Debug)]
pub struct PersonCreditEdgeName;

impl EdgeNameType for PersonCreditEdgeName {
    fn type_name<T: OutputType>() -> String {
        "PersonCreditEdge".to_owned()
    }
}

/// A page of a person's tags.
pub type PersonCreditConnection = Connection<
    OpaqueCursor,
    TagObject,
    EmptyFields,
    EmptyFields,
    PersonCreditConnectionName,
    PersonCreditEdgeName,
>;

/// The libraries the caller can see, hidden ones included.
pub(crate) async fn libraries_in_view(context: &Context<'_>) -> Result<Vec<LibrarySection>> {
    let visible = crate::access::visible_sections(context);
    // Libraries are a handful; one window holds them all.
    let window = Window::new(None, None, Some(MAX_ITEMS), None)?;
    Ok(schema::library_repository(context)?
        .sections(visible.as_deref(), window)
        .await?
        .items)
}

/// The query listing what credits a tag or a person, as `Tag.items` and `Person.items`
/// serve it.
///
/// Hidden libraries are left out, as the reference leaves them out of a person's page: what
/// Plex calls hiding a library is keeping it off the lists that span libraries.
async fn works(
    context: &Context<'_>,
    credited: CreditRef,
    filter: Option<ItemFilterInput>,
) -> Result<ItemQuery> {
    let mut libraries = libraries_in_view(context).await?;
    libraries.retain(|library| !library.hidden);
    libraries.sort_by(|left, right| {
        left.title
            .cmp(&right.title)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut filter: ItemFilter = filter.unwrap_or_default().try_into()?;
    filter.viewer = crate::access::viewer_id(context);
    filter.credited = Some(credited);
    Ok(
        ItemQuery::across(libraries.into_iter().map(|library| library.id).collect())
            .filtered(filter)
            .sorted_by(ItemSort::Filmography, Direction::Ascending),
    )
}

async fn page_of_works(
    context: &Context<'_>,
    works: ItemQuery,
    (after, before, first, last): (Option<String>, Option<String>, Option<i32>, Option<i32>),
    offset: Option<u32>,
) -> Result<ItemConnection> {
    let skip = u64::from(offset.unwrap_or(0));
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = schema::window_for(after, before, first, last, schema::DEFAULT_PAGE_SIZE)?
                .skipping(skip);
            let page = schema::metadata(context)?.items(&works, window).await?;

            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection
                .edges
                .extend(page.items.into_iter().map(|listed| {
                    Edge::new(OpaqueCursor(listed.cursor), Item::from_domain(listed.item))
                }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// Whether an item in a library the caller can see credits this, hidden libraries included.
///
/// What makes a tag or a person exist for a caller: one that a user's libraries do not
/// credit answers as if it did not exist, and so does a tag nothing carries any more, which
/// the reference answers `404`.
async fn credited_in_view(context: &Context<'_>, credited: CreditRef) -> Result<bool> {
    let sections: Vec<SectionId> = libraries_in_view(context)
        .await?
        .into_iter()
        .map(|library| library.id)
        .collect();
    let query = ItemQuery::across(sections).filtered(ItemFilter {
        credited: Some(credited),
        depth: ItemDepth::All,
        ..ItemFilter::default()
    });
    let window = Window::new(None, None, Some(1), None)?;
    Ok(!schema::metadata(context)?
        .items(&query, window)
        .await?
        .items
        .is_empty())
}

/// A tag, if an item the caller can see carries it.
pub(crate) async fn readable_tag(context: &Context<'_>, id: TagId) -> Result<Option<Tag>> {
    let Some(tag) = schema::metadata(context)?.tag(id).await? else {
        return Ok(None);
    };
    Ok(credited_in_view(context, CreditRef::Tag(id))
        .await?
        .then_some(tag))
}

/// A person, if an item the caller can see credits them.
pub(crate) async fn readable_person(context: &Context<'_>, id: PersonId) -> Result<Option<Person>> {
    let Some(person) = schema::metadata(context)?.person(id).await? else {
        return Ok(None);
    };
    Ok(credited_in_view(context, CreditRef::Person(id))
        .await?
        .then_some(person))
}

/// A person's tags carried in a library the caller can see.
async fn credits_in_view(context: &Context<'_>, person: PersonId) -> Result<Vec<Tag>> {
    Ok(context
        .data::<DataLoader<PersonCreditsLoader>>()?
        .load_one(person)
        .await?
        .unwrap_or_default()
        .into_iter()
        .filter(|credit| {
            credit
                .sections
                .iter()
                .any(|section| crate::access::can_read(context, *section))
        })
        .map(|credit| credit.tag)
        .collect())
}

/// A tag as one library's items carry it: what a search box offers as "Tom Hanks in the
/// films", leading to that library's items filtered on him.
#[derive(Debug, Clone)]
pub struct TagUseObject(pub TagUse);

#[Object(name = "TagUse")]
impl TagUseObject {
    /// The tag.
    async fn tag(&self) -> TagObject {
        TagObject(self.0.tag.clone())
    }

    /// The library whose items carry it.
    async fn library(&self, context: &Context<'_>) -> Result<Option<crate::library::Library>> {
        crate::schema::load_library(context, self.0.section).await
    }

    /// How many of the library's items carry the tag, whatever their kind: in a series
    /// library, the series and the episodes crediting someone.
    async fn item_count(&self) -> u64 {
        self.0.item_count
    }
}

/// A page of tags as libraries carry them.
pub type TagUseConnection = Connection<OpaqueCursor, TagUseObject>;

/// The tags whose name matches a search, among the libraries the caller can see.
pub(crate) async fn search_tags(
    context: &Context<'_>,
    term: String,
    kinds: Option<Vec<TagType>>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<TagUseConnection> {
    let search = TagSearch {
        word_prefix: term,
        kinds: kinds
            .unwrap_or_default()
            .into_iter()
            .map(TagKind::from)
            .collect(),
        sections: crate::access::visible_sections(context),
    };
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = crate::schema::window_for(
                after,
                before,
                first,
                last,
                crate::schema::DEFAULT_PAGE_SIZE,
            )?;
            let page = crate::schema::metadata(context)?
                .search_tags(&search, window)
                .await?;
            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection.edges.extend(
                page.items
                    .into_iter()
                    .map(|found| Edge::new(OpaqueCursor(found.cursor()), TagUseObject(found))),
            );
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// What a list of every tag of a kind says beyond its tags.
#[derive(Debug, Clone)]
pub struct TagListTotal {
    sections: Vec<SectionId>,
    kind: TagKind,
}

#[Object]
impl TagListTotal {
    /// How many tags the list holds in all, across every page.
    async fn total_count(&self, context: &Context<'_>) -> Result<u64> {
        Ok(schema::metadata(context)?
            .tag_count_in_libraries(&self.sections, self.kind)
            .await?)
    }
}

/// Names the connection apart from `TagConnection`, whose edges describe an attachment to
/// an item.
#[derive(Debug)]
pub struct TagListConnectionName;

impl ConnectionNameType for TagListConnectionName {
    fn type_name<T: OutputType>() -> String {
        "TagListConnection".to_owned()
    }
}

#[derive(Debug)]
pub struct TagListEdgeName;

impl EdgeNameType for TagListEdgeName {
    fn type_name<T: OutputType>() -> String {
        "TagListEdge".to_owned()
    }
}

/// A page of every tag of a kind.
pub type TagListConnection = Connection<
    OpaqueCursor,
    TagObject,
    TagListTotal,
    EmptyFields,
    TagListConnectionName,
    TagListEdgeName,
>;

/// Every tag of one kind carried by an item of the caller's libraries, at any depth and
/// hidden libraries included, in the order the tags were first recorded.
pub(crate) async fn tags_of_kind(
    context: &Context<'_>,
    kind: TagType,
    (after, before, first, last): (Option<String>, Option<String>, Option<i32>, Option<i32>),
    offset: Option<u32>,
) -> Result<TagListConnection> {
    let sections: Vec<SectionId> = libraries_in_view(context)
        .await?
        .into_iter()
        .map(|library| library.id)
        .collect();
    let kind = TagKind::from(kind);
    let skip = u64::from(offset.unwrap_or(0));
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = schema::window_for(after, before, first, last, schema::DEFAULT_PAGE_SIZE)?
                .skipping(skip);
            let page = schema::metadata(context)?
                .tags_in_libraries(&sections, kind, window)
                .await?;
            let mut connection = Connection::with_additional_fields(
                page.has_previous,
                page.has_next,
                TagListTotal {
                    sections: sections.clone(),
                    kind,
                },
            );
            connection.edges.extend(
                page.items
                    .into_iter()
                    .map(|tag| Edge::new(OpaqueCursor(tag.recorded_cursor()), TagObject(tag))),
            );
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// What a tag's attachment to one item says beyond the tag itself.
#[derive(Debug, Clone, SimpleObject)]
pub struct Attachment {
    /// The character played, for a cast member, as the source credits it — sometimes
    /// several, as in `Homer Simpson / Barney Gumble`. Null for any other kind, and for a
    /// guest gathered from episodes, whose character varies from one to the next.
    pub character: Option<String>,
    /// How many episodes below the item carry this credit. Null on an item with no episodes:
    /// a film, or an episode itself.
    pub episode_count: Option<u32>,
}

/// A page of tags.
pub type TagConnection = Connection<OpaqueCursor, TagObject, EmptyFields, Attachment>;

/// Where an item's tags come from.
///
/// Every item answers every kind, which is what keeps the schema the same whatever the
/// item: an episode's genres are its series', and a series' crew includes whoever worked on
/// its episodes.
#[derive(Debug, Clone, Copy)]
pub struct ItemTags {
    /// The item whose own credits are read.
    pub item: RatingKey,
    /// The item genres and countries are read from: the item itself, or the series of a
    /// season or an episode.
    pub classified_by: RatingKey,
    /// Also gather the credits of the episodes below the item, for a series or a season.
    pub with_episodes: bool,
}

/// One item's genres, countries and credits.
#[Object]
impl ItemTags {
    /// Genres, most significant first. A season's and an episode's are their series'.
    async fn genres(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<TagConnection> {
        let tags = self.classification(context, TagKind::Genre).await?;
        tag_page(tags, after, before, first, last).await
    }

    /// Countries of production. A season's and an episode's are their series'.
    async fn countries(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<TagConnection> {
        let tags = self.classification(context, TagKind::Country).await?;
        tag_page(tags, after, before, first, last).await
    }

    /// People credited as director. On a series or a season, its own first, then those of
    /// its episodes, most episodes first.
    async fn directors(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<TagConnection> {
        let credits = self.credits(context, TagKind::Director).await?;
        tag_page(credits, after, before, first, last).await
    }

    /// People credited as writer. On a series or a season, its own first, then those of its
    /// episodes, most episodes first.
    async fn writers(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<TagConnection> {
        let credits = self.credits(context, TagKind::Writer).await?;
        tag_page(credits, after, before, first, last).await
    }

    /// The cast, in billing order, with the character each plays. On a series or a season,
    /// the regular cast first, then the episodes' guests, most episodes first.
    async fn cast(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<TagConnection> {
        let credits = self.credits(context, TagKind::Actor).await?;
        tag_page(credits, after, before, first, last).await
    }

    /// People credited as producer. On a series or a season, its own first, then those of
    /// its episodes, most episodes first.
    async fn producers(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<TagConnection> {
        let credits = self.credits(context, TagKind::Producer).await?;
        tag_page(credits, after, before, first, last).await
    }
}

impl ItemTags {
    async fn classification(&self, context: &Context<'_>, kind: TagKind) -> Result<Vec<Credit>> {
        Ok(own_tags(context, self.classified_by, kind)
            .await?
            .into_iter()
            .map(|attached| Credit {
                tag: attached.tag,
                role: attached.role,
                episodes: None,
            })
            .collect())
    }

    async fn credits(&self, context: &Context<'_>, kind: TagKind) -> Result<Vec<Credit>> {
        let own = own_tags(context, self.item, kind).await?;
        if !self.with_episodes {
            return Ok(own
                .into_iter()
                .map(|attached| Credit {
                    tag: attached.tag,
                    role: attached.role,
                    episodes: None,
                })
                .collect());
        }
        let below: Vec<_> = context
            .data::<DataLoader<CreditsBelowLoader>>()?
            .load_one(self.item)
            .await?
            .unwrap_or_default()
            .into_iter()
            .filter(|credit| credit.tag.kind == kind)
            .collect();
        Ok(credits_with_episodes(own, &below))
    }
}

async fn own_tags(
    context: &Context<'_>,
    item: RatingKey,
    kind: TagKind,
) -> Result<Vec<klapz_core::ItemTag>> {
    Ok(context
        .data::<DataLoader<TagsLoader>>()?
        .load_one(item)
        .await?
        .unwrap_or_default()
        .into_iter()
        .filter(|attached| attached.tag.kind == kind)
        .collect())
}

async fn tag_page(
    credits: Vec<Credit>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<TagConnection> {
    let rows = credits
        .into_iter()
        .map(|credit| {
            (
                TagObject(credit.tag),
                Attachment {
                    character: credit.role,
                    episode_count: credit.episodes,
                },
            )
        })
        .collect();
    page_in_memory(rows, after, before, first, last).await
}

/// An identifier the item has at an external metadata provider.
#[derive(Debug, Clone)]
pub struct ExternalIdObject(pub ExternalId);

#[Object(name = "ExternalId")]
impl ExternalIdObject {
    /// The provider: `imdb`, `tmdb` or `tvdb` in the reference library.
    async fn source(&self) -> &str {
        &self.0.source
    }

    /// The identifier at that provider, for example `tt0293662`.
    async fn value(&self) -> &str {
        &self.0.value
    }
}

/// Who a rating speaks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum RatingType {
    /// Professional critics.
    Critic,
    /// The provider's users.
    Audience,
}

impl From<RatingKind> for RatingType {
    fn from(kind: RatingKind) -> Self {
        match kind {
            RatingKind::Critic => Self::Critic,
            RatingKind::Audience => Self::Audience,
        }
    }
}

/// A score an external provider gives the item.
#[derive(Debug, Clone)]
pub struct RatingObject(pub Rating);

#[Object(name = "Rating")]
impl RatingObject {
    /// The provider: `imdb`, `rottentomatoes` or `themoviedb` in the reference library.
    async fn source(&self) -> &str {
        &self.0.source
    }

    /// Who the score speaks for.
    async fn kind(&self) -> RatingType {
        self.0.kind.into()
    }

    /// The score out of 10, as the provider states it.
    async fn value(&self) -> f64 {
        self.0.value
    }

    /// The provider's badge for this score, for example
    /// `rottentomatoes://image.rating.ripe` — which for Rotten Tomatoes also says whether the
    /// score counts as fresh.
    async fn image(&self) -> &str {
        &self.0.image
    }
}

/// A page of external identifiers.
pub type ExternalIdConnection =
    Connection<OpaqueCursor, ExternalIdObject, EmptyFields, EmptyFields>;
/// A page of ratings.
pub type RatingConnection = Connection<OpaqueCursor, RatingObject, EmptyFields, EmptyFields>;

/// An item's external identifiers, in the order the source lists them.
pub(crate) async fn external_ids_of(
    context: &Context<'_>,
    item: RatingKey,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<ExternalIdConnection> {
    let rows = context
        .data::<DataLoader<ExternalIdsLoader>>()?
        .load_one(item)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(|id| (ExternalIdObject(id), EmptyFields))
        .collect();
    page_in_memory(rows, after, before, first, last).await
}

/// An item's ratings, in the order the source lists them.
pub(crate) async fn ratings_of(
    context: &Context<'_>,
    item: RatingKey,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<RatingConnection> {
    let rows = context
        .data::<DataLoader<RatingsLoader>>()?
        .load_one(item)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(|rating| (RatingObject(rating), EmptyFields))
        .collect();
    page_in_memory(rows, after, before, first, last).await
}

/// Windows a list already in memory, the cursor being the position in it.
///
/// The same rules as a storage window: at most [`MAX_ITEMS`] per page, [`DEFAULT_PAGE_SIZE`]
/// when neither end is asked for.
pub(crate) async fn page_in_memory<T, E, C, N>(
    rows: Vec<(T, E)>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<Connection<OpaqueCursor, T, EmptyFields, E, C, N>>
where
    T: OutputType,
    E: ObjectType,
    C: ConnectionNameType,
    N: EdgeNameType,
{
    query(
        after,
        before,
        first,
        last,
        |after: Option<OpaqueCursor>, before: Option<OpaqueCursor>, first, last| async move {
            let cap = MAX_ITEMS as usize;
            if first.or(last).is_some_and(|count| count > cap) {
                return Err(async_graphql::Error::new(format!(
                    "a page of more than {cap} is not served"
                )));
            }
            let position = |cursor: OpaqueCursor| usize::try_from(cursor.0.id).unwrap_or(0);
            let mut start = after
                .map_or(0, |cursor| position(cursor) + 1)
                .min(rows.len());
            let mut end = before.map_or(rows.len(), position).clamp(start, rows.len());
            match (first, last) {
                (Some(first), _) => end = end.min(start + first),
                (None, Some(last)) => start = start.max(end.saturating_sub(last)),
                (None, None) => end = end.min(start + DEFAULT_PAGE_SIZE),
            }

            let mut connection = Connection::new(start > 0, end < rows.len());
            connection.edges.extend(
                rows.into_iter()
                    .enumerate()
                    .skip(start)
                    .take(end - start)
                    .map(|(index, (node, fields))| {
                        let cursor = Cursor::by_id(i64::try_from(index).unwrap_or(i64::MAX));
                        Edge::with_additional_fields(OpaqueCursor(cursor), node, fields)
                    }),
            );
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
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
    /// Tags by name, matched exactly but ignoring case. A name shared by two people matches
    /// both.
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

    use async_graphql::connection::CursorType;

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

    type InMemoryPage = Connection<OpaqueCursor, String, EmptyFields, EmptyFields>;

    #[tokio::test]
    async fn pages_a_list_in_memory_by_position() {
        let rows: Vec<(String, EmptyFields)> =
            (0..5).map(|n| (format!("tag {n}"), EmptyFields)).collect();

        let first_two: InMemoryPage = page_in_memory(rows.clone(), None, None, Some(2), None)
            .await
            .unwrap();
        assert_eq!(first_two.edges.len(), 2);
        assert!(first_two.has_next_page);
        assert!(!first_two.has_previous_page);

        let after = first_two.edges[1].cursor.encode_cursor();
        let rest: InMemoryPage = page_in_memory(rows.clone(), Some(after), None, Some(10), None)
            .await
            .unwrap();
        let nodes: Vec<&str> = rest.edges.iter().map(|edge| edge.node.as_str()).collect();
        assert_eq!(nodes, ["tag 2", "tag 3", "tag 4"]);
        assert!(!rest.has_next_page);

        let last_one: InMemoryPage = page_in_memory(rows, None, None, None, Some(1))
            .await
            .unwrap();
        assert_eq!(last_one.edges[0].node, "tag 4");
        assert!(last_one.has_previous_page);
    }
}
