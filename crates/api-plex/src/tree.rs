//! `GET /library/metadata/{ids}/tree`: an item and everything under it, down to the tracks of
//! its files, in the reference's storage vocabulary rather than its listing one.
//!
//! What the reference writes is its own rows nearly as stored: `MetadataItem`, `MediaItem`,
//! `MediaPart` and `MediaStream`, a track's analysis as the raw keys it keeps them under. The
//! rules below were measured on its films, series, seasons and episodes (`docs/spec-gaps.md`).
//!
//! - The path takes a list, and exactly one of its numbers must name something: two known
//!   numbers, even twice the same, answer `404`, as does none. Unknown numbers are dropped.
//! - A season is the root of its own tree. An episode is not: the tree starts at its series
//!   and holds only the branch leading to it.
//! - Every element of a tree carries the `parentTitle` of its root, the root's parent's title.
//! - A series carries its preferences, after its seasons, for an account holding a state of it.
//!
//! Times are published as stored. The reference shifts this tree's, and no other endpoint's,
//! by its system's UTC offset.

use std::collections::BTreeMap;

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, State};
use axum::routing::get;
use axum::Router;
use klapz_api::collection::COLLECTION_TYPE;
use klapz_api::item::ITEM_TYPE;
use klapz_api::GlobalId;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::gql::date_time;
use crate::items::{LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION};
use crate::prefs::Setting;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/library/metadata/{ids}/tree", get(tree))
}

/// The reference writes `-1` for a time or a size it does not have, and `""` for a text.
const ABSENT: i64 = -1;

/// Every film the reference holds has index 1, all 18,332 of them; a series likewise.
const FILM_INDEX: i32 = 1;

/// What `absoluteIndex` is on every one of the reference's 2,299 collections.
const COLLECTION_ABSOLUTE_INDEX: i32 = 1;

/// `MediaPart.metadataType` on an episode's files, and on nothing else's.
const EPISODE_METADATA_TYPE: i32 = 4;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeContainer {
    size: u32,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(rename = "MetadataItem")]
    metadata_item: Vec<TreeItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeItem {
    id: i64,
    title: String,
    guid: String,
    parent_title: String,
    instance_rating_key: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<i64>,
    originally_available_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    absolute_index: Option<i32>,
    added_at: i64,
    refreshed_at: i64,
    #[serde(rename = "MediaItem", skip_serializing_if = "Vec::is_empty")]
    media_item: Vec<TreeVersion>,
    #[serde(rename = "MetadataItem", skip_serializing_if = "Vec::is_empty")]
    metadata_item: Vec<TreeItem>,
    #[serde(rename = "Setting", skip_serializing_if = "Vec::is_empty")]
    setting: Vec<Setting>,
}

#[derive(Debug, Serialize)]
struct TreeVersion {
    id: i64,
    #[serde(rename = "MediaPart")]
    media_part: Vec<TreePart>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreePart {
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata_type: Option<i32>,
    id: i64,
    file: String,
    size: i64,
    open_subtitle_hash: String,
    hash: String,
    duration: i64,
    #[serde(rename = "MediaStream", skip_serializing_if = "Vec::is_empty")]
    media_stream: Vec<TreeTrack>,
}

/// A track: its columns, then its analysis under the reference's own key names, in byte
/// order — `DOVIPresent` before `bitDepth`.
#[derive(Debug, Serialize)]
struct TreeTrack {
    id: i64,
    /// Absent on a sidecar file, which the reference keeps without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<i32>,
    #[serde(rename = "type")]
    kind: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(flatten)]
    analysis: BTreeMap<&'static str, Json>,
}

const RESOLVE_QUERY: &str = r"
query PlexTreeResolve($items: [ID!]!, $collections: [ID!]!) {
  items: nodes(ids: $items) { id }
  collections: nodes(ids: $collections) { ... on Collection { id } }
}
";

#[derive(Debug, Deserialize)]
struct ResolveData {
    items: Vec<Option<Json>>,
    collections: Vec<Option<Json>>,
}

const NODE_FRAGMENT: &str = r"
fragment TreeNode on Item {
  id
  title
  guid
  addedAt
  refreshedAt
  ... on Movie { releasedOn }
  ... on Show {
    number
    releasedOn
    watchState { viewCount offsetMs lastViewedAt }
    userRating { value }
  }
  ... on Season { number }
  ... on Episode { number releasedOn }
}
";

