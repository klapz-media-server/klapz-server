//! What is being played, on the official API.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, ID};
use klapz_core::page::Cursor;
use klapz_core::play_queue::PlayQueueChange;
use klapz_core::playback::{
    NewPlaybackSession, PlaybackOutput, PlaybackRepository, PlaybackSession, PlaybackState,
    SessionStatus, StreamDecision,
};
use klapz_core::{now_epoch, PartId, RatingKey};

use crate::cursor::OpaqueCursor;
use crate::item::Item;
use crate::loader::ItemLoader;
use crate::node::GlobalId;
use crate::schema::ApiContext;
use crate::DateTime;

pub const PLAYBACK_TYPE: &str = "PlaybackSession";
const DEFAULT_PAGE_SIZE: u32 = 20;

pub type PlaybackConnection =
    Connection<OpaqueCursor, PlaybackSessionObject, EmptyFields, EmptyFields>;

pub(crate) fn playback<'c>(context: &Context<'c>) -> Result<&'c dyn PlaybackRepository> {
    Ok(context.data::<ApiContext>()?.playback.as_ref())
}

/// How far along a playback is, as the player last reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum PlaybackStateType {
    /// The picture is moving.
    Playing,
    /// The player is holding still.
    Paused,
    /// The player is waiting for data.
    Buffering,
}

impl From<PlaybackState> for PlaybackStateType {
    fn from(state: PlaybackState) -> Self {
        match state {
            PlaybackState::Playing => Self::Playing,
            PlaybackState::Paused => Self::Paused,
            PlaybackState::Buffering => Self::Buffering,
        }
    }
}

/// Where a player says it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ReportedPlaybackState {
    /// The picture is moving.
    Playing,
    /// The player is holding still.
    Paused,
    /// The player is waiting for data.
    Buffering,
    /// The player is done: the playback's sessions close.
    Stopped,
}

/// What the server does to a track on its way to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum StreamDecisionType {
    /// Passed along untouched.
    Copy,
    /// Converted to something the player can read.
    Transcode,
}

impl From<StreamDecision> for StreamDecisionType {
    fn from(decision: StreamDecision) -> Self {
        match decision {
            StreamDecision::Copy => Self::Copy,
            StreamDecision::Transcode => Self::Transcode,
        }
    }
}

impl From<StreamDecisionType> for StreamDecision {
    fn from(decision: StreamDecisionType) -> Self {
        match decision {
            StreamDecisionType::Copy => Self::Copy,
            StreamDecisionType::Transcode => Self::Transcode,
        }
    }
}

/// Which sessions `playbackSessions` lists. Expired ones are in neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Enum)]
#[graphql(name = "PlaybackSessionStatus")]
pub enum PlaybackSessionStatusType {
    /// Still playing.
    #[default]
    Playing,
    /// Terminated by an owner: their stream is stopped, and they stay until their player
    /// stops or they expire, so that the player can learn why.
    Terminated,
}

impl From<PlaybackSessionStatusType> for SessionStatus {
    fn from(status: PlaybackSessionStatusType) -> Self {
        match status {
            PlaybackSessionStatusType::Playing => Self::Playing,
            PlaybackSessionStatusType::Terminated => Self::Terminated,
        }
    }
}

/// One playback in progress. Expires by itself when the player stops giving signs of life.
#[derive(Debug, Clone)]
pub struct PlaybackSessionObject(pub PlaybackSession);

#[Object(name = "PlaybackSession")]
impl PlaybackSessionObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this session.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(PLAYBACK_TYPE, self.0.id))
    }

    /// When an owner terminated the session. Its stream is stopped then, and the session
    /// stays until its player stops or it expires, so that the player can learn why.
    async fn terminated_at(&self) -> Option<DateTime> {
        self.0
            .termination
            .as_ref()
            .map(|termination| termination.at.into())
    }

    /// What the owner who terminated the session told the player, when it said anything.
    async fn termination_reason(&self) -> Option<&str> {
        self.0
            .termination
            .as_ref()
            .and_then(|termination| termination.reason.as_deref())
    }

    /// The identifier the client chose for this playback.
    async fn key(&self, context: &Context<'_>) -> Result<&str> {
        Ok(crate::access::require_caller(context)?.client_key(&self.0.key))
    }

    /// The player's identifier for the whole playback, when it gave one. Unlike `key`, it
    /// stays the same when the quality changes.
    async fn playback_id(&self, context: &Context<'_>) -> Result<Option<&str>> {
        let caller = crate::access::require_caller(context)?;
        Ok(self
            .0
            .playback_id
            .as_deref()
            .map(|playback_id| caller.client_key(playback_id)))
    }

    /// What is being played. Null if it has left the library since.
    async fn item(&self, context: &Context<'_>) -> Result<Option<Item>> {
        Ok(context
            .data::<DataLoader<ItemLoader>>()?
            .load_one(self.0.item)
            .await?
            .filter(|item| crate::access::can_read(context, item.section_id))
            .map(Item::from_domain))
    }

    /// Where playback stands.
    async fn state(&self) -> PlaybackStateType {
        self.0.state.into()
    }

    /// What happens to the video track.
    async fn video_decision(&self) -> StreamDecisionType {
        self.0.video.into()
    }

    /// What happens to the audio track.
    async fn audio_decision(&self) -> StreamDecisionType {
        self.0.audio.into()
    }

    /// The client's stable identifier for itself, when it gave one.
    async fn client_identifier(&self) -> Option<&str> {
        self.0.client.as_deref()
    }

    /// The application playing, when it said: `Plex Web`.
    async fn client_product(&self) -> Option<&str> {
        self.0.product.as_deref()
    }

    /// Position in the item, in milliseconds.
    async fn position_ms(&self) -> i64 {
        self.0.position_ms
    }

    /// The bitrate of the video sent to the player, in bits per second, when known.
    async fn video_bitrate(&self) -> Option<i32> {
        self.0.output.video_bitrate
    }

    /// The bitrate of the audio sent to the player, in bits per second, when known.
    async fn audio_bitrate(&self) -> Option<i32> {
        self.0.output.audio_bitrate
    }

    /// The width of the picture sent to the player, when known.
    async fn width(&self) -> Option<i32> {
        self.0.output.width
    }

    /// The height of the picture sent to the player, when known.
    async fn height(&self) -> Option<i32> {
        self.0.output.height
    }

    /// When playback began.
    async fn started_at(&self) -> DateTime {
        self.0.started_at.into()
    }

    /// The last sign of life.
    async fn updated_at(&self) -> DateTime {
        self.0.updated_at.into()
    }

    /// When the session goes away without a further sign of life.
    async fn expires_at(&self) -> DateTime {
        self.0.expires_at.into()
    }
}

