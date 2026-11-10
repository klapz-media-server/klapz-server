//! Response bodies for library listings and single items.
//!
//! Field order is the one a reference server emits, derived by topologically ordering the
//! attribute sequences of 288 items — films, series, seasons *and* episodes, the last two
//! fetched through `/children` and `/grandchildren` because a library listing never returns
//! them. The union of all observed orders has no cycle. XML attribute order is observable, so
//! it is part of the contract; an earlier derivation that sampled only listings got the
//! episode fields wrong twice — `grandparentTitle` after `index` instead of after `title`,
//! and the `librarySection*` trio before `grandparentKey` and `parentKey` instead of after.
//!
//! Everything is `Option` and skipped when absent, because that is how Plex distinguishes
//! "no value" from "empty": an absent attribute, never an empty string.

use serde::Serialize;

use crate::language::Language;

use crate::gql_items::{
    GqlConnection, GqlEdge, GqlItem, GqlLibraryRef, GqlPart, GqlProfiled, GqlRating, GqlStream,
    GqlTag, GqlVersion, GqlWatchState,
};
use crate::stream_labels;

/// Identifier of the metadata provider every library response claims.
///
/// A constant: clients match on it to tell a library response from a channel or a provider
/// response, and `com.plexapp.plugins.library` is the only value a server sends here.
pub const LIBRARY_IDENTIFIER: &str = "com.plexapp.plugins.library";

/// Prefix clients prepend to a tag name to build its flag image URL.
pub const MEDIA_TAG_PREFIX: &str = "/system/bundle/media/flags/";

/// Layout the web client restores for a library view. Opaque to the server.
pub const GRID_VIEW_MODE: u32 = 65_592;

/// Generation of the tag flag images, which clients append to a tag image URL to bust their
/// cache.
///
/// Zero because there is no generation to report: the flag images it versions are not
/// served, so no client can build such a URL. A timestamp here would be an invented value
/// that looks meaningful.
pub const MEDIA_TAG_VERSION: i64 = 0;

/// Response body for `GET /library/sections/{id}/all`.
///
/// `totalSize` and `offset` are always present, which is a deliberate deviation: a reference
/// server omits them (and returns the whole library in one response) unless the client sends
/// both container parameters. We always window — `page::MAX_ITEMS` is a ceiling the project
/// applies everywhere, and a library of 171,018 items is exactly what it exists for — so the
/// response has to say where the window sits. `size` then always matches the number of items
/// actually returned, rather than claiming a total the body does not contain.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemsContainer {
    pub size: u32,
    pub total_size: u64,
    pub offset: u64,
    pub allow_sync: bool,
    /// Backdrop for the library view. A static resource per media kind, not per library.
    pub art: &'static str,
    pub identifier: &'static str,
    #[serde(rename = "librarySectionID")]
    pub library_section_id: i64,
    pub library_section_title: String,
    #[serde(rename = "librarySectionUUID")]
    pub library_section_uuid: String,
    pub media_tag_prefix: &'static str,
    /// Generation of the tag images, used by clients for cache busting.
    pub media_tag_version: i64,
    pub thumb: &'static str,
    /// The library's name.
    pub title1: String,
    /// The view's name within the library.
    pub title2: String,
    /// Which layout to use: `movie`, `show`, `season` or `episode`.
    pub view_group: &'static str,
    pub view_mode: u32,
    #[serde(rename = "Meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<crate::meta::Meta>,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
}

/// Response body for `GET /library/metadata/{ids}`.
///
/// Carries no `totalSize`, `offset` or view attributes: it is a lookup by identifier, not a
/// window over a collection, and a reference server omits them here too.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemContainer {
    pub size: u32,
    pub allow_sync: bool,
    pub identifier: &'static str,
    #[serde(rename = "librarySectionID", skip_serializing_if = "Option::is_none")]
    pub library_section_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_section_title: Option<String>,
    #[serde(rename = "librarySectionUUID", skip_serializing_if = "Option::is_none")]
    pub library_section_uuid: Option<String>,
    pub media_tag_prefix: &'static str,
    pub media_tag_version: i64,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
}

/// One item in a listing or a lookup.
///
/// A single flat type for all four kinds rather than one per kind, because the wire format is
/// itself a flat union: Plex emits whichever attributes apply and omits the rest. The kind
/// survives in `kind`, which drives both the `key` suffix and the XML element name.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataEntry {
    pub rating_key: String,
    /// What to request next: the item itself for a film, its children for a series.
    pub key: String,
    /// The season of an episode, the series of a season.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_rating_key: Option<String>,
    /// The series of an episode, two levels up. Clients link an episode straight to its
    /// series without walking the season.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_rating_key: Option<String>,
    pub guid: String,
    /// The season's identity for an episode, the series' for a season.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_guid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_guid: Option<String>,
    /// The series' studio, which a season repeats: a season has none of its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_studio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio: Option<String>,
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_sort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    /// Set only on a lookup. In a listing the library is stated once on the container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_section_title: Option<String>,
    #[serde(rename = "librarySectionID", skip_serializing_if = "Option::is_none")]
    pub library_section_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_section_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_title: Option<String>,
    /// The season's display title for an episode, the series' title for a season.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating_age: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Season or episode number. Plex calls it `index`; `index` is a reserved SQL word, which
    /// is why the name stops at the wire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// The season number of an episode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience_rating: Option<f64>,
    /// How many times the caller watched it to the end; absent when never.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_count: Option<u32>,
    /// Where the caller's unfinished viewing stopped, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_offset: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_viewed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// The series' year, on a season that has none of its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    /// Runtime in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub originally_available_at: Option<String>,
    /// Episodes in the series or season.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leaf_count: Option<u64>,
    /// The caller's watched episodes; present, zero included, on every series and season.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewed_leaf_count: Option<u64>,
    /// Seasons in the series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<u64>,
    pub added_at: i64,
    pub updated_at: i64,
    #[serde(rename = "Media", skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<MediaEntry>,
    #[serde(rename = "Genre", skip_serializing_if = "Vec::is_empty")]
    pub genre: Vec<TagEntry>,
    #[serde(rename = "Country", skip_serializing_if = "Vec::is_empty")]
    pub country: Vec<TagEntry>,
    /// External identifiers, `imdb://tt0293662`. A lookup carries them, a listing does not.
    #[serde(rename = "Guid", skip_serializing_if = "Vec::is_empty")]
    pub guids: Vec<GuidEntry>,
    #[serde(rename = "Rating", skip_serializing_if = "Vec::is_empty")]
    pub ratings: Vec<RatingEntry>,
    #[serde(rename = "Director", skip_serializing_if = "Vec::is_empty")]
    pub director: Vec<TagEntry>,
    #[serde(rename = "Writer", skip_serializing_if = "Vec::is_empty")]
    pub writer: Vec<TagEntry>,
    /// The cast. Plex names the element after the credit, not the person.
    #[serde(rename = "Role", skip_serializing_if = "Vec::is_empty")]
    pub role: Vec<TagEntry>,
    #[serde(rename = "Producer", skip_serializing_if = "Vec::is_empty")]
    pub producer: Vec<TagEntry>,
}