const VERSION_FRAGMENT: &str = r"
fragment TreeVersion on MediaVersion {
  id
  files {
    id
    path
    sizeBytes
    durationMs
    contentHash
    openSubtitlesHash
    streams {
      id
      kind
      position
      codec
      language
      channels
      externalUrl
      title
      streamIdentifier
      profile
      bitDepth
      requiredBandwidths
      traits {
        isOriginal
        isDub
        isCommentary
        isHearingImpaired
        isVisualImpaired
        hasHeaderCompression
      }
      video {
        width
        height
        codedWidth
        codedHeight
        frameRate
        level
        refFrames
        scan
        chromaSubsampling
        chromaLocation
        color { range space primaries transfer }
        hasScalingMatrix
        isAnamorphic
        pixelAspectRatio
        fourcc
        hasClosedCaptions
        hasSeparateFields
        hasHdr10Plus
        dolbyVision {
          version
          profile
          level
          baseLayerCompatibility
          hasBaseLayer
          hasEnhancementLayer
          hasRpu
        }
      }
      audio {
        samplingRate
        channelLayout
        loudness { analysisVersion integrated range truePeak threshold gainOffset }
      }
      subtitle { format isEmbeddedInVideo }
    }
  }
}
";

/// The query with the fragments it spreads, and only those: GraphQL rejects a document
/// carrying one it never uses.
fn with_fragments(query: &str) -> String {
    let versions = if query.contains("...TreeVersion") {
        VERSION_FRAGMENT
    } else {
        ""
    };
    format!("{query}{NODE_FRAGMENT}{versions}")
}

const ROOT_QUERY: &str = r"
query PlexTree($id: ID!) {
  node(id: $id) {
    __typename
    ...TreeNode
    ... on Movie { versions { ...TreeVersion } }
    ... on Season { show { ...TreeNode } }
    ... on Episode {
      versions { ...TreeVersion }
      season { ...TreeNode show { ...TreeNode } }
    }
  }
}
";

const SEASONS_QUERY: &str = r"
query PlexTreeSeasons($id: ID!, $first: Int!, $after: String) {
  node(id: $id) {
    ... on Show {
      children: seasons(first: $first, after: $after) {
        pageInfo { hasNextPage endCursor }
        edges { node { ...TreeNode } }
      }
    }
  }
}
";

const EPISODES_QUERY: &str = r"
query PlexTreeEpisodes($id: ID!, $first: Int!, $after: String) {
  node(id: $id) {
    ... on Season {
      children: episodes(first: $first, after: $after) {
        pageInfo { hasNextPage endCursor }
        edges { node { ...TreeNode versions { ...TreeVersion } } }
      }
    }
  }
}
";

const COLLECTION_QUERY: &str = r"
query PlexTreeCollection($id: ID!) {
  node(id: $id) { ... on Collection { id title guid tagNumber addedAt } }
}
";

