//! Markers on the official API: `Movie.markers`, `Episode.markers`, and the three mutations
//! that add, change and remove one.
//!
//! Who may write follows whose a marker is. An intro, the credits or a commercial belongs to
//! the work, and is written by the item's library's admins and the server's owners, as an item's
//! metadata is. A bookmark belongs to the caller who keeps it, and to nobody else: any caller
//! who reads the item may keep one, and only its owner sees, moves or removes it. What a caller
//! may not write answers as if it did not exist.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::connection::{Connection, EmptyFields};
use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, InputObject, MaybeUndefined, Object, Result, SimpleObject, ID};
use open_plex_core::marker::{Marker, MarkerError, MarkerKind, MarkerValues, NewMarker};
use open_plex_core::metadata::{MetadataError, MetadataItem, MetadataRepository};
use open_plex_core::{MarkerId, RatingKey, UserId};

use crate::cursor::OpaqueCursor;
use crate::item::{Item, ITEM_TYPE};
use crate::node::GlobalId;

pub const MARKER_TYPE: &str = "Marker";

/// What a marker marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "MarkerKind")]
pub enum MarkerKindType {
    /// The opening titles, which a player offers to skip.
    Intro,
    /// Credits, at the end or within the item; the ones that run to the end are `final`.
    Credits,
    /// An advertisement break.
    Commercial,
    /// A place the caller kept. Seen by the caller alone.
    Bookmark,
}

impl From<MarkerKind> for MarkerKindType {
    fn from(kind: MarkerKind) -> Self {
        match kind {
            MarkerKind::Intro => Self::Intro,
            MarkerKind::Credits => Self::Credits,
            MarkerKind::Commercial => Self::Commercial,
            MarkerKind::Bookmark => Self::Bookmark,
        }
    }
}

impl From<MarkerKindType> for MarkerKind {
    fn from(kind: MarkerKindType) -> Self {
        match kind {
            MarkerKindType::Intro => Self::Intro,
            MarkerKindType::Credits => Self::Credits,
            MarkerKindType::Commercial => Self::Commercial,
            MarkerKindType::Bookmark => Self::Bookmark,
        }
    }
}

/// A span of a film or an episode: its intro, credits or a commercial, which every viewer
/// sees, or a place the caller bookmarked.
#[derive(Debug, Clone)]
pub struct MarkerObject(pub Marker);

#[Object(name = "Marker")]
impl MarkerObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this marker.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(MARKER_TYPE, self.0.id.get()))
    }

    /// The film or episode it marks.
    async fn item(&self, context: &Context<'_>) -> Result<Option<Item>> {
        Ok(crate::schema::readable_item(context, self.0.item)
            .await?
            .map(Item::from_domain))
    }

    /// What it marks.
    async fn kind(&self) -> MarkerKindType {
        self.0.kind.into()
    }

    /// Where it starts, in milliseconds from the start of the media.
    async fn start_ms(&self) -> i64 {
        self.0.start_ms
    }

    /// Where it ends, in milliseconds from the start of the media. Null for a bookmark that
    /// marks a point. Every other marker has one, though a detection may have written one
    /// before its start: such markers are published as found.
    async fn end_ms(&self) -> Option<i64> {
        self.0.end_ms
    }

    /// Whether these are the credits that run to the end of the item, after which a player
    /// moves to what comes next. False for every other marker.
    #[graphql(name = "final")]
    async fn is_final(&self) -> bool {
        self.0.is_final
    }

    /// The version of the detection that found it. Null for a marker set or changed by hand.
    async fn version(&self) -> Option<i32> {
        self.0.version
    }

    /// A bookmark's name. Null when it has none, and on every other marker.
    async fn title(&self) -> Option<String> {
        self.0.title.clone()
    }

    /// A bookmark's colour, as the client that set it wrote it (`ff0000`). Null when it has
    /// none, and on every other marker.
    async fn color(&self) -> Option<String> {
        self.0.color.clone()
    }
}

/// An item's markers, in the order they were added.
pub type MarkerConnection = Connection<OpaqueCursor, MarkerObject, EmptyFields, EmptyFields>;

/// Loads the markers of several items, for one viewer or none, in one call.
#[derive(Debug, Clone)]
pub struct MarkersLoader {
    repository: Arc<dyn MetadataRepository>,
}

