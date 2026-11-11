//! Streams on the official API: a file sent to a player as it is, or converted on the way.
//!
//! `startStream` decides and starts; what it returns is an address on this server carrying a
//! secret of its own, so the player needs no account token to read it. The playback session
//! it records is the stream's identity: `stopStream` names it.

use async_graphql::{Context, Enum, InputObject, Result, SimpleObject, ID};
use open_plex_core::lifecycle::StartError;
use open_plex_core::playback::{NewPlaybackSession, PlaybackOutput, StreamDecision};
use open_plex_core::stream::{
    Delivery, StartedStream, StreamError, StreamLimits, StreamOrder, StreamProtocol, StreamQuality,
    StreamSource, StreamTrack, SubtitleOutput,
};
use open_plex_core::tracks::selected_tracks;
use open_plex_core::{now_epoch, MediaStream, PartId, RatingKey, StreamKind};

use crate::node::GlobalId;
use crate::playback::{PlaybackSessionObject, StreamDecisionType};
use crate::schema::ApiContext;

/// How a converted stream is cut up for the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum StreamProtocolType {
    /// MPEG-DASH, fragmented MP4.
    Dash,
    /// HTTP Live Streaming, MPEG-TS segments.
    Hls,
}

impl From<StreamProtocolType> for StreamProtocol {
    fn from(protocol: StreamProtocolType) -> Self {
        match protocol {
            StreamProtocolType::Dash => Self::Dash,
            StreamProtocolType::Hls => Self::Hls,
        }
    }
}

/// How the file reaches the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum StreamDeliveryType {
    /// The file as it is.
    DirectPlay,
    /// The tracks repackaged, the video untouched.
    DirectStream,
    /// The video re-encoded.
    Transcode,
}

/// How a conversion's quality follows the link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum StreamQualityType {
    /// One quality for the whole stream, the best within `maxVideoBitrate`.
    Fixed,
    /// One quality announced, which the server lowers and raises by measuring how fast the
    /// player receives it. Always a re-encode, at one picture size.
    ServerAdaptive,
    /// Every quality announced, the video copied first when the file allows it, and the player
    /// chooses among them as it goes.
    ClientAdaptive,
}

impl From<StreamQualityType> for StreamQuality {
    fn from(quality: StreamQualityType) -> Self {
        match quality {
            StreamQualityType::Fixed => Self::Fixed,
            StreamQualityType::ServerAdaptive => Self::ServerAdaptive,
            StreamQualityType::ClientAdaptive => Self::ClientAdaptive,
        }
    }
}

/// How a subtitle reaches the player of a conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum SubtitleDeliveryType {
    /// No subtitle, whatever was chosen.
    None,
    /// Drawn into the picture, which re-encodes the video.
    Burn,
    /// As ASS text beside the stream, for the player to draw. DASH only.
    Text,
}

/// A stream to start.
#[derive(Debug, InputObject)]
pub struct StartStreamInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The file to play, as returned in `MediaFile.id`.
    pub file: ID,
    /// The player reads the file as it is: it is sent untouched.
    #[graphql(default)]
    pub direct_play: bool,
    /// How a conversion is cut up.
    #[graphql(default_with = "StreamProtocolType::Dash")]
    pub protocol: StreamProtocolType,
    /// The audio track, as returned in `MediaStream.id`. Left out, the caller's choice.
    pub audio_track: Option<ID>,
    /// The subtitle, as returned in `MediaStream.id`. Left out, the caller's choice.
    pub subtitle_track: Option<ID>,
    /// How the subtitle reaches the player.
    #[graphql(default_with = "SubtitleDeliveryType::None")]
    pub subtitles: SubtitleDeliveryType,
    /// The size burned subtitles are drawn at, 1.0 being the normal size.
    #[graphql(default = 1.0)]
    pub subtitle_scale: f64,
    /// The most the video may weigh, in bits per second: for an adaptive stream, a ceiling on
    /// every quality.
    pub max_video_bitrate: Option<i32>,
    /// Convert the audio even when the player could take it as it is.
    #[graphql(default)]
    pub convert_audio: bool,
    /// How the quality follows the link.
    #[graphql(default_with = "StreamQualityType::Fixed")]
    pub quality: StreamQualityType,
    /// Where to start, in milliseconds.
    #[graphql(default)]
    pub position_ms: i64,
    /// The client's own name for this stream: a later start under it replaces this one. A
    /// fresh name when left out.
    pub key: Option<String>,
    /// The player's identifier for the whole playback, across changes of quality.
    pub playback_id: Option<String>,
    /// The client's stable identifier for itself.
    pub client_identifier: Option<String>,
    /// The application playing.
    pub client_product: Option<String>,
}

