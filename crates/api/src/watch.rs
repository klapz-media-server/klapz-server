//! What the caller has watched, and the mutations that move it.
//!
//! Every field and mutation here is the caller's own: a user reads and writes its own state,
//! and a request with no caller has none. The rules are in
//! `core::watch`; these resolvers find the item, check it is visible, and call them.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::connection::{
    query, Connection, ConnectionNameType, Edge, EdgeNameType, EmptyFields,
};
use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, InputObject, Object, OutputType, Result, SimpleObject, ID};
use open_plex_core::metadata::{MetadataItem, StreamKind};
use open_plex_core::page::{Cursor, MAX_ITEMS};
use open_plex_core::tracks::{SubtitleChoice, TrackChoice};
use open_plex_core::watch::{
    DeckReason, ReportOutcome, WatchError, WatchRepository, WatchState, Watching,
};
use open_plex_core::{now_epoch, PartId, RatingKey, SectionId, UserId};

use crate::cursor::OpaqueCursor;
use crate::item::{Item, ITEM_TYPE};
use crate::library::LIBRARY_TYPE;
use crate::loader::ItemLoader;
use crate::node::GlobalId;
use crate::schema::ApiContext;
use crate::DateTime;

/// What the caller has watched of a work.
///
/// Shared by every copy of the work — the same film in two libraries, the same series in
/// its 4K library — because it is kept per `guid`.
#[derive(Debug, Clone, Copy)]
pub struct WatchStateObject(pub WatchState);

#[Object(name = "WatchState")]
impl WatchStateObject {
    /// How many times it was watched to the end. On a series or a season, how many of its
    /// episodes were, one by one: marking the whole season counts its episodes, not it.
    async fn view_count(&self) -> u32 {
        self.0.view_count
    }

    /// Whether it was watched to the end at least once.
    async fn is_watched(&self) -> bool {
        self.0.view_count > 0
    }

    /// Where an unfinished viewing stopped, in milliseconds. Null when there is none: never
    /// played, played for less than a minute, or watched to the end since.
    async fn offset_ms(&self) -> Option<i64> {
        self.0.offset_ms
    }

    /// When it was last played or watched. On a series or a
    /// season, when any of its episodes was.
    async fn last_viewed_at(&self) -> Option<DateTime> {
        self.0.last_viewed_at.map(DateTime::from)
    }
}

/// Loads the caller's state of several works in one call per viewer.
#[derive(Debug, Clone)]
pub struct WatchStateLoader {
    repository: Arc<dyn WatchRepository>,
}

