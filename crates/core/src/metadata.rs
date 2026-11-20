//! Library contents: items, their media, and the queries bindings run against them.

use crate::chapter::{Chapter, ChapterSource};
use crate::edit::{ItemEdit, ItemField};
use crate::extra::Extra;
use crate::ids::{
    ChapterId, ExtraId, MarkerId, MediaId, PartId, PersonId, RatingKey, SectionId, StreamId, TagId,
    UserId,
};
use crate::marker::{Marker, NewMarker};
use crate::page::{Cursor, CursorKey, Direction, Page, Window};

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("no item with id {0}")]
    ItemNotFound(RatingKey),
    /// A cursor that cannot locate a position under the requested ordering — a client
    /// mistake, typically reusing a cursor after changing the sort field.
    #[error("the cursor does not match the requested ordering")]
    CursorMismatch,
    /// Storage failed. Boxed so the cause chain survives for the server's logs without
    /// `core` knowing which engine is in use.
    #[error("metadata storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// What an item is.
///
/// The four kinds share one storage table and one hierarchy, but they are not
/// interchangeable: a show has children and no media, a movie the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Movie,
    Show,
    Season,
    Episode,
}

impl ItemKind {
    /// Whether items of this kind carry media.
    ///
    /// Measured on the reference library: every movie and episode has media, no show or
    /// season does.
    pub const fn is_playable(self) -> bool {
        matches!(self, Self::Movie | Self::Episode)
    }
}

/// An item in a library.
///
/// One struct for all four kinds, because they share a table and the bindings are what
/// present them as distinct types. Fields that only apply to some kinds are optional rather
/// than split out — `studio` is set on 61% of movies and no episodes.
#[derive(Debug, Clone)]
pub struct MetadataItem {
    pub id: RatingKey,
    pub section_id: SectionId,
    /// The season for an episode, the show for a season, `None` for a movie or a show.
    pub parent_id: Option<RatingKey>,
    pub kind: ItemKind,
    /// Identity of the work, shared by the same film in several libraries.
    pub guid: String,
    /// Absent for almost every season: the source stores none, and a display name is
    /// localized presentation that belongs to a binding.
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub original_title: Option<String>,
    pub summary: Option<String>,
    pub tagline: Option<String>,
    pub studio: Option<String>,
    pub content_rating: Option<String>,
    pub content_rating_age: Option<i32>,
    /// Episode or season number within its parent.
    pub position: Option<i32>,
    pub year: Option<i32>,
    /// The release date, as its midnight UTC in seconds since the Unix epoch.
    pub released_on: Option<i64>,
    pub duration_ms: Option<i64>,
    pub rating: Option<f64>,
    pub audience_rating: Option<f64>,
    pub added_at: i64,
    pub updated_at: i64,
    /// When the metadata was last refreshed from its agent. `None` for an item never refreshed.
    pub refreshed_at: Option<i64>,
    /// Where its chapters came from; `None` when it has none.
    pub chapter_source: Option<ChapterSource>,
}

/// One encoding of an item.
#[derive(Debug, Clone)]
pub struct MediaVersion {
    pub id: MediaId,
    pub item_id: RatingKey,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Measured display aspect ratio, e.g. 2.3762. Not snapped to a standard ratio.
    pub aspect_ratio: Option<f64>,
    pub duration_ms: Option<i64>,
    /// Bits per second, as the source measured it — not kilobits.
    pub bitrate: Option<i32>,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i32>,
    /// Frames per second as measured, e.g. 23.976. Turning that into a label like `24p` is a
    /// wire concern.
    pub frame_rate: Option<f64>,
    pub interlaced: bool,
    pub optimized_for_streaming: bool,
}

impl MediaVersion {
    /// The audio track this version's `audio_codec` and `audio_channels` describe.
    ///
    /// The first audio track matching both, else the default one, else the first. Matched
    /// rather than assumed to be the default: on 243,112 of the reference's 243,159 media
    /// this is the track whose profile the version reports, and the default track is not
    /// on 6,671 of them.
    pub fn primary_audio<'a>(&self, tracks: &'a [MediaStream]) -> Option<&'a MediaStream> {
        let audio = || {
            tracks
                .iter()
                .filter(|track| track.kind == StreamKind::Audio)
        };
        audio()
            .find(|track| track.codec == self.audio_codec && track.channels == self.audio_channels)
            .or_else(|| audio().find(|track| track.is_default))
            .or_else(|| audio().next())
    }
}

/// One file on disk.
#[derive(Debug, Clone)]
pub struct MediaFile {
    pub id: PartId,
    pub version_id: MediaId,
    pub position: i32,
    /// Absolute path on the server's filesystem.
    ///
    /// Published by both surfaces, as Plex does. What that does *not* change: playback still
    /// resolves a file from its id and never from a client-supplied path. A path the server
    /// hands out is reference data; a path a client hands back is an attack surface.
    ///
    /// Empty for a file a provider streams, which is on no disk of the server's.
    pub path: String,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    pub layout: FileLayout,
    /// Where a provider streams the file from: the trailers the reference's metadata provider
    /// supplies. `None` for a file of the library.
    pub stream_url: Option<String>,
    /// The SHA-1 the scanner computed over the file's contents, in hex: two copies of one
    /// file share it.
    pub content_hash: Option<String>,
    /// The `OpenSubtitles` hash, in hex: the file's size plus the 64-bit sums of its first and
    /// last 64 KiB, what subtitle services find a file by.
    pub open_subtitles_hash: Option<String>,
}

/// How a file is laid out, as its analysis found it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileLayout {
    /// An embedded cover image.
    pub has_thumbnail: bool,
    /// Whether an MP4 uses 64-bit chunk offsets. `None` when the file is not MP4.
    pub has_64bit_offsets: Option<bool>,
    /// Whether an MP4's index precedes its data, so it plays before it fully arrives.
    /// `None` when the file is not MP4.
    pub optimized_for_streaming: Option<bool>,
    /// The packet size of an MPEG-TS file: 188, or 192 for Blu-ray's M2TS.
    pub packet_length: Option<i32>,
    pub has_chapter_text_stream: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
}

/// One track inside a file, or a sidecar subtitle next to it.
#[derive(Debug, Clone)]
pub struct MediaStream {
    pub id: StreamId,
    pub file_id: PartId,
    pub kind: StreamKind,
    /// The index the transcoder addresses this track by.
    pub position: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<i32>,
    /// Bits per second, as the source measured it — not kilobits.
    pub bitrate: Option<i32>,
    pub is_default: bool,
    pub is_forced: bool,
    /// Set when the track lives in its own file.
    pub external_path: Option<String>,
    /// The name the file gives the track, often a release tag: `VFF AC3 5.1 @448kb`.
    /// Empty, not `None`, when the file writes an empty name: clients label the two apart.
    pub title: Option<String>,
    /// The container's own number for the track: a Matroska track number, an MPEG-TS PID.
    pub stream_identifier: Option<i64>,
    /// The codec profile: `high` or `main 10` for video, `lc` for AAC, `ma` for DTS-HD
    /// Master Audio.
    pub profile: Option<String>,
    /// Bits per sample.
    pub bit_depth: Option<i32>,
    pub traits: TrackTraits,
    /// Set on a video track, and only there. Boxed: it is most of a track's size, and only
    /// one track in three carries it.
    pub video: Option<Box<VideoAnalysis>>,
    /// Set on an audio track, and only there.
    pub audio: Option<AudioAnalysis>,
    /// Set on a subtitle track, and only there.
    pub subtitle: Option<SubtitleAnalysis>,
    /// Kilobits per second the track needs to play without stalling, one per buffer size
    /// from the smallest, as the analysis found them. Empty when it never measured them.
    pub required_bandwidths: Vec<i32>,
}