impl StartStreamInput {
    fn delivery(&self) -> Delivery {
        if self.direct_play {
            Delivery::Original
        } else {
            Delivery::Converted(self.protocol.into())
        }
    }

    fn limits(&self) -> StreamLimits {
        StreamLimits {
            max_video_bitrate: self
                .max_video_bitrate
                .and_then(|bits| u32::try_from(bits).ok()),
            convert_audio: self.convert_audio,
            quality: self.quality.into(),
        }
    }

    fn subtitle_output(&self, track: &MediaStream) -> Option<SubtitleOutput> {
        let track = StreamTrack::of(track);
        match self.subtitles {
            SubtitleDeliveryType::None => None,
            SubtitleDeliveryType::Burn => Some(SubtitleOutput::Burn {
                track,
                scale: self.subtitle_scale,
            }),
            SubtitleDeliveryType::Text => Some(SubtitleOutput::Text { track }),
        }
    }
}

/// A stream that was started.
#[derive(Debug, SimpleObject)]
pub struct StreamObject {
    /// How the file reaches the player.
    pub delivery: StreamDeliveryType,
    /// How it is cut up. Null for direct play.
    pub protocol: Option<StreamProtocolType>,
    /// How its quality follows the link, as asked.
    pub quality: StreamQualityType,
    /// What happens to the video track.
    pub video_decision: StreamDecisionType,
    /// What happens to the audio track.
    pub audio_decision: StreamDecisionType,
    /// Where the player reads it, a path on this server: the manifest of a conversion, the file
    /// itself for direct play. It carries its own secret and needs no token.
    pub url: String,
    /// Where the player reads text subtitles, when they were asked for.
    pub subtitles_url: Option<String>,
    /// The bitrate of the video sent, in bits per second, when known: at the start, for an
    /// adaptive stream.
    pub video_bitrate: Option<i32>,
    /// The bitrate of the audio sent, in bits per second, when known.
    pub audio_bitrate: Option<i32>,
    /// The width of the picture sent, when known.
    pub width: Option<i32>,
    /// The height of the picture sent, when known.
    pub height: Option<i32>,
}

/// Why a stream could not be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum StreamErrorCode {
    /// No such file, or not one the caller can see.
    FileNotFound,
    /// The file is known but not on this machine.
    FileMissing,
    /// The track named is not one of the file's, of that kind; or the file has none.
    TrackNotFound,
    /// The video would need a conversion this server does not do.
    VideoNotConvertible,
    /// The subtitle cannot be delivered the way asked: an image subtitle, text over HLS, or no
    /// fonts to burn with.
    SubtitleNotDeliverable,
    /// Too many streams are running.
    TooManyStreams,
    /// The key names a session the operator terminated: its player has to stop first.
    SessionTerminated,
}

/// A problem with what was asked, as opposed to a failure of the server.
#[derive(Debug, Clone, SimpleObject)]
pub struct StreamUserError {
    /// The input field at fault, as a path from the input's root.
    pub field: Vec<String>,
    /// What went wrong, for a person.
    pub message: String,
    /// What went wrong, for a program.
    pub code: StreamErrorCode,
}

impl StreamUserError {
    fn new(field: &str, code: StreamErrorCode) -> Self {
        let message = match code {
            StreamErrorCode::FileNotFound => "no such file",
            StreamErrorCode::FileMissing => "the file is not on this machine",
            StreamErrorCode::TrackNotFound => "no such track in this file",
            StreamErrorCode::VideoNotConvertible => {
                "the video would need a conversion this server does not do"
            }
            StreamErrorCode::SubtitleNotDeliverable => "this subtitle cannot be delivered this way",
            StreamErrorCode::TooManyStreams => "too many streams are running",
            StreamErrorCode::SessionTerminated => "this session was terminated",
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }
}

/// What `startStream` did.
#[derive(Debug, SimpleObject)]
pub struct StartStreamPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The stream, unless it could not be started.
    pub stream: Option<StreamObject>,
    /// The playback session recording it, which `stopStream` names.
    pub session: Option<PlaybackSessionObject>,
    /// Why it could not be started. Empty when it was.
    pub user_errors: Vec<StreamUserError>,
}

