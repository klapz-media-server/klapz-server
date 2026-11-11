//! Playlists on the official API.
//!
//! A playlist is its owner's: the operator's, or one user's. Nobody else reaches it, and
//! asking for someone else's answers as asking for nothing. Its items stay under their
//! libraries' rights, so what a playlist shows follows what its owner may read today.
//!
//! A smart playlist keeps a library listing — filter, order, direction — in the shape
//! `Library.items` takes it, and lists it anew on every read.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{
    Context, Enum, InputObject, InputType, Object, Result, SimpleObject, Value, ID,
};
use open_plex_core::metadata::{ItemFilter, ItemKind, ItemQuery};
use open_plex_core::page::{Page, Window};
use open_plex_core::playlist::{
    Destination, NewPlaylist, Playlist, PlaylistEdit, PlaylistEntry, PlaylistError, PlaylistField,
    PlaylistFilter, PlaylistKind, PlaylistQuery, PlaylistRepository, PlaylistSort, PlaylistTotals,
    SmartRule,
};
use open_plex_core::watch::Viewer;
use open_plex_core::{
    now_epoch, MetadataItem, PlaylistEntryId, PlaylistId, RatingKey, SectionId, SectionKind,
};

use crate::cursor::OpaqueCursor;
use crate::filter::{ItemFilterInput, ItemOrder, SortDirection};
use crate::item::{Item, ItemConnection, ITEM_TYPE};
use crate::library::{Library, LIBRARY_TYPE};
use crate::loader::ItemLoader;
use crate::node::GlobalId;
use crate::schema::{self, ApiContext};

pub const PLAYLIST_TYPE: &str = "Playlist";
pub const PLAYLIST_ENTRY_TYPE: &str = "PlaylistEntry";
const DEFAULT_PAGE_SIZE: u32 = 50;

pub type PlaylistConnection = Connection<OpaqueCursor, PlaylistObject, PlaylistTotal, EmptyFields>;
pub type PlaylistEntryConnection =
    Connection<OpaqueCursor, PlaylistEntryObject, EmptyFields, EmptyFields>;

fn playlists<'c>(context: &Context<'c>) -> Result<&'c dyn PlaylistRepository> {
    Ok(context.data::<ApiContext>()?.playlists.as_ref())
}

/// Whose playlists the caller reads and writes. A caller with no watch state, a visitor, has
/// none.
fn viewer(context: &Context<'_>) -> Option<Viewer> {
    crate::access::caller(context).and_then(Viewer::of)
}

/// What a playlist plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "PlaylistKind")]
pub enum PlaylistKindType {
    /// Films and episodes.
    Video,
    /// Music. Nothing here holds any, so such a playlist stays empty.
    Audio,
    /// Photographs. Nothing here holds any, so such a playlist stays empty.
    Photo,
}

impl From<PlaylistKind> for PlaylistKindType {
    fn from(kind: PlaylistKind) -> Self {
        match kind {
            PlaylistKind::Video => Self::Video,
            PlaylistKind::Audio => Self::Audio,
            PlaylistKind::Photo => Self::Photo,
        }
    }
}

impl From<PlaylistKindType> for PlaylistKind {
    fn from(kind: PlaylistKindType) -> Self {
        match kind {
            PlaylistKindType::Video => Self::Video,
            PlaylistKindType::Audio => Self::Audio,
            PlaylistKindType::Photo => Self::Photo,
        }
    }
}

/// A playlist field an editor can lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "PlaylistField")]
pub enum PlaylistFieldType {
    /// `title`.
    Title,
    /// `summary`.
    Summary,
}

impl From<PlaylistField> for PlaylistFieldType {
    fn from(field: PlaylistField) -> Self {
        match field {
            PlaylistField::Title => Self::Title,
            PlaylistField::Summary => Self::Summary,
        }
    }
}

impl From<PlaylistFieldType> for PlaylistField {
    fn from(field: PlaylistFieldType) -> Self {
        match field {
            PlaylistFieldType::Title => Self::Title,
            PlaylistFieldType::Summary => Self::Summary,
        }
    }
}

/// How the caller's playlists are ordered. Equal values follow in title order, ascending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Enum)]
pub enum PlaylistOrder {
    /// Smart playlists first, whichever the direction, then by title, case-insensitively.
    #[default]
    Title,
    /// By when the playlist was created.
    AddedAt,
    /// By when the playlist or its entries last changed.
    UpdatedAt,
    /// By when the playlist was last played, its creation counting as a play.
    LastViewedAt,
}

impl From<PlaylistOrder> for PlaylistSort {
    fn from(order: PlaylistOrder) -> Self {
        match order {
            PlaylistOrder::Title => Self::Title,
            PlaylistOrder::AddedAt => Self::AddedAt,
            PlaylistOrder::UpdatedAt => Self::UpdatedAt,
            PlaylistOrder::LastViewedAt => Self::LastViewedAt,
        }
    }
}