/// An external identifier, as one `source://value` string.
#[derive(Debug, Serialize)]
pub struct GuidEntry {
    pub id: String,
}

/// A provider's score, with the badge clients draw beside it.
#[derive(Debug, Serialize)]
pub struct RatingEntry {
    pub image: String,
    pub value: f64,
    /// `critic` or `audience`.
    #[serde(rename = "type")]
    pub kind: &'static str,
}

impl RatingEntry {
    fn from_graphql(rating: &GqlRating) -> Self {
        Self {
            image: rating.image.clone(),
            // Published to one decimal, like an item's own ratings: TMDB's 5.525 comes out as
            // 5.5 on the reference.
            value: one_decimal(rating.value),
            kind: if rating.kind == "CRITIC" {
                "critic"
            } else {
                "audience"
            },
        }
    }
}

/// A genre, a country or a credit.
///
/// In a lookup every attribute that applies is present; in a listing only `tag`, because
/// that is all the reference sends there. Which form comes out depends on what the query
/// selected, so the two endpoints share this type.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// The listing filter that selects this tag, for example `genre=38`. Clients follow it
    /// as is rather than building it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    pub tag: String,
    /// The metadata provider's key for a person.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_key: Option<String>,
    /// The character an actor plays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

impl TagEntry {
    /// `field` is the listing filter's name for this kind — `actor` for a `Role` element,
    /// which is the one place the two names differ.
    fn from_graphql(tag: &GqlTag, field: &str, role: Option<&str>) -> Self {
        let id = tag.database_id();
        Self {
            id,
            filter: id.map(|id| format!("{field}={id}")),
            tag: tag.name.clone(),
            tag_key: tag
                .person
                .as_ref()
                .and_then(|person| person.provider_key.clone()),
            role: role.map(ToOwned::to_owned),
            thumb: tag
                .person
                .as_ref()
                .and_then(|person| person.thumb_url.clone()),
        }
    }

    /// A cast entry. In a lookup it always carries `role`, as the reference's does: the
    /// character's name with an ASCII first letter capitalized, or empty when the source
    /// names none.
    fn from_cast_member(member: &GqlEdge<GqlTag>) -> Self {
        let mut entry = Self::from_graphql(&member.node, "actor", None);
        if entry.id.is_some() {
            entry.role = Some(credited_character(
                member.character.as_deref().unwrap_or(""),
            ));
        }
        entry
    }

    fn all(tags: &GqlConnection<GqlTag>, field: &str) -> Vec<Self> {
        tags.nodes()
            .map(|tag| Self::from_graphql(tag, field, None))
            .collect()
    }
}

/// One encoding of an item.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaEntry {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    /// Marketing label for the frame size — `4k`, `1080`, `sd`. Derived, see
    /// [`crate::resolution`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_resolution: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Frame-rate label, for example `24p`. Derived, see [`frame_rate_label`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_frame_rate: Option<&'static str>,
    /// A number, `0` or `1`, where the part carries a boolean: the reference's types.
    /// Present only for a file whose layout was analysed, which is an MP4.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimized_for_streaming: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_profile: Option<String>,
    #[serde(rename = "has64bitOffsets", skip_serializing_if = "Option::is_none")]
    pub has_64bit_offsets: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_profile: Option<String>,
    #[serde(rename = "Part", skip_serializing_if = "Vec::is_empty")]
    pub part: Vec<PartEntry>,
}

/// One file of an encoding.
///
/// `key` is still absent. It would be a streaming URL into `/library/parts/…`, which has no
/// handler yet, and pointing a client at a route that answers 501 makes an item look playable
/// when it is not. It arrives with playback.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartEntry {
    pub id: i64,
    /// Where a client fetches the file to play it directly.
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    /// Absolute path on the server's filesystem, as Plex publishes it.
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(rename = "has64bitOffsets", skip_serializing_if = "Option::is_none")]
    pub has_64bit_offsets: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_chapter_text_stream: Option<bool>,
    /// The string `"1"`, where the neighbouring flags are booleans: the reference's types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_thumbnail: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimized_for_streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet_length: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_profile: Option<String>,
    #[serde(rename = "Stream", skip_serializing_if = "Vec::is_empty")]
    pub stream: Vec<StreamEntry>,
}