/// What a file says a track is for, beyond its language.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags the source records one by one, not states to fold into an enum"
)]
pub struct TrackTraits {
    /// In the work's original language.
    pub original: bool,
    pub dub: bool,
    pub commentary: bool,
    pub hearing_impaired: bool,
    /// Audio description.
    pub visual_impaired: bool,
    /// Matroska header stripping, which some players cannot decode.
    pub header_compression: bool,
}

/// Progressive or interlaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanType {
    Progressive,
    Interlaced,
}

/// What the analysis measured about a video track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoAnalysis {
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Dimensions as encoded, before cropping: 1088 for most 1080-line H.264.
    pub coded_width: Option<i32>,
    pub coded_height: Option<i32>,
    /// Frames per second, as measured: 23.976.
    pub frame_rate: Option<f64>,
    /// The codec's level indicator as coded: 41 is H.264 level 4.1, 120 is HEVC level 4.
    pub level: Option<i32>,
    pub ref_frames: Option<i32>,
    pub scan_type: Option<ScanType>,
    /// `4:2:0`.
    pub chroma_subsampling: Option<String>,
    pub chroma_location: Option<String>,
    pub color: ColorDescription,
    /// `None` when the analysis did not say, which is not the same as false.
    pub has_scaling_matrix: Option<bool>,
    pub anamorphic: Option<bool>,
    /// Sample aspect ratio as a ratio of integers: `16:15`.
    pub pixel_aspect_ratio: Option<String>,
    /// The codec tag a legacy container carries: `XVID`, `DX50`.
    pub fourcc: Option<String>,
    pub closed_captions: bool,
    /// Fields coded separately, as an interlaced picture can be.
    pub separate_fields: bool,
    pub hdr10_plus: bool,
    pub dolby_vision: Option<DolbyVision>,
}

/// How a video track's colors are to be read, in ffmpeg's vocabulary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorDescription {
    /// `tv` (limited) or `pc` (full).
    pub range: Option<String>,
    /// The matrix: `bt709`, `bt2020nc`.
    pub space: Option<String>,
    pub primaries: Option<String>,
    /// The transfer characteristic: `smpte2084` is PQ, the HDR10 curve; `arib-std-b67`
    /// is HLG.
    pub transfer: Option<String>,
}

/// A Dolby Vision configuration record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolbyVision {
    /// As written: `1.0`.
    pub version: String,
    pub profile: i32,
    pub level: i32,
    /// What the base layer plays as without Dolby Vision: 1 HDR10, 2 SDR, 4 HLG,
    /// 6 Blu-ray HDR10.
    pub base_layer_compatibility: Option<i32>,
    pub has_base_layer: bool,
    pub has_enhancement_layer: bool,
    /// Whether the track carries the reference processing unit, its dynamic metadata.
    pub has_rpu: bool,
}

/// What the analysis measured about an audio track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioAnalysis {
    /// Hertz.
    pub sampling_rate: Option<i32>,
    /// In ffmpeg's vocabulary: `stereo`, `5.1(side)`.
    pub channel_layout: Option<String>,
    pub loudness: Option<Loudness>,
}

/// An EBU R128 loudness measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Loudness {
    /// As written: `0.02`.
    pub analysis_version: String,
    /// Integrated loudness, in LUFS.
    pub integrated: f64,
    /// Loudness range, in LU.
    pub range: f64,
    /// True peak, in dBTP.
    pub true_peak: f64,
    /// The gating threshold of the integration, in LUFS.
    pub threshold: f64,
    /// The gain, in dB, that brings the track to the target loudness.
    pub gain_offset: f64,
}

/// What the analysis says about a subtitle track.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubtitleAnalysis {
    /// The format of a sidecar file: `srt`, `ssa`.
    pub format: Option<String>,
    /// Captions carried inside the video bitstream (EIA-608) rather than as a track.
    pub embedded_in_video: bool,
}

/// What a tag classifies or credits.
///
/// The six the reference server publishes on an item and filters by. Collections are not
/// among them: a collection is an item of its own, with its own endpoints, and treating it
/// as a label would leave nowhere to put its artwork and summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagKind {
    Genre,
    Country,
    Director,
    Writer,
    Actor,
    Producer,
}

/// Someone credited on an item.
///
/// Stored apart from their credits, which are tags: the name and portrait belong to the
/// person, while what they did on an item belongs to the tag. Two people may share a name —
/// 11,879 actor names in the reference library belong to more than one person — so identity
/// is the id, and the provider key is what told them apart at import.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub id: PersonId,
    pub name: String,
    /// The metadata provider's identifier for this person.
    pub provider_key: Option<String>,
    /// A portrait, hosted by the metadata provider.
    pub thumb_url: Option<String>,
}

/// A genre, a country, or a person in one credit role.
///
/// A person credited in two roles is two tags — Luc Besson the writer and Luc Besson the
/// producer are distinct rows with distinct ids, as they are in the source — because what a
/// filter asks for is "films he wrote", not "films he touched".
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub id: TagId,
    pub kind: TagKind,
    /// The name as credited, which can differ from the person's current name: the source
    /// credits `Andy Garcia` on some items and `Andy García` on others.
    pub name: String,
    /// Who a credit names. `None` for a genre or a country.
    pub person: Option<Person>,
}

/// Which tags a list of a library's tags covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagScope {
    pub kind: TagKind,
    /// Only credits billed within an item's first `n` positions of their kind: the leads,
    /// for a cast. `None` takes every credit.
    pub billed_within: Option<u32>,
}

/// A column whose distinct values a library can be browsed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueField {
    Year,
    ContentRating,
    Studio,
    /// A value of the items' tracks, from every version.
    Track(TrackField),
}

/// One distinct value of a [`ValueField`].
///
/// Years are listed newest first, because that is what someone browsing by year wants to
/// see; text values in ascending byte order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Year(i32),
    Text(String),
}

impl FieldValue {
    /// The position of this value in its list. Values are distinct, so the value alone
    /// locates it and the id is unused.
    pub fn cursor(&self) -> Cursor {
        match self {
            Self::Year(year) => Cursor::new(CursorKey::Int(i64::from(*year)), 0),
            Self::Text(text) => Cursor::new(CursorKey::Text(text.clone()), 0),
        }
    }
}

impl Tag {
    /// The position of this tag in a list ordered by name. Names repeat — two directors can
    /// share one — so the id breaks the tie.
    pub fn name_cursor(&self) -> Cursor {
        Cursor::new(CursorKey::Text(self.name.clone()), self.id.get())
    }

    /// The position of this tag in a list in the order tags were recorded, which is their
    /// ids'.
    pub const fn recorded_cursor(&self) -> Cursor {
        Cursor::by_id(self.id.get())
    }
}

/// A tag as attached to one item.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemTag {
    pub item_id: RatingKey,
    pub tag: Tag,
    /// The character an actor plays. Never set for any other kind.
    pub role: Option<String>,
}

/// A credit counted across the episodes below a series or a season.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeCredit {
    /// The series or season the episodes belong to.
    pub parent: RatingKey,
    pub tag: Tag,
    /// How many of those episodes carry the credit.
    pub episodes: u32,
}

/// A credit as an item presents it, its own or gathered from its episodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Credit {
    pub tag: Tag,
    pub role: Option<String>,
    /// How many episodes below the item carry it. `None` on an item with no episodes.
    pub episodes: Option<u32>,
}