/// What a smart playlist lists: one library's items, as `Library.items` lists them.
#[derive(Debug, Clone, InputObject)]
pub struct SmartPlaylistRuleInput {
    /// The library, as returned in `Library.id`. It must hold films or series.
    pub library_id: ID,
    /// Narrows the listing, as `Library.items` takes it.
    pub filter: Option<ItemFilterInput>,
    /// How to order it. Omitted, by title.
    pub order_by: Option<ItemOrder>,
    /// Which direction to order in. Omitted, ascending.
    pub direction: Option<SortDirection>,
}

impl SmartPlaylistRuleInput {
    /// The rule as stored: this input, in its own JSON shape.
    fn stored(&self, section: SectionId) -> Result<SmartRule> {
        Ok(SmartRule {
            section,
            listing: serde_json::to_string(&self.to_value())?,
        })
    }

    fn read(rule: &SmartRule) -> Result<Self> {
        let value: Value = serde_json::from_str(&rule.listing)?;
        // Only this module writes the rule, so one that no longer parses was stored by an
        // older schema.
        Self::parse(Some(value))
            .map_err(|_| async_graphql::Error::new("a stored rule no longer reads as a rule"))
    }

    fn query(&self, context: &Context<'_>, section: SectionId) -> Result<ItemQuery> {
        let sort = self.order_by.unwrap_or_default().sort(viewer(context), 0);
        crate::library::listing_query(context, section, self.filter.clone(), sort, self.direction)
    }
}

/// A smart playlist's rule.
#[derive(Debug, Clone)]
pub struct SmartPlaylistRule {
    section: SectionId,
    rule: SmartPlaylistRuleInput,
}

#[Object]
impl SmartPlaylistRule {
    /// The library it lists. Null once the owner may no longer read it, and then the
    /// playlist lists nothing.
    async fn library(&self, context: &Context<'_>) -> Result<Option<Library>> {
        if !crate::access::can_read(context, self.section) {
            return Ok(None);
        }
        schema::load_library(context, self.section).await
    }

    /// How the listing is ordered.
    async fn order_by(&self) -> ItemOrder {
        self.rule.order_by.unwrap_or_default()
    }

    /// Which direction it is ordered in.
    async fn direction(&self) -> SortDirection {
        self.rule.direction.unwrap_or_default()
    }
}

/// How many playlists a listing holds, past the page.
#[derive(Debug, Clone)]
pub struct PlaylistTotal {
    query: Option<PlaylistQuery>,
}

#[Object]
impl PlaylistTotal {
    /// How many playlists the listing holds, every page together.
    async fn total_count(&self, context: &Context<'_>) -> Result<u64> {
        match &self.query {
            Some(query) => Ok(playlists(context)?.count(query).await?),
            None => Ok(0),
        }
    }
}

/// A list of what to play, kept by its owner.
#[derive(Debug, Clone)]
pub struct PlaylistObject(pub Playlist);