/// One track inside a file.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEntry {
    pub id: i64,
    /// 1 video, 2 audio, 3 subtitle — Plex's numbering, which clients switch on.
    pub stream_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forced: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// Absent for a sidecar file, which is no track of the media's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<i32>,
    /// The language's name in the client's language: `Anglais (R.-U.)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_code: Option<String>,
    #[serde(flatten)]
    pub analysis: StreamAnalysis,
    pub display_title: String,
    pub extended_display_title: String,
}

/// What the analysis measured, under the names the reference gives it.
///
/// The reference writes these from a packed key/value column, sorted by key, so they come in
/// ASCII order whatever the track's kind: capitals first. Types are per key and do not
/// follow the value: `streamIdentifier` and `DOVIVersion` are strings, loudness is strings
/// with two decimals, `embeddedInVideo` is `"1"`, and flags are booleans.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamAnalysis {
    #[serde(rename = "DOVIBLCompatID", skip_serializing_if = "Option::is_none")]
    pub dovi_bl_compat_id: Option<i32>,
    #[serde(rename = "DOVIBLPresent", skip_serializing_if = "Option::is_none")]
    pub dovi_bl_present: Option<bool>,
    #[serde(rename = "DOVIELPresent", skip_serializing_if = "Option::is_none")]
    pub dovi_el_present: Option<bool>,
    #[serde(rename = "DOVILevel", skip_serializing_if = "Option::is_none")]
    pub dovi_level: Option<i32>,
    #[serde(rename = "DOVIPresent", skip_serializing_if = "Option::is_none")]
    pub dovi_present: Option<bool>,
    #[serde(rename = "DOVIProfile", skip_serializing_if = "Option::is_none")]
    pub dovi_profile: Option<i32>,
    #[serde(rename = "DOVIRPUPresent", skip_serializing_if = "Option::is_none")]
    pub dovi_rpu_present: Option<bool>,
    #[serde(rename = "DOVIVersion", skip_serializing_if = "Option::is_none")]
    pub dovi_version: Option<String>,
    #[serde(rename = "HDR10PlusPresent", skip_serializing_if = "Option::is_none")]
    pub hdr10_plus_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anamorphic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_channel_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chroma_location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chroma_subsampling: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_captions: Option<bool>,
    #[serde(rename = "codecID", skip_serializing_if = "Option::is_none")]
    pub codec_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coded_height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coded_width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_primaries: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_trc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dub: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedded_in_video: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain_offset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_scaling_matrix: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_compression: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hearing_impaired: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loudness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loudness_analysis_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lra: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixel_aspect_ratio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_frames: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_rate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub separate_fields: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual_impaired: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
}

impl StreamAnalysis {
    fn from_graphql(stream: &GqlStream) -> Self {
        let when = |flag: bool| flag.then_some(true);
        let traits = &stream.traits;
        let mut analysis = Self {
            // An empty name is not published, though it still shapes the extended label.
            title: stream.title.clone().filter(|title| !title.is_empty()),
            stream_identifier: stream.stream_identifier.map(|id| id.to_string()),
            profile: stream.profile.clone(),
            bit_depth: stream.bit_depth,
            original: when(traits.is_original),
            dub: when(traits.is_dub),
            comment: when(traits.is_commentary),
            hearing_impaired: when(traits.is_hearing_impaired),
            visual_impaired: when(traits.is_visual_impaired),
            header_compression: when(traits.has_header_compression),
            ..Self::default()
        };
        if let Some(video) = &stream.video {
            analysis.width = video.width;
            analysis.height = video.height;
            analysis.coded_width = video.coded_width;
            analysis.coded_height = video.coded_height;
            analysis.frame_rate = video.frame_rate;
            analysis.level = video.level;
            analysis.ref_frames = video.ref_frames;
            analysis.scan_type = match video.scan.as_deref() {
                Some("PROGRESSIVE") => Some("progressive"),
                Some("INTERLACED") => Some("interlaced"),
                _ => None,
            };
            analysis
                .chroma_subsampling
                .clone_from(&video.chroma_subsampling);
            analysis.chroma_location.clone_from(&video.chroma_location);
            analysis.color_range.clone_from(&video.color.range);
            analysis.color_space.clone_from(&video.color.space);
            analysis.color_primaries.clone_from(&video.color.primaries);
            analysis.color_trc.clone_from(&video.color.transfer);
            analysis.has_scaling_matrix = video.has_scaling_matrix;
            analysis.anamorphic = video.is_anamorphic;
            analysis
                .pixel_aspect_ratio
                .clone_from(&video.pixel_aspect_ratio);
            analysis.codec_id.clone_from(&video.fourcc);
            analysis.closed_captions = when(video.has_closed_captions);
            analysis.separate_fields = when(video.has_separate_fields);
            analysis.hdr10_plus_present = when(video.has_hdr10_plus);
            if let Some(dolby) = &video.dolby_vision {
                analysis.dovi_present = Some(true);
                analysis.dovi_version = Some(dolby.version.clone());
                analysis.dovi_profile = Some(dolby.profile);
                analysis.dovi_level = Some(dolby.level);
                analysis.dovi_bl_compat_id = dolby.base_layer_compatibility;
                analysis.dovi_bl_present = Some(dolby.has_base_layer);
                analysis.dovi_el_present = Some(dolby.has_enhancement_layer);
                analysis.dovi_rpu_present = Some(dolby.has_rpu);
            }
        }
        if let Some(audio) = &stream.audio {
            analysis.sampling_rate = audio.sampling_rate;
            analysis
                .audio_channel_layout
                .clone_from(&audio.channel_layout);
            if let Some(loudness) = &audio.loudness {
                let measure = |value: f64| Some(format!("{value:.2}"));
                analysis.loudness = measure(loudness.integrated);
                analysis.lra = measure(loudness.range);
                analysis.peak = measure(loudness.true_peak);
                analysis.threshold = measure(loudness.threshold);
                analysis.gain_offset = measure(loudness.gain_offset);
                analysis.loudness_analysis_version = Some(loudness.analysis_version.clone());
            }
        }
        if let Some(subtitle) = &stream.subtitle {
            analysis.format.clone_from(&subtitle.format);
            analysis.embedded_in_video = subtitle.is_embedded_in_video.then_some("1");
        }
        analysis
    }
}