/// An item's own credits followed by the ones only its episodes carry.
///
/// The item's own come first, in billing order: a series' regular cast is what its page
/// leads with. Then everyone else credited on an episode, most episodes first — the
/// director of twenty-five episodes before the guest of one. A credit present in both keeps
/// its own position and gains the episode count.
///
/// A guest's character is not carried over: it varies from one episode to the next, and
/// any one choice would misdescribe the others.
pub fn credits_with_episodes(own: Vec<ItemTag>, below: &[EpisodeCredit]) -> Vec<Credit> {
    let count = |tag: &Tag| {
        below
            .iter()
            .find(|credit| credit.tag.id == tag.id)
            .map(|credit| credit.episodes)
    };
    let mut credits: Vec<Credit> = own
        .into_iter()
        .map(|attached| Credit {
            episodes: Some(count(&attached.tag).unwrap_or(0)),
            tag: attached.tag,
            role: attached.role,
        })
        .collect();
    for credit in below {
        if !credits.iter().any(|known| known.tag.id == credit.tag.id) {
            credits.push(Credit {
                tag: credit.tag.clone(),
                role: None,
                episodes: Some(credit.episodes),
            });
        }
    }
    credits
}

/// An identifier the item has at an external metadata provider: `imdb` / `tt0293662`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalId {
    pub item_id: RatingKey,
    pub source: String,
    pub value: String,
}

/// Who a rating speaks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatingKind {
    Critic,
    Audience,
}

/// A score an external provider gives the item.
#[derive(Debug, Clone, PartialEq)]
pub struct Rating {
    pub item_id: RatingKey,
    /// The provider: `imdb`, `rottentomatoes`, `themoviedb`.
    pub source: String,
    pub kind: RatingKind,
    /// Out of 10, as the provider states it.
    pub value: f64,
    /// The provider's badge for this score, `rottentomatoes://image.rating.ripe` — which
    /// for Rotten Tomatoes also says whether the score is fresh or rotten.
    pub image: String,
}

/// Which tag a filter names: by identifier, or by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagRef {
    Id(TagId),
    /// Matched exactly, ignoring case.
    Name(String),
}

/// Keeps items carrying at least one of the listed tags of one kind.
///
/// Several conditions combine with `AND`, the values inside one with `OR`: "a comedy *and* a
/// drama" is two conditions, "a comedy *or* a drama" is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCondition {
    pub kind: TagKind,
    pub any_of: Vec<TagRef>,
}

impl TagCondition {
    /// How many tags one condition may name.
    ///
    /// The list comes from the client and each value becomes a bound parameter, so it needs
    /// a ceiling like any other client-sized input. Fifty is far past what a filter UI
    /// builds and far below what an engine's parameter limit tolerates.
    pub const MAX_VALUES: usize = 50;
}

/// A value a track carries, which an item holds when any track of any of its versions does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackField {
    VideoCodec,
    AudioCodec,
    /// The channel layout, as the analysis names it: `5.1(side)`, `stereo`.
    AudioLayout,
    SubtitleCodec,
}

impl TrackField {
    /// The kind of track the value is read from.
    pub const fn kind(self) -> StreamKind {
        match self {
            Self::VideoCodec => StreamKind::Video,
            Self::AudioCodec | Self::AudioLayout => StreamKind::Audio,
            Self::SubtitleCodec => StreamKind::Subtitle,
        }
    }

    /// The value a track holds, if it holds one.
    pub fn of(self, track: &MediaStream) -> Option<&str> {
        if track.kind != self.kind() {
            return None;
        }
        match self {
            Self::AudioLayout => track.audio.as_ref()?.channel_layout.as_deref(),
            Self::VideoCodec | Self::AudioCodec | Self::SubtitleCodec => track.codec.as_deref(),
        }
    }
}

/// Keeps items with a track holding one of the values, or, negated, those with none.
///
/// Matched exactly, case included: the reference finds 8,854 films for `videoCodec=h264`
/// and none for `H264`. Several conditions combine with `AND`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackCondition {
    pub field: TrackField,
    pub any_of: Vec<String>,
    pub negated: bool,
}

impl TrackCondition {
    /// Whether an item whose tracks are these satisfies the condition.
    pub fn holds<'a>(&self, tracks: impl IntoIterator<Item = &'a MediaStream>) -> bool {
        let found = tracks.into_iter().any(|track| {
            self.field
                .of(track)
                .is_some_and(|value| self.any_of.iter().any(|wanted| wanted == value))
        });
        found != self.negated
    }
}

/// Whose credits a [`ItemFilter::credited`] filter follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditRef {
    /// One tag: a genre, or a person in one role.
    Tag(TagId),
    /// Every tag naming one person, whatever the role.
    Person(PersonId),
}

/// One of a person's credits, with the libraries holding an item that carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonCredit {
    pub tag: Tag,
    /// Each library once, in no particular order. Empty for a tag nothing carries any more.
    pub sections: Vec<SectionId>,
}

/// How a listing is ordered.
///
/// An item with no value sorts before every item with one — first ascending, last
/// descending — as on the reference server, whose engine orders `NULL` that way. Items with
/// equal values follow in title order, ascending whichever way the listing runs: also the
/// reference's rule, and what keeps 5,434 unrated films readable at the end of a list by
/// rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemSort {
    /// By sort title, falling back to the title when there is none.
    #[default]
    Title,
    /// By when the item was added to the library.
    AddedAt,
    /// By when the item was added, ties in the order items arrived — by id — rather than by
    /// title: the reference's cross-library "recently added", where 62 of 62 ties follow it.
    Arrival,
    /// By release date.
    ReleasedOn,
    /// By critics' rating.
    Rating,
    /// By audience rating.
    AudienceRating,
    /// By the running time of the item's versions: an item ranks by its shortest one
    /// ascending and by its longest descending, as an item reached through each of its
    /// versions does on the reference.
    Duration,
    /// By the frame height of the item's versions, the same way.
    Height,
    /// By when this viewer last played it. `None`, a caller with no watch state, has
    /// played nothing.
    LastViewedAt(Option<UserId>),
    /// By this viewer's rating of it. `None`, a caller with no watch state, has rated
    /// nothing.
    UserRating(Option<UserId>),
    /// By when a series' episodes were added: its first ascending, its latest descending,
    /// the way [`ItemSort::Duration`] ranks versions.
    EpisodeAddedAt,
    /// Episodes by series, season and episode: the series' sort title, the season's number,
    /// the episode's, its release date — each absent one last — then its own sort title.
    /// Anything that is not an episode follows its own title. The reference's default for
    /// episodes, which its `Meta` states as
    /// `show.titleSort,season.index:nullsLast,episode.index:nullsLast,`
    /// `episode.originallyAvailableAt:nullsLast,episode.titleSort,episode.id`.
    Episodes,
    /// A shuffle: the same seed gives the same order, which lets pages agree.
    Shuffled(u32),
    /// Library by library, in the order [`ItemQuery::sections`] lists them; within one, films
    /// by title and series in the order they arrived, by id. The order of a person's works on
    /// the reference, measured over six people and eight libraries: its series follow their
    /// ids whatever their titles.
    Filmography,
}