#[Object(name = "Playlist")]
impl PlaylistObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this playlist.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(PLAYLIST_TYPE, self.0.id.get()))
    }

    /// An identifier unique across servers: a UUID, drawn when the playlist was created.
    async fn guid(&self) -> &str {
        &self.0.guid
    }

    /// Its name.
    async fn title(&self) -> &str {
        &self.0.title
    }

    /// Its description. Empty when it has none.
    async fn summary(&self) -> &str {
        &self.0.summary
    }

    /// What it plays.
    async fn kind(&self) -> PlaylistKindType {
        self.0.kind.into()
    }

    /// Whether it lists what a rule finds rather than items chosen one by one.
    async fn is_smart(&self) -> bool {
        self.0.is_smart()
    }

    /// The rule of a smart playlist. Null for one of chosen items.
    async fn rule(&self) -> Result<Option<SmartPlaylistRule>> {
        let Some(rule) = &self.0.rule else {
            return Ok(None);
        };
        Ok(Some(SmartPlaylistRule {
            section: rule.section,
            rule: SmartPlaylistRuleInput::read(rule)?,
        }))
    }

    /// Where its content came from, as the client that created it named it. Null when it
    /// named none.
    async fn source_uri(&self) -> Option<&str> {
        self.0.source_uri.as_deref()
    }

    /// The fields an editor locked.
    async fn locked_fields(&self) -> Vec<PlaylistFieldType> {
        self.0.locked.iter().copied().map(Into::into).collect()
    }

    /// How many times it was played, its creation counting as the first.
    async fn view_count(&self) -> u64 {
        self.0.view_count
    }

    /// When it was last played or, never played, created; in seconds since the Unix epoch.
    async fn last_viewed_at(&self) -> i64 {
        self.0.last_viewed_at
    }

    /// When it was created, in seconds since the Unix epoch.
    async fn added_at(&self) -> i64 {
        self.0.added_at
    }

    /// When it or its entries last changed, in seconds since the Unix epoch.
    async fn updated_at(&self) -> i64 {
        self.0.updated_at
    }

    /// How many items it lists, of those its owner may read.
    async fn item_count(&self, context: &Context<'_>) -> Result<u64> {
        match &self.0.rule {
            Some(rule) => match smart_query(context, rule)? {
                Some(query) => Ok(schema::metadata(context)?.item_count(&query).await?),
                None => Ok(0),
            },
            None => Ok(totals(context, self.0.id).await?.entries),
        }
    }

    /// Its items' durations summed, in milliseconds, of those its owner may read. Null for a
    /// smart playlist, whose duration is not computed.
    async fn duration_ms(&self, context: &Context<'_>) -> Result<Option<i64>> {
        if self.0.is_smart() {
            return Ok(None);
        }
        Ok(Some(totals(context, self.0.id).await?.duration_ms))
    }

    /// Its items in order, of those its owner may read: the chosen ones, or what the rule
    /// finds now.
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
        #[graphql(desc = "Skip this many items first, to land in the middle of the playlist.")]
        offset: Option<u32>,
    ) -> Result<ItemConnection> {
        let skip = u64::from(offset.unwrap_or(0));
        let playlist = &self.0;
        query(
            after,
            before,
            first,
            last,
            |after, before, first, last| async move {
                let window = schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                    .skipping(skip);
                let mut connection = Connection::new(false, false);
                if let Some(rule) = &playlist.rule {
                    let Some(query) = smart_query(context, rule)? else {
                        return Ok(connection);
                    };
                    let page = schema::metadata(context)?.items(&query, window).await?;
                    connection.has_previous_page = page.has_previous;
                    connection.has_next_page = page.has_next;
                    connection
                        .edges
                        .extend(page.items.into_iter().map(|listed| {
                            Edge::new(OpaqueCursor(listed.cursor), Item::from_domain(listed.item))
                        }));
                } else {
                    let (page, items) = entries_with_items(context, playlist.id, window).await?;
                    connection.has_previous_page = page.has_previous;
                    connection.has_next_page = page.has_next;
                    connection
                        .edges
                        .extend(page.items.iter().zip(items).map(|(entry, item)| {
                            Edge::new(OpaqueCursor(entry.cursor()), Item::from_domain(item))
                        }));
                }
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }

    /// Its entries in order, of those its owner may read: one per chosen item. Empty for a
    /// smart playlist, which holds none.
    #[allow(clippy::too_many_arguments)]
    async fn entries(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
        #[graphql(desc = "Skip this many entries first, to land in the middle of the playlist.")]
        offset: Option<u32>,
    ) -> Result<PlaylistEntryConnection> {
        let skip = u64::from(offset.unwrap_or(0));
        let playlist = &self.0;
        query(
            after,
            before,
            first,
            last,
            |after, before, first, last| async move {
                if playlist.is_smart() {
                    return Ok(Connection::new(false, false));
                }
                let window = schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                    .skipping(skip);
                let sections = crate::access::visible_sections(context);
                let page = playlists(context)?
                    .entries(playlist.id, sections.as_deref(), window)
                    .await?;
                let mut connection = Connection::new(page.has_previous, page.has_next);
                connection.edges.extend(page.items.into_iter().map(|entry| {
                    Edge::new(OpaqueCursor(entry.cursor()), PlaylistEntryObject(entry))
                }));
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }
}

/// One chosen item at its place in a playlist.
#[derive(Debug, Clone, Copy)]
pub struct PlaylistEntryObject(pub PlaylistEntry);

#[Object(name = "PlaylistEntry")]
impl PlaylistEntryObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this entry.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(PLAYLIST_ENTRY_TYPE, self.0.id.get()))
    }

    /// The playlist it belongs to, as returned in `Playlist.id`.
    async fn playlist_id(&self) -> ID {
        ID(GlobalId::encode(PLAYLIST_TYPE, self.0.playlist.get()))
    }

    /// What it plays. Null if the owner may no longer read it.
    async fn item(&self, context: &Context<'_>) -> Result<Option<Item>> {
        Ok(schema::readable_item(context, self.0.item)
            .await?
            .map(Item::from_domain))
    }
}

/// The listing a smart playlist runs for its reader, or `None` when the reader may not read
/// its library.
fn smart_query(context: &Context<'_>, rule: &SmartRule) -> Result<Option<ItemQuery>> {
    if !crate::access::can_read(context, rule.section) {
        return Ok(None);
    }
    Ok(Some(
        SmartPlaylistRuleInput::read(rule)?.query(context, rule.section)?,
    ))
}

async fn entries_with_items(
    context: &Context<'_>,
    playlist: PlaylistId,
    window: Window,
) -> Result<(Page<PlaylistEntry>, Vec<MetadataItem>)> {
    let sections = crate::access::visible_sections(context);
    let mut page = playlists(context)?
        .entries(playlist, sections.as_deref(), window)
        .await?;
    let keys: Vec<RatingKey> = page.items.iter().map(|entry| entry.item).collect();
    let mut loaded = context
        .data::<DataLoader<ItemLoader>>()?
        .load_many(keys)
        .await?;
    // Storage joined each entry to its item; one deleted since is dropped from the page.
    page.items.retain(|entry| loaded.contains_key(&entry.item));
    let items = page
        .items
        .iter()
        .filter_map(|entry| loaded.remove(&entry.item))
        .collect();
    Ok((page, items))
}