/// Which static artwork and layout a library view uses, by media kind.
///
/// These are the reference server's own resource paths. They are per kind, not per library:
/// every movie library shares one backdrop.
#[derive(Debug, Clone, Copy)]
pub struct ViewStyle {
    pub art: &'static str,
    pub thumb: &'static str,
    pub view_group: &'static str,
}

impl ViewStyle {
    /// The style for a library of this kind, as the reference server reports it.
    pub fn for_kind(kind: &str) -> Self {
        match kind {
            "SHOW" => Self {
                art: "/:/resources/show-fanart.jpg",
                thumb: "/:/resources/show.png",
                view_group: "show",
            },
            "ARTIST" => Self {
                art: "/:/resources/artist-fanart.jpg",
                thumb: "/:/resources/artist.png",
                view_group: "artist",
            },
            "PHOTO" => Self {
                art: "/:/resources/photo-fanart.jpg",
                thumb: "/:/resources/photo.png",
                view_group: "photo",
            },
            _ => Self {
                art: "/:/resources/movie-fanart.jpg",
                thumb: "/:/resources/movie.png",
                view_group: "movie",
            },
        }
    }

    /// The view's title, which a reference server words differently per kind: a movie
    /// library's listing is "All <library>", a show library's simply "All Shows".
    pub fn title2(self, library_title: &str) -> String {
        match self.view_group {
            "show" => "All Shows".to_owned(),
            "artist" => "All Artists".to_owned(),
            _ => format!("All {library_title}"),
        }
    }
}

impl ItemsContainer {
    /// Builds a listing response from one page of the official API.
    pub fn new(
        library: &GqlLibraryRef,
        items: Vec<MetadataEntry>,
        offset: u64,
        total: u64,
    ) -> Self {
        let style = ViewStyle::for_kind(&library.kind);
        Self {
            size: u32::try_from(items.len()).unwrap_or(u32::MAX),
            total_size: total,
            offset,
            allow_sync: true,
            art: style.art,
            identifier: LIBRARY_IDENTIFIER,
            library_section_id: library.database_id(),
            library_section_title: library.title.clone(),
            library_section_uuid: library.uuid.clone(),
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            thumb: style.thumb,
            title1: library.title.clone(),
            title2: style.title2(&library.title),
            view_group: style.view_group,
            view_mode: GRID_VIEW_MODE,
            meta: None,
            metadata: items,
        }
    }
}

impl ItemContainer {
    /// Builds a lookup response. The library attributes come from the first item, since a
    /// lookup of several identifiers can in principle span libraries.
    pub fn new(items: Vec<MetadataEntry>, library: Option<&GqlLibraryRef>) -> Self {
        Self {
            size: u32::try_from(items.len()).unwrap_or(u32::MAX),
            allow_sync: true,
            identifier: LIBRARY_IDENTIFIER,
            library_section_id: library.map(GqlLibraryRef::database_id),
            library_section_title: library.map(|library| library.title.clone()),
            library_section_uuid: library.map(|library| library.uuid.clone()),
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            metadata: items,
        }
    }
}

