//! What the caller has watched, and the mutations that move it.
//!
//! Every field and mutation here is the caller's own: a user reads and writes its own state,
//! the operator its own, and a request with no caller has none. The rules are in
//! `core::watch`; these resolvers find the item, check it is visible, and call them.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, ID};
use open_plex_core::metadata::MetadataItem;
use open_plex_core::watch::{
    ReportOutcome, Viewer, WatchError, WatchRepository, WatchState, Watching,
};
use open_plex_core::{now_epoch, RatingKey};

use crate::item::ITEM_TYPE;
use crate::loader::ItemLoader;
use crate::node::GlobalId;
use crate::schema::ApiContext;

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

    /// When it was last played or watched, in seconds since the Unix epoch. On a series or a
    /// season, when any of its episodes was.
    async fn last_viewed_at(&self) -> Option<i64> {
        self.0.last_viewed_at
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

impl Loader<(Viewer, String)> for WatchStateLoader {
    type Value = WatchState;
    type Error = Arc<WatchError>;

    async fn load(
        &self,
        keys: &[(Viewer, String)],
    ) -> Result<HashMap<(Viewer, String), Self::Value>, Self::Error> {
        let mut by_viewer: HashMap<Viewer, Vec<String>> = HashMap::new();
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

impl Loader<(Viewer, RatingKey)> for WatchedLeavesLoader {
    type Value = u64;
    type Error = Arc<WatchError>;

    async fn load(
        &self,
        keys: &[(Viewer, RatingKey)],
    ) -> Result<HashMap<(Viewer, RatingKey), Self::Value>, Self::Error> {
        let mut by_viewer: HashMap<Viewer, Vec<RatingKey>> = HashMap::new();
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

/// The caller's state of a work. Null for a caller with none.
pub(crate) async fn state_of(
    context: &Context<'_>,
    guid: &str,
) -> Result<Option<WatchStateObject>> {
    let Some(viewer) = crate::access::caller(context).and_then(Viewer::of) else {
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
    let Some(viewer) = crate::access::caller(context).and_then(Viewer::of) else {
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
async fn visible_item(context: &Context<'_>, id: &ID) -> Result<(MetadataItem, Viewer)> {
    let viewer = crate::access::caller(context)
        .and_then(Viewer::of)
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

fn watching<'a>(context: &'a Context<'_>, viewer: Viewer) -> Result<Watching<'a>> {
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
    viewer: Viewer,
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