/// Loads playlists' totals for one reader: the libraries it sees are part of the key, since
/// two readers of one playlist can see different parts of it.
#[derive(Debug, Clone)]
pub struct PlaylistTotalsLoader {
    repository: Arc<dyn PlaylistRepository>,
}

impl PlaylistTotalsLoader {
    pub const fn new(repository: Arc<dyn PlaylistRepository>) -> Self {
        Self { repository }
    }
}

type TotalsKey = (PlaylistId, Option<Vec<SectionId>>);

impl Loader<TotalsKey> for PlaylistTotalsLoader {
    type Value = PlaylistTotals;
    type Error = Arc<PlaylistError>;

    async fn load(
        &self,
        keys: &[TotalsKey],
    ) -> Result<HashMap<TotalsKey, Self::Value>, Self::Error> {
        let mut by_sections: HashMap<Option<Vec<SectionId>>, Vec<PlaylistId>> = HashMap::new();
        for (playlist, sections) in keys {
            by_sections
                .entry(sections.clone())
                .or_default()
                .push(*playlist);
        }
        let mut found = HashMap::new();
        for (sections, ids) in by_sections {
            for (playlist, totals) in self
                .repository
                .totals(&ids, sections.as_deref())
                .await
                .map_err(Arc::new)?
            {
                found.insert((playlist, sections.clone()), totals);
            }
        }
        Ok(found)
    }
}

async fn totals(context: &Context<'_>, playlist: PlaylistId) -> Result<PlaylistTotals> {
    let key = (playlist, crate::access::visible_sections(context));
    Ok(context
        .data::<DataLoader<PlaylistTotalsLoader>>()?
        .load_one(key)
        .await?
        .unwrap_or_default())
}

/// The caller's playlist under this id, if it is one.
async fn owned(context: &Context<'_>, id: PlaylistId) -> Result<Option<Playlist>> {
    let Some(viewer) = viewer(context) else {
        return Ok(None);
    };
    Ok(playlists(context)?
        .playlist(id)
        .await?
        .filter(|playlist| playlist.owner == viewer))
}

async fn owned_entry(
    context: &Context<'_>,
    id: PlaylistEntryId,
) -> Result<Option<(Playlist, PlaylistEntry)>> {
    let Some(entry) = playlists(context)?.entry(id).await? else {
        return Ok(None);
    };
    // An entry its owner can no longer read answers as one that does not exist.
    if schema::readable_item(context, entry.item).await?.is_none() {
        return Ok(None);
    }
    Ok(owned(context, entry.playlist)
        .await?
        .map(|playlist| (playlist, entry)))
}

pub(crate) async fn playlist_node(
    context: &Context<'_>,
    key: i64,
) -> Result<Option<PlaylistObject>> {
    Ok(owned(context, PlaylistId::new(key))
        .await?
        .map(PlaylistObject))
}

pub(crate) async fn entry_node(
    context: &Context<'_>,
    key: i64,
) -> Result<Option<PlaylistEntryObject>> {
    Ok(owned_entry(context, PlaylistEntryId::new(key))
        .await?
        .map(|(_, entry)| PlaylistEntryObject(entry)))
}

