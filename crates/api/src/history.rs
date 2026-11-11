//! What was watched, viewing by viewing, on the official API.
//!
//! A user reads and deletes its own history; an owner, who manages the server, reads and
//! deletes everyone's. A request with no caller has no history and sees none. Within that, an
//! entry is visible only while its item's library is, as everywhere.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, ID};
use open_plex_core::history::{
    HistoryEntry, HistoryOrder, HistoryQuery, HistoryScope, ViewingKind,
};
use open_plex_core::{HistoryId, RatingKey, SectionId, UserId};

use crate::cursor::OpaqueCursor;
use crate::filter::SortDirection;
use crate::item::{Item, ITEM_TYPE};
use crate::library::LIBRARY_TYPE;
use crate::node::GlobalId;
use crate::schema::ApiContext;
use crate::DateTime;

pub const HISTORY_ENTRY_TYPE: &str = "HistoryEntry";
const DEFAULT_PAGE_SIZE: u32 = 50;

/// How a viewing came about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ViewingKindType {
    /// A playback reached 90% of the runtime.
    Played,
    /// The item was marked watched without being played: each episode of a season or a
    /// series marked at once is one.
    Marked,
}

impl From<ViewingKind> for ViewingKindType {
    fn from(kind: ViewingKind) -> Self {
        match kind {
            ViewingKind::Played => Self::Played,
            ViewingKind::Marked => Self::Marked,
        }
    }
}

impl From<ViewingKindType> for ViewingKind {
    fn from(kind: ViewingKindType) -> Self {
        match kind {
            ViewingKindType::Played => Self::Played,
            ViewingKindType::Marked => Self::Marked,
        }
    }
}

/// How a history is ordered. Ties go to the order the viewings were recorded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Enum)]
pub enum HistoryOrderType {
    /// The order the viewings were recorded in.
    #[default]
    Recorded,
    /// By when the viewing happened.
    ViewedAt,
    /// By the library the item is in, in the order of the libraries' identifiers.
    Library,
}

impl From<HistoryOrderType> for HistoryOrder {
    fn from(order: HistoryOrderType) -> Self {
        match order {
            HistoryOrderType::Recorded => Self::Recorded,
            HistoryOrderType::ViewedAt => Self::ViewedAt,
            HistoryOrderType::Library => Self::Library,
        }
    }
}

/// Narrows a history. Every field is combined with `AND`.
#[derive(Debug, Default, InputObject)]
pub struct HistoryFilterInput {
    /// Only the caller's own viewings, which for an owner leaves out everyone else's.
    pub mine: Option<bool>,
    /// Only this user's viewings, as returned in `User.id`. A user naming another sees
    /// nothing.
    pub user: Option<ID>,
    /// Only this item's viewings, as returned in `id`; for a series or a season, its
    /// episodes'.
    pub item: Option<ID>,
    /// Only viewings of items in these libraries, as returned in `Library.id`.
    pub libraries: Option<Vec<ID>>,
    /// Only viewings of this kind.
    pub kind: Option<ViewingKindType>,
    /// Only viewings at or after this time.
    pub viewed_since: Option<DateTime>,
    /// Only viewings at or before this time.
    pub viewed_until: Option<DateTime>,
}

/// One viewing: an item played to the end, or marked watched.
#[derive(Debug, Clone, Copy)]
pub struct HistoryEntryObject(pub HistoryEntry);

#[Object(name = "HistoryEntry")]
impl HistoryEntryObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this entry.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(HISTORY_ENTRY_TYPE, self.0.id.get()))
    }

    /// The item watched: the copy, in the library it was watched in.
    async fn item(&self, context: &Context<'_>) -> Result<Option<Item>> {
        Ok(crate::schema::readable_item(context, self.0.item)
            .await?
            .map(Item::from_domain))
    }

    /// The library the item is in, as returned in `Library.id`.
    async fn library_id(&self) -> ID {
        ID(GlobalId::encode(LIBRARY_TYPE, self.0.section.get()))
    }

    /// Who watched. Null once that user is deleted.
    async fn user(&self, context: &Context<'_>) -> Result<Option<crate::access::UserObject>> {
        crate::access::user_node(context, self.0.viewer.get()).await
    }

    /// How the viewing came about.
    async fn kind(&self) -> ViewingKindType {
        self.0.kind.into()
    }

    /// When the item was watched.
    async fn viewed_at(&self) -> DateTime {
        self.0.viewed_at.into()
    }
}

/// What a history says beyond its entries.
#[derive(Debug, Clone)]
pub struct HistoryTotal(Option<HistoryQuery>);

#[Object]
impl HistoryTotal {
    /// How many entries the history holds in all, across every page.
    async fn total_count(&self, context: &Context<'_>) -> Result<u64> {
        match &self.0 {
            Some(query) => Ok(context
                .data::<ApiContext>()?
                .watch
                .history_count(query)
                .await?),
            None => Ok(0),
        }
    }
}

/// A page of viewing history.
pub type HistoryConnection =
    Connection<OpaqueCursor, HistoryEntryObject, HistoryTotal, EmptyFields>;

fn decoded(id: &ID, type_name: &str) -> Result<i64> {
    GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == type_name)
        .map(|decoded| decoded.key)
        .ok_or_else(|| async_graphql::Error::new(format!("not a {type_name} id")))
}

