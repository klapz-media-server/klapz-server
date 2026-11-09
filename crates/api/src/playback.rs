//! What is being played, on the official API.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, InputObject, Object, Result, ID};
use open_plex_core::page::Cursor;
use open_plex_core::playback::{
    NewPlaybackSession, PlaybackOutput, PlaybackRepository, PlaybackSession, PlaybackState,
    StreamDecision,
};
use open_plex_core::{now_epoch, PartId, RatingKey};

use crate::cursor::OpaqueCursor;
use crate::item::Item;
use crate::loader::ItemLoader;
use crate::node::GlobalId;
use crate::schema::ApiContext;

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

/// One playback in progress. Expires by itself when the player stops giving signs of life.
#[derive(Debug, Clone)]
pub struct PlaybackSessionObject(pub PlaybackSession);

#[Object(name = "PlaybackSession")]
impl PlaybackSessionObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this session.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(PLAYBACK_TYPE, self.0.id))
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

    /// When playback began, in seconds since the epoch.
    async fn started_at(&self) -> i64 {
        self.0.started_at
    }

    /// The last sign of life, in seconds since the epoch.
    async fn updated_at(&self) -> i64 {
        self.0.updated_at
    }

    /// When the session goes away without a further sign of life, in seconds since the epoch.
    async fn expires_at(&self) -> i64 {
        self.0.expires_at
    }
}

pub(crate) async fn sessions(
    context: &Context<'_>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<PlaybackConnection> {
    let owner = crate::access::require_caller(context)?.user;
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = crate::schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?;
            let page = playback(context)?
                .active(owner, now_epoch(), window)
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

    /// Records a sign of life, pushing the session's expiry forward. Whether it is still
    /// running.
    async fn touch_playback_session(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The identifier the client chose for this playback.")] key: String,
    ) -> Result<bool> {
        let key = crate::access::require_caller(context)?.session_key(&key);
        Ok(playback(context)?.touch(&key, now_epoch()).await?)
    }

    /// Records where a player stands, pushing its sessions' expiry forward, or closes them
    /// when it has stopped. Whether the playback had any session.
    async fn report_playback(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The player's report.")] input: ReportPlaybackInput,
    ) -> Result<bool> {
        let repository = playback(context)?;
        let playback_id = crate::access::require_caller(context)?.session_key(&input.playback_id);
        let state = match input.state {
            ReportedPlaybackState::Playing => PlaybackState::Playing,
            ReportedPlaybackState::Paused => PlaybackState::Paused,
            ReportedPlaybackState::Buffering => PlaybackState::Buffering,
            ReportedPlaybackState::Stopped => {
                return Ok(repository.end_playback(&playback_id).await?);
            }
        };
        Ok(repository
            .report(&playback_id, state, input.position_ms, now_epoch())
            .await?)
    }

    /// Creates a play queue from a list of items.
    async fn create_play_queue(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The queue to create.")] input: crate::play_queue::CreatePlayQueueInput,
    ) -> Result<crate::play_queue::PlayQueueObject> {
        crate::play_queue::create(context, input).await
    }

    /// Closes a session. Whether there was one.
    async fn end_playback_session(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The identifier the client chose for this playback.")] key: String,
    ) -> Result<bool> {
        let key = crate::access::require_caller(context)?.session_key(&key);
        Ok(playback(context)?.end(&key).await?)
    }

    /// Creates a user. For the server's operator only.
    async fn create_user(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user's e-mail address.")] email: String,
    ) -> Result<crate::access::UserObject> {
        crate::access::create_user(context, email).await
    }

    /// Deletes a user, its permissions and its connections. Whether there was one. For the
    /// server's operator only.
    async fn delete_user(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user, as returned in `User.id`.")] id: ID,
    ) -> Result<bool> {
        crate::access::delete_user(context, &id).await
    }

    /// Gives a user a role on a library, replacing the one it had. For the server's operator
    /// only.
    async fn set_library_permission(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The role to give.")] input: crate::access::SetLibraryPermissionInput,
    ) -> Result<bool> {
        crate::access::set_library_permission(context, input).await
    }

    /// Takes a library away from a user. Whether it had access. For the server's operator only.
    async fn remove_library_permission(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user, as returned in `User.id`.")] user_id: ID,
        #[graphql(desc = "The library, as returned in `Library.id`.")] library_id: ID,
    ) -> Result<bool> {
        crate::access::remove_library_permission(context, &user_id, &library_id).await
    }

    /// Connects a user through an extension, returning a new token. Called again, it replaces
    /// the token and keeps the identifier clients recognize. For the server's operator only.
    async fn connect_extension(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The user, as returned in `User.id`.")] user_id: ID,
        #[graphql(desc = "The protocol to connect through.")]
        extension: crate::access::ExtensionType,
    ) -> Result<crate::access::ExtensionCredentials> {
        crate::access::connect_extension(context, &user_id, extension).await
    }
}