#[derive(Debug, Deserialize)]
struct NodeData<T> {
    node: Option<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlNode {
    #[serde(rename = "__typename", default)]
    typename: String,
    id: String,
    title: Option<String>,
    guid: String,
    #[serde(deserialize_with = "date_time::seconds")]
    added_at: i64,
    #[serde(default, deserialize_with = "date_time::optional_seconds")]
    refreshed_at: Option<i64>,
    #[serde(default, deserialize_with = "date_time::optional_calendar_date")]
    released_on: Option<String>,
    number: Option<i64>,
    #[serde(default)]
    versions: Vec<GqlVersion>,
    show: Option<Box<GqlNode>>,
    season: Option<Box<GqlNode>>,
    watch_state: Option<GqlWatchState>,
    user_rating: Option<Json>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlWatchState {
    view_count: u32,
    offset_ms: Option<i64>,
    last_viewed_at: Option<String>,
}

impl GqlNode {
    /// The reference writes a series' preferences only for an account holding a state of it,
    /// whatever that state says: on 60 series, the 8 with such a row and none of the others.
    /// An empty state here is no state at all.
    fn caller_holds_state(&self) -> bool {
        let watched = self.watch_state.as_ref().is_some_and(|state| {
            state.view_count > 0 || state.offset_ms.is_some() || state.last_viewed_at.is_some()
        });
        watched || self.user_rating.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct GqlVersion {
    id: String,
    files: Vec<GqlFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlFile {
    id: String,
    path: String,
    #[serde(default, deserialize_with = "klapz_api::int64::deserialize_optional")]
    size_bytes: Option<i64>,
    duration_ms: Option<i64>,
    content_hash: Option<String>,
    open_subtitles_hash: Option<String>,
    streams: Vec<GqlTrack>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlTrack {
    id: String,
    kind: String,
    position: i32,
    codec: Option<String>,
    language: Option<String>,
    channels: Option<i32>,
    external_url: Option<String>,
    title: Option<String>,
    stream_identifier: Option<i64>,
    profile: Option<String>,
    bit_depth: Option<i32>,
    required_bandwidths: Vec<f64>,
    traits: GqlTraits,
    video: Option<GqlVideo>,
    audio: Option<GqlAudio>,
    subtitle: Option<GqlSubtitle>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the official API's TrackTraits, one flag per trait"
)]
struct GqlTraits {
    is_original: bool,
    is_dub: bool,
    is_commentary: bool,
    is_hearing_impaired: bool,
    is_visual_impaired: bool,
    has_header_compression: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the official API's VideoAnalysis, whose flags are independent"
)]
struct GqlVideo {
    width: Option<i32>,
    height: Option<i32>,
    coded_width: Option<i32>,
    coded_height: Option<i32>,
    frame_rate: Option<f64>,
    level: Option<i32>,
    ref_frames: Option<i32>,
    scan: Option<String>,
    chroma_subsampling: Option<String>,
    chroma_location: Option<String>,
    color: GqlColor,
    has_scaling_matrix: Option<bool>,
    is_anamorphic: Option<bool>,
    pixel_aspect_ratio: Option<String>,
    fourcc: Option<String>,
    has_closed_captions: bool,
    has_separate_fields: bool,
    has_hdr10_plus: bool,
    dolby_vision: Option<GqlDolbyVision>,
}

#[derive(Debug, Deserialize)]
struct GqlColor {
    range: Option<String>,
    space: Option<String>,
    primaries: Option<String>,
    transfer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlDolbyVision {
    version: String,
    profile: i32,
    level: i32,
    base_layer_compatibility: Option<i32>,
    has_base_layer: bool,
    has_enhancement_layer: bool,
    has_rpu: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlAudio {
    sampling_rate: Option<i32>,
    channel_layout: Option<String>,
    loudness: Option<GqlLoudness>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlLoudness {
    analysis_version: String,
    integrated: f64,
    range: f64,
    true_peak: f64,
    threshold: f64,
    gain_offset: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlSubtitle {
    format: Option<String>,
    is_embedded_in_video: bool,
}

#[derive(Debug, Deserialize)]
struct ChildrenNode {
    children: ChildPage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChildPage {
    page_info: PageInfo,
    edges: Vec<ChildEdge>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChildEdge {
    node: GqlNode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCollection {
    id: String,
    title: String,
    guid: String,
    tag_number: String,
    #[serde(deserialize_with = "date_time::seconds")]
    added_at: i64,
}

/// What one number of the path names.
enum Found {
    Item(i64),
    Collection(i64),
}

async fn tree(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<TreeContainer>, ApiError> {
    let root = match resolve(&state, &ids).await? {
        Found::Item(key) => item_tree(&state, key).await?,
        Found::Collection(key) => collection_tree(&state, key).await?,
    };
    Ok(PlexResponse::new(
        TreeContainer {
            size: 1,
            identifier: LIBRARY_IDENTIFIER,
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            metadata_item: vec![root],
        },
        format,
    ))
}

/// The one thing the list names, as items and collections share the numbering. Untrimmed:
/// ` 21824` names nothing on the reference.
async fn resolve(state: &PlexState, ids: &str) -> Result<Found, ApiError> {
    let keys: Vec<i64> = ids.split(',').filter_map(|key| key.parse().ok()).collect();
    if keys.is_empty() || keys.len() > MAX_ITEMS as usize {
        return Err(ApiError::NotFound);
    }
    let encoded = |kind: &str| {
        Value::List(
            keys.iter()
                .map(|key| Value::String(GlobalId::encode(kind, *key)))
                .collect(),
        )
    };
    let mut variables = Variables::default();
    variables.insert(Name::new("items"), encoded(ITEM_TYPE));
    variables.insert(Name::new("collections"), encoded(COLLECTION_TYPE));
    let data: ResolveData = gql::execute(&state.schema, RESOLVE_QUERY, variables).await?;

    let mut found = keys
        .iter()
        .zip(data.items.iter().zip(&data.collections))
        .filter_map(|(key, (item, collection))| match (item, collection) {
            (Some(_), _) => Some(Found::Item(*key)),
            (None, Some(_)) => Some(Found::Collection(*key)),
            (None, None) => None,
        });
    match (found.next(), found.next()) {
        (Some(only), None) => Ok(only),
        _ => Err(ApiError::NotFound),
    }
}

async fn item_tree(state: &PlexState, key: i64) -> Result<TreeItem, ApiError> {
    let data: NodeData<GqlNode> = gql::execute(
        &state.schema,
        &with_fragments(ROOT_QUERY),
        id_variables(ITEM_TYPE, key),
    )
    .await?;
    let mut node = data.node.ok_or(ApiError::NotFound)?;

    match node.typename.as_str() {
        "Movie" => {
            let versions = std::mem::take(&mut node.versions);
            let mut item = tree_item(&node, "", Some(i64::from(FILM_INDEX)))?;
            item.media_item = tree_versions(&versions, None)?;
            Ok(item)
        }
        "Show" => show_tree(state, &node).await,
        "Season" => {
            let parent_title = node
                .show
                .as_ref()
                .and_then(|show| show.title.clone())
                .unwrap_or_default();
            season_tree(state, &node, &parent_title).await
        }
        "Episode" => {
            let season = node.season.take().ok_or(ApiError::NotFound)?;
            let show = season.show.as_deref().ok_or(ApiError::NotFound)?;
            let versions = std::mem::take(&mut node.versions);
            let mut episode = tree_item(&node, "", node.number)?;
            episode.media_item = tree_versions(&versions, Some(EPISODE_METADATA_TYPE))?;
            let mut season_item = tree_item(&season, "", season.number)?;
            season_item.metadata_item = vec![episode];
            let mut show_item = tree_item(show, "", show.number)?;
            show_item.metadata_item = vec![season_item];
            if show.caller_holds_state() {
                show_item.setting = show_settings();
            }
            Ok(show_item)
        }
        _ => Err(ApiError::NotFound),
    }
}

async fn show_tree(state: &PlexState, show: &GqlNode) -> Result<TreeItem, ApiError> {
    let mut item = tree_item(show, "", show.number)?;
    for season in children(state, SEASONS_QUERY, &show.id).await? {
        item.metadata_item
            .push(season_tree(state, &season, "").await?);
    }
    if show.caller_holds_state() {
        item.setting = show_settings();
    }
    Ok(item)
}

async fn season_tree(
    state: &PlexState,
    season: &GqlNode,
    parent_title: &str,
) -> Result<TreeItem, ApiError> {
    let mut item = tree_item(season, parent_title, season.number)?;
    for episode in children(state, EPISODES_QUERY, &season.id).await? {
        let mut entry = tree_item(&episode, parent_title, episode.number)?;
        entry.media_item = tree_versions(&episode.versions, Some(EPISODE_METADATA_TYPE))?;
        item.metadata_item.push(entry);
    }
    Ok(item)
}

/// Every child of a series or a season, a page at a time: a tree is not windowed.
async fn children(state: &PlexState, query: &str, id: &str) -> Result<Vec<GqlNode>, ApiError> {
    let query = with_fragments(query);
    let mut nodes = Vec::new();
    let mut after = None;
    loop {
        let mut variables = Variables::default();
        variables.insert(Name::new("id"), Value::String(id.to_owned()));
        variables.insert(Name::new("first"), Value::Number(MAX_ITEMS.into()));
        if let Some(cursor) = after.take() {
            variables.insert(Name::new("after"), Value::String(cursor));
        }
        let data: NodeData<ChildrenNode> = gql::execute(&state.schema, &query, variables).await?;
        let Some(page) = data.node.map(|node| node.children) else {
            return Ok(nodes);
        };
        nodes.extend(page.edges.into_iter().map(|edge| edge.node));
        match page.page_info.end_cursor {
            Some(cursor) if page.page_info.has_next_page => after = Some(cursor),
            _ => return Ok(nodes),
        }
    }
}

async fn collection_tree(state: &PlexState, key: i64) -> Result<TreeItem, ApiError> {
    let data: NodeData<GqlCollection> = gql::execute(
        &state.schema,
        COLLECTION_QUERY,
        id_variables(COLLECTION_TYPE, key),
    )
    .await?;
    let collection = data.node.ok_or(ApiError::NotFound)?;
    Ok(TreeItem {
        id: number_of(&collection.id)?,
        title: collection.title,
        guid: collection.guid,
        parent_title: String::new(),
        instance_rating_key: "",
        // The reference's collection row keeps the number of the tag it gathers there.
        index: collection.tag_number.parse().ok(),
        originally_available_at: String::new(),
        absolute_index: Some(COLLECTION_ABSOLUTE_INDEX),
        added_at: collection.added_at,
        refreshed_at: ABSENT,
        media_item: Vec::new(),
        metadata_item: Vec::new(),
        setting: Vec::new(),
    })
}

fn id_variables(kind: &str, key: i64) -> Variables {
    let mut variables = Variables::default();
    variables.insert(Name::new("id"), Value::String(GlobalId::encode(kind, key)));
    variables
}

fn number_of(id: &str) -> Result<i64, ApiError> {
    GlobalId::decode(id)
        .map(|id| id.key)
        .map_err(|error| ApiError::Internal(Box::new(error)))
}

fn tree_item(node: &GqlNode, parent_title: &str, index: Option<i64>) -> Result<TreeItem, ApiError> {
    Ok(TreeItem {
        id: number_of(&node.id)?,
        title: node.title.clone().unwrap_or_default(),
        guid: node.guid.clone(),
        parent_title: parent_title.to_owned(),
        instance_rating_key: "",
        index,
        originally_available_at: node.released_on.clone().unwrap_or_default(),
        absolute_index: None,
        added_at: node.added_at,
        refreshed_at: node.refreshed_at.unwrap_or(ABSENT),
        media_item: Vec::new(),
        metadata_item: Vec::new(),
        setting: Vec::new(),
    })
}

fn tree_versions(
    versions: &[GqlVersion],
    metadata_type: Option<i32>,
) -> Result<Vec<TreeVersion>, ApiError> {
    versions
        .iter()
        .map(|version| {
            Ok(TreeVersion {
                id: number_of(&version.id)?,
                media_part: version
                    .files
                    .iter()
                    .map(|file| tree_part(file, metadata_type))
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect()
}

fn tree_part(file: &GqlFile, metadata_type: Option<i32>) -> Result<TreePart, ApiError> {
    let mut tracks = file
        .streams
        .iter()
        .map(|track| Ok((number_of(&track.id)?, track)))
        .collect::<Result<Vec<_>, ApiError>>()?;
    // By index then id, captions carried in the video sharing its index; sidecar files,
    // which have none, behind.
    tracks.sort_by_key(|(id, track)| (track.external_url.is_some(), track.position, *id));
    Ok(TreePart {
        metadata_type,
        id: number_of(&file.id)?,
        file: file.path.clone(),
        size: file.size_bytes.unwrap_or(ABSENT),
        open_subtitle_hash: file.open_subtitles_hash.clone().unwrap_or_default(),
        hash: file.content_hash.clone().unwrap_or_default(),
        duration: file.duration_ms.unwrap_or(ABSENT),
        media_stream: tracks
            .into_iter()
            .map(|(_, track)| tree_track(track))
            .collect::<Result<_, _>>()?,
    })
}

fn tree_track(track: &GqlTrack) -> Result<TreeTrack, ApiError> {
    let kind = match track.kind.as_str() {
        "VIDEO" => 1,
        "AUDIO" => 2,
        _ => 3,
    };
    Ok(TreeTrack {
        id: number_of(&track.id)?,
        index: track.external_url.is_none().then_some(track.position),
        kind,
        url: track.external_url.clone(),
        channels: track.channels,
        codec: track.codec.clone(),
        language: track.language.clone(),
        analysis: analysis_attributes(track),
    })
}

/// A track's analysis under the keys the reference stores it by, its `ma:` and `ln:`
/// prefixes dropped, each value written as the analysis writes it. Sorted by the map.
fn analysis_attributes(track: &GqlTrack) -> BTreeMap<&'static str, Json> {
    let mut keys = AnalysisKeys::default();
    keys.text("title", track.title.as_deref());
    keys.number("streamIdentifier", track.stream_identifier);
    keys.text("profile", track.profile.as_deref());
    keys.number("bitDepth", track.bit_depth);
    if !track.required_bandwidths.is_empty() {
        let kilobits: Vec<String> = track
            .required_bandwidths
            .iter()
            .map(|bits| format!("{:.0}", bits / 1000.0))
            .collect();
        keys.set("requiredBandwidths", kilobits.join(","));
    }

    let traits = &track.traits;
    keys.flag("original", traits.is_original);
    keys.flag("dub", traits.is_dub);
    keys.flag("comment", traits.is_commentary);
    keys.flag("hearingImpaired", traits.is_hearing_impaired);
    keys.flag("visualImpaired", traits.is_visual_impaired);
    keys.flag("headerCompression", traits.has_header_compression);

    if let Some(video) = &track.video {
        video_keys(&mut keys, video);
    }
    if let Some(audio) = &track.audio {
        keys.number("samplingRate", audio.sampling_rate);
        keys.text("audioChannelLayout", audio.channel_layout.as_deref());
        if let Some(loudness) = &audio.loudness {
            keys.set("loudnessAnalysisVersion", loudness.analysis_version.clone());
            keys.set("loudness", format!("{:.2}", loudness.integrated));
            keys.set("lra", format!("{:.2}", loudness.range));
            keys.set("peak", format!("{:.2}", loudness.true_peak));
            keys.set("threshold", format!("{:.2}", loudness.threshold));
            keys.set("gainOffset", format!("{:.2}", loudness.gain_offset));
        }
    }
    if let Some(subtitle) = &track.subtitle {
        keys.text("format", subtitle.format.as_deref());
        keys.flag("embeddedInVideo", subtitle.is_embedded_in_video);
    }
    keys.0
}

fn video_keys(keys: &mut AnalysisKeys, video: &GqlVideo) {
    keys.number("width", video.width);
    keys.number("height", video.height);
    keys.number("codedWidth", video.coded_width);
    keys.number("codedHeight", video.coded_height);
    if let Some(rate) = video.frame_rate {
        keys.set("frameRate", format!("{rate:.3}"));
    }
    keys.number("level", video.level);
    keys.number("refFrames", video.ref_frames);
    keys.text(
        "scanType",
        video.scan.as_deref().map(|scan| match scan {
            "INTERLACED" => "interlaced",
            _ => "progressive",
        }),
    );
    keys.text("chromaSubsampling", video.chroma_subsampling.as_deref());
    keys.text("chromaLocation", video.chroma_location.as_deref());
    keys.text("colorRange", video.color.range.as_deref());
    keys.text("colorSpace", video.color.space.as_deref());
    keys.text("colorPrimaries", video.color.primaries.as_deref());
    keys.text("colorTrc", video.color.transfer.as_deref());
    keys.tristate("hasScalingMatrix", video.has_scaling_matrix);
    keys.tristate("anamorphic", video.is_anamorphic);
    keys.text("pixelAspectRatio", video.pixel_aspect_ratio.as_deref());
    keys.text("codecID", video.fourcc.as_deref());
    keys.flag("closedCaptions", video.has_closed_captions);
    keys.flag("separateFields", video.has_separate_fields);
    keys.flag("HDR10PlusPresent", video.has_hdr10_plus);
    if let Some(dolby) = &video.dolby_vision {
        keys.set("DOVIPresent", "1".to_owned());
        keys.set("DOVIVersion", dolby.version.clone());
        keys.set("DOVIProfile", dolby.profile.to_string());
        keys.set("DOVILevel", dolby.level.to_string());
        keys.number("DOVIBLCompatID", dolby.base_layer_compatibility);
        keys.tristate("DOVIBLPresent", Some(dolby.has_base_layer));
        keys.tristate("DOVIELPresent", Some(dolby.has_enhancement_layer));
        keys.tristate("DOVIRPUPresent", Some(dolby.has_rpu));
    }
}

/// The analysis keys of one track, every value a string as the reference stores them.
#[derive(Default)]
struct AnalysisKeys(BTreeMap<&'static str, Json>);

impl AnalysisKeys {
    fn set(&mut self, key: &'static str, value: String) {
        self.0.insert(key, Json::String(value));
    }

    /// An empty text is not written: a track the file names `""` has no `title` here.
    fn text(&mut self, key: &'static str, value: Option<&str>) {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            self.set(key, value.to_owned());
        }
    }

    fn number(&mut self, key: &'static str, value: Option<impl ToString>) {
        if let Some(value) = value {
            self.set(key, value.to_string());
        }
    }

    /// A flag the analysis writes only when it is set.
    fn flag(&mut self, key: &'static str, set: bool) {
        if set {
            self.set(key, "1".to_owned());
        }
    }

    /// A flag the analysis writes either way, when it looked.
    fn tristate(&mut self, key: &'static str, value: Option<bool>) {
        if let Some(value) = value {
            self.set(key, if value { "1" } else { "0" }.to_owned());
        }
    }
}

/// The languages a series' preferred audio and subtitles can be set to, as the reference
/// lists them: named in its system's language, French, whatever the client's, and ordered
/// by those names. Captured whole.
const LANGUAGE_CHOICES: &str = ":Account default|de-DE:Allemand|en-US:Anglais|en-AU:Anglais (Australie)|en-CA:Anglais (Canada)|en-GB:Anglais (R.-U.)|ar-SA:Arabe (Arabie saoudite)|bg-BG:Bulgare|ca-ES:Catalan|zh-CN:Chinois|zh-HK:Chinois (Hong Kong)|zh-TW:Chinois (Taïwan)|ko-KR:Coréen|hr-HR:Croate|da-DK:Danois|es-ES:Espagnol|es-MX:Espagnol (Mexique)|et-EE:Estonien|fi-FI:Finnois|fr-FR:Français|fr-CA:Français (Canada)|el-GR:Grec|he-IL:Hébreu|hi-IN:Hindi|hu-HU:Hongrois|id-ID:Indonésien|is-IS:Islandais|it-IT:Italien|ja-JP:Japonais|lv-LV:Letton|lt-LT:Lituanien|nl-NL:Néerlandais|nb-NO:Norvégien bokmål|fa-IR:Persan|pl-PL:Polonais|pt-BR:Portugais|pt-PT:Portugais (Portugal)|ro-RO:Roumain|ru-RU:Russe|sk-SK:Slovaque|sv-SE:Suédois|cs-CZ:Tchèque|th-TH:Thaï|tr-TR:Turc|uk-UA:Ukrainien|vi-VN:Vietnamien";

/// A series' preferences, which the reference writes into its tree. None is kept here, so
/// each stands at its default; the labels are the reference's, untranslated for every client.
fn show_settings() -> Vec<Setting> {
    let setting = |id, label, summary, kind, default: &str, enum_values: Option<&str>| Setting {
        id,
        label,
        summary,
        kind,
        default: default.to_owned(),
        value: default.to_owned(),
        hidden: false,
        advanced: false,
        group: "",
        enum_values: enum_values.map(ToOwned::to_owned),
    };
    vec![
        setting(
            "episodeSort",
            "Episode sorting",
            "How to sort the episodes for this show.",
            "text",
            "-1",
            Some("-1:Library default|0:Oldest first|1:Newest first"),
        ),
        setting(
            "autoDeletionItemPolicyUnwatchedLibrary",
            "Keep",
            "Set the maximum number of unplayed episodes to keep for the show.",
            "int",
            "0",
            Some("0:All episodes|5:5 latest episodes|3:3 latest episodes|1:Latest episode|-3:Episodes added in the past 3 days|-7:Episodes added in the past 7 days|-30:Episodes added in the past 30 days"),
        ),
        setting(
            "autoDeletionItemPolicyWatchedLibrary",
            "Delete episodes after playing",
            "Choose how quickly episodes are removed after the server admin has watched them.",
            "int",
            "0",
            Some("0:Never|1:After a day|7:After a week|30:After a month|100:On next refresh"),
        ),
        setting(
            "flattenSeasons",
            "Seasons",
            "Choose whether to display seasons.",
            "int",
            "-1",
            Some("-1:Library default|0:Show|1:Hide"),
        ),
        setting("showOrdering", "", "", "text", "", None),
        setting(
            "audioLanguage",
            "Preferred audio language",
            "",
            "text",
            "",
            Some(LANGUAGE_CHOICES),
        ),
        setting(
            "subtitleLanguage",
            "Preferred subtitle language",
            "",
            "text",
            "",
            Some(LANGUAGE_CHOICES),
        ),
        setting(
            "subtitleMode",
            "Auto-select subtitle mode",
            "",
            "int",
            "-1",
            Some("-1:Account default|0:Manually selected|1:Shown with foreign audio|2:Always enabled"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    fn track(official: &Json) -> Json {
        let track: GqlTrack = serde_json::from_value(official.clone()).unwrap();
        serde_json::to_value(tree_track(&track).unwrap()).unwrap()
    }

    fn no_traits() -> Json {
        json!({
            "isOriginal": false, "isDub": false, "isCommentary": false,
            "isHearingImpaired": false, "isVisualImpaired": false, "hasHeaderCompression": false
        })
    }

    #[test]
    fn writes_dolby_vision_before_the_lower_case_keys() {
        // Track 11990163 of the reference, as its tree writes it.
        let official = json!({
            "id": GlobalId::encode("MediaStream", 11_990_163), "kind": "VIDEO", "position": 0,
            "codec": "hevc", "language": "en", "channels": null, "externalUrl": null,
            "title": null, "streamIdentifier": null, "profile": "main 10", "bitDepth": 10,
            "requiredBandwidths": [76_457_000, 58_845_000, 27_378_000, 26_588_000, 25_924_000,
                25_344_000, 25_196_000, 25_196_000],
            "traits": no_traits(),
            "video": {
                "width": 3840, "height": 1920, "codedWidth": 3840, "codedHeight": 1920,
                "frameRate": 23.976, "level": 150, "refFrames": 1, "scan": null,
                "chromaSubsampling": "4:2:0", "chromaLocation": "topleft",
                "color": { "range": "tv", "space": "bt2020nc", "primaries": "bt2020",
                    "transfer": "smpte2084" },
                "hasScalingMatrix": null, "isAnamorphic": null, "pixelAspectRatio": null,
                "fourcc": null, "hasClosedCaptions": false, "hasSeparateFields": false,
                "hasHdr10Plus": true,
                "dolbyVision": { "version": "1.0", "profile": 8, "level": 6,
                    "baseLayerCompatibility": 1, "hasBaseLayer": true,
                    "hasEnhancementLayer": false, "hasRpu": true }
            },
            "audio": null, "subtitle": null
        });
        let written = track(&official);
        let keys: Vec<&str> = written
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "id",
                "index",
                "type",
                "codec",
                "language",
                "DOVIBLCompatID",
                "DOVIBLPresent",
                "DOVIELPresent",
                "DOVILevel",
                "DOVIPresent",
                "DOVIProfile",
                "DOVIRPUPresent",
                "DOVIVersion",
                "HDR10PlusPresent",
                "bitDepth",
                "chromaLocation",
                "chromaSubsampling",
                "codedHeight",
                "codedWidth",
                "colorPrimaries",
                "colorRange",
                "colorSpace",
                "colorTrc",
                "frameRate",
                "height",
                "level",
                "profile",
                "refFrames",
                "requiredBandwidths",
                "width"
            ]
        );
        assert_eq!(written["DOVIELPresent"], "0");
        assert_eq!(written["frameRate"], "23.976");
        assert_eq!(
            written["requiredBandwidths"],
            "76457,58845,27378,26588,25924,25344,25196,25196"
        );
    }

    #[test]
    fn writes_loudness_to_two_decimals_among_the_analysis() {
        // Track 1668218 of the reference.
        let official = json!({
            "id": GlobalId::encode("MediaStream", 1_668_218), "kind": "AUDIO", "position": 1,
            "codec": "ac3", "language": "en", "channels": 6, "externalUrl": null,
            "title": "French (Canada) : Dolby Digital 5.1", "streamIdentifier": null,
            "profile": null, "bitDepth": null, "requiredBandwidths": vec![448_000; 8],
            "traits": no_traits(), "video": null,
            "audio": { "samplingRate": 48000, "channelLayout": "5.1(side)",
                "loudness": { "analysisVersion": "0.02", "integrated": -26.8, "range": 14.9,
                    "truePeak": -2.15, "threshold": -37.85, "gainOffset": 0.07 } },
            "subtitle": null
        });
        assert_eq!(
            track(&official),
            json!({
                "id": 1_668_218, "index": 1, "type": 2, "channels": 6, "codec": "ac3",
                "language": "en", "audioChannelLayout": "5.1(side)", "gainOffset": "0.07",
                "loudness": "-26.80", "loudnessAnalysisVersion": "0.02", "lra": "14.90",
                "peak": "-2.15", "requiredBandwidths": "448,448,448,448,448,448,448,448",
                "samplingRate": "48000", "threshold": "-37.85",
                "title": "French (Canada) : Dolby Digital 5.1"
            })
        );
    }

    #[test]
    fn a_sidecar_has_its_address_and_no_index() {
        // Track 11247877 of the reference, a subtitle beside an episode.
        let url = "file:///media/The%20Walking%20Dead%20S04E03.en.srt";
        let official = json!({
            "id": GlobalId::encode("MediaStream", 11_247_877), "kind": "SUBTITLE",
            "position": 0, "codec": "srt", "language": "en", "channels": null,
            "externalUrl": url, "title": null, "streamIdentifier": null, "profile": null,
            "bitDepth": null, "requiredBandwidths": [], "traits": no_traits(), "video": null,
            "audio": null, "subtitle": { "format": "srt", "isEmbeddedInVideo": false }
        });
        let written = track(&official);
        let keys: Vec<&str> = written
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["id", "type", "url", "codec", "language", "format"]);
        assert_eq!(written["url"], url);
    }

    #[test]
    fn leaves_out_an_empty_name_and_keeps_the_unfillable_bandwidth() {
        // Track 8975927 of the reference, named `""` by its file, and a bandwidth the analysis
        // writes as the largest 32-bit integer.
        let official = json!({
            "id": GlobalId::encode("MediaStream", 8_975_927), "kind": "SUBTITLE", "position": 2,
            "codec": "srt", "language": "fr", "channels": null, "externalUrl": null,
            "title": "", "streamIdentifier": null, "profile": null, "bitDepth": null,
            "requiredBandwidths": [2_147_483_647_000.0, 165_438_000.0],
            "traits": no_traits(), "video": null, "audio": null,
            "subtitle": { "format": null, "isEmbeddedInVideo": false }
        });
        let written = track(&official);
        assert!(written.get("title").is_none());
        assert_eq!(written["requiredBandwidths"], "2147483647,165438");
    }
}