/// What the caller may read of the history it asked for. `None` when that is nothing.
fn history_query(
    context: &Context<'_>,
    filter: HistoryFilterInput,
    order: HistoryOrderType,
    direction: SortDirection,
) -> Result<Option<HistoryQuery>> {
    let Some(caller) = crate::access::caller(context) else {
        return Ok(None);
    };
    let named = filter
        .user
        .as_ref()
        .map(|id| decoded(id, crate::access::USER_TYPE).map(UserId::new))
        .transpose()?;
    let mine = filter.mine.unwrap_or(false);
    if mine && named.is_some() {
        return Err(async_graphql::Error::new(
            "`mine` and `user` cannot both be given",
        ));
    }
    let scope = if caller.is_owner() {
        match (mine, named) {
            (true, _) => HistoryScope::Viewer(caller.user),
            (false, Some(user)) => HistoryScope::Viewer(user),
            (false, None) => HistoryScope::Everyone,
        }
    } else if named.is_none_or(|named| named == caller.user) {
        HistoryScope::Viewer(caller.user)
    } else {
        return Ok(None);
    };

    let asked = filter
        .libraries
        .map(|ids| {
            ids.iter()
                .map(|id| decoded(id, LIBRARY_TYPE).map(SectionId::new))
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    let sections = match (crate::access::visible_sections(context), asked) {
        (Some(visible), Some(asked)) => Some(
            asked
                .into_iter()
                .filter(|section| visible.contains(section))
                .collect(),
        ),
        (visible, None) => visible,
        (None, asked) => asked,
    };
    let item = filter
        .item
        .as_ref()
        .map(|id| decoded(id, ITEM_TYPE).map(RatingKey::new))
        .transpose()?;

    Ok(Some(HistoryQuery {
        scope,
        item,
        sections,
        kind: filter.kind.map(Into::into),
        viewed_since: filter.viewed_since.map(DateTime::unix_seconds),
        viewed_until: filter.viewed_until.map(DateTime::unix_seconds),
        order: order.into(),
        direction: direction.into(),
    }))
}

// One Rust parameter per field argument, as for `Library.items`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn viewing_history(
    context: &Context<'_>,
    filter: Option<HistoryFilterInput>,
    order_by: Option<HistoryOrderType>,
    direction: Option<SortDirection>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
    offset: Option<u32>,
) -> Result<HistoryConnection> {
    let history = history_query(
        context,
        filter.unwrap_or_default(),
        order_by.unwrap_or_default(),
        direction.unwrap_or_default(),
    )?;
    let skip = u64::from(offset.unwrap_or(0));
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let Some(history) = history else {
                return Ok(Connection::with_additional_fields(
                    false,
                    false,
                    HistoryTotal(None),
                ));
            };
            let window = crate::schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                .skipping(skip);
            let page = context
                .data::<ApiContext>()?
                .watch
                .history(&history, window)
                .await?;
            let mut connection = Connection::with_additional_fields(
                page.has_previous,
                page.has_next,
                HistoryTotal(Some(history.clone())),
            );
            connection.edges.extend(page.items.into_iter().map(|entry| {
                Edge::new(
                    OpaqueCursor(history.cursor(&entry)),
                    HistoryEntryObject(entry),
                )
            }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// An entry the caller may see: its own, or anyone's for an owner, in a library it can
/// read. Unknown and not visible read the same.
pub(crate) async fn visible_entry(
    context: &Context<'_>,
    id: HistoryId,
) -> Result<Option<HistoryEntryObject>> {
    let Some(caller) = crate::access::caller(context) else {
        return Ok(None);
    };
    let entry = context
        .data::<ApiContext>()?
        .watch
        .history_entry(id)
        .await?;
    Ok(entry
        .filter(|entry| caller.owns(entry.viewer))
        .filter(|entry| crate::access::can_read(context, entry.section))
        .map(HistoryEntryObject))
}

/// An entry to delete.
#[derive(Debug, InputObject)]
pub struct DeleteHistoryEntryInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The entry, as returned in `HistoryEntry.id`.
    pub entry: ID,
}

/// Why an entry could not be deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum HistoryErrorCode {
    /// No such entry, or not one the caller can see.
    EntryNotFound,
}

/// A problem with what was asked, as opposed to a failure of the server.
#[derive(Debug, Clone, SimpleObject)]
pub struct HistoryUserError {
    /// The input field at fault, as a path from the input's root.
    pub field: Vec<String>,
    /// What went wrong, for a person.
    pub message: String,
    /// What went wrong, for a program.
    pub code: HistoryErrorCode,
}

/// What `deleteHistoryEntry` did.
#[derive(Debug, SimpleObject)]
pub struct DeleteHistoryEntryPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The entry deleted, as it was given. Null when nothing was.
    pub deleted_entry_id: Option<ID>,
    /// Why nothing was deleted. Empty when the entry was.
    pub user_errors: Vec<HistoryUserError>,
}

pub(crate) async fn delete_history_entry(
    context: &Context<'_>,
    input: DeleteHistoryEntryInput,
) -> Result<DeleteHistoryEntryPayload> {
    let visible = match decoded(&input.entry, HISTORY_ENTRY_TYPE) {
        Ok(key) => visible_entry(context, HistoryId::new(key)).await?,
        Err(_) => None,
    };
    let deleted = match visible {
        Some(entry) => {
            context
                .data::<ApiContext>()?
                .watch
                .delete_history_entry(entry.0.id)
                .await?
        }
        None => false,
    };
    if deleted {
        return Ok(DeleteHistoryEntryPayload {
            client_mutation_id: input.client_mutation_id,
            deleted_entry_id: Some(input.entry),
            user_errors: Vec::new(),
        });
    }
    Ok(DeleteHistoryEntryPayload {
        client_mutation_id: input.client_mutation_id,
        deleted_entry_id: None,
        user_errors: vec![HistoryUserError {
            field: vec!["input".to_owned(), "entry".to_owned()],
            message: "no such entry".to_owned(),
            code: HistoryErrorCode::EntryNotFound,
        }],
    })
}
