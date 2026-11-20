//! Subtitles in files of their own beside the media: their text, the offset each viewer
//! shifts a subtitle by, and deleting one.
//!
//! A sidecar is read and deleted through the media engine, from the path its track stores and
//! nothing else, and only where a sidecar of its media may be: within its library, in the
//! media's directory or below. The rules on locations are in `core::sidecar`.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, ID};
use klapz_core::sidecar::{
    check_offset_target, subtitle_text, OffsetRefusal, Sidecar, SidecarError, SidecarSource,
};
use klapz_core::watch::{WatchError, WatchRepository};
use klapz_core::{MediaFile, MediaStream, MetadataItem, StreamId, UserId};

use crate::media::{Track, STREAM_TYPE};
use crate::node::GlobalId;
use crate::schema::ApiContext;

/// A subtitle kept in a file of its own.
#[derive(Debug, Clone)]
pub struct SidecarFile {
    stream: MediaStream,
    sidecar: Sidecar,
}

impl SidecarFile {
    pub(crate) fn of(stream: &MediaStream) -> Option<Self> {
        Sidecar::of(stream).map(|sidecar| Self {
            stream: stream.clone(),
            sidecar,
        })
    }
}

#[allow(clippy::missing_errors_doc)]
#[Object(name = "SidecarFile")]
impl SidecarFile {
    /// The file's path, as the library was scanned. Null for a subtitle downloaded into the
    /// reference server's own storage, which was not imported.
    async fn path(&self) -> Option<&str> {
        self.sidecar.path()
    }

    /// The last segment of `path`.
    async fn filename(&self) -> Option<&str> {
        self.sidecar.path().map(crate::media::filename_of)
    }

    /// Whether the subtitle was downloaded from a provider rather than found beside the media.
    async fn is_downloaded(&self) -> bool {
        self.sidecar == Sidecar::Downloaded
    }

    /// The subtitle's text, as UTF-8: as written when the file is UTF-8, byte-order mark
    /// included, and read as Windows-1252 otherwise. Null when this server does not hold it:
    /// a downloaded subtitle, or a file that is not on this machine or not where a sidecar of
    /// its media may be — within its library, in the media's directory or below.
    async fn text(&self, context: &Context<'_>) -> Result<Option<String>> {
        let Some(path) = self.sidecar.path() else {
            return Ok(None);
        };
        let Some(placed) = placed(context, &self.stream).await? else {
            return Ok(None);
        };
        let source = placed.source(context, path).await?;
        match context
            .data::<ApiContext>()?
            .streams
            .read_sidecar(&source)
            .await
        {
            Ok(bytes) => Ok(Some(subtitle_text(bytes))),
            Err(SidecarError::Missing) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// The file a track belongs to, and its item, when the caller may read them.
struct Placed {
    file: MediaFile,
    item: MetadataItem,
}

impl Placed {
    async fn source(&self, context: &Context<'_>, path: &str) -> Result<SidecarSource> {
        let roots = context
            .data::<ApiContext>()?
            .library
            .roots_for(&[self.item.section_id])
            .await?
            .into_iter()
            .map(|root| root.path)
            .collect();
        Ok(SidecarSource {
            path: path.to_owned(),
            media_path: self.file.path.clone(),
            roots,
        })
    }
}

async fn placed(context: &Context<'_>, stream: &MediaStream) -> Result<Option<Placed>> {
    let metadata = crate::schema::metadata(context)?;
    let Some(file) = crate::schema::readable_file(context, stream.file_id).await? else {
        return Ok(None);
    };
    let Some(version) = metadata.version(file.version_id).await? else {
        return Ok(None);
    };
    Ok(crate::schema::readable_item(context, version.item_id)
        .await?
        .map(|item| Placed { file, item }))
}

/// Loads the offsets viewers shift several subtitles by, in one call per viewer.
#[derive(Debug, Clone)]
pub struct SubtitleOffsetLoader {
    repository: Arc<dyn WatchRepository>,
}

impl SubtitleOffsetLoader {
    pub const fn new(repository: Arc<dyn WatchRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<(UserId, StreamId)> for SubtitleOffsetLoader {
    type Value = i32;
    type Error = Arc<WatchError>;

    async fn load(
        &self,
        keys: &[(UserId, StreamId)],
    ) -> Result<HashMap<(UserId, StreamId), Self::Value>, Self::Error> {
        let mut by_viewer: HashMap<UserId, Vec<StreamId>> = HashMap::new();
        for (viewer, track) in keys {
            by_viewer.entry(*viewer).or_default().push(*track);
        }
        let mut found = HashMap::new();
        for (viewer, tracks) in by_viewer {
            for (track, offset) in self
                .repository
                .subtitle_offsets(viewer, &tracks)
                .await
                .map_err(Arc::new)?
            {
                found.insert((viewer, track), offset);
            }
        }
        Ok(found)
    }
}

/// The offset the caller shifts a track by: 0 when it set none, and for a request with no
/// caller.
pub(crate) async fn offset_of(context: &Context<'_>, track: StreamId) -> Result<i32> {
    let Some(viewer) = crate::access::viewer_id(context) else {
        return Ok(0);
    };
    Ok(context
        .data::<DataLoader<SubtitleOffsetLoader>>()?
        .load_one((viewer, track))
        .await?
        .unwrap_or(0))
}

/// Why a subtitle mutation refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum SubtitleErrorCode {
    /// No track the caller may read has this id.
    TrackNotFound,
    /// The track is a video or an audio track.
    NotASubtitle,
    /// The subtitle is inside the media file, not a file of its own.
    NotASidecar,
    /// The caller may read the library but not change it: only its admins and the server's
    /// owners delete a subtitle.
    NotAllowed,
}

/// A request a subtitle mutation refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct SubtitleUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: SubtitleErrorCode,
}

impl SubtitleUserError {
    fn on_track(code: SubtitleErrorCode) -> Self {
        let message = match code {
            SubtitleErrorCode::TrackNotFound => "no such track",
            SubtitleErrorCode::NotASubtitle => "only a subtitle is shifted",
            SubtitleErrorCode::NotASidecar => "this subtitle is inside its media file",
            SubtitleErrorCode::NotAllowed => "only the library's admins delete a subtitle",
        };
        Self {
            field: vec!["input".to_owned(), "trackId".to_owned()],
            message: message.to_owned(),
            code,
        }
    }
}

/// The track an input names, when the caller may read it.
async fn named_track(context: &Context<'_>, id: &ID) -> Result<Option<MediaStream>> {
    let Some(key) = GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == STREAM_TYPE)
        .map(|decoded| StreamId::new(decoded.key))
    else {
        return Ok(None);
    };
    let Some(stream) = crate::schema::metadata(context)?.stream(key).await? else {
        return Ok(None);
    };
    Ok(crate::schema::readable_file(context, stream.file_id)
        .await?
        .map(|_| stream))
}

/// An offset to shift a subtitle by.
#[derive(Debug, InputObject)]
pub struct SetSubtitleOffsetInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The subtitle, as returned in `MediaStream.id`: one inside its media or a file of its
    /// own.
    pub track_id: ID,
    /// In milliseconds, for the caller's player to apply. 0 removes the offset.
    pub offset_ms: i32,
}

/// What `setSubtitleOffset` did.
#[derive(Debug, SimpleObject)]
pub struct SetSubtitleOffsetPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The subtitle, with its new offset. Null when the request was refused.
    pub track: Option<Track>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<SubtitleUserError>,
}