/// `Query.playlists`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn list(
    context: &Context<'_>,
    kind: Option<PlaylistKindType>,
    smart: Option<bool>,
    library: Option<ID>,
    order_by: Option<PlaylistOrder>,
    direction: Option<SortDirection>,
    (after, before, first, last, offset): (
        Option<String>,
        Option<String>,
        Option<i32>,
        Option<i32>,
        Option<u32>,
    ),
) -> Result<PlaylistConnection> {
    let mut filter = PlaylistFilter {
        kind: kind.map(Into::into),
        smart,
        library: None,
    };
    let mut empty = viewer(context).is_none();
    if let Some(id) = library {
        let section = GlobalId::decode(&id)
            .ok()
            .filter(|decoded| decoded.type_name == LIBRARY_TYPE)
            .map(|decoded| SectionId::new(decoded.key))
            .ok_or_else(|| async_graphql::Error::new("not a library id"))?;
        match library_kind(context, section).await? {
            Some(plays) if filter.kind.is_none_or(|kind| kind == plays) => {
                filter.kind = Some(plays);
                filter.library = Some(section);
            }
            _ => empty = true,
        }
    }
    let playlist_query = viewer(context).map(|owner| PlaylistQuery {
        owner,
        filter,
        sort: order_by.unwrap_or_default().into(),
        direction: direction.unwrap_or_default().into(),
    });
    let skip = u64::from(offset.unwrap_or(0));
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let total = PlaylistTotal {
                query: playlist_query.clone().filter(|_| !empty),
            };
            let Some(playlist_query) = playlist_query.filter(|_| !empty) else {
                return Ok(Connection::with_additional_fields(false, false, total));
            };
            let window =
                schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?.skipping(skip);
            let page = playlists(context)?
                .playlists(&playlist_query, window)
                .await?;
            let mut connection =
                Connection::with_additional_fields(page.has_previous, page.has_next, total);
            connection
                .edges
                .extend(page.items.into_iter().map(|playlist| {
                    Edge::new(
                        OpaqueCursor(playlist_query.cursor(&playlist)),
                        PlaylistObject(playlist),
                    )
                }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// What a library's playlists play, for a library the caller can read.
async fn library_kind(context: &Context<'_>, section: SectionId) -> Result<Option<PlaylistKind>> {
    if !crate::access::can_read(context, section) {
        return Ok(None);
    }
    let Some(library) = context
        .data::<DataLoader<crate::loader::LibraryLoader>>()?
        .load_one(section)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(match library.kind {
        SectionKind::Artist => PlaylistKind::Audio,
        SectionKind::Photo => PlaylistKind::Photo,
        SectionKind::Movie | SectionKind::Show => PlaylistKind::Video,
    }))
}

/// Why a playlist mutation did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum PlaylistErrorCode {
    /// No playlist of the caller's carries this id.
    PlaylistNotFound,
    /// No entry of the caller's playlist carries this id.
    EntryNotFound,
    /// No item the caller can read carries this id.
    ItemNotFound,
    /// The playlist cannot hold an item of this kind: a video playlist holds films and
    /// episodes, and the others hold nothing this server has.
    NotPlayable,
    /// The playlist is smart: its items come from its rule, and cannot be changed one by one.
    SmartPlaylist,
    /// The playlist holds chosen items, and has no rule to change.
    NotSmart,
    /// The rule names no library the caller can read, a library of the wrong kind, or a
    /// filter it cannot honor.
    InvalidRule,
    /// A title cannot be empty.
    BlankTitle,
}

/// A request a playlist mutation refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct PlaylistUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: PlaylistErrorCode,
}

impl PlaylistUserError {
    fn new(field: &str, code: PlaylistErrorCode) -> Self {
        let message = match code {
            PlaylistErrorCode::PlaylistNotFound => "no such playlist",
            PlaylistErrorCode::EntryNotFound => "no such entry in this playlist",
            PlaylistErrorCode::ItemNotFound => "no such item",
            PlaylistErrorCode::NotPlayable => "this playlist cannot hold this item",
            PlaylistErrorCode::SmartPlaylist => "a smart playlist's items come from its rule",
            PlaylistErrorCode::NotSmart => "this playlist has no rule",
            PlaylistErrorCode::InvalidRule => "this rule cannot be listed",
            PlaylistErrorCode::BlankTitle => "a title cannot be empty",
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }
}

type Refusal<T> = std::result::Result<T, PlaylistUserError>;

fn playlist_id(id: &ID, field: &str) -> Refusal<PlaylistId> {
    GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == PLAYLIST_TYPE)
        .map(|decoded| PlaylistId::new(decoded.key))
        .ok_or_else(|| PlaylistUserError::new(field, PlaylistErrorCode::PlaylistNotFound))
}

fn entry_id(id: &ID, field: &str) -> Refusal<PlaylistEntryId> {
    GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == PLAYLIST_ENTRY_TYPE)
        .map(|decoded| PlaylistEntryId::new(decoded.key))
        .ok_or_else(|| PlaylistUserError::new(field, PlaylistErrorCode::EntryNotFound))
}

/// The caller's playlist, refused as not found otherwise.
async fn target(context: &Context<'_>, id: &ID, field: &str) -> Result<Refusal<Playlist>> {
    let id = match playlist_id(id, field) {
        Ok(id) => id,
        Err(refusal) => return Ok(Err(refusal)),
    };
    Ok(owned(context, id)
        .await?
        .ok_or_else(|| PlaylistUserError::new(field, PlaylistErrorCode::PlaylistNotFound)))
}

/// The items to add, checked one by one: each must be readable and playable.
async fn playable_items(
    context: &Context<'_>,
    kind: PlaylistKind,
    ids: &[ID],
) -> Result<Refusal<Vec<RatingKey>>> {
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(key) = GlobalId::decode(id)
            .ok()
            .filter(|decoded| decoded.type_name == ITEM_TYPE)
            .map(|decoded| RatingKey::new(decoded.key))
        else {
            return Ok(Err(PlaylistUserError::new(
                "itemIds",
                PlaylistErrorCode::ItemNotFound,
            )));
        };
        let Some(item) = schema::readable_item(context, key).await? else {
            return Ok(Err(PlaylistUserError::new(
                "itemIds",
                PlaylistErrorCode::ItemNotFound,
            )));
        };
        let playable =
            kind == PlaylistKind::Video && matches!(item.kind, ItemKind::Movie | ItemKind::Episode);
        if !playable {
            return Ok(Err(PlaylistUserError::new(
                "itemIds",
                PlaylistErrorCode::NotPlayable,
            )));
        }
        items.push(key);
    }
    Ok(Ok(items))
}