/// Narrows a listing to a subset of a library.
///
/// Every field is an `AND`: a filter with a kind and a year range matches items satisfying
/// both. Absent fields place no constraint. An item with no value fails a predicate on it: a
/// film with no year is in no year range.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemFilter {
    /// Restricts to one kind. `None` lists every kind the library holds.
    pub kind: Option<ItemKind>,
    /// Set when the caller asked for a kind of item this server holds none of, music: then
    /// nothing matches, whatever the other fields say. A flag beside `kind` rather than more
    /// kinds, because an item of a kind that cannot be stored must not be representable.
    pub kind_not_held: bool,
    /// How deep into the library's hierarchy to look.
    pub depth: ItemDepth,
    /// Earliest release year, inclusive.
    pub year_from: Option<i32>,
    /// Earliest release, inclusive, in seconds since the Unix epoch; a release date counts as
    /// its midnight UTC.
    pub released_from: Option<i64>,
    /// Lowest audience rating, inclusive, on its ten-point scale.
    pub audience_rating_from: Option<f64>,
    /// Latest release year, inclusive.
    pub year_to: Option<i32>,
    /// Exact age-rating label, as the metadata source expresses it.
    pub content_rating: Option<String>,
    /// Keeps only items whose studio *contains* this text, case-insensitively.
    ///
    /// A substring and not an exact name, because that is what the reference server does:
    /// `studio=Films` matches 992 of 9,667 films, which is every studio with "Films"
    /// anywhere in it, not the 14 whose name starts with it. Folding is ASCII-only.
    ///
    /// The one filter besides `min_width` that cannot use an index: a leading wildcard has
    /// nothing to seek to. It is applied after the indexed section predicate, so the scan is
    /// bounded by the section rather than by the table.
    pub studio_contains: Option<String>,
    /// Keeps only items having a word that starts with this text, in their title, sort
    /// title or original title. Accents are folded.
    ///
    /// A word prefix and not a column prefix, because that is what the reference server
    /// does: `le` finds "Le Nouveau Jouet" (whose sort title opens with `[CAM]`) and
    /// "Sans Laisser de Traces" alike. Served by a word index, not by the title column —
    /// see `search` for the tokenizer the index and this filter must share.
    pub title_word_prefix: Option<String>,
    /// Keeps only items whose title this text completes: it begins a word of the title, read
    /// as one phrase, case aside. The rule is [`crate::completion::completes`]; unlike
    /// `title_word_prefix` it reads the title alone and minds accents, as the reference's
    /// autocompletion does.
    pub title_completion: Option<String>,
    /// Keeps only items with a version whose frame falls in this range.
    ///
    /// A resolution filter is a property of the file rather than the item, so it is a join
    /// rather than a column — which is why it is the one filter that can slow a listing.
    pub frame: Option<FrameRange>,
    /// Keeps only items whose own tracks satisfy each condition: a series has none, its
    /// episodes' being theirs, as on the reference.
    pub tracks: Vec<TrackCondition>,
    /// Keeps only these items. Empty places no constraint.
    pub ids: Vec<RatingKey>,
    /// Keeps only items carrying the named tags. Each condition must hold.
    pub tags: Vec<TagCondition>,
    /// Keeps only items in the named collections. Each condition must hold.
    pub collections: Vec<crate::collection::CollectionCondition>,
    /// Keeps only items crediting this tag or person themselves or on an item below them: a
    /// series counts for an actor who played in one of its episodes, as on the reference's
    /// person pages, where a tag filter (`tags`) reads only the item's own.
    pub credited: Option<CreditRef>,
    /// Leaves this one item out: "the rest of this director's films".
    pub excluded: Option<RatingKey>,
    /// Keeps only items the viewer watched to the end at least once (`true`), or never
    /// (`false`). A series counts its own viewings, not its episodes': one whose watched
    /// episode was unmarked since stays watched, as on the reference.
    pub watched: Option<bool>,
    /// Keeps only items with a position kept (`true`), or without (`false`).
    pub in_progress: Option<bool>,
    /// Keeps only items every episode of which is watched (`true`), or not (`false`). An
    /// item with no episode has none unwatched.
    pub episodes_watched: Option<bool>,
    /// Keeps only items with an episode watched or in progress (`true`), or with none
    /// (`false`): the series a viewer has not started.
    pub episodes_started: Option<bool>,
    /// Keeps only items the viewer last played at or after this time, in seconds since the
    /// Unix epoch. An item never played is in no range.
    pub last_viewed_from: Option<i64>,
    /// Keeps only items the viewer last played at or before this time.
    pub last_viewed_to: Option<i64>,
    /// Keeps only items the viewer rated, or did not rate, as the condition says.
    pub user_rating: Option<crate::user_rating::RatingCondition>,
    /// Whose watch state the filters above read. `None`, a caller with no watch state, has
    /// watched nothing and has nothing in progress.
    pub viewer: Option<UserId>,
}

/// A frame size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: i32,
    pub height: i32,
}

/// The frames a resolution filter keeps.
///
/// Two bounds, because that is the shape of a resolution class: a frame reaches a class by
/// either dimension — 1920×800 is as much 1080p as 1440×1080 — and leaves it by reaching
/// the next one. Which sizes the classes have is presentation, chosen by each binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRange {
    /// Kept when at least this wide or at least this tall.
    pub reaching: Option<FrameSize>,
    /// Kept when narrower and shorter than this.
    pub short_of: Option<FrameSize>,
}

impl FrameRange {
    /// Whether a frame of this size is in the range. A frame of unknown size is in none.
    pub fn contains(&self, width: Option<i32>, height: Option<i32>) -> bool {
        let (Some(width), Some(height)) = (width, height) else {
            return false;
        };
        self.reaching
            .is_none_or(|bound| width >= bound.width || height >= bound.height)
            && self
                .short_of
                .is_none_or(|bound| width < bound.width && height < bound.height)
    }
}

impl ItemFilter {
    /// Whether anything here reaches past the item table.
    pub fn needs_media(&self) -> bool {
        self.frame.is_some() || !self.tracks.is_empty()
    }

    /// Whether to restrict the listing to items with no parent.
    ///
    /// Naming a kind overrides the depth, because it is already a statement about depth: a
    /// caller asking for episodes wants episodes, and answering nothing because episodes
    /// have parents would be obtuse.
    pub const fn top_level_only(&self) -> bool {
        matches!(self.depth, ItemDepth::TopLevel) && self.kind.is_none()
    }

    /// Whether to restrict the listing to items with nothing below them.
    pub const fn leaves_only(&self) -> bool {
        matches!(self.depth, ItemDepth::Leaves)
    }
}

/// How deep into a library's hierarchy a listing reaches.
///
/// A library of series holds three generations in one table — 1,980 shows, 5,157 seasons and
/// 79,518 episodes in the reference library — and listing all of them flat is almost never
/// what a caller wants. The top level is what a client puts on a library's front page, and
/// the rest is reached by navigating into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemDepth {
    /// Items with no parent: the films of a film library, the series of a series library.
    #[default]
    TopLevel,
    /// Every item in the library, at any depth.
    All,
    /// Items that have nothing below them: the films of a film library, the episodes of a
    /// series library. Unlike the top level, a named kind narrows this rather than replacing
    /// it, so asking for the leaves' series finds none — as the reference answers
    /// `/library/sections/{id}/allLeaves?type=2`.
    Leaves,
}

/// One letter of a library's A–Z jump bar, with how many items fall under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstCharacter {
    /// The bucket: an uppercase letter, or `#` for everything that does not start with one.
    pub character: String,
    pub count: u64,
}

/// Which bucket a sort key falls into.
///
/// Folded first, so `Çélèbre` lands under `C` — matching the reference server, which reports
/// 594 films under `C` where a raw first character gives 593. Ligatures are not folded and so
/// get their own bucket, which is also what the reference does: `Œ` is a letter there, with
/// two films under it.
///
/// Everything that does not start with a letter — digits, `#`, `[` — collapses into `#`, the
/// single bucket the reference opens its list with.
pub fn first_character_bucket(sort_key: &str) -> String {
    let folded = crate::search::fold(sort_key);
    match folded.chars().next() {
        Some(first) if first.is_alphabetic() => first.to_uppercase().collect(),
        _ => "#".to_owned(),
    }
}