impl MarkersLoader {
    pub const fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<(Option<UserId>, RatingKey)> for MarkersLoader {
    type Value = Vec<Marker>;
    type Error = Arc<MetadataError>;

    async fn load(
        &self,
        keys: &[(Option<UserId>, RatingKey)],
    ) -> Result<HashMap<(Option<UserId>, RatingKey), Self::Value>, Self::Error> {
        let mut by_viewer: HashMap<Option<UserId>, Vec<RatingKey>> = HashMap::new();
        for (viewer, item) in keys {
            by_viewer.entry(*viewer).or_default().push(*item);
        }
        let mut found: HashMap<(Option<UserId>, RatingKey), Self::Value> = HashMap::new();
        for (viewer, items) in by_viewer {
            for marker in self
                .repository
                .markers(&items, viewer)
                .await
                .map_err(Arc::new)?
            {
                found.entry((viewer, marker.item)).or_default().push(marker);
            }
        }
        Ok(found)
    }
}

fn viewer_of(context: &Context<'_>) -> Option<UserId> {
    crate::access::viewer_id(context)
}

/// The markers of an item the caller reads, in the order they were added.
pub(crate) async fn markers_of(
    context: &Context<'_>,
    item: RatingKey,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<MarkerConnection> {
    let rows = context
        .data::<DataLoader<MarkersLoader>>()?
        .load_one((viewer_of(context), item))
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(|marker| (MarkerObject(marker), EmptyFields))
        .collect();
    crate::tag::page_in_memory(rows, after, before, first, last).await
}

/// A marker as `node` refetches it: one the caller sees, on an item it reads.
pub(crate) async fn marker_node(context: &Context<'_>, id: i64) -> Result<Option<MarkerObject>> {
    Ok(visible_marker(context, MarkerId::new(id))
        .await?
        .map(|(marker, _)| MarkerObject(marker)))
}

async fn visible_marker(
    context: &Context<'_>,
    id: MarkerId,
) -> Result<Option<(Marker, MetadataItem)>> {
    let Some(marker) = crate::schema::metadata(context)?.marker(id).await? else {
        return Ok(None);
    };
    if !marker.is_visible_to(viewer_of(context)) {
        return Ok(None);
    }
    Ok(crate::schema::readable_item(context, marker.item)
        .await?
        .map(|item| (marker, item)))
}

/// Whether the caller may write markers of this kind on this item: the work's for the
/// library's editors, a bookmark for any caller who has a viewer to keep it for.
fn may_write(context: &Context<'_>, kind: MarkerKind, item: &MetadataItem) -> bool {
    if kind.is_personal() {
        viewer_of(context).is_some()
    } else {
        crate::access::can_edit(context, item.section_id)
    }
}

/// Why a marker mutation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum MarkerErrorCode {
    /// No item the caller may mark has this id.
    ItemNotFound,
    /// No marker the caller may change has this id.
    MarkerNotFound,
    /// The item is a series or a season, which nothing plays.
    NotPlayable,
    /// The start is before the start of the media.
    NegativeStart,
    /// The end is not after the start.
    EmptySpan,
    /// Only a bookmark can mark a point; every other marker needs an end.
    MissingEnd,
    /// Only credits can be final.
    FinalNotCredits,
    /// Only a bookmark has a title or a colour.
    NotBookmark,
}

/// A request a marker mutation refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct MarkerUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: MarkerErrorCode,
}