impl MetadataEntry {
    /// Translates one item from the official API.
    ///
    /// `with_library` distinguishes the two endpoints: a listing states the library once on
    /// the container, a lookup repeats it on each item. Matching that is not cosmetic — a
    /// client reading `librarySectionID` off the item would find nothing in a listing.
    pub fn from_graphql(item: &GqlItem, with_library: bool, language: Language) -> Option<Self> {
        let (view_count, view_offset, last_viewed_at) = watched(item.watch_state);
        let rating_key = item.database_id()?;
        let kind = item.plex_type();
        let library = item.library.as_ref();

        Some(Self {
            rating_key: rating_key.to_string(),
            // A film is fetched as itself; a series or season is browsed into, so the key a
            // client follows is the one that lists the children.
            key: if item.has_children() {
                format!("/library/metadata/{rating_key}/children")
            } else {
                format!("/library/metadata/{rating_key}")
            },
            guid: item.guid.clone(),
            parent_guid: item.parent().and_then(|parent| parent.guid.clone()),
            grandparent_guid: item.grandparent().and_then(|parent| parent.guid.clone()),
            parent_studio: item.parent().and_then(|parent| parent.studio.clone()),
            studio: item.studio.clone(),
            kind,
            title: item.display_title(language),
            // Omitted when it says nothing the *stored* title does not: the importer stores
            // a sort title for every row, while a reference server sends one only where it
            // differs. Compared with the stored title, not the displayed one, because a
            // season named at read time still reports `titleSort="Season 1"`.
            title_sort: item
                .title_sort
                .clone()
                .filter(|sort| Some(sort) != item.title.as_ref()),
            library_section_title: with_library
                .then(|| library.map(|l| l.title.clone()))
                .flatten(),
            library_section_id: with_library
                .then(|| library.map(GqlLibraryRef::database_id))
                .flatten(),
            library_section_key: with_library
                .then(|| library.map(|l| format!("/library/sections/{}", l.database_id())))
                .flatten(),
            original_title: item.original_title.clone(),
            content_rating: item.content_rating.clone(),
            content_rating_age: item.content_rating_age,
            // A season with no synopsis reports an empty one, where every other kind omits
            // the attribute. Measured on seven seasons across two libraries, no exception.
            summary: item
                .summary
                .clone()
                .or_else(|| (item.typename == "Season").then(String::new)),
            index: item.number,
            rating: item.rating.map(one_decimal),
            audience_rating: item.audience_rating.map(one_decimal),
            parent_rating_key: item.parent_rating_key(),
            grandparent_rating_key: item.grandparent_rating_key(),
            // Derived from the keys above: a client follows these to walk up the hierarchy.
            parent_key: item
                .parent_rating_key()
                .map(|key| format!("/library/metadata/{key}")),
            grandparent_key: item
                .grandparent_rating_key()
                .map(|key| format!("/library/metadata/{key}")),
            parent_index: item.parent_index(),
            grandparent_title: item.grandparent_title(),
            parent_title: item.parent_title(language),
            view_count,
            view_offset,
            last_viewed_at,
            year: item.year,
            // Only a season falls back to its series' year, and only when it has none: the
            // reference sends one or the other, never both.
            parent_year: (item.typename == "Season" && item.year.is_none())
                .then(|| item.parent().and_then(|parent| parent.year))
                .flatten(),
            tagline: item.tagline.clone(),
            duration: runtime(item),
            originally_available_at: item.released_on.clone(),
            leaf_count: item.leaf_count(),
            viewed_leaf_count: item.viewed_leaf_count(),
            child_count: item.child_count(),
            added_at: item.added_at,
            updated_at: item.updated_at,
            media: item
                .versions
                .iter()
                .map(|version| MediaEntry::from_graphql(version, language))
                .collect(),
            genre: TagEntry::all(&item.tags.genres, "genre"),
            country: TagEntry::all(&item.tags.countries, "country"),
            guids: item
                .external_ids
                .nodes()
                .map(|id| GuidEntry {
                    id: format!("{}://{}", id.source, id.value),
                })
                .collect(),
            ratings: item
                .ratings
                .nodes()
                .map(RatingEntry::from_graphql)
                .collect(),
            director: TagEntry::all(&item.tags.directors, "director"),
            writer: TagEntry::all(&item.tags.writers, "writer"),
            role: item
                .tags
                .cast
                .edges
                .iter()
                .map(TagEntry::from_cast_member)
                .collect(),
            producer: TagEntry::all(&item.tags.producers, "producer"),
        })
    }
}

/// The media's runtime where there is media: a reference server reports 6,050,048 ms for an
/// item whose metadata row says 6,060,000. The metadata value comes from the matched release,
/// the media value from the actual file, and a client seeking by percentage needs the one
/// that matches what it is playing.
fn runtime(item: &GqlItem) -> Option<i64> {
    item.versions
        .first()
        .and_then(|version| version.duration_ms)
        .or(item.duration_ms)
}

/// The caller's count, offset and date, as published: a count only once there is one.
fn watched(state: Option<GqlWatchState>) -> (Option<u32>, Option<i64>, Option<i64>) {
    let Some(state) = state else {
        return (None, None, None);
    };
    (
        Some(state.view_count).filter(|count| *count > 0),
        state.offset_ms,
        state.last_viewed_at,
    )
}

impl MediaEntry {
    fn from_graphql(version: &GqlVersion, language: Language) -> Self {
        let first_file = version.files.first();
        Self {
            id: version.database_id(),
            duration: version.duration_ms,
            bitrate: kilobits(version.bitrate),
            width: version.width,
            height: version.height,
            aspect_ratio: version.aspect_ratio.map(snap_aspect_ratio),
            audio_channels: version.audio_channels,
            audio_codec: composed_audio_codec(
                version.audio_codec.as_deref(),
                profile_of(version.primary_audio.as_ref()).as_deref(),
            ),
            video_codec: version.video_codec.clone(),
            video_resolution: crate::resolution::label(version.width, version.height),
            container: version.container.clone(),
            video_frame_rate: version.frame_rate.and_then(frame_rate_label),
            optimized_for_streaming: first_file
                .and_then(|file| file.optimized_for_streaming)
                .map(|_| u8::from(version.optimized_for_streaming)),
            audio_profile: profile_of(version.primary_audio.as_ref()),
            has_64bit_offsets: first_file.and_then(|file| file.has_64bit_offsets),
            video_profile: profile_of(version.primary_video.as_ref()),
            part: version
                .files
                .iter()
                .map(|file| PartEntry::from_graphql(file, version, language))
                .collect(),
        }
    }
}

impl PartEntry {
    /// The container and the profiles are the version's: the reference repeats them on
    /// every part, a part having none of its own.
    fn from_graphql(file: &GqlPart, version: &GqlVersion, language: Language) -> Self {
        // The middle segment is a change stamp on the reference, and the name is cosmetic:
        // neither is read back.
        let extension = version.container.as_deref().unwrap_or("bin");
        Self {
            id: file.database_id(),
            key: format!("/library/parts/{}/0/file.{extension}", file.database_id()),
            duration: file.duration_ms,
            file: file.path.clone(),
            size: file.size_bytes,
            audio_profile: profile_of(version.primary_audio.as_ref()),
            container: version.container.clone(),
            has_64bit_offsets: file.has_64bit_offsets,
            has_chapter_text_stream: file.has_chapter_text_stream.then_some(true),
            has_thumbnail: file.has_thumbnail.then_some("1"),
            optimized_for_streaming: file.optimized_for_streaming,
            packet_length: file.packet_length,
            video_profile: profile_of(version.primary_video.as_ref()),
            stream: with_audio_selected(
                file.streams
                    .iter()
                    .map(|stream| StreamEntry::from_graphql(stream, language))
                    .collect(),
            ),
        }
    }
}