/// What a title-ordered listing sorts an item by: its sort title, else its title, lowercased.
///
/// Stored beside the titles so one index serves ordering and prefix search; every write of a
/// title goes through this so the column cannot disagree with them.
pub fn sort_key(title_sort: Option<&str>, title: Option<&str>) -> String {
    title_sort.or(title).unwrap_or_default().to_lowercase()
}

/// Which items a listing covers, and in what order.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemQuery {
    /// The libraries listed: one for a library's own listing, several for what a viewer's
    /// libraries of a kind added lately. None lists nothing.
    pub sections: Vec<SectionId>,
    pub filter: ItemFilter,
    pub sort: ItemSort,
    pub direction: Direction,
}

impl ItemQuery {
    pub fn new(section_id: SectionId) -> Self {
        Self::across(vec![section_id])
    }

    pub fn across(sections: Vec<SectionId>) -> Self {
        Self {
            sections,
            filter: ItemFilter::default(),
            sort: ItemSort::Title,
            direction: Direction::Ascending,
        }
    }

    #[must_use]
    pub fn of_kind(mut self, kind: ItemKind) -> Self {
        self.filter.kind = Some(kind);
        self
    }

    #[must_use]
    pub const fn sorted_by(mut self, sort: ItemSort, direction: Direction) -> Self {
        self.sort = sort;
        self.direction = direction;
        self
    }

    /// Sets the title prefix, treating blank input as no filter.
    ///
    /// An empty search box must not become a constraint that matches everything.
    #[must_use]
    pub fn matching(mut self, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        self.filter.title_word_prefix = (!prefix.trim().is_empty()).then_some(prefix);
        self
    }

    #[must_use]
    pub fn filtered(mut self, filter: ItemFilter) -> Self {
        self.filter = filter;
        self
    }
}

/// An item with the cursor locating it in the listing it came from.
///
/// Built by storage, because only storage has the ordering value at hand: a version's height
/// or a viewer's last view is not a field of the item.
#[derive(Debug, Clone)]
pub struct Listed {
    pub item: MetadataItem,
    pub cursor: Cursor,
}

/// A search across every library.
///
/// Separate from [`ItemQuery`], which is always rooted in one library: a global search is a
/// different question and would otherwise mean an optional section id that every caller has
/// to think about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Matches items having a word beginning with each word of this text. See
    /// [`crate::search`].
    pub word_prefix: String,
    /// Restricts to these kinds. Empty searches every kind.
    pub kinds: Vec<ItemKind>,
    /// Restricts to these sections, the ones a caller may see. `None` searches them all.
    pub sections: Option<Vec<crate::ids::SectionId>>,
    pub order: SearchOrder,
    /// Also matches an episode through its series' titles, as the reference's search shelves
    /// match every episode of *Kaamelott* for `kaamelott` (its global `/search` does not).
    /// Such an episode ranks as a match anywhere.
    pub episodes_by_series: bool,
}

/// How a search orders what it matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchOrder {
    /// The best match first, by [`crate::search::score`]: its rung, then its audience
    /// rating, then its sort title and id, so that equal scores still have a total order.
    #[default]
    Relevance,
    /// By sort title, then id.
    Title,
}

impl SearchQuery {
    pub fn new(word_prefix: impl Into<String>) -> Self {
        Self {
            word_prefix: word_prefix.into(),
            kinds: Vec::new(),
            sections: None,
            order: SearchOrder::default(),
            episodes_by_series: false,
        }
    }

    /// Whether the search has anything to match.
    ///
    /// A blank term is not an error and not a match-everything: a client clearing its search
    /// box must get nothing back rather than the whole server.
    pub fn is_blank(&self) -> bool {
        crate::search::fold(&self.word_prefix).trim().is_empty()
    }

    /// How well an item answers the term.
    pub fn rung(&self, item: &MetadataItem) -> crate::search::Rung {
        crate::search::RankedTerm::new(&self.word_prefix).map_or(
            crate::search::Rung::Anywhere,
            |term| {
                term.rung(&crate::search::title_keys(
                    item.title.as_deref(),
                    item.title_sort.as_deref(),
                    item.original_title.as_deref(),
                ))
            },
        )
    }

    /// The position of an item in this search's order.
    ///
    /// By relevance, the score in two exact parts rather than one float: hundredths, which
    /// the rung and the kind make, then the audience rating as stored (absent counting as
    /// zero, as it does in the score). An exact cursor is what lets a page resume on equality.
    pub fn cursor(&self, item: &MetadataItem) -> Cursor {
        let sort_key = CursorKey::Text(sort_key(item.title_sort.as_deref(), item.title.as_deref()));
        match self.order {
            SearchOrder::Title => Cursor::new(sort_key, item.id.get()),
            SearchOrder::Relevance => Cursor {
                keys: vec![
                    CursorKey::Int(crate::search::hundredths(item.kind, self.rung(item))),
                    CursorKey::Real(item.audience_rating.unwrap_or(0.0)),
                    sort_key,
                ],
                id: item.id.get(),
            },
        }
    }
}

/// A search over tags by name, across every library: the people and genres a search box
/// offers beside the items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSearch {
    /// Matches tags having a word beginning with each word of this text, as [`SearchQuery`]
    /// matches titles.
    pub word_prefix: String,
    /// Restricts to these kinds. Empty searches every kind.
    pub kinds: Vec<TagKind>,
    /// Restricts to the items of these sections, the ones a caller may see. `None` counts
    /// them all.
    pub sections: Option<Vec<crate::ids::SectionId>>,
}

impl TagSearch {
    /// Whether the search has anything to match, by the rule of [`SearchQuery::is_blank`].
    pub fn is_blank(&self) -> bool {
        crate::search::fold(&self.word_prefix).trim().is_empty()
    }
}

/// A tag as one library's items carry it.
///
/// One per library rather than per tag: what a client offers is "Tom Hanks in the films",
/// which opens that library's listing filtered on him. So a library is listed only when one
/// of its films or series carries the tag: a guest credited on episodes alone would lead to
/// an empty listing, and the reference leaves such a library out too.
#[derive(Debug, Clone, PartialEq)]
pub struct TagUse {
    pub tag: Tag,
    pub section: crate::ids::SectionId,
    /// How many of the library's items carry the tag, whatever their kind: a series library
    /// counts the series and the episodes crediting someone.
    pub item_count: u64,
}

impl TagUse {
    /// The position of this use in a search, which orders by item count, most first, then
    /// by tag and library.
    pub fn cursor(&self) -> Cursor {
        Cursor {
            keys: vec![
                CursorKey::Int(i64::try_from(self.item_count).unwrap_or(i64::MAX)),
                CursorKey::Int(self.tag.id.get()),
            ],
            id: self.section.get(),
        }
    }
}

/// Storage port for library contents.
///
/// Split from `LibraryRepository` because they are different aggregates: one is a handful of
/// rows describing structure, the other is the hundreds of thousands of rows inside it.
#[async_trait::async_trait]
pub trait MetadataRepository: Send + Sync + std::fmt::Debug {
    /// One page of a library's items.
    async fn items(&self, query: &ItemQuery, window: Window)
        -> Result<Page<Listed>, MetadataError>;

    /// How many items the query matches.
    async fn item_count(&self, query: &ItemQuery) -> Result<u64, MetadataError>;