impl StartStreamPayload {
    fn refused(client_mutation_id: Option<String>, error: StreamUserError) -> Self {
        Self {
            client_mutation_id,
            stream: None,
            session: None,
            user_errors: vec![error],
        }
    }
}

/// A stream to stop.
#[derive(Debug, InputObject)]
pub struct StopStreamInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The stream's session, as returned in `StartStreamPayload.session.id`.
    pub session: ID,
}

/// What `stopStream` did.
#[derive(Debug, SimpleObject)]
pub struct StopStreamPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// Whether there was a stream to stop.
    pub stopped: bool,
}

fn decoded(id: &ID, type_name: &str) -> Option<i64> {
    GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == type_name)
        .map(|decoded| decoded.key)
}

/// The track an input names, which must be the file's and of the kind.
fn named_track<'a>(
    streams: &'a [MediaStream],
    id: &ID,
    kind: StreamKind,
) -> Option<&'a MediaStream> {
    let key = decoded(id, crate::media::STREAM_TYPE)?;
    streams
        .iter()
        .find(|stream| stream.id.get() == key && stream.kind == kind)
}

/// A stream ready to start, and what its session records.
struct Prepared {
    order: StreamOrder,
    item: RatingKey,
    file: PartId,
}

/// The order an input describes, or what is wrong with it.
async fn prepare(
    context: &Context<'_>,
    input: &StartStreamInput,
) -> Result<std::result::Result<Prepared, StreamUserError>> {
    let caller = crate::access::require_caller(context)?;
    let refused = |field: &str, code: StreamErrorCode| Ok(Err(StreamUserError::new(field, code)));

    let file = match decoded(&input.file, crate::media::FILE_TYPE) {
        Some(key) => crate::schema::readable_file(context, PartId::new(key)).await?,
        None => None,
    };
    let Some(file) = file else {
        return refused("file", StreamErrorCode::FileNotFound);
    };
    let api = context.data::<ApiContext>()?;
    let Some(version) = api.metadata.version(file.version_id).await? else {
        return refused("file", StreamErrorCode::FileNotFound);
    };
    let Some(item) = crate::schema::readable_item(context, version.item_id).await? else {
        return refused("file", StreamErrorCode::FileNotFound);
    };
    let streams = api.metadata.streams_for(&[file.id]).await?;
    let selected = selected_tracks(
        &streams,
        crate::watch::track_choice_of(context, file.id).await?,
    );
    let chosen = |kind: StreamKind, id: Option<open_plex_core::StreamId>| {
        id.and_then(|id| streams.iter().find(|s| s.kind == kind && s.id == id))
    };

    let Some(video) = streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Video)
    else {
        return refused("file", StreamErrorCode::TrackNotFound);
    };
    let audio = match &input.audio_track {
        Some(id) => named_track(&streams, id, StreamKind::Audio),
        None => chosen(StreamKind::Audio, selected.audio),
    };
    let Some(audio) = audio else {
        return refused("audioTrack", StreamErrorCode::TrackNotFound);
    };
    let subtitle = match &input.subtitle_track {
        Some(id) => match named_track(&streams, id, StreamKind::Subtitle) {
            Some(track) => Some(track),
            None => return refused("subtitleTrack", StreamErrorCode::TrackNotFound),
        },
        None => chosen(StreamKind::Subtitle, selected.subtitle),
    };
    let subtitle = subtitle.and_then(|track| input.subtitle_output(track));

    let roots = api
        .library
        .roots_for(&[item.section_id])
        .await?
        .into_iter()
        .map(|root| root.path)
        .collect();
    let key = input
        .key
        .clone()
        .unwrap_or_else(open_plex_core::access::new_token);
    Ok(Ok(Prepared {
        order: StreamOrder {
            key: caller.session_key(&key),
            playback: input
                .playback_id
                .as_deref()
                .map(|playback| caller.session_key(playback)),
            source: StreamSource {
                path: file.path.clone(),
                roots,
                width: version.width,
                height: version.height,
                bitrate: version.bitrate,
                frame_rate: version.frame_rate,
                duration_ms: version.duration_ms.or(file.duration_ms),
                video: StreamTrack::of(video),
                audio: StreamTrack::of(audio),
            },
            delivery: input.delivery(),
            limits: input.limits(),
            subtitle,
            position_ms: input.position_ms.max(0),
        },
        item: item.id,
        file: file.id,
    }))
}