pub(crate) async fn set_offset(
    context: &Context<'_>,
    input: SetSubtitleOffsetInput,
) -> Result<SetSubtitleOffsetPayload> {
    let refused = |code| {
        Ok(SetSubtitleOffsetPayload {
            client_mutation_id: input.client_mutation_id.clone(),
            track: None,
            user_errors: vec![SubtitleUserError::on_track(code)],
        })
    };
    let Some(viewer) = crate::access::viewer_id(context) else {
        return refused(SubtitleErrorCode::TrackNotFound);
    };
    let Some(stream) = named_track(context, &input.track_id).await? else {
        return refused(SubtitleErrorCode::TrackNotFound);
    };
    if let Err(OffsetRefusal::NotASubtitle) = check_offset_target(&stream) {
        return refused(SubtitleErrorCode::NotASubtitle);
    }
    context
        .data::<ApiContext>()?
        .watch
        .set_subtitle_offset(viewer, stream.id, input.offset_ms)
        .await?;
    // The loader may hold the offset from before.
    context
        .data::<DataLoader<SubtitleOffsetLoader>>()?
        .feed_one((viewer, stream.id), input.offset_ms)
        .await;
    Ok(SetSubtitleOffsetPayload {
        client_mutation_id: input.client_mutation_id,
        track: Some(Track(stream)),
        user_errors: Vec::new(),
    })
}

/// A subtitle file to delete.
#[derive(Debug, InputObject)]
pub struct DeleteSidecarInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The subtitle, as returned in `MediaStream.id`. It must be a file of its own
    /// (`MediaStream.sidecar`).
    pub track_id: ID,
}

/// What `deleteSidecar` did.
#[derive(Debug, SimpleObject)]
pub struct DeleteSidecarPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The subtitle deleted, as it was returned in `MediaStream.id`. Null when refused.
    pub deleted_track_id: Option<ID>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<SubtitleUserError>,
}

pub(crate) async fn delete_sidecar(
    context: &Context<'_>,
    input: DeleteSidecarInput,
) -> Result<DeleteSidecarPayload> {
    let refused = |code| {
        Ok(DeleteSidecarPayload {
            client_mutation_id: input.client_mutation_id.clone(),
            deleted_track_id: None,
            user_errors: vec![SubtitleUserError::on_track(code)],
        })
    };
    let Some(stream) = named_track(context, &input.track_id).await? else {
        return refused(SubtitleErrorCode::TrackNotFound);
    };
    let Some(placed) = placed(context, &stream).await? else {
        return refused(SubtitleErrorCode::TrackNotFound);
    };
    let Some(sidecar) = Sidecar::of(&stream) else {
        return refused(SubtitleErrorCode::NotASidecar);
    };
    if !crate::access::can_edit(context, placed.item.section_id) {
        return refused(SubtitleErrorCode::NotAllowed);
    }
    let api = context.data::<ApiContext>()?;
    if let Sidecar::File(path) = &sidecar {
        let source = placed.source(context, path).await?;
        match api.streams.remove_sidecar(&source).await {
            // Not on this machine, or somewhere this server never reads a sidecar from: the
            // disk is left alone, and the track forgotten all the same.
            Ok(()) | Err(SidecarError::Missing) => {}
            Err(error) => return Err(error.into()),
        }
    }
    api.metadata.delete_stream(stream.id).await?;
    Ok(DeleteSidecarPayload {
        client_mutation_id: input.client_mutation_id,
        deleted_track_id: Some(input.track_id),
        user_errors: Vec::new(),
    })
}