/// The version's audio codec as the reference names it: `dca-ma` for DTS-HD Master Audio,
/// the stored codec otherwise. Measured over every codec and profile pair of the library;
/// no other pair is composed, not even `dca` with `hra` or `es`.
fn composed_audio_codec(codec: Option<&str>, profile: Option<&str>) -> Option<String> {
    match (codec, profile) {
        (Some("dca"), Some("ma")) => Some("dca-ma".to_owned()),
        (codec, _) => codec.map(ToOwned::to_owned),
    }
}

fn profile_of(track: Option<&GqlProfiled>) -> Option<String> {
    track.and_then(|track| track.profile.clone())
}

/// Marks the audio track that plays when none is asked for: the first default, or else the
/// first. Plex Web's player settings read the selected track and crash without one. The
/// reference also honors an account's language, which agrees on 206 of 219 files measured; we
/// have no such preference. Subtitles stay unselected: nothing records a viewer's choice.
fn with_audio_selected(mut streams: Vec<StreamEntry>) -> Vec<StreamEntry> {
    let audio = |stream: &StreamEntry| stream.stream_type == 2;
    let chosen = streams
        .iter()
        .position(|stream| audio(stream) && stream.default == Some(true))
        .or_else(|| streams.iter().position(audio));
    if let Some(chosen) = chosen {
        streams[chosen].selected = Some(true);
    }
    streams
}

impl StreamEntry {
    fn from_graphql(stream: &GqlStream, language: Language) -> Self {
        let labels = stream_labels::labels(stream, language);
        let named = stream
            .language
            .as_deref()
            .map(|tag| stream_labels::language(tag, language));
        Self {
            id: stream.database_id(),
            stream_type: match stream.kind.as_str() {
                "VIDEO" => 1,
                "AUDIO" => 2,
                _ => 3,
            },
            selected: None,
            default: stream.is_default.then_some(true),
            forced: stream.is_forced.then_some(true),
            codec: stream.codec.clone(),
            index: (!stream.is_external).then_some(stream.position),
            channels: stream.channels,
            bitrate: kilobits(stream.bitrate),
            language: named.as_ref().map(|label| label.name.clone()),
            language_tag: named.as_ref().map(|label| label.tag.clone()),
            language_code: named.map(|label| label.code),
            analysis: StreamAnalysis::from_graphql(stream),
            display_title: labels.display,
            extended_display_title: labels.extended,
        }
    }
}

/// A character's name as the reference publishes it: the first letter capitalized when it is
/// ASCII, everything else untouched.
///
/// Measured on 25 credits the source stores in lower case, all capitalized on the way out —
/// `actor` becomes `Actor` — while `čarodějobijec`, `министр` and `(uncredited)` come back as
/// stored. The official API keeps the source text; this is presentation.
fn credited_character(stored: &str) -> String {
    let mut characters = stored.chars();
    match characters.next() {
        Some(first) if first.is_ascii_lowercase() => {
            first.to_ascii_uppercase().to_string() + characters.as_str()
        }
        _ => stored.to_owned(),
    }
}

/// Rounds a rating to the one decimal Plex publishes.
///
/// Verified against 280 rating values on the reference server, which reports 7.9 for a stored
/// 7.924 without exception. The official API keeps the stored precision.
fn one_decimal(rating: f64) -> f64 {
    (rating * 10.0).round() / 10.0
}

/// The cinema ratios Plex snaps a measurement onto.
///
/// Measured against 720 media items on the reference server, which report exactly these six
/// values and no others — 1.5019 comes back as 1.66 and 2.0084 as 1.85, which is what rules
/// out the intermediate standards (1.5, 2.0) being on the ladder at all.
const ASPECT_RATIOS: [f64; 6] = [1.33, 1.66, 1.78, 1.85, 2.2, 2.35];

/// Snaps a measured aspect ratio onto the nearest cinema ratio.
///
/// Clamped at both ends, which matters: a 3.56 ultrawide reports 2.35 and a 0.89 portrait
/// transfer reports 1.33.
fn snap_aspect_ratio(measured: f64) -> f64 {
    ASPECT_RATIOS
        .into_iter()
        .min_by(|left, right| (left - measured).abs().total_cmp(&(right - measured).abs()))
        .unwrap_or(measured)
}

/// Converts a measured bit rate to the kilobits Plex publishes.
///
/// Rounded, not truncated: 11,285,835 bits/s comes back as 11,286 on the reference server,
/// and a 63 bit/s subtitle track as 0. Done in integers so the rounding is exact rather than
/// subject to a float conversion.
fn kilobits(bits_per_second: Option<i32>) -> Option<i32> {
    bits_per_second.map(|bits| {
        let rounded = (i64::from(bits) + 500) / 1000;
        i32::try_from(rounded).unwrap_or(i32::MAX)
    })
}