    /// One page of a search across every library, in [`SearchQuery::order`]; a page's
    /// cursors are [`SearchQuery::cursor`].
    async fn search(
        &self,
        query: &SearchQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError>;

    /// One page of the items whose title is closest to a term, closest first, then by the
    /// title as compared, then by id. See [`crate::closest`] for the measure.
    async fn closest_titles(
        &self,
        query: &crate::closest::ClosestTitles,
        window: Window,
    ) -> Result<Page<crate::closest::TitleMatch>, MetadataError>;

    /// One page of the tags of one kind carried by a query's items whose name `text`
    /// completes, by [`crate::completion::completes`], ordered by name as
    /// [`MetadataRepository::tags_in`] orders them.
    async fn tags_completing(
        &self,
        query: &ItemQuery,
        scope: TagScope,
        text: &str,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError>;

    /// One page of the tags whose name matches a search, each with the libraries carrying it.
    ///
    /// Ordered by item count, most first, then by tag and library: a common name matches
    /// thousands of people, and the ones a library is full of are the ones searched for.
    async fn search_tags(
        &self,
        query: &TagSearch,
        window: Window,
    ) -> Result<Page<TagUse>, MetadataError>;

    /// The first-letter buckets of a query's items, with their counts.
    ///
    /// One row per letter rather than per item: the A–Z bar a client draws beside a long list
    /// needs 28 numbers, and paging 9,667 items to count them would be absurd.
    async fn first_characters(
        &self,
        query: &ItemQuery,
    ) -> Result<Vec<FirstCharacter>, MetadataError>;

    /// One page of the tags of one kind carried by a query's items, ordered by name.
    ///
    /// What a client offers as the choices of a tag filter: only the genres a library's
    /// films actually have, not every genre the server knows.
    async fn tags_in(
        &self,
        query: &ItemQuery,
        scope: TagScope,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError>;

    /// How many distinct tags [`MetadataRepository::tags_in`] would list.
    async fn tag_count_in(&self, query: &ItemQuery, scope: TagScope) -> Result<u64, MetadataError>;

    /// One page of the tags of one kind carried by any item of these libraries, at any
    /// depth, in the order they were recorded.
    ///
    /// Apart from [`MetadataRepository::tags_in`] because it spans whole libraries: asked
    /// tag by tag whether something there carries it, the 240,000 actors of the reference
    /// library page in milliseconds, where gathering every item first takes a second.
    async fn tags_in_libraries(
        &self,
        sections: &[crate::ids::SectionId],
        kind: TagKind,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError>;

    /// How many tags [`MetadataRepository::tags_in_libraries`] would list.
    async fn tag_count_in_libraries(
        &self,
        sections: &[crate::ids::SectionId],
        kind: TagKind,
    ) -> Result<u64, MetadataError>;

    /// One page of the distinct values a query's items hold in a field, absent ones left out.
    async fn values_in(
        &self,
        query: &ItemQuery,
        field: ValueField,
        window: Window,
    ) -> Result<Page<FieldValue>, MetadataError>;

    /// How many distinct values [`MetadataRepository::values_in`] would list.
    async fn value_count_in(
        &self,
        query: &ItemQuery,
        field: ValueField,
    ) -> Result<u64, MetadataError>;

    async fn item(&self, id: RatingKey) -> Result<MetadataItem, MetadataError>;

    /// Several items at once, for the loaders that resolve a parent from a child.
    ///
    /// Items that do not exist are absent from the result rather than an error: a loader asks
    /// about ids it read from other rows, and a dangling reference is a data problem to
    /// report, not a request to reject.
    async fn items_by_id(&self, ids: &[RatingKey]) -> Result<Vec<MetadataItem>, MetadataError>;

    /// One page of a parent's children, ordered by their position.
    ///
    /// Not batched across parents, unlike the loaders below: each parent carries its own
    /// window, and a window cannot be shared. Listing fifty shows and each one's seasons is
    /// therefore fifty queries — an unusual shape, since a client browsing a series fetches
    /// one.
    async fn children(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError>;

    /// One page of a parent's grandchildren — every episode of a series, across its
    /// seasons — ordered by the child's position, then their own.
    ///
    /// A separate port rather than `children` applied twice: a client opening a series
    /// wants its episodes as one list, and walking each season in turn would cost a query
    /// per season and leave the page boundary to the caller.
    async fn grandchildren(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError>;

    /// How many direct children each parent has.
    async fn child_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError>;

    /// How many descendants two levels down each parent has — a show's episodes.
    async fn leaf_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError>;

    async fn versions_for(&self, items: &[RatingKey]) -> Result<Vec<MediaVersion>, MetadataError>;

    /// The files of these versions, an item's or an extra's.
    async fn files_for(&self, versions: &[MediaId]) -> Result<Vec<MediaFile>, MetadataError>;

    /// The tracks of these files, an item's or an extra's.
    async fn streams_for(&self, files: &[PartId]) -> Result<Vec<MediaStream>, MetadataError>;

    /// One encoding by id, an item's or an extra's, for a Relay refetch.
    async fn version(&self, id: MediaId) -> Result<Option<MediaVersion>, MetadataError>;

    /// One file by id, for a Relay refetch and for resolving a path before playback.
    async fn file(&self, id: PartId) -> Result<Option<MediaFile>, MetadataError>;

    /// One track by id, for a Relay refetch.
    async fn stream(&self, id: StreamId) -> Result<Option<MediaStream>, MetadataError>;

    /// Forgets a track: a sidecar subtitle whose file is deleted. Whether there was one.
    async fn delete_stream(&self, id: StreamId) -> Result<bool, MetadataError>;

    /// Every tag of several items, in billing order within each item and kind.
    ///
    /// The order *across* kinds is unspecified: a consumer groups by kind, and promising one
    /// would constrain storage for nothing.
    ///
    /// All kinds at once: an item page shows genres, cast and crew together, and one query
    /// for the six is cheaper than six for one each.
    async fn tags_for(&self, items: &[RatingKey]) -> Result<Vec<ItemTag>, MetadataError>;

    /// The credits carried by the episodes below several series or seasons, each counted
    /// once per episode, most frequent first within a parent.
    ///
    /// Genres and countries are not counted: no episode carries any.
    async fn credits_below(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<EpisodeCredit>, MetadataError>;

    /// The external identifiers of several items, in the order the source lists them.
    async fn external_ids_for(&self, items: &[RatingKey])
        -> Result<Vec<ExternalId>, MetadataError>;

    /// The ratings of several items, in the order the source lists them.
    async fn ratings_for(&self, items: &[RatingKey]) -> Result<Vec<Rating>, MetadataError>;

    /// One tag by id, for a Relay refetch.
    async fn tag(&self, id: TagId) -> Result<Option<Tag>, MetadataError>;

    /// One person by id, for a Relay refetch.
    async fn person(&self, id: PersonId) -> Result<Option<Person>, MetadataError>;

    /// Every tag naming several people, by tag id, with the libraries where each is carried.
    ///
    /// The libraries come with the tags so that a caller can keep the credits its viewer may
    /// see without a query per tag: a person has a handful of tags, each in a few libraries.
    async fn credits_of(&self, people: &[PersonId]) -> Result<Vec<PersonCredit>, MetadataError>;

    /// Up to `limit` episodes of a library by when they were added, newest first, starting
    /// strictly after `after` — the `(added_at, episode)` of the last one already read.
    ///
    /// Keyset rather than offset: grouping reads page after page, and an offset would make
    /// each page cost every row before it.
    async fn episode_additions(
        &self,
        section: SectionId,
        after: Option<(i64, RatingKey)>,
        limit: u32,
    ) -> Result<Vec<crate::recent::EpisodeAddition>, MetadataError>;

    /// Everything below a series or a season: its seasons and episodes.
    ///
    /// Unwindowed, and bounded by the series: marking a series watched has to reach every
    /// episode, and finding the next one has to see them all — 750 for the longest here.
    async fn descendants(&self, item: RatingKey) -> Result<Vec<MetadataItem>, MetadataError>;

    /// Every copy of these works, in the libraries given, or in every library.
    ///
    /// Unwindowed: bounded by the works asked for, a handful of copies each.
    async fn items_by_guid(
        &self,
        guids: &[String],
        sections: Option<&[SectionId]>,
    ) -> Result<Vec<MetadataItem>, MetadataError>;

    /// The items of a library, of one kind, holding any of these tags, with how many.
    ///
    /// Unwindowed on purpose, and bounded by the library: a score has to see every candidate
    /// before it can say which come first.
    async fn items_sharing_tags(
        &self,
        section: SectionId,
        kind: ItemKind,
        tags: &[TagId],
    ) -> Result<Vec<crate::similar::TagOverlap>, MetadataError>;

    /// The fields locked on these items, in no particular order.
    async fn locked_fields(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<(RatingKey, ItemField)>, MetadataError>;

    /// The backdrop colors of those of these items that have them, in no particular order.
    async fn corner_colors(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<(RatingKey, crate::backdrop::CornerColors)>, MetadataError>;

    /// Applies an edit to an item, with everything it derives, in one transaction.
    ///
    /// `ItemNotFound` for an item that is not of the kind the edit was checked against.
    async fn edit_item(&self, id: RatingKey, edit: &ItemEdit) -> Result<Edited, MetadataError>;

    /// The markers on these items that `viewer` sees — the works' own, and its bookmarks — in
    /// the order they were added.
    ///
    /// Unwindowed: bounded by the items, a handful of markers each.
    async fn markers(
        &self,
        items: &[RatingKey],
        viewer: Option<UserId>,
    ) -> Result<Vec<Marker>, MetadataError>;

    /// One marker, whoever owns it.
    async fn marker(&self, id: MarkerId) -> Result<Option<Marker>, MetadataError>;

    /// Adds a marker, unless one it [duplicates](NewMarker::duplicates) is there already:
    /// the marker either way, and whether it was added.
    async fn add_marker(&self, marker: &NewMarker) -> Result<(Marker, bool), MetadataError>;

    /// Writes a marker's values, its item, kind and owner left as they are. `None` when it no
    /// longer exists.
    async fn update_marker(&self, marker: &Marker) -> Result<Option<Marker>, MetadataError>;

    /// Removes a marker; false when there was none.
    async fn delete_marker(&self, id: MarkerId) -> Result<bool, MetadataError>;

    /// The chapters of these items, each item's in order.
    ///
    /// Unwindowed: bounded by the items, a few dozen chapters each.
    async fn chapters(&self, items: &[RatingKey]) -> Result<Vec<Chapter>, MetadataError>;

    /// One chapter.
    async fn chapter(&self, id: ChapterId) -> Result<Option<Chapter>, MetadataError>;

    /// What every item a query keeps has in common: fields, tags, collections and locks.
    ///
    /// Aggregated where the items are rather than read and compared, because the set can be
    /// a whole library: what a client edits when every item is selected.
    async fn common_fields(
        &self,
        query: &ItemQuery,
    ) -> Result<crate::common::CommonFields, MetadataError>;

    /// The extras of these items, each item's by position then id, an extra without a
    /// position first.
    ///
    /// Unwindowed: bounded by the items, 50 extras at most on one of the reference's.
    async fn extras(&self, items: &[RatingKey]) -> Result<Vec<Extra>, MetadataError>;

    /// One extra, for a Relay refetch.
    async fn extra(&self, id: ExtraId) -> Result<Option<Extra>, MetadataError>;

    /// The versions of these extras, each extra's by id. Their files and tracks come through
    /// [`files_for`](Self::files_for) and [`streams_for`](Self::streams_for), as an item's do.
    async fn extra_versions(
        &self,
        extras: &[ExtraId],
    ) -> Result<Vec<(ExtraId, MediaVersion)>, MetadataError>;
}

/// An item after an edit, with its locks.
#[derive(Debug, Clone)]
pub struct Edited {
    pub item: MetadataItem,
    pub locked: Vec<ItemField>,
}

/// Lets an `Arc<dyn MetadataRepository>` be used wherever a repository is expected, so the
/// bindings can hold a trait object instead of being generic over the storage type.
#[async_trait::async_trait]
impl<T: MetadataRepository + ?Sized> MetadataRepository for std::sync::Arc<T> {
    async fn items(
        &self,
        query: &ItemQuery,
        window: Window,
    ) -> Result<Page<Listed>, MetadataError> {
        (**self).items(query, window).await
    }

    async fn item_count(&self, query: &ItemQuery) -> Result<u64, MetadataError> {
        (**self).item_count(query).await
    }

    async fn search(
        &self,
        query: &SearchQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        (**self).search(query, window).await
    }

    async fn closest_titles(
        &self,
        query: &crate::closest::ClosestTitles,
        window: Window,
    ) -> Result<Page<crate::closest::TitleMatch>, MetadataError> {
        (**self).closest_titles(query, window).await
    }

    async fn tags_completing(
        &self,
        query: &ItemQuery,
        scope: TagScope,
        text: &str,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError> {
        (**self).tags_completing(query, scope, text, window).await
    }

    async fn search_tags(
        &self,
        query: &TagSearch,
        window: Window,
    ) -> Result<Page<TagUse>, MetadataError> {
        (**self).search_tags(query, window).await
    }

    async fn first_characters(
        &self,
        query: &ItemQuery,
    ) -> Result<Vec<FirstCharacter>, MetadataError> {
        (**self).first_characters(query).await
    }

    async fn tags_in(
        &self,
        query: &ItemQuery,
        scope: TagScope,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError> {
        (**self).tags_in(query, scope, window).await
    }

    async fn tag_count_in(&self, query: &ItemQuery, scope: TagScope) -> Result<u64, MetadataError> {
        (**self).tag_count_in(query, scope).await
    }

    async fn tags_in_libraries(
        &self,
        sections: &[crate::ids::SectionId],
        kind: TagKind,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError> {
        (**self).tags_in_libraries(sections, kind, window).await
    }

    async fn tag_count_in_libraries(
        &self,
        sections: &[crate::ids::SectionId],
        kind: TagKind,
    ) -> Result<u64, MetadataError> {
        (**self).tag_count_in_libraries(sections, kind).await
    }

    async fn values_in(
        &self,
        query: &ItemQuery,
        field: ValueField,
        window: Window,
    ) -> Result<Page<FieldValue>, MetadataError> {
        (**self).values_in(query, field, window).await
    }

    async fn value_count_in(
        &self,
        query: &ItemQuery,
        field: ValueField,
    ) -> Result<u64, MetadataError> {
        (**self).value_count_in(query, field).await
    }

    async fn descendants(&self, item: RatingKey) -> Result<Vec<MetadataItem>, MetadataError> {
        (**self).descendants(item).await
    }

    async fn items_by_guid(
        &self,
        guids: &[String],
        sections: Option<&[SectionId]>,
    ) -> Result<Vec<MetadataItem>, MetadataError> {
        (**self).items_by_guid(guids, sections).await
    }

    async fn item(&self, id: RatingKey) -> Result<MetadataItem, MetadataError> {
        (**self).item(id).await
    }

    async fn items_by_id(&self, ids: &[RatingKey]) -> Result<Vec<MetadataItem>, MetadataError> {
        (**self).items_by_id(ids).await
    }

    async fn children(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        (**self).children(parent, window).await
    }

    async fn grandchildren(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        (**self).grandchildren(parent, window).await
    }

    async fn child_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        (**self).child_counts(parents).await
    }

    async fn leaf_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        (**self).leaf_counts(parents).await
    }

    async fn versions_for(&self, items: &[RatingKey]) -> Result<Vec<MediaVersion>, MetadataError> {
        (**self).versions_for(items).await
    }

    async fn files_for(&self, versions: &[MediaId]) -> Result<Vec<MediaFile>, MetadataError> {
        (**self).files_for(versions).await
    }

    async fn streams_for(&self, files: &[PartId]) -> Result<Vec<MediaStream>, MetadataError> {
        (**self).streams_for(files).await
    }

    async fn version(&self, id: MediaId) -> Result<Option<MediaVersion>, MetadataError> {
        (**self).version(id).await
    }

    async fn file(&self, id: PartId) -> Result<Option<MediaFile>, MetadataError> {
        (**self).file(id).await
    }

    async fn stream(&self, id: StreamId) -> Result<Option<MediaStream>, MetadataError> {
        (**self).stream(id).await
    }

    async fn delete_stream(&self, id: StreamId) -> Result<bool, MetadataError> {
        (**self).delete_stream(id).await
    }

    async fn tags_for(&self, items: &[RatingKey]) -> Result<Vec<ItemTag>, MetadataError> {
        (**self).tags_for(items).await
    }

    async fn credits_below(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<EpisodeCredit>, MetadataError> {
        (**self).credits_below(parents).await
    }

    async fn external_ids_for(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<ExternalId>, MetadataError> {
        (**self).external_ids_for(items).await
    }

    async fn ratings_for(&self, items: &[RatingKey]) -> Result<Vec<Rating>, MetadataError> {
        (**self).ratings_for(items).await
    }

    async fn tag(&self, id: TagId) -> Result<Option<Tag>, MetadataError> {
        (**self).tag(id).await
    }

    async fn person(&self, id: PersonId) -> Result<Option<Person>, MetadataError> {
        (**self).person(id).await
    }

    async fn credits_of(&self, people: &[PersonId]) -> Result<Vec<PersonCredit>, MetadataError> {
        (**self).credits_of(people).await
    }

    async fn episode_additions(
        &self,
        section: SectionId,
        after: Option<(i64, RatingKey)>,
        limit: u32,
    ) -> Result<Vec<crate::recent::EpisodeAddition>, MetadataError> {
        (**self).episode_additions(section, after, limit).await
    }

    async fn items_sharing_tags(
        &self,
        section: SectionId,
        kind: ItemKind,
        tags: &[TagId],
    ) -> Result<Vec<crate::similar::TagOverlap>, MetadataError> {
        (**self).items_sharing_tags(section, kind, tags).await
    }

    async fn locked_fields(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<(RatingKey, ItemField)>, MetadataError> {
        (**self).locked_fields(items).await
    }

    async fn corner_colors(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<(RatingKey, crate::backdrop::CornerColors)>, MetadataError> {
        (**self).corner_colors(items).await
    }

    async fn edit_item(&self, id: RatingKey, edit: &ItemEdit) -> Result<Edited, MetadataError> {
        (**self).edit_item(id, edit).await
    }

    async fn markers(
        &self,
        items: &[RatingKey],
        viewer: Option<UserId>,
    ) -> Result<Vec<Marker>, MetadataError> {
        (**self).markers(items, viewer).await
    }

    async fn marker(&self, id: MarkerId) -> Result<Option<Marker>, MetadataError> {
        (**self).marker(id).await
    }

    async fn add_marker(&self, marker: &NewMarker) -> Result<(Marker, bool), MetadataError> {
        (**self).add_marker(marker).await
    }

    async fn update_marker(&self, marker: &Marker) -> Result<Option<Marker>, MetadataError> {
        (**self).update_marker(marker).await
    }

    async fn delete_marker(&self, id: MarkerId) -> Result<bool, MetadataError> {
        (**self).delete_marker(id).await
    }

    async fn chapters(&self, items: &[RatingKey]) -> Result<Vec<Chapter>, MetadataError> {
        (**self).chapters(items).await
    }

    async fn chapter(&self, id: ChapterId) -> Result<Option<Chapter>, MetadataError> {
        (**self).chapter(id).await
    }

    async fn common_fields(
        &self,
        query: &ItemQuery,
    ) -> Result<crate::common::CommonFields, MetadataError> {
        (**self).common_fields(query).await
    }

    async fn extras(&self, items: &[RatingKey]) -> Result<Vec<Extra>, MetadataError> {
        (**self).extras(items).await
    }

    async fn extra(&self, id: ExtraId) -> Result<Option<Extra>, MetadataError> {
        (**self).extra(id).await
    }

    async fn extra_versions(
        &self,
        extras: &[ExtraId],
    ) -> Result<Vec<(ExtraId, MediaVersion)>, MetadataError> {
        (**self).extra_versions(extras).await
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn only_movies_and_episodes_are_playable() {
        // The schema relies on this: shows and seasons expose no media field at all.
        assert!(ItemKind::Movie.is_playable());
        assert!(ItemKind::Episode.is_playable());
        assert!(!ItemKind::Show.is_playable());
        assert!(!ItemKind::Season.is_playable());
    }

    fn director(id: i64, name: &str) -> Tag {
        Tag {
            id: TagId::new(id),
            kind: TagKind::Director,
            name: name.to_owned(),
            person: None,
        }
    }

    #[test]
    fn a_series_leads_with_its_own_credits_then_its_episodes_by_count() {
        let series = RatingKey::new(32_803);
        let own = vec![ItemTag {
            item_id: series,
            tag: director(1, "Jenji Kohan"),
            role: None,
        }];
        let below = [
            EpisodeCredit {
                parent: series,
                tag: director(2, "Craig Zisk"),
                episodes: 12,
            },
            EpisodeCredit {
                parent: series,
                tag: director(1, "Jenji Kohan"),
                episodes: 3,
            },
            EpisodeCredit {
                parent: series,
                tag: director(3, "Scott Ellis"),
                episodes: 1,
            },
        ];

        let credits = credits_with_episodes(own, &below);
        let summary: Vec<(&str, Option<u32>)> = credits
            .iter()
            .map(|credit| (credit.tag.name.as_str(), credit.episodes))
            .collect();
        assert_eq!(
            summary,
            [
                ("Jenji Kohan", Some(3)),
                ("Craig Zisk", Some(12)),
                ("Scott Ellis", Some(1)),
            ]
        );
    }

    #[test]
    fn an_own_credit_no_episode_carries_counts_zero() {
        let series = RatingKey::new(32_803);
        let credits = credits_with_episodes(
            vec![ItemTag {
                item_id: series,
                tag: director(1, "Jenji Kohan"),
                role: None,
            }],
            &[],
        );
        assert_eq!(credits[0].episodes, Some(0));
    }

    #[test]
    fn a_blank_search_is_no_search() {
        // An empty box in a UI must not become a constraint that matches everything.
        assert_eq!(
            ItemQuery::new(SectionId::new(4))
                .matching("   ")
                .filter
                .title_word_prefix,
            None
        );
        assert_eq!(
            ItemQuery::new(SectionId::new(4))
                .matching("jura")
                .filter
                .title_word_prefix,
            Some("jura".to_owned())
        );
    }
}