/// A rule checked against what the caller may list, in the form it is kept.
async fn checked_rule(
    context: &Context<'_>,
    kind: PlaylistKind,
    rule: &SmartPlaylistRuleInput,
) -> Result<Refusal<SmartRule>> {
    let invalid = || {
        Ok(Err(PlaylistUserError::new(
            "rule",
            PlaylistErrorCode::InvalidRule,
        )))
    };
    let Some(section) = GlobalId::decode(&rule.library_id)
        .ok()
        .filter(|decoded| decoded.type_name == LIBRARY_TYPE)
        .map(|decoded| SectionId::new(decoded.key))
    else {
        return invalid();
    };
    if kind != PlaylistKind::Video
        || library_kind(context, section).await? != Some(PlaylistKind::Video)
    {
        return invalid();
    }
    if ItemFilter::try_from(rule.filter.clone().unwrap_or_default()).is_err() {
        return invalid();
    }
    Ok(Ok(rule.stored(section)?))
}

/// A playlist to create: of chosen items, or smart with a rule.
#[derive(Debug, InputObject)]
pub struct CreatePlaylistInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// Its name. Not empty.
    pub title: String,
    /// Its description.
    #[graphql(default)]
    pub summary: String,
    /// What it plays.
    pub kind: PlaylistKindType,
    /// Its first items, in order, as returned in `Item.id`: films and episodes. A repeat is
    /// kept once. Not with `rule`.
    #[graphql(default)]
    pub item_ids: Vec<ID>,
    /// Makes it smart: it lists what this rule finds.
    pub rule: Option<SmartPlaylistRuleInput>,
    /// Where the content came from, as the client names it; kept and returned as given.
    pub source_uri: Option<String>,
}

/// The playlist created.
#[derive(Debug, SimpleObject)]
pub struct CreatePlaylistPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// Null when the request was refused.
    pub playlist: Option<PlaylistObject>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<PlaylistUserError>,
}

pub(crate) async fn create(
    context: &Context<'_>,
    input: CreatePlaylistInput,
) -> Result<CreatePlaylistPayload> {
    let refuse = |error| CreatePlaylistPayload {
        client_mutation_id: input.client_mutation_id.clone(),
        playlist: None,
        user_errors: vec![error],
    };
    let Some(owner) = viewer(context) else {
        return Err(crate::access::denied());
    };
    if input.title.trim().is_empty() {
        return Ok(refuse(PlaylistUserError::new(
            "title",
            PlaylistErrorCode::BlankTitle,
        )));
    }
    let kind = input.kind.into();
    let rule = match &input.rule {
        Some(_) if !input.item_ids.is_empty() => {
            return Ok(refuse(PlaylistUserError::new(
                "itemIds",
                PlaylistErrorCode::SmartPlaylist,
            )))
        }
        Some(rule) => match checked_rule(context, kind, rule).await? {
            Ok(rule) => Some(rule),
            Err(error) => return Ok(refuse(error)),
        },
        None => None,
    };
    let items = match playable_items(context, kind, &input.item_ids).await? {
        Ok(items) => items,
        Err(error) => return Ok(refuse(error)),
    };
    let playlist = playlists(context)?
        .create(
            NewPlaylist {
                owner,
                title: input.title,
                summary: input.summary,
                kind,
                items,
                rule,
                source_uri: input.source_uri,
            },
            now_epoch(),
        )
        .await?;
    Ok(CreatePlaylistPayload {
        client_mutation_id: input.client_mutation_id,
        playlist: Some(PlaylistObject(playlist)),
        user_errors: Vec::new(),
    })
}

/// Changes to a playlist's own fields. An absent field is left alone.
#[derive(Debug, InputObject)]
pub struct UpdatePlaylistInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The playlist, as returned in `Playlist.id`.
    pub id: ID,
    /// Its new name. Not empty.
    pub title: Option<String>,
    /// Its new description; empty clears it.
    pub summary: Option<String>,
    /// Fields to lock.
    #[graphql(default)]
    pub lock: Vec<PlaylistFieldType>,
    /// Fields to unlock.
    #[graphql(default)]
    pub unlock: Vec<PlaylistFieldType>,
}

/// A playlist after a change.
#[derive(Debug, SimpleObject)]
pub struct PlaylistPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The playlist as it now stands. Null when the request was refused.
    pub playlist: Option<PlaylistObject>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<PlaylistUserError>,
}

impl PlaylistPayload {
    fn of(client_mutation_id: Option<String>, outcome: Refusal<Playlist>) -> Self {
        match outcome {
            Ok(playlist) => Self {
                client_mutation_id,
                playlist: Some(PlaylistObject(playlist)),
                user_errors: Vec::new(),
            },
            Err(error) => Self {
                client_mutation_id,
                playlist: None,
                user_errors: vec![error],
            },
        }
    }
}

/// Reads a playlist back after a change, or says it went in between.
async fn reread(context: &Context<'_>, id: PlaylistId) -> Result<Refusal<Playlist>> {
    Ok(owned(context, id)
        .await?
        .ok_or_else(|| PlaylistUserError::new("id", PlaylistErrorCode::PlaylistNotFound)))
}