impl MarkerUserError {
    fn new(field: &str, code: MarkerErrorCode) -> Self {
        let message = match code {
            MarkerErrorCode::ItemNotFound => "no such item",
            MarkerErrorCode::MarkerNotFound => "no such marker",
            MarkerErrorCode::NotPlayable => "only films and episodes carry markers",
            MarkerErrorCode::NegativeStart => "a marker cannot start before the media",
            MarkerErrorCode::EmptySpan => "a marker must end after it starts",
            MarkerErrorCode::MissingEnd => "only a bookmark can mark a point",
            MarkerErrorCode::FinalNotCredits => "only credits can be final",
            MarkerErrorCode::NotBookmark => "only a bookmark has a title or a colour",
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }

    fn of(error: MarkerError) -> Self {
        match error {
            MarkerError::NotPlayable => Self::new("itemId", MarkerErrorCode::NotPlayable),
            // Checked against the caller before the rule ever is, so a refusal for want of a
            // viewer reads as the item being out of reach.
            MarkerError::NoViewer => Self::new("itemId", MarkerErrorCode::ItemNotFound),
            MarkerError::NegativeStart => Self::new("startMs", MarkerErrorCode::NegativeStart),
            MarkerError::EmptySpan => Self::new("endMs", MarkerErrorCode::EmptySpan),
            MarkerError::MissingEnd => Self::new("endMs", MarkerErrorCode::MissingEnd),
            MarkerError::FinalNotCredits => Self::new("final", MarkerErrorCode::FinalNotCredits),
            MarkerError::NotPersonal => Self::new("title", MarkerErrorCode::NotBookmark),
        }
    }
}

type Refusal<T> = std::result::Result<T, MarkerUserError>;

/// A marker to add to an item.
#[derive(Debug, InputObject)]
pub struct AddMarkerInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The film or episode, as returned in its `id`.
    pub item_id: ID,
    /// What it marks. A bookmark is the caller's; any other kind is the work's, and needs the
    /// right to edit the item's library.
    pub kind: MarkerKindType,
    /// Where it starts, in milliseconds from the start of the media.
    pub start_ms: i64,
    /// Where it ends, in milliseconds, after `startMs`. Required but for a bookmark.
    pub end_ms: Option<i64>,
    /// Whether these are the credits that run to the end. Credits only; false when absent.
    #[graphql(name = "final")]
    pub is_final: Option<bool>,
    /// A bookmark's name.
    pub title: Option<String>,
    /// A bookmark's colour, kept as written.
    pub color: Option<String>,
}

/// What `addMarker` did.
#[derive(Debug, SimpleObject)]
pub struct AddMarkerPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The marker, unless the request was refused. An add identical in kind and span to a
    /// marker of the item's already there — the caller's own, for a bookmark — returns that
    /// one, its title and colour as they were.
    pub marker: Option<MarkerObject>,
    /// Whether the marker returned is a new one. False when it was already there, and when
    /// the request was refused.
    pub added: bool,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<MarkerUserError>,
}

/// Changes to a marker. An absent field is left as it is; `null` clears what can be cleared.
#[derive(Debug, InputObject)]
pub struct UpdateMarkerInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The marker, as returned in its `id`.
    pub id: ID,
    /// Where it starts, in milliseconds.
    pub start_ms: Option<i64>,
    /// Where it ends, in milliseconds; `null` leaves a bookmark marking a point.
    pub end_ms: MaybeUndefined<i64>,
    /// Whether these are the credits that run to the end.
    #[graphql(name = "final")]
    pub is_final: Option<bool>,
    /// A bookmark's name; `null` or empty removes it.
    pub title: MaybeUndefined<String>,
    /// A bookmark's colour; `null` or empty removes it.
    pub color: MaybeUndefined<String>,
}

/// What `updateMarker` did.
#[derive(Debug, SimpleObject)]
pub struct UpdateMarkerPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The marker as it now is, unless the request was refused. A marker of the work changed
    /// by hand no longer carries the detection's `version`.
    pub marker: Option<MarkerObject>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<MarkerUserError>,
}

/// A marker to remove.
#[derive(Debug, InputObject)]
pub struct DeleteMarkerInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The marker, as returned in its `id`.
    pub id: ID,
}

/// What `deleteMarker` did.
#[derive(Debug, SimpleObject)]
pub struct DeleteMarkerPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The marker removed, as it was returned in `Marker.id`. Null when refused.
    pub deleted_marker_id: Option<ID>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<MarkerUserError>,
}

fn decode(id: &ID, type_name: &str) -> Option<i64> {
    GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == type_name)
        .map(|decoded| decoded.key)
}

/// The marker an input names, if the caller may write it.
async fn writable_marker(context: &Context<'_>, id: &ID) -> Result<Refusal<Marker>> {
    let not_found = Err(MarkerUserError::new("id", MarkerErrorCode::MarkerNotFound));
    let Some(id) = decode(id, MARKER_TYPE) else {
        return Ok(not_found);
    };
    match visible_marker(context, MarkerId::new(id)).await? {
        Some((marker, item)) if may_write(context, marker.kind, &item) => Ok(Ok(marker)),
        _ => Ok(not_found),
    }
}