impl WatchStateLoader {
    pub const fn new(repository: Arc<dyn WatchRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<(UserId, String)> for WatchStateLoader {
    type Value = WatchState;
    type Error = Arc<WatchError>;

    async fn load(
        &self,
        keys: &[(UserId, String)],
    ) -> Result<HashMap<(UserId, String), Self::Value>, Self::Error> {
        let mut by_viewer: HashMap<UserId, Vec<String>> = HashMap::new();
        for (viewer, guid) in keys {
            by_viewer.entry(*viewer).or_default().push(guid.clone());
        }
        let mut found = HashMap::new();
        for (viewer, guids) in by_viewer {
            for (guid, state) in self
                .repository
                .states(viewer, &guids)
                .await
                .map_err(Arc::new)?
            {
                found.insert((viewer, guid), state);
            }
        }
        Ok(found)
    }
}

/// Loads how many episodes the caller watched below several series or seasons.
#[derive(Debug, Clone)]
pub struct WatchedLeavesLoader {
    repository: Arc<dyn WatchRepository>,
}

impl WatchedLeavesLoader {
    pub const fn new(repository: Arc<dyn WatchRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<(UserId, RatingKey)> for WatchedLeavesLoader {
    type Value = u64;
    type Error = Arc<WatchError>;

    async fn load(
        &self,
        keys: &[(UserId, RatingKey)],
    ) -> Result<HashMap<(UserId, RatingKey), Self::Value>, Self::Error> {
        let mut by_viewer: HashMap<UserId, Vec<RatingKey>> = HashMap::new();
        for (viewer, parent) in keys {
            by_viewer.entry(*viewer).or_default().push(*parent);
        }
        let mut found = HashMap::new();
        for (viewer, parents) in by_viewer {
            let counts = self
                .repository
                .watched_leaves(viewer, &parents)
                .await
                .map_err(Arc::new)?;
            for (parent, count) in counts {
                found.insert((viewer, parent), count);
            }
        }
        Ok(found)
    }
}

/// Loads the tracks viewers chose on several files.
#[derive(Debug, Clone)]
pub struct TrackChoiceLoader {
    repository: Arc<dyn WatchRepository>,
}

impl TrackChoiceLoader {
    pub const fn new(repository: Arc<dyn WatchRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<(UserId, PartId)> for TrackChoiceLoader {
    type Value = TrackChoice;
    type Error = Arc<WatchError>;

    async fn load(
        &self,
        keys: &[(UserId, PartId)],
    ) -> Result<HashMap<(UserId, PartId), Self::Value>, Self::Error> {
        let mut by_viewer: HashMap<UserId, Vec<PartId>> = HashMap::new();
        for (viewer, file) in keys {
            by_viewer.entry(*viewer).or_default().push(*file);
        }
        let mut found = HashMap::new();
        for (viewer, files) in by_viewer {
            for (file, choice) in self
                .repository
                .track_choices(viewer, &files)
                .await
                .map_err(Arc::new)?
            {
                found.insert((viewer, file), choice);
            }
        }
        Ok(found)
    }
}

/// What the caller chose on a file. Nothing for a caller with no state, who gets the automatic
/// choice.
pub(crate) async fn track_choice_of(context: &Context<'_>, file: PartId) -> Result<TrackChoice> {
    let Some(viewer) = crate::access::viewer_id(context) else {
        return Ok(TrackChoice::default());
    };
    Ok(context
        .data::<DataLoader<TrackChoiceLoader>>()?
        .load_one((viewer, file))
        .await?
        .unwrap_or_default())
}

/// Records the caller's choice of tracks on a file, keeping what it chose before for the kind
/// it does not name. The tracks must be the file's, and of the kind.
pub(crate) async fn choose_tracks(
    context: &Context<'_>,
    file: &ID,
    audio: Option<ID>,
    subtitle: Option<ID>,
    subtitles_off: bool,
) -> Result<crate::media::File> {
    let Some(viewer) = crate::access::viewer_id(context) else {
        return Err(async_graphql::Error::new(
            "a caller with no watch state has no tracks to choose",
        ));
    };
    if audio.is_none() && subtitle.is_none() && !subtitles_off {
        return Err(async_graphql::Error::new(
            "choose an audio track or a subtitle",
        ));
    }
    if subtitle.is_some() && subtitles_off {
        return Err(async_graphql::Error::new(
            "a subtitle track and no subtitles cannot both be chosen",
        ));
    }
    let id = GlobalId::decode(file)
        .ok()
        .filter(|decoded| decoded.type_name == crate::media::FILE_TYPE)
        .map(|decoded| PartId::new(decoded.key))
        .ok_or_else(|| async_graphql::Error::new("not a file id"))?;
    let readable = crate::schema::readable_file(context, id)
        .await?
        .ok_or_else(|| async_graphql::Error::new("no such file"))?;
    let api = context.data::<ApiContext>()?;
    let streams = api.metadata.streams_for(&[id]).await?;
    let track_of = |track: &ID, kind: StreamKind| {
        GlobalId::decode(track)
            .ok()
            .filter(|decoded| decoded.type_name == crate::media::STREAM_TYPE)
            .and_then(|decoded| {
                streams
                    .iter()
                    .find(|stream| stream.id.get() == decoded.key && stream.kind == kind)
            })
            .map(|stream| stream.id)
            .ok_or_else(|| async_graphql::Error::new("not a track of this kind in this file"))
    };

    let mut choice = api
        .watch
        .track_choices(viewer, &[id])
        .await?
        .into_iter()
        .next()
        .map(|(_, choice)| choice)
        .unwrap_or_default();
    if let Some(audio) = &audio {
        choice.audio = Some(track_of(audio, StreamKind::Audio)?);
    }
    if let Some(subtitle) = &subtitle {
        choice.subtitle = Some(SubtitleChoice::Track(track_of(
            subtitle,
            StreamKind::Subtitle,
        )?));
    }
    if subtitles_off {
        choice.subtitle = Some(SubtitleChoice::Off);
    }
    api.watch.choose_tracks(viewer, id, choice).await?;
    // The loader may hold the choice from before.
    context
        .data::<DataLoader<TrackChoiceLoader>>()?
        .feed_one((viewer, id), choice)
        .await;
    Ok(crate::media::File(readable))
}

/// The caller's state of a work. Null for a caller with none.
pub(crate) async fn state_of(
    context: &Context<'_>,
    guid: &str,
) -> Result<Option<WatchStateObject>> {
    let Some(viewer) = crate::access::viewer_id(context) else {
        return Ok(None);
    };
    let state = context
        .data::<DataLoader<WatchStateLoader>>()?
        .load_one((viewer, guid.to_owned()))
        .await?
        .unwrap_or_default();
    Ok(Some(WatchStateObject(state)))
}

/// How many episodes below a series or a season the caller watched. Zero for a caller with
/// no watch state.
pub(crate) async fn watched_below(context: &Context<'_>, parent: RatingKey) -> Result<u64> {
    let Some(viewer) = crate::access::viewer_id(context) else {
        return Ok(0);
    };
    Ok(context
        .data::<DataLoader<WatchedLeavesLoader>>()?
        .load_one((viewer, parent))
        .await?
        .unwrap_or(0))
}

/// What a report did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ViewingOutcome {
    /// Nothing was kept: no runtime given, or a playback that opened past the end.
    Ignored,
    /// Nothing kept yet: the playback is under way but short of a minute.
    Early,
    /// The position was kept.
    Progress,
    /// The playback counted as a viewing, with this report or an earlier one.
    Completed,
}

impl From<ReportOutcome> for ViewingOutcome {
    fn from(outcome: ReportOutcome) -> Self {
        match outcome {
            ReportOutcome::Ignored => Self::Ignored,
            ReportOutcome::Early => Self::Early,
            ReportOutcome::Progress => Self::Progress,
            ReportOutcome::Completed => Self::Completed,
        }
    }
}

/// A player's report on a playback.
#[derive(Debug, InputObject)]
pub struct ReportViewingInput {
    /// The item played, as returned in `id`.
    pub item_id: ID,
    /// The player's own identifier for this playback. Reports under one identifier are one
    /// playback: crossing 90% of the runtime counts a viewing only if an earlier report of
    /// the same playback fell short of it.
    pub playback_id: String,
    /// Where the player stands, in milliseconds.
    pub position_ms: i64,
    /// The runtime the player plays, in milliseconds. Without it nothing is kept: there is
    /// no end to measure against.
    pub duration_ms: Option<i64>,
}

/// What a report did, and the state it left.
#[derive(Debug, SimpleObject)]
pub struct ViewingReport {
    /// What the report did.
    pub outcome: ViewingOutcome,
    /// The item's state after it.
    pub state: WatchStateObject,
}

/// Finds an item the caller may see, with the caller's viewer.
async fn visible_item(context: &Context<'_>, id: &ID) -> Result<(MetadataItem, UserId)> {
    let viewer = crate::access::viewer_id(context)
        .ok_or_else(|| async_graphql::Error::new("no watch state for this caller"))?;
    let key = GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == ITEM_TYPE)
        .ok_or_else(|| async_graphql::Error::new("not an item id"))?
        .key;
    let item = context
        .data::<DataLoader<ItemLoader>>()?
        .load_one(RatingKey::new(key))
        .await?
        .filter(|item| crate::access::can_read(context, item.section_id))
        // Unknown and not given read the same, as everywhere.
        .ok_or_else(|| async_graphql::Error::new("no such item"))?;
    Ok((item, viewer))
}

fn watching<'a>(context: &'a Context<'_>, viewer: UserId) -> Result<Watching<'a>> {
    let api = context.data::<ApiContext>()?;
    Ok(Watching {
        metadata: api.metadata.as_ref(),
        watch: api.watch.as_ref(),
        viewer,
    })
}

/// The item's state as just written, read straight from storage: the loader may hold the
/// value from before the mutation.
async fn fresh_state(
    context: &Context<'_>,
    viewer: UserId,
    guid: &str,
) -> Result<WatchStateObject> {
    let state = context
        .data::<ApiContext>()?
        .watch
        .states(viewer, &[guid.to_owned()])
        .await?
        .into_iter()
        .next()
        .map(|(_, state)| state)
        .unwrap_or_default();
    Ok(WatchStateObject(state))
}

/// What the caller should play once this episode ends. None without a caller.
pub(crate) async fn after_episode(
    context: &Context<'_>,
    episode: &MetadataItem,
) -> Result<Option<MetadataItem>> {
    let Some(viewer) = crate::access::viewer_id(context) else {
        return Ok(None);
    };
    Ok(watching(context, viewer)?
        .after_episode(episode, now_epoch())
        .await?)
}

pub(crate) async fn mark_watched(context: &Context<'_>, id: &ID) -> Result<WatchStateObject> {
    let (item, viewer) = visible_item(context, id).await?;
    watching(context, viewer)?
        .mark_watched(&item, now_epoch())
        .await?;
    fresh_state(context, viewer, &item.guid).await
}

pub(crate) async fn mark_unwatched(context: &Context<'_>, id: &ID) -> Result<WatchStateObject> {
    let (item, viewer) = visible_item(context, id).await?;
    watching(context, viewer)?.mark_unwatched(&item).await?;
    fresh_state(context, viewer, &item.guid).await
}

pub(crate) async fn report_viewing(
    context: &Context<'_>,
    input: ReportViewingInput,
) -> Result<ViewingReport> {
    let (item, viewer) = visible_item(context, &input.item_id).await?;
    let playback = crate::access::require_caller(context)?.session_key(&input.playback_id);
    let outcome = watching(context, viewer)?
        .report(
            &item,
            &playback,
            input.position_ms,
            input.duration_ms,
            now_epoch(),
        )
        .await?;
    Ok(ViewingReport {
        outcome: outcome.into(),
        state: fresh_state(context, viewer, &item.guid).await?,
    })
}

pub(crate) async fn keep_watch_position(
    context: &Context<'_>,
    id: &ID,
    position_ms: i64,
) -> Result<WatchStateObject> {
    let (item, viewer) = visible_item(context, id).await?;
    watching(context, viewer)?
        .keep_position(&item, position_ms, now_epoch())
        .await?;
    fresh_state(context, viewer, &item.guid).await
}

/// Why an item is in Continue Watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ContinueReason {
    /// Started and not finished: a film, or an episode.
    InProgress,
    /// The episode after the one last watched: not started yet.
    UpNext,
}

impl From<DeckReason> for ContinueReason {
    fn from(reason: DeckReason) -> Self {
        match reason {
            DeckReason::InProgress => Self::InProgress,
            DeckReason::UpNext => Self::UpNext,
        }
    }
}

/// What a Continue Watching entry says beyond the item.
#[derive(Debug, Clone, SimpleObject)]
pub struct ContinueWatchingEdgeFields {
    /// Why the item is listed.
    pub reason: ContinueReason,
    /// When the work behind it was last played: the film's, or for an episode its series',
    /// which the list is ordered by.
    pub last_viewed_at: DateTime,
}

/// Names the connection apart from `ItemConnection`, which shares its node type.
#[derive(Debug)]
pub struct ContinueWatchingConnectionName;

impl ConnectionNameType for ContinueWatchingConnectionName {
    fn type_name<T: OutputType>() -> String {
        "ContinueWatchingConnection".to_owned()
    }
}

#[derive(Debug)]
pub struct ContinueWatchingEdgeName;

impl EdgeNameType for ContinueWatchingEdgeName {
    fn type_name<T: OutputType>() -> String {
        "ContinueWatchingEdge".to_owned()
    }
}

/// A page of Continue Watching.
pub type ContinueWatchingConnection = Connection<
    OpaqueCursor,
    Item,
    EmptyFields,
    ContinueWatchingEdgeFields,
    ContinueWatchingConnectionName,
    ContinueWatchingEdgeName,
>;

/// Default page size of Continue Watching.
const DEFAULT_ENTRIES: usize = 50;

pub(crate) async fn continue_watching(
    context: &Context<'_>,
    library: Option<ID>,
    after: Option<String>,
    first: Option<i32>,
) -> Result<ContinueWatchingConnection> {
    let visible = crate::access::visible_sections(context);
    let only = match library {
        Some(id) => {
            let section = GlobalId::decode(&id)
                .ok()
                .filter(|decoded| decoded.type_name == LIBRARY_TYPE)
                .map(|decoded| SectionId::new(decoded.key))
                .ok_or_else(|| async_graphql::Error::new("not a library id"))?;
            // A library not given reads as one with nothing in it.
            if visible
                .as_ref()
                .is_some_and(|sections| !sections.contains(&section))
            {
                Some(Vec::new())
            } else {
                Some(vec![section])
            }
        }
        None => visible,
    };
    query(
        after,
        None,
        first,
        None,
        |after: Option<OpaqueCursor>, _: Option<OpaqueCursor>, first, _| async move {
            let start = after.map_or(0, |cursor| {
                usize::try_from(cursor.0.id).map_or(0, |position| position + 1)
            });
            let count = first.unwrap_or(DEFAULT_ENTRIES);
            if count > MAX_ITEMS as usize {
                return Err(async_graphql::Error::new(format!(
                    "a page of more than {MAX_ITEMS} is not served"
                )));
            }
            let Some(viewer) = crate::access::viewer_id(context) else {
                return Ok(Connection::new(false, false));
            };
            let entries = watching(context, viewer)?
                .continue_watching(only.as_deref(), now_epoch())
                .await?;
            let has_next = entries.len() > start + count;
            let mut connection = Connection::new(start > 0, has_next);
            connection
                .edges
                .extend(entries.into_iter().enumerate().skip(start).take(count).map(
                    |(position, entry)| {
                        Edge::with_additional_fields(
                            OpaqueCursor(Cursor::by_id(
                                i64::try_from(position).unwrap_or(i64::MAX),
                            )),
                            Item::from_domain(entry.item),
                            ContinueWatchingEdgeFields {
                                reason: entry.reason.into(),
                                last_viewed_at: entry.last_viewed_at.into(),
                            },
                        )
                    },
                ));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}