pub(crate) async fn update(
    context: &Context<'_>,
    input: UpdatePlaylistInput,
) -> Result<PlaylistPayload> {
    let outcome = async {
        let playlist = match target(context, &input.id, "id").await? {
            Ok(playlist) => playlist,
            Err(error) => return Ok(Err(error)),
        };
        if input
            .title
            .as_deref()
            .is_some_and(|title| title.trim().is_empty())
        {
            return Ok(Err(PlaylistUserError::new(
                "title",
                PlaylistErrorCode::BlankTitle,
            )));
        }
        let edit = PlaylistEdit {
            title: input.title,
            summary: input.summary,
            lock: input.lock.into_iter().map(Into::into).collect(),
            unlock: input.unlock.into_iter().map(Into::into).collect(),
        };
        Ok::<_, async_graphql::Error>(Ok(playlists(context)?
            .edit(playlist.id, &edit, now_epoch())
            .await?))
    }
    .await?;
    Ok(PlaylistPayload::of(input.client_mutation_id, outcome))
}

/// A playlist to delete.
#[derive(Debug, InputObject)]
pub struct DeletePlaylistInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The playlist, as returned in `Playlist.id`.
    pub id: ID,
}

/// What a deletion removed.
#[derive(Debug, SimpleObject)]
pub struct DeletePlaylistPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The playlist deleted, as it was returned in `Playlist.id`. Null when refused.
    pub deleted_playlist_id: Option<ID>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<PlaylistUserError>,
}

pub(crate) async fn delete(
    context: &Context<'_>,
    input: DeletePlaylistInput,
) -> Result<DeletePlaylistPayload> {
    let outcome = match target(context, &input.id, "id").await? {
        Ok(playlist) if playlists(context)?.delete(playlist.id).await? => Ok(input.id),
        Ok(_) => Err(PlaylistUserError::new(
            "id",
            PlaylistErrorCode::PlaylistNotFound,
        )),
        Err(error) => Err(error),
    };
    let (deleted_playlist_id, user_errors) = match outcome {
        Ok(id) => (Some(id), Vec::new()),
        Err(error) => (None, vec![error]),
    };
    Ok(DeletePlaylistPayload {
        client_mutation_id: input.client_mutation_id,
        deleted_playlist_id,
        user_errors,
    })
}

/// Items to append to a playlist of chosen items.
#[derive(Debug, InputObject)]
pub struct AddToPlaylistInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The playlist, as returned in `Playlist.id`.
    pub playlist_id: ID,
    /// The items, in order, as returned in `Item.id`: films and episodes. One the playlist
    /// holds already is not added again.
    pub item_ids: Vec<ID>,
}

/// A playlist after items were appended.
#[derive(Debug, SimpleObject)]
pub struct AddToPlaylistPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The playlist as it now stands. Null when the request was refused.
    pub playlist: Option<PlaylistObject>,
    /// How many of the items were added: those it did not hold already, each once.
    pub added_count: u64,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<PlaylistUserError>,
}

pub(crate) async fn add(
    context: &Context<'_>,
    input: AddToPlaylistInput,
) -> Result<AddToPlaylistPayload> {
    let outcome = async {
        let playlist = match target(context, &input.playlist_id, "playlistId").await? {
            Ok(playlist) => playlist,
            Err(error) => return Ok(Err(error)),
        };
        if playlist.is_smart() {
            return Ok(Err(PlaylistUserError::new(
                "playlistId",
                PlaylistErrorCode::SmartPlaylist,
            )));
        }
        let items = match playable_items(context, playlist.kind, &input.item_ids).await? {
            Ok(items) => items,
            Err(error) => return Ok(Err(error)),
        };
        let added = playlists(context)?
            .append(playlist.id, &items, now_epoch())
            .await?;
        Ok::<_, async_graphql::Error>(reread(context, playlist.id).await?.map(|p| (p, added)))
    }
    .await?;
    Ok(match outcome {
        Ok((playlist, added_count)) => AddToPlaylistPayload {
            client_mutation_id: input.client_mutation_id,
            playlist: Some(PlaylistObject(playlist)),
            added_count,
            user_errors: Vec::new(),
        },
        Err(error) => AddToPlaylistPayload {
            client_mutation_id: input.client_mutation_id,
            playlist: None,
            added_count: 0,
            user_errors: vec![error],
        },
    })
}

/// A new rule for a smart playlist.
#[derive(Debug, InputObject)]
pub struct ReplacePlaylistRuleInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The playlist, as returned in `Playlist.id`.
    pub playlist_id: ID,
    /// What it lists from now on.
    pub rule: SmartPlaylistRuleInput,
    /// Where the content came from, as the client names it; kept and returned as given.
    pub source_uri: Option<String>,
}