pub(crate) async fn start_stream(
    context: &Context<'_>,
    input: StartStreamInput,
) -> Result<StartStreamPayload> {
    let caller = crate::access::require_caller(context)?;
    let api = context.data::<ApiContext>()?;
    let refuse = |error: StreamUserError| {
        Ok(StartStreamPayload::refused(
            input.client_mutation_id.clone(),
            error,
        ))
    };
    let Prepared { order, item, file } = match prepare(context, &input).await? {
        Ok(prepared) => prepared,
        Err(error) => return refuse(error),
    };
    let key = order.key.clone();
    let delivery = order.delivery;
    let quality = input.quality;
    let playback_id = order.playback.clone();
    let position_ms = input.position_ms.max(0);
    let record = |started: &StartedStream| NewPlaybackSession {
        key,
        user: caller.user,
        playback_id,
        item,
        part: file,
        client: input.client_identifier.clone(),
        product: input.client_product.clone(),
        video: started.video,
        audio: started.audio,
        position_ms,
        output: started.output,
    };

    let (started, session) = match api.lifecycle().start(order, record, now_epoch()).await {
        Ok(started) => started,
        Err(StartError::Storage(error)) => return Err(error.into()),
        Err(StartError::Terminated) => {
            return refuse(StreamUserError::new(
                "key",
                StreamErrorCode::SessionTerminated,
            ));
        }
        Err(StartError::Stream(error)) => {
            let (field, code) = match error {
                StreamError::MissingFile => ("file", StreamErrorCode::FileMissing),
                StreamError::VideoNotConvertible => ("file", StreamErrorCode::VideoNotConvertible),
                StreamError::SubtitleNotDeliverable => {
                    ("subtitles", StreamErrorCode::SubtitleNotDeliverable)
                }
                StreamError::TooManyStreams => ("file", StreamErrorCode::TooManyStreams),
                StreamError::Engine(_) => return Err(error.into()),
            };
            return refuse(StreamUserError::new(field, code));
        }
    };
    Ok(StartStreamPayload {
        client_mutation_id: input.client_mutation_id,
        stream: Some(stream_object(started, delivery, quality)),
        session: Some(PlaybackSessionObject(session)),
        user_errors: Vec::new(),
    })
}

fn stream_object(
    started: StartedStream,
    delivery: Delivery,
    quality: StreamQualityType,
) -> StreamObject {
    let PlaybackOutput {
        video_bitrate,
        audio_bitrate,
        width,
        height,
    } = started.output;
    let (delivery, protocol) = match delivery {
        Delivery::Original => (StreamDeliveryType::DirectPlay, None),
        Delivery::Converted(protocol) => (
            match started.video {
                StreamDecision::Copy => StreamDeliveryType::DirectStream,
                StreamDecision::Transcode => StreamDeliveryType::Transcode,
            },
            Some(match protocol {
                StreamProtocol::Dash => StreamProtocolType::Dash,
                StreamProtocol::Hls => StreamProtocolType::Hls,
            }),
        ),
    };
    StreamObject {
        delivery,
        protocol,
        quality,
        video_decision: started.video.into(),
        audio_decision: started.audio.into(),
        url: started.url,
        subtitles_url: started.subtitles_url,
        video_bitrate,
        audio_bitrate,
        width,
        height,
    }
}

pub(crate) async fn stop_stream(
    context: &Context<'_>,
    input: StopStreamInput,
) -> Result<StopStreamPayload> {
    let caller = crate::access::require_caller(context)?;
    let api = context.data::<ApiContext>()?;
    let session = match decoded(&input.session, crate::playback::PLAYBACK_TYPE) {
        Some(id) => api.playback.session(id, now_epoch()).await?,
        None => None,
    };
    let stopped = match session.filter(|session| caller.owns(session.user)) {
        Some(session) => api.lifecycle().end(&session.key).await?,
        None => false,
    };
    Ok(StopStreamPayload {
        client_mutation_id: input.client_mutation_id,
        stopped,
    })
}