pub(crate) async fn sessions(
    context: &Context<'_>,
    status: PlaybackSessionStatusType,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<PlaybackConnection> {
    let caller = crate::access::require_caller(context)?;
    let owner = (!caller.is_owner()).then_some(caller.user);
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = crate::schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?;
            let page = playback(context)?
                .sessions(owner, status.into(), now_epoch(), window)
                .await?;
            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection
                .edges
                .extend(page.items.into_iter().map(|session| {
                    Edge::new(
                        OpaqueCursor(Cursor::by_id(session.id)),
                        PlaybackSessionObject(session),
                    )
                }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// A playback beginning.
#[derive(Debug, InputObject)]
pub struct StartPlaybackInput {
    /// The identifier the client chose for this playback.
    pub key: String,
    /// The player's identifier for the whole playback, which `reportPlayback` names.
    pub playback_id: Option<String>,
    /// What is being played, as returned in `Item.id`.
    pub item_id: ID,
    /// The file being played, as returned in `MediaFile.id`.
    pub file_id: ID,
    /// The client's stable identifier for itself.
    pub client_identifier: Option<String>,
    /// The application playing.
    pub client_product: Option<String>,
    /// What happens to the video track.
    pub video_decision: StreamDecisionType,
    /// What happens to the audio track.
    pub audio_decision: StreamDecisionType,
    /// Where playback starts, in milliseconds.
    pub position_ms: i64,
    /// The bitrate of the video sent to the player, in bits per second.
    pub video_bitrate: Option<i32>,
    /// The bitrate of the audio sent to the player, in bits per second.
    pub audio_bitrate: Option<i32>,
    /// The width of the picture sent to the player.
    pub width: Option<i32>,
    /// The height of the picture sent to the player.
    pub height: Option<i32>,
}

/// A player saying where it stands.
#[derive(Debug, InputObject)]
pub struct ReportPlaybackInput {
    /// The player's identifier for the whole playback.
    pub playback_id: String,
    /// Where playback stands.
    pub state: ReportedPlaybackState,
    /// Position in the item, in milliseconds. Left out, the last one stays.
    pub position_ms: Option<i64>,
}

/// A session for an owner to end.
#[derive(Debug, InputObject)]
pub struct TerminatePlaybackSessionInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The session, as returned in `PlaybackSession.id`.
    pub session: ID,
    /// What its player is told, for a person to read.
    pub reason: Option<String>,
}

/// Why a session could not be terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TerminatePlaybackErrorCode {
    /// No such session playing: it ended, expired or was terminated already.
    SessionNotFound,
}

/// A problem with what was asked, as opposed to a failure of the server.
#[derive(Debug, Clone, SimpleObject)]
pub struct TerminatePlaybackUserError {
    /// The input field at fault, as a path from the input's root.
    pub field: Vec<String>,
    /// What went wrong, for a person.
    pub message: String,
    /// What went wrong, for a program.
    pub code: TerminatePlaybackErrorCode,
}

/// What `terminatePlaybackSession` did.
#[derive(Debug, SimpleObject)]
pub struct TerminatePlaybackSessionPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The session as it now stands, terminated, unless it could not be.
    pub session: Option<PlaybackSessionObject>,
    /// Why it could not be terminated. Empty when it was.
    pub user_errors: Vec<TerminatePlaybackUserError>,
}

async fn terminate(
    context: &Context<'_>,
    input: TerminatePlaybackSessionInput,
) -> Result<TerminatePlaybackSessionPayload> {
    crate::access::require_owner(context)?;
    let api = context.data::<ApiContext>()?;
    let now = now_epoch();
    let found = match GlobalId::decode(&input.session) {
        Ok(global) if global.type_name == PLAYBACK_TYPE => {
            api.playback.session(global.key, now).await?
        }
        _ => None,
    };
    let terminated = match found {
        Some(session) => {
            api.lifecycle()
                .terminate(&session.key, input.reason, now)
                .await?
        }
        None => None,
    };
    let user_errors = if terminated.is_some() {
        Vec::new()
    } else {
        vec![TerminatePlaybackUserError {
            field: vec!["input".to_owned(), "session".to_owned()],
            message: "no such session playing".to_owned(),
            code: TerminatePlaybackErrorCode::SessionNotFound,
        }]
    };
    Ok(TerminatePlaybackSessionPayload {
        client_mutation_id: input.client_mutation_id,
        session: terminated.map(PlaybackSessionObject),
        user_errors,
    })
}

/// Changes to what is being played.
#[derive(Debug, Default)]
pub struct Mutation;

fn key_of(id: &ID) -> Result<i64> {
    Ok(GlobalId::decode(id)?.key)
}

#[Object]
impl Mutation {
    /// Opens a playback session, or restarts the one already under this key.
    async fn start_playback_session(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The playback beginning.")] input: StartPlaybackInput,
    ) -> Result<PlaybackSessionObject> {
        let caller = crate::access::require_caller(context)?;
        let item = RatingKey::new(key_of(&input.item_id)?);
        if crate::schema::readable_item(context, item).await?.is_none() {
            return Err(crate::access::denied());
        }
        let session = NewPlaybackSession {
            item,
            part: PartId::new(key_of(&input.file_id)?),
            key: caller.session_key(&input.key),
            user: caller.user,
            playback_id: input
                .playback_id
                .map(|playback_id| caller.session_key(&playback_id)),
            client: input.client_identifier,
            product: input.client_product,
            video: input.video_decision.into(),
            audio: input.audio_decision.into(),
            position_ms: input.position_ms,
            output: PlaybackOutput {
                video_bitrate: input.video_bitrate,
                audio_bitrate: input.audio_bitrate,
                width: input.width,
                height: input.height,
            },
        };
        Ok(PlaybackSessionObject(
            playback(context)?.start(session, now_epoch()).await?,
        ))
    }

    /// Starts sending a file to a player: as it is when the player reads it, its tracks
    /// repackaged when they fit, the video re-encoded when they do not or a limit asks for it.
    /// Records the playback session. A start under a key already streaming the same thing
    /// keeps it running.
    async fn start_stream(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The stream to start.")] input: crate::stream::StartStreamInput,
    ) -> Result<crate::stream::StartStreamPayload> {
        crate::stream::start_stream(context, input).await
    }

    /// Ends a session for an owner: its stream stops at once, and its player learns why from
    /// `PlaybackSession.terminationReason`, listed under `playbackSessions(status: TERMINATED)`
    /// until it stops. For the server's owners only.
    async fn terminate_playback_session(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The session to terminate.")] input: TerminatePlaybackSessionInput,
    ) -> Result<TerminatePlaybackSessionPayload> {
        terminate(context, input).await
    }

    /// Stops a stream and closes its session.
    async fn stop_stream(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The stream to stop.")] input: crate::stream::StopStreamInput,
    ) -> Result<crate::stream::StopStreamPayload> {
        crate::stream::stop_stream(context, input).await
    }

    /// Restarts a stream's conversion the more conservative way, under the same addresses, as a
    /// player asks when it cannot decode what it receives: from hardware to software, from HEVC
    /// to H.264. A sign of life for the session. This server's conversions all decode in
    /// software and encode H.264 already, so a running one answers `NO_FALLBACK`.
    async fn fall_back_stream(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The stream to restart.")] input: crate::stream::FallBackStreamInput,
    ) -> Result<crate::stream::FallBackStreamPayload> {
        crate::stream::fall_back_stream(context, input).await
    }

    /// Records a sign of life, pushing the session's expiry forward and keeping its stream
    /// running. Whether it is still playing: false once it has ended, expired or been
    /// terminated.
    async fn touch_playback_session(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The identifier the client chose for this playback.")] key: String,
    ) -> Result<bool> {
        let key = crate::access::require_caller(context)?.session_key(&key);
        let api = context.data::<ApiContext>()?;
        Ok(api.lifecycle().touch(&key, now_epoch()).await?)
    }

    /// Records where a player stands, pushing its sessions' expiry forward and keeping their
    /// streams running, or ends them with their streams when it has stopped. Whether the
    /// playback had any session playing, or any at all for a stop.
    async fn report_playback(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The player's report.")] input: ReportPlaybackInput,
    ) -> Result<bool> {
        let lifecycle = context.data::<ApiContext>()?.lifecycle();
        let playback_id = crate::access::require_caller(context)?.session_key(&input.playback_id);
        let state = match input.state {
            ReportedPlaybackState::Playing => PlaybackState::Playing,
            ReportedPlaybackState::Paused => PlaybackState::Paused,
            ReportedPlaybackState::Buffering => PlaybackState::Buffering,
            ReportedPlaybackState::Stopped => {
                return Ok(lifecycle.end_playback(&playback_id).await?);
            }
        };
        Ok(lifecycle
            .report(&playback_id, state, input.position_ms, now_epoch())
            .await?)
    }

    /// Chooses, for the caller, the audio track or the subtitle a file plays with — or no
    /// subtitles. What is not named keeps its earlier choice. Returns the file.
    async fn choose_tracks(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The file, as returned in `MediaFile.id`.")] file: ID,
        #[graphql(desc = "One of the file's audio tracks, as returned in `MediaStream.id`.")]
        audio_track: Option<ID>,
        #[graphql(desc = "One of the file's subtitles, as returned in `MediaStream.id`.")]
        subtitle_track: Option<ID>,
        #[graphql(desc = "Play the file without subtitles, forced ones included.")]
        subtitles_off: Option<bool>,
    ) -> Result<crate::media::File> {
        crate::watch::choose_tracks(
            context,
            &file,
            audio_track,
            subtitle_track,
            subtitles_off.unwrap_or(false),
        )
        .await
    }

    /// Marks an item watched for the caller: a film or an episode once more, a season or a
    /// series by each of its episodes once more. Counted on the episode's season and series
    /// too. Returns the item's state after it.
    async fn mark_watched(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The item, as returned in `id`.")] id: ID,
    ) -> Result<crate::watch::WatchStateObject> {
        crate::watch::mark_watched(context, &id).await
    }

    /// Forgets what the caller watched of an item: an episode alone, a season with its
    /// episodes, a series with its seasons and episodes. Returns the item's state after it.
    async fn mark_unwatched(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The item, as returned in `id`.")] id: ID,
    ) -> Result<crate::watch::WatchStateObject> {
        crate::watch::mark_unwatched(context, &id).await
    }

    /// Records where a player stands in a playback. Past a minute and short of 90% of the
    /// runtime the position is kept; crossing 90% within the playback counts a viewing and
    /// forgets it.
    async fn report_viewing(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The player's report.")] input: crate::watch::ReportViewingInput,
    ) -> Result<crate::watch::ViewingReport> {
        crate::watch::report_viewing(context, input).await
    }

    /// Keeps a position for the caller without asking whether the item is finished: an
    /// explicit "remember where I stopped". Under a minute, nothing is kept.
    async fn keep_watch_position(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The item, as returned in `id`.")] id: ID,
        #[graphql(desc = "Where to resume, in milliseconds.")] position_ms: i64,
    ) -> Result<crate::watch::WatchStateObject> {
        crate::watch::keep_watch_position(context, &id, position_ms).await
    }

    /// Rates a film, a series, a season or an episode for the caller, or clears its rating.
    /// The rating is the work's, shared by every copy of it, and nothing adds it up: a series
    /// is rated on its own, whatever its episodes are.
    async fn rate_item(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The rating to give.")] input: crate::user_rating::RateItemInput,
    ) -> Result<crate::user_rating::RateItemPayload> {
        crate::user_rating::rate_item(context, input).await
    }

    /// Makes a film, a series, a season or an episode a favorite of the caller's, or takes it
    /// back. The favorite is the work's: every copy of it is one.
    async fn set_favorite(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The item and whether it is a favorite.")]
        input: crate::favorite::SetFavoriteInput,
    ) -> Result<crate::favorite::SetFavoritePayload> {
        crate::favorite::set_favorite(context, input).await
    }

    /// Sets the offset the caller's player shifts a subtitle by, inside its media or a file
    /// of its own. The offset is the caller's, on this one track.
    async fn set_subtitle_offset(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The subtitle and its offset.")]
        input: crate::sidecar::SetSubtitleOffsetInput,
    ) -> Result<crate::sidecar::SetSubtitleOffsetPayload> {
        crate::sidecar::set_offset(context, input).await
    }

    /// Deletes a subtitle kept in a file of its own: the file, from disk when it is on this
    /// machine where a sidecar of its media may be, and the track. For the library's admins
    /// and the server's owners.
    async fn delete_sidecar(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The subtitle to delete.")] input: crate::sidecar::DeleteSidecarInput,
    ) -> Result<crate::sidecar::DeleteSidecarPayload> {
        crate::sidecar::delete_sidecar(context, input).await
    }

    /// Starts scanning a library in the background: new files become items at once, and are
    /// read afterwards, as are changed ones (`Library.analysis`); what is gone is removed —
    /// unless its whole folder cannot be reached, which keeps it. For the library's admins and
    /// the server's owners.
    async fn scan_library(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The library to scan.")] input: crate::scan::ScanLibraryInput,
    ) -> Result<crate::scan::ScanLibraryPayload> {
        crate::scan::scan_library(context, input).await
    }

    /// Asks a library's running scan to stop, and stops reading its files until its next
    /// scan. What it already wrote stays.
    async fn cancel_library_scan(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The library whose scan to stop.")]
        input: crate::scan::CancelLibraryScanInput,
    ) -> Result<crate::scan::CancelLibraryScanPayload> {
        crate::scan::cancel_library_scan(context, input).await
    }

    /// Reads a file's headers now when the server has not yet — its tracks, codecs and
    /// durations, which a player needs before choosing how to play it — ahead of the files a
    /// scan queued. Nothing to do for a file read already; `startStream` does it itself. For
    /// whoever can see the file.
    async fn read_media_file(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The file to read.")] input: crate::scan::ReadMediaFileInput,
    ) -> Result<crate::scan::ReadMediaFilePayload> {
        crate::scan::read_media_file(context, input).await
    }

    /// Deletes one viewing from a history: the caller's own, or anyone's for an owner.
    /// What was watched stays as it is, as on the reference.
    async fn delete_history_entry(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The entry to delete.")] input: crate::history::DeleteHistoryEntryInput,
    ) -> Result<crate::history::DeleteHistoryEntryPayload> {
        crate::history::delete_history_entry(context, input).await
    }

    /// Creates a play queue from a list of items, replacing the player's previous queue of
    /// the same kind.
    async fn create_play_queue(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The queue to create.")] input: crate::play_queue::CreatePlayQueueInput,
    ) -> Result<crate::play_queue::PlayQueueObject> {
        crate::play_queue::create(context, input).await
    }

    /// Queues items by hand in one of the caller's play queues.
    async fn add_to_play_queue(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The queue, the items and where they go.")]
        input: crate::play_queue::AddToPlayQueueInput,
    ) -> Result<crate::play_queue::PlayQueuePayload> {
        crate::play_queue::add(context, input).await
    }

    /// Moves one entry of one of the caller's play queues after another, or first.
    async fn move_play_queue_entry(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The entry and where it goes.")]
        input: crate::play_queue::MovePlayQueueEntryInput,
    ) -> Result<crate::play_queue::PlayQueuePayload> {
        crate::play_queue::move_entry(context, input).await
    }

    /// Takes one entry out of one of the caller's play queues.
    async fn remove_from_play_queue(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The entry.")] input: crate::play_queue::RemoveFromPlayQueueInput,
    ) -> Result<crate::play_queue::PlayQueuePayload> {
        crate::play_queue::remove(context, input).await
    }

    /// Removes every entry of one of the caller's play queues but the one playing, which ends
    /// what was queued by hand.
    async fn clear_play_queue(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The queue.")] input: crate::play_queue::PlayQueueInput,
    ) -> Result<crate::play_queue::PlayQueuePayload> {
        crate::play_queue::edit_whole(context, input, PlayQueueChange::Clear).await
    }

    /// Makes the first entry of one of the caller's play queues the one playing. Leaves the
    /// version as it is, as the reference does.
    async fn reset_play_queue(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The queue.")] input: crate::play_queue::PlayQueueInput,
    ) -> Result<crate::play_queue::PlayQueuePayload> {
        crate::play_queue::edit_whole(context, input, PlayQueueChange::Reset).await
    }

    /// Rebuilds one of the caller's play queues from its source in random order, the entry
    /// playing kept and first; every other entry is new. Refused while an entry queued by
    /// hand remains.
    async fn shuffle_play_queue(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The queue.")] input: crate::play_queue::PlayQueueInput,
    ) -> Result<crate::play_queue::PlayQueuePayload> {
        crate::play_queue::edit_whole(context, input, PlayQueueChange::Shuffle).await
    }

    /// Rebuilds one of the caller's play queues from its source in natural order, the entry
    /// playing kept where its item falls; every other entry is new. Refused while an entry
    /// queued by hand remains.
    async fn unshuffle_play_queue(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The queue.")] input: crate::play_queue::PlayQueueInput,
    ) -> Result<crate::play_queue::PlayQueuePayload> {
        crate::play_queue::edit_whole(context, input, PlayQueueChange::Unshuffle).await
    }

    /// Closes a session and stops its stream. Whether there was either.
    async fn end_playback_session(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The identifier the client chose for this playback.")] key: String,
    ) -> Result<bool> {
        let key = crate::access::require_caller(context)?.session_key(&key);
        let api = context.data::<ApiContext>()?;
        Ok(api.lifecycle().end(&key).await?)
    }

    /// Creates a user, active, with no password and no library. For the server's owners only.
    async fn create_user(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user's username, and its address if it has one.")]
        input: crate::access::CreateUserInput,
    ) -> Result<crate::access::CreateUserPayload> {
        crate::access::create_user(context, input).await
    }

    /// Deletes a user, its permissions and its connections. For the server's owners only; the
    /// built-in `admin` account cannot be deleted.
    async fn delete_user(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user to delete.")] input: crate::access::UserInput,
    ) -> Result<crate::access::DeleteUserPayload> {
        crate::access::delete_user(context, input).await
    }

    /// Makes a user an owner of the server, or no longer one. For the server's owners only; the
    /// built-in `admin` account cannot be demoted.
    async fn set_user_role(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user and its new role.")] input: crate::access::SetUserRoleInput,
    ) -> Result<crate::access::UserPayload> {
        crate::access::set_role(context, input).await
    }

    /// Gives a user a role on a library, replacing the one it had. For the server's owners
    /// only.
    async fn set_library_permission(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The role to give.")] input: crate::access::SetLibraryPermissionInput,
    ) -> Result<bool> {
        crate::access::set_library_permission(context, input).await
    }

    /// Takes a library away from a user. Whether it had access. For the server's owners only.
    async fn remove_library_permission(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user, as returned in `User.id`.")] user_id: ID,
        #[graphql(desc = "The library, as returned in `Library.id`.")] library_id: ID,
    ) -> Result<bool> {
        crate::access::remove_library_permission(context, &user_id, &library_id).await
    }

    /// Switches a user's connection off: its token stops working, its devices are forgotten
    /// and its identifier is kept. Whether it was on. For the server's owners; through Jellyfin,
    /// for the user itself too.
    async fn disable_extension(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user, as returned in `User.id`.")] user_id: ID,
        #[graphql(desc = "The protocol to disconnect.")] extension: crate::access::ExtensionType,
    ) -> Result<bool> {
        crate::access::disable_extension(context, &user_id, extension).await
    }

    /// Connects a user through an extension, returning a new token. Called again, it replaces
    /// the token and keeps the identifier clients recognize. For the server's owners; through
    /// Jellyfin, whose clients then sign in with the user's password, for the user itself too.
    async fn connect_extension(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user, as returned in `User.id`.")] user_id: ID,
        #[graphql(desc = "The protocol to connect through.")]
        extension: crate::access::ExtensionType,
    ) -> Result<crate::access::ExtensionCredentials> {
        crate::access::connect_extension(context, &user_id, extension).await
    }

    /// Signs one device out of a user's connection: its token opens nothing more, and the
    /// device signs in again with the password. Whether there was such a device. For the
    /// server's owners, and for the user itself.
    async fn forget_extension_device(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user, as returned in `User.id`.")] user_id: ID,
        #[graphql(desc = "The protocol the device signed in through.")]
        extension: crate::access::ExtensionType,
        #[graphql(desc = "The device, as returned in `ExtensionDevice.deviceId`.")]
        device_id: String,
    ) -> Result<bool> {
        crate::access::forget_extension_device(context, &user_id, extension, &device_id).await
    }

    /// Signs a browser in with a one-time ticket, as `signIn` does with a password: the server
    /// hands such a ticket out only in an address it opens itself (its tray icon on Windows).
    /// A ticket works once, for a minute.
    async fn sign_in_with_ticket(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The ticket the address carried.")]
        input: crate::account::SignInWithTicketInput,
    ) -> Result<crate::account::SignInPayload> {
        crate::account::sign_in_with_ticket(context, input).await
    }

    /// Gives the caller a new password, which also signs it in through Jellyfin's clients; its
    /// current password is asked for when it has one. An owner sets or resets any user's,
    /// with none asked. The built-in `admin` account takes `KLAPZ_ADMIN_PASSWORD` again at the
    /// next start when that is set.
    async fn set_password(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The current password, and the new one.")]
        input: crate::account::SetPasswordInput,
    ) -> Result<crate::account::SetPasswordPayload> {
        crate::account::set_password(context, input).await
    }

    /// Ties a user's connection to its account on the extension's own service, and tells that
    /// service where the server can be reached. For the server's owners, and for the user
    /// itself.
    async fn pair_extension(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The account's pairing code, and where to reach the server.")]
        input: crate::access::PairExtensionInput,
    ) -> Result<crate::access::AccountLinkObject> {
        crate::access::pair_extension(context, input).await
    }

    /// Changes where a paired server is reached, and tells the extension's service at once:
    /// what a tunnel or a new domain needs, without pairing again. For the server's owners,
    /// and for the user itself.
    async fn move_extension(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The paired connection, and its new address.")]
        input: crate::access::MoveExtensionInput,
    ) -> Result<crate::access::AccountLinkObject> {
        crate::access::move_extension(context, input).await
    }

    /// Forgets the account tied to a user's connection. Whether there was one. For the server's
    /// owners, and for the user itself.
    async fn unpair_extension(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user, as returned in `User.id`.")] user_id: ID,
        #[graphql(desc = "The protocol to unpair.")] extension: crate::access::ExtensionType,
    ) -> Result<bool> {
        crate::access::unpair_extension(context, &user_id, extension).await
    }

    /// Creates an account that signs in with a password. It waits, inactive, until an owner
    /// lets it in. Open to anyone.
    async fn sign_up(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The username, address and password of the new account.")]
        input: crate::account::SignUpInput,
    ) -> Result<crate::account::SignUpPayload> {
        crate::account::sign_up(context, input).await
    }

    /// Checks a username or an address and its password and, when they match an active user,
    /// opens a session for it: a cookie the server reads on every request, whose value a
    /// client that is no browser can present as `Authorization: Bearer`. Open to anyone;
    /// failed attempts on one username or address are limited.
    async fn sign_in(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The username or address, and the password, to sign in with.")]
        input: crate::account::SignInInput,
    ) -> Result<crate::account::SignInPayload> {
        crate::account::sign_in(context, input).await
    }

    /// Clears the session cookie.
    async fn sign_out(&self, context: &Context<'_>) -> Result<bool> {
        crate::account::sign_out(context)
    }

    /// Checks a username or an address and its password, as `signIn` does, for a client of an
    /// extension whose clients sign in with a password — Jellyfin's — and gives the client's
    /// device a token of its own. The user must be connected through the extension, and its
    /// connection on. Open to anyone; failed attempts count with `signIn`'s.
    async fn sign_in_through_extension(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The extension, the credentials and the device signing in.")]
        input: crate::account::ExtensionSignInInput,
    ) -> Result<crate::account::ExtensionSignInPayload> {
        crate::account::sign_in_through_extension(context, input).await
    }

    /// Signs a device out of an extension: its token opens nothing more. Only the caller's own
    /// devices; whether the token was one.
    async fn sign_out_of_extension(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The protocol the token was issued for.")]
        extension: crate::access::ExtensionType,
        #[graphql(secret, desc = "The token the device was given.")] token: String,
    ) -> Result<bool> {
        crate::account::sign_out_of_extension(context, extension, &token).await
    }

    /// Lets in a user who signed up, or one that was shut out. For the server's owners only.
    async fn activate_user(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user to let in.")] input: crate::access::UserInput,
    ) -> Result<crate::access::UserPayload> {
        crate::access::set_active(context, input, true).await
    }

    /// Shuts a user out: its sessions and its tokens stop opening anything at once, and it
    /// keeps its libraries for when it is let in again. For the server's owners only; the
    /// built-in `admin` account cannot be shut out.
    async fn deactivate_user(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user to shut out.")] input: crate::access::UserInput,
    ) -> Result<crate::access::UserPayload> {
        crate::access::set_active(context, input, false).await
    }

    /// Matches a film or a series to a record of its library's provider — the one named, else
    /// the one it shows already, else the best search result — and fetches its metadata and
    /// images in the background. For a library's admins and the server's owners.
    async fn match_item(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The item and the record.")] input: crate::source_metadata::MatchItemInput,
    ) -> Result<crate::source_metadata::ItemMetadataPayload> {
        crate::source_metadata::match_item(context, input).await
    }

    /// Fetches a film's or a series' metadata and images again from the record it shows, in
    /// the background; one never matched is searched for. For a library's admins and the
    /// server's owners.
    async fn refresh_item_metadata(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The item.")] input: crate::source_metadata::ItemMetadataInput,
    ) -> Result<crate::source_metadata::ItemMetadataPayload> {
        crate::source_metadata::refresh_item_metadata(context, input).await
    }

    /// Forgets the record a film or a series and its seasons and episodes show: they take back
    /// the identity a scan gives, and keep their texts until matched again. For a library's
    /// admins and the server's owners.
    async fn unmatch_item(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The item.")] input: crate::source_metadata::ItemMetadataInput,
    ) -> Result<crate::source_metadata::ItemMetadataPayload> {
        crate::source_metadata::unmatch_item(context, input).await
    }

    /// Changes a film's metadata and locks. For a library's admins and the server's owners.
    async fn update_movie(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The changes.")] input: crate::edit::UpdateMovieInput,
    ) -> Result<crate::edit::UpdateMoviePayload> {
        crate::edit::update_movie(context, input).await
    }

    /// Changes a series' metadata and locks. For a library's admins and the server's owners.
    async fn update_show(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The changes.")] input: crate::edit::UpdateShowInput,
    ) -> Result<crate::edit::UpdateShowPayload> {
        crate::edit::update_show(context, input).await
    }

    /// Changes an episode's metadata and locks. For a library's admins and the server's owners.
    async fn update_episode(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The changes.")] input: crate::edit::UpdateEpisodeInput,
    ) -> Result<crate::edit::UpdateEpisodePayload> {
        crate::edit::update_episode(context, input).await
    }

    /// Adds a marker to a film or an episode: an intro, credits or a commercial, for its
    /// library's admins and the server's owners, or a bookmark of the caller's own.
    async fn add_marker(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The marker to add.")] input: crate::marker::AddMarkerInput,
    ) -> Result<crate::marker::AddMarkerPayload> {
        crate::marker::add(context, input).await
    }

    /// Moves a marker or changes what it says. The work's markers for its library's admins
    /// and the server's owners, a bookmark for its owner.
    async fn update_marker(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The changes.")] input: crate::marker::UpdateMarkerInput,
    ) -> Result<crate::marker::UpdateMarkerPayload> {
        crate::marker::update(context, input).await
    }

    /// Removes a marker. The work's markers for its library's admins and the server's owners, a
    /// bookmark for its owner.
    async fn delete_marker(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The marker to remove.")] input: crate::marker::DeleteMarkerInput,
    ) -> Result<crate::marker::DeleteMarkerPayload> {
        crate::marker::delete(context, input).await
    }

    /// Creates a playlist of the caller's: of chosen items, or smart with a rule.
    async fn create_playlist(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The playlist to create.")] input: crate::playlist::CreatePlaylistInput,
    ) -> Result<crate::playlist::CreatePlaylistPayload> {
        crate::playlist::create(context, input).await
    }

    /// Renames, describes, locks or unlocks one of the caller's playlists.
    async fn update_playlist(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The changes.")] input: crate::playlist::UpdatePlaylistInput,
    ) -> Result<crate::playlist::PlaylistPayload> {
        crate::playlist::update(context, input).await
    }

    /// Deletes one of the caller's playlists, and its entries with it.
    async fn delete_playlist(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The playlist to delete.")] input: crate::playlist::DeletePlaylistInput,
    ) -> Result<crate::playlist::DeletePlaylistPayload> {
        crate::playlist::delete(context, input).await
    }

    /// Appends items to one of the caller's playlists of chosen items, skipping those it holds.
    async fn add_to_playlist(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The playlist and the items.")] input: crate::playlist::AddToPlaylistInput,
    ) -> Result<crate::playlist::AddToPlaylistPayload> {
        crate::playlist::add(context, input).await
    }

    /// Takes one entry out of one of the caller's playlists.
    async fn remove_from_playlist(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The entry.")] input: crate::playlist::RemoveFromPlaylistInput,
    ) -> Result<crate::playlist::PlaylistPayload> {
        crate::playlist::remove(context, input).await
    }

    /// Moves one entry of one of the caller's playlists after another, or first.
    async fn move_playlist_entry(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The entry and where it goes.")]
        input: crate::playlist::MovePlaylistEntryInput,
    ) -> Result<crate::playlist::PlaylistPayload> {
        crate::playlist::move_entry(context, input).await
    }

    /// Empties one of the caller's playlists of chosen items.
    async fn clear_playlist(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The playlist to empty.")] input: crate::playlist::ClearPlaylistInput,
    ) -> Result<crate::playlist::PlaylistPayload> {
        crate::playlist::clear(context, input).await
    }

    /// Creates a library over directories of the server, then scans it. For the server's
    /// owners only.
    async fn create_library(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The library to create.")]
        input: crate::library_management::CreateLibraryInput,
    ) -> Result<crate::library_management::LibraryPayload> {
        crate::library_management::create(context, input).await
    }

    /// Renames a library or changes its language, for its admins and the server's owners;
    /// changes the directories it scans, for the owners only.
    async fn update_library(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The changes.")] input: crate::library_management::UpdateLibraryInput,
    ) -> Result<crate::library_management::LibraryPayload> {
        crate::library_management::update(context, input).await
    }

    /// Deletes a library with its items, collections and the permissions given on it, ending
    /// every playback of its items; its files stay on disk, and watch state and ratings stay
    /// with the works. For the server's owners only.
    async fn delete_library(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The library to delete.")]
        input: crate::library_management::DeleteLibraryInput,
    ) -> Result<crate::library_management::DeleteLibraryPayload> {
        crate::library_management::delete(context, input).await
    }

    /// Creates a collection in a library: of chosen items, or smart with a rule. For the
    /// library's admins and the server's owners.
    async fn create_collection(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The collection to create.")]
        input: crate::collection::CreateCollectionInput,
    ) -> Result<crate::collection::CollectionPayload> {
        crate::collection::create(context, input).await
    }

    /// Renames, describes, sets, locks or unlocks a collection. For its library's admins and
    /// the server's owners.
    async fn update_collection(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The changes.")] input: crate::collection::UpdateCollectionInput,
    ) -> Result<crate::collection::CollectionPayload> {
        crate::collection::update(context, input).await
    }

    /// Deletes a collection; its items stay in the library. For its library's admins and the
    /// server's owners.
    async fn delete_collection(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The collection to delete.")]
        input: crate::collection::DeleteCollectionInput,
    ) -> Result<crate::collection::DeleteCollectionPayload> {
        crate::collection::delete(context, input).await
    }

    /// Adds items to a collection of chosen items, skipping those it holds. For its library's
    /// admins and the server's owners.
    async fn add_to_collection(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The collection and the items.")]
        input: crate::collection::AddToCollectionInput,
    ) -> Result<crate::collection::AddToCollectionPayload> {
        crate::collection::add(context, input).await
    }

    /// Takes an item out of a collection of chosen items. For its library's admins and the
    /// server's owners.
    async fn remove_from_collection(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The collection and the item.")]
        input: crate::collection::RemoveFromCollectionInput,
    ) -> Result<crate::collection::RemoveFromCollectionPayload> {
        crate::collection::remove(context, input).await
    }

    /// Moves an item of a collection after another, or first, in its custom order. For its
    /// library's admins and the server's owners.
    async fn move_collection_item(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The item and where it goes.")]
        input: crate::collection::MoveCollectionItemInput,
    ) -> Result<crate::collection::CollectionPayload> {
        crate::collection::move_item(context, input).await
    }

    /// Takes every item out of a collection of chosen items, keeping the collection. For its
    /// library's admins and the server's owners.
    async fn clear_collection(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The collection to empty.")]
        input: crate::collection::ClearCollectionInput,
    ) -> Result<crate::collection::CollectionPayload> {
        crate::collection::clear(context, input).await
    }

    /// Replaces the rule of one of the caller's smart playlists.
    async fn replace_playlist_rule(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The playlist and its new rule.")]
        input: crate::playlist::ReplacePlaylistRuleInput,
    ) -> Result<crate::playlist::PlaylistPayload> {
        crate::playlist::replace_rule(context, input).await
    }
}