pub(crate) async fn replace_rule(
    context: &Context<'_>,
    input: ReplacePlaylistRuleInput,
) -> Result<PlaylistPayload> {
    let outcome = async {
        let playlist = match target(context, &input.playlist_id, "playlistId").await? {
            Ok(playlist) => playlist,
            Err(error) => return Ok(Err(error)),
        };
        if !playlist.is_smart() {
            return Ok(Err(PlaylistUserError::new(
                "playlistId",
                PlaylistErrorCode::NotSmart,
            )));
        }
        let rule = match checked_rule(context, playlist.kind, &input.rule).await? {
            Ok(rule) => rule,
            Err(error) => return Ok(Err(error)),
        };
        Ok::<_, async_graphql::Error>(Ok(playlists(context)?
            .replace_rule(playlist.id, rule, input.source_uri, now_epoch())
            .await?))
    }
    .await?;
    Ok(PlaylistPayload::of(input.client_mutation_id, outcome))
}

/// An entry to take out of its playlist.
#[derive(Debug, InputObject)]
pub struct RemoveFromPlaylistInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The entry, as returned in `PlaylistEntry.id`.
    pub entry_id: ID,
}

/// The caller's entry with its playlist, refused when the playlist is smart.
async fn target_entry(
    context: &Context<'_>,
    id: &ID,
    field: &str,
) -> Result<Refusal<(Playlist, PlaylistEntry)>> {
    let id = match entry_id(id, field) {
        Ok(id) => id,
        Err(error) => return Ok(Err(error)),
    };
    Ok(owned_entry(context, id)
        .await?
        .ok_or_else(|| PlaylistUserError::new(field, PlaylistErrorCode::EntryNotFound)))
}

pub(crate) async fn remove(
    context: &Context<'_>,
    input: RemoveFromPlaylistInput,
) -> Result<PlaylistPayload> {
    let outcome = async {
        let (playlist, entry) = match target_entry(context, &input.entry_id, "entryId").await? {
            Ok(found) => found,
            Err(error) => return Ok(Err(error)),
        };
        playlists(context)?.remove(entry.id, now_epoch()).await?;
        reread(context, playlist.id).await
    }
    .await?;
    Ok(PlaylistPayload::of(input.client_mutation_id, outcome))
}

/// An entry to move within its playlist.
#[derive(Debug, InputObject)]
pub struct MovePlaylistEntryInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The entry, as returned in `PlaylistEntry.id`.
    pub entry_id: ID,
    /// The entry to put it after, of the same playlist. Omitted, it goes first.
    pub after_entry_id: Option<ID>,
}

pub(crate) async fn move_entry(
    context: &Context<'_>,
    input: MovePlaylistEntryInput,
) -> Result<PlaylistPayload> {
    let outcome = async {
        let (playlist, entry) = match target_entry(context, &input.entry_id, "entryId").await? {
            Ok(found) => found,
            Err(error) => return Ok(Err(error)),
        };
        let destination = match &input.after_entry_id {
            None => Destination::First,
            Some(after) => match target_entry(context, after, "afterEntryId").await? {
                Ok((_, after)) if after.playlist == playlist.id => {
                    if after.id == entry.id {
                        return reread(context, playlist.id).await;
                    }
                    Destination::After(after.id)
                }
                Ok(_) => {
                    return Ok(Err(PlaylistUserError::new(
                        "afterEntryId",
                        PlaylistErrorCode::EntryNotFound,
                    )))
                }
                Err(error) => return Ok(Err(error)),
            },
        };
        match playlists(context)?
            .move_entry(entry.id, destination, now_epoch())
            .await
        {
            Ok(()) => reread(context, playlist.id).await,
            Err(PlaylistError::EntryNotFound(_)) => Ok(Err(PlaylistUserError::new(
                "entryId",
                PlaylistErrorCode::EntryNotFound,
            ))),
            Err(error) => Err(error.into()),
        }
    }
    .await?;
    Ok(PlaylistPayload::of(input.client_mutation_id, outcome))
}

/// A playlist of chosen items to empty.
#[derive(Debug, InputObject)]
pub struct ClearPlaylistInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The playlist, as returned in `Playlist.id`.
    pub id: ID,
}

pub(crate) async fn clear(
    context: &Context<'_>,
    input: ClearPlaylistInput,
) -> Result<PlaylistPayload> {
    let outcome = async {
        let playlist = match target(context, &input.id, "id").await? {
            Ok(playlist) => playlist,
            Err(error) => return Ok(Err(error)),
        };
        if playlist.is_smart() {
            return Ok(Err(PlaylistUserError::new(
                "id",
                PlaylistErrorCode::SmartPlaylist,
            )));
        }
        playlists(context)?.clear(playlist.id, now_epoch()).await?;
        reread(context, playlist.id).await
    }
    .await?;
    Ok(PlaylistPayload::of(input.client_mutation_id, outcome))
}

/// Counts a play of the caller's playlist, when one is named: a play queue made from it.
pub(crate) async fn record_play(context: &Context<'_>, id: PlaylistId) -> Result<()> {
    if owned(context, id).await?.is_some() {
        playlists(context)?.record_play(id, now_epoch()).await?;
    }
    Ok(())
}