pub(crate) async fn add(context: &Context<'_>, input: AddMarkerInput) -> Result<AddMarkerPayload> {
    let outcome = add_marker(context, &input).await?;
    let (marker, added, user_errors) = match outcome {
        Ok((marker, added)) => (Some(MarkerObject(marker)), added, Vec::new()),
        Err(error) => (None, false, vec![error]),
    };
    Ok(AddMarkerPayload {
        client_mutation_id: input.client_mutation_id,
        marker,
        added,
        user_errors,
    })
}

async fn add_marker(
    context: &Context<'_>,
    input: &AddMarkerInput,
) -> Result<Refusal<(Marker, bool)>> {
    let not_found = Err(MarkerUserError::new(
        "itemId",
        MarkerErrorCode::ItemNotFound,
    ));
    let Some(id) = decode(&input.item_id, ITEM_TYPE) else {
        return Ok(not_found);
    };
    let kind = MarkerKind::from(input.kind);
    let Some(item) = crate::schema::readable_item(context, RatingKey::new(id)).await? else {
        return Ok(not_found);
    };
    if !may_write(context, kind, &item) {
        return Ok(not_found);
    }
    let values = MarkerValues {
        start_ms: input.start_ms,
        end_ms: input.end_ms,
        is_final: input.is_final.unwrap_or(false),
        title: input.title.clone(),
        color: input.color.clone(),
    };
    let marker = match NewMarker::new(&item, kind, viewer_of(context), values) {
        Ok(marker) => marker,
        Err(error) => return Ok(Err(MarkerUserError::of(error))),
    };
    Ok(Ok(crate::schema::metadata(context)?
        .add_marker(&marker)
        .await?))
}

pub(crate) async fn update(
    context: &Context<'_>,
    input: UpdateMarkerInput,
) -> Result<UpdateMarkerPayload> {
    let client_mutation_id = input.client_mutation_id.clone();
    let (marker, user_errors) = match update_marker(context, input).await? {
        Ok(marker) => (Some(MarkerObject(marker)), Vec::new()),
        Err(error) => (None, vec![error]),
    };
    Ok(UpdateMarkerPayload {
        client_mutation_id,
        marker,
        user_errors,
    })
}

async fn update_marker(context: &Context<'_>, input: UpdateMarkerInput) -> Result<Refusal<Marker>> {
    let marker = match writable_marker(context, &input.id).await? {
        Ok(marker) => marker,
        Err(error) => return Ok(Err(error)),
    };
    let mut values = marker.values();
    if let Some(start_ms) = input.start_ms {
        values.start_ms = start_ms;
    }
    if let Some(end_ms) = Option::<Option<i64>>::from(input.end_ms) {
        values.end_ms = end_ms;
    }
    if let Some(is_final) = input.is_final {
        values.is_final = is_final;
    }
    if let Some(title) = Option::<Option<String>>::from(input.title) {
        values.title = title;
    }
    if let Some(color) = Option::<Option<String>>::from(input.color) {
        values.color = color;
    }
    let edited = match marker.edited(values) {
        Ok(edited) => edited,
        Err(error) => return Ok(Err(MarkerUserError::of(error))),
    };
    let Some(updated) = crate::schema::metadata(context)?
        .update_marker(&edited)
        .await?
    else {
        // Removed since it was read.
        return Ok(Err(MarkerUserError::new(
            "id",
            MarkerErrorCode::MarkerNotFound,
        )));
    };
    Ok(Ok(updated))
}

pub(crate) async fn delete(
    context: &Context<'_>,
    input: DeleteMarkerInput,
) -> Result<DeleteMarkerPayload> {
    let refused = |error| DeleteMarkerPayload {
        client_mutation_id: input.client_mutation_id.clone(),
        deleted_marker_id: None,
        user_errors: vec![error],
    };
    let marker = match writable_marker(context, &input.id).await? {
        Ok(marker) => marker,
        Err(error) => return Ok(refused(error)),
    };
    if !crate::schema::metadata(context)?
        .delete_marker(marker.id)
        .await?
    {
        return Ok(refused(MarkerUserError::new(
            "id",
            MarkerErrorCode::MarkerNotFound,
        )));
    }
    Ok(DeleteMarkerPayload {
        client_mutation_id: input.client_mutation_id.clone(),
        deleted_marker_id: Some(ID(GlobalId::encode(MARKER_TYPE, marker.id.get()))),
        user_errors: Vec::new(),
    })
}