/// Plex's label for a measured frame rate, or none when it has no name for it.
///
/// The official API publishes the measurement (23.976); this is the display string clients
/// show. Only three standards have labels, and anything else is **omitted** rather than
/// approximated — a 50 fps file reports no `videoFrameRate` at all on the reference server,
/// which is how this table was derived rather than guessed.
fn frame_rate_label(fps: f64) -> Option<&'static str> {
    const TOLERANCE: f64 = 0.05;
    let near = |standard: f64| (fps - standard).abs() <= TOLERANCE;

    if near(23.976) || near(24.0) {
        Some("24p")
    } else if near(25.0) {
        Some("PAL")
    } else if near(29.97) || near(30.0) {
        Some("NTSC")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::{with_audio_selected, StreamEntry};

    fn stream(stream_type: u8, default: bool) -> StreamEntry {
        StreamEntry {
            id: 1,
            stream_type,
            selected: None,
            default: default.then_some(true),
            forced: None,
            codec: None,
            index: Some(0),
            channels: None,
            bitrate: None,
            language: None,
            language_tag: None,
            language_code: None,
            analysis: super::StreamAnalysis::default(),
            display_title: String::new(),
            extended_display_title: String::new(),
        }
    }

    fn selected(streams: Vec<StreamEntry>) -> Vec<bool> {
        with_audio_selected(streams)
            .iter()
            .map(|stream| stream.selected == Some(true))
            .collect()
    }

    #[test]
    fn the_first_default_audio_track_is_selected() {
        assert_eq!(
            selected(vec![
                stream(1, true),
                stream(2, false),
                stream(2, true),
                stream(2, true),
            ]),
            [false, false, true, false]
        );
    }

    #[test]
    fn without_a_default_the_first_audio_track_is_selected() {
        assert_eq!(
            selected(vec![stream(1, false), stream(2, false), stream(2, false)]),
            [false, true, false]
        );
    }

    #[test]
    fn neither_video_nor_subtitles_are_selected() {
        assert_eq!(
            selected(vec![stream(1, true), stream(3, true)]),
            [false, false]
        );
    }

    use super::*;

    #[test]
    fn a_characters_ascii_first_letter_is_capitalized_and_nothing_else() {
        assert_eq!(credited_character("actor"), "Actor");
        assert_eq!(credited_character("self - host"), "Self - host");
        assert_eq!(credited_character("Frank Martin"), "Frank Martin");
        assert_eq!(credited_character("čarodějobijec"), "čarodějobijec");
        assert_eq!(credited_character("(uncredited)"), "(uncredited)");
        assert_eq!(credited_character(""), "");
    }

    #[test]
    fn labels_only_the_frame_rates_plex_names() {
        assert_eq!(frame_rate_label(23.976_024), Some("24p"));
        assert_eq!(frame_rate_label(24.0), Some("24p"));
        assert_eq!(frame_rate_label(25.0), Some("PAL"));
        assert_eq!(frame_rate_label(29.970_03), Some("NTSC"));
        assert_eq!(frame_rate_label(30.0), Some("NTSC"));
        // 50 fps has no label on the reference server, which omits the attribute rather
        // than calling it PAL.
        assert_eq!(frame_rate_label(50.0), None);
        assert_eq!(frame_rate_label(23.809_5), None);
    }

    #[test]
    fn snaps_aspect_ratios_onto_the_cinema_ladder() {
        // Every pair observed on the reference server for the same media item.
        assert!((snap_aspect_ratio(2.376_237_6) - 2.35).abs() < 1e-9);
        assert!((snap_aspect_ratio(1.777_1) - 1.78).abs() < 1e-9);
        assert!((snap_aspect_ratio(1.814_4) - 1.78).abs() < 1e-9);
        assert!((snap_aspect_ratio(1.842_3) - 1.85).abs() < 1e-9);
        assert!((snap_aspect_ratio(2.008_4) - 1.85).abs() < 1e-9);
        assert!((snap_aspect_ratio(1.501_9) - 1.66).abs() < 1e-9);
        assert!((snap_aspect_ratio(2.069) - 2.2).abs() < 1e-9);
        // Clamped at both ends.
        assert!((snap_aspect_ratio(3.555_6) - 2.35).abs() < 1e-9);
        assert!((snap_aspect_ratio(0.888_9) - 1.33).abs() < 1e-9);
    }

    #[test]
    fn rounds_ratings_to_one_decimal() {
        assert!((one_decimal(7.924) - 7.9).abs() < 1e-9);
        assert!((one_decimal(8.5) - 8.5).abs() < 1e-9);
        assert!((one_decimal(2.95) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn converts_measured_bitrates_to_kilobits() {
        // Storage holds bits per second; Plex publishes kilobits, rounded.
        assert_eq!(kilobits(Some(11_285_835)), Some(11_286));
        assert_eq!(kilobits(Some(8_213_835)), Some(8_214));
        assert_eq!(kilobits(Some(1_536_000)), Some(1_536));
        // A subtitle track measured at 63 bits/s reports 0, not 1.
        assert_eq!(kilobits(Some(63)), Some(0));
        assert_eq!(kilobits(None), None);
    }

    #[test]
    fn jump_bar_keys_are_percent_encoded() {
        // Both values the reference emits for this library: `#` and the `Œ` ligature.
        assert_eq!(CharacterDirectory::new("#", 162).key, "%23");
        assert_eq!(CharacterDirectory::new("Œ", 2).key, "%C5%92");
        assert_eq!(CharacterDirectory::new("A", 524).key, "A");
        // The title keeps the character itself; only the key is encoded.
        assert_eq!(CharacterDirectory::new("#", 162).title, "#");
        assert_eq!(CharacterDirectory::new("#", 162).size, 162);
    }

    #[test]
    fn view_styles_follow_the_library_kind() {
        let show = ViewStyle::for_kind("SHOW");
        assert_eq!(show.view_group, "show");
        assert_eq!(show.title2("VF - Series"), "All Shows");

        let movie = ViewStyle::for_kind("MOVIE");
        assert_eq!(movie.thumb, "/:/resources/movie.png");
        // A movie library names the view after itself; a show library does not.
        assert_eq!(movie.title2("VF - Films"), "All VF - Films");
    }
}

/// Response body for `GET /library/sections/{id}/firstCharacter` — the A–Z jump bar.
///
/// A "secondary" view in Plex's vocabulary: a list of buckets to jump into rather than a list
/// of items. Hence `content` and `viewGroup` both reading `secondary`, and the absence of any
/// paging attribute.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharactersContainer {
    pub size: u32,
    pub allow_sync: bool,
    pub art: &'static str,
    pub content: &'static str,
    pub identifier: &'static str,
    pub media_tag_prefix: &'static str,
    pub media_tag_version: i64,
    pub thumb: &'static str,
    pub title1: String,
    pub title2: &'static str,
    pub view_group: &'static str,
    pub view_mode: u32,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    pub directory: Vec<CharacterDirectory>,
}

/// One bucket of the jump bar.
#[derive(Debug, Serialize)]
pub struct CharacterDirectory {
    /// How many items fall under this letter. Named `size`, not `count`, unlike every other
    /// `Directory` in the surface — a reference server's inconsistency, reproduced.
    pub size: u64,
    /// The letter, percent-encoded: it goes into a query string, and `#` would otherwise
    /// start a fragment.
    pub key: String,
    pub title: String,
}

impl CharacterDirectory {
    pub fn new(character: &str, count: u64) -> Self {
        Self {
            size: count,
            key: percent_encode(character),
            title: character.to_owned(),
        }
    }
}

impl CharactersContainer {
    pub fn new(library: &GqlLibraryRef, buckets: Vec<CharacterDirectory>) -> Self {
        let style = ViewStyle::for_kind(&library.kind);
        Self {
            size: u32::try_from(buckets.len()).unwrap_or(u32::MAX),
            allow_sync: false,
            art: style.art,
            content: "secondary",
            identifier: LIBRARY_IDENTIFIER,
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            thumb: style.thumb,
            title1: library.title.clone(),
            title2: "By First Letter",
            view_group: "secondary",
            view_mode: GRID_VIEW_MODE,
            directory: buckets,
        }
    }
}

/// Percent-encodes everything outside the unreserved set.
///
/// Hand-rolled rather than pulled in: one caller, one rule, and the rule is short. The
/// unreserved set is RFC 3986's — letters, digits, `-`, `.`, `_`, `~` — so `#` becomes `%23`
/// and `Œ` becomes `%C5%92`, which is what the reference emits.
fn percent_encode(text: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            // Writing into a `String` cannot fail, so there is nothing to handle.
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Response body for `GET /search` — results from every library at once.
///
/// No `totalSize` and no view attributes: a search is not a window over a named collection.
/// `size` counts the items **plus** the providers, which is what the reference reports (37
/// items and 4 providers add up to the 41 it sends).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchContainer {
    pub size: u32,
    pub identifier: &'static str,
    pub media_tag_prefix: &'static str,
    pub media_tag_version: i64,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<SearchEntry>,
    #[serde(rename = "Provider")]
    pub provider: Vec<SearchProvider>,
}

/// One search result.
///
/// The library attributes come **before** the item's own, which is why this is a wrapper
/// rather than a flag on [`MetadataEntry`]: the lookup endpoint puts the same attributes after
/// `titleSort`, and attribute order is observable in XML.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchEntry {
    pub allow_sync: bool,
    #[serde(rename = "librarySectionID")]
    pub library_section_id: i64,
    pub library_section_title: String,
    #[serde(rename = "librarySectionUUID")]
    pub library_section_uuid: String,
    /// Whether the library belongs to the account asking. Always true: there is no sharing
    /// model yet, so every library is the owner's own.
    pub personal: bool,
    /// Which server holds it, by name — a search can span servers on Plex, never here.
    pub source_title: String,
    #[serde(flatten)]
    pub item: MetadataEntry,
}

/// A search source the client may query separately.
///
/// Static, and the four the reference advertises. Listed so a client's source picker is not
/// empty; each key is a route of its own, and only the first is this endpoint.
#[derive(Debug, Serialize)]
pub struct SearchProvider {
    pub key: &'static str,
    #[serde(rename = "m_title")]
    pub title: &'static str,
    #[serde(rename = "m_type")]
    pub kind: &'static str,
}

const SEARCH_PROVIDERS: &[SearchProvider] = &[
    SearchProvider {
        key: "/system/search",
        title: "Local Network",
        kind: "mixed",
    },
    SearchProvider {
        key: "/search?type=10",
        title: "Tracks",
        kind: "tracks",
    },
    SearchProvider {
        key: "/search/actor",
        title: "Actors",
        kind: "actors",
    },
    SearchProvider {
        key: "/search/director",
        title: "Directors",
        kind: "directors",
    },
];

impl SearchContainer {
    pub fn new(items: Vec<SearchEntry>) -> Self {
        let providers: Vec<SearchProvider> = SEARCH_PROVIDERS
            .iter()
            .map(|provider| SearchProvider {
                key: provider.key,
                title: provider.title,
                kind: provider.kind,
            })
            .collect();
        Self {
            // Items *and* providers, as the reference counts them.
            size: u32::try_from(items.len() + providers.len()).unwrap_or(u32::MAX),
            identifier: LIBRARY_IDENTIFIER,
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            metadata: items,
            provider: providers,
        }
    }
}

impl SearchEntry {
    /// Wraps a result with the library attributes a search puts in front.
    ///
    /// `None` when the item carries no library, which the schema only allows for an item whose
    /// section has gone missing — a result nobody could navigate into.
    pub fn from_graphql(item: &GqlItem, server_name: &str, language: Language) -> Option<Self> {
        let library = item.library.clone()?;
        Some(Self {
            allow_sync: true,
            library_section_id: library.database_id(),
            library_section_title: library.title.clone(),
            library_section_uuid: library.uuid.clone(),
            personal: true,
            source_title: server_name.to_owned(),
            item: MetadataEntry::from_graphql(item, false, language)?,
        })
    }
}
