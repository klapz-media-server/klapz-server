//! Extras on the Plex surface: `GET /library/metadata/{ids}/extras`, `Extras` on a lookup with
//! `includeExtras=1`, and `primaryExtraKey` on films and series.
//!
//! Measured on the reference's test libraries, with local extras of every kind beside a film,
//! a series, a season and an episode:
//!
//! - the list is the item's own extras, by position then id, whatever the item's kind: a
//!   series lists its own, not its seasons'. `extraType`, `type` and `sort` change nothing;
//!   `X-Plex-Container-Start` and `-Size` together window it, with `totalSize`, `offset` and
//!   the paging headers;
//! - a list of ids, a number naming nothing or something that is no item is a `404`; an
//!   extra's own extras a `400`;
//! - an entry is a `clip` element with `subtype` naming the kind and `extraType` numbering it;
//!   in a lookup's `Extras` it is the same without `art`. `Extras` comes last, after
//!   `Location`, and only on `includeExtras=1` exactly: on every item of a lookup, an empty
//!   one included, and on no listing;
//! - `primaryExtraKey` names the first trailer, on films and series wherever they are listed,
//!   and is absent without a trailer.
//!
//! The reference hides the trailers its metadata provider supplied from an account without a
//! subscription, which is all of them on the reference library: their entries follow the
//! reference's lookup of each, with `premium`, `protocol` and the provider's address as the
//! part's key. Artwork is left out, as on items.
//!
//! `POST` adds an extra from a URL the client names, which is a path by another name: refused
//! `403`, after the checks the reference makes first.

use std::sync::LazyLock;

use async_graphql::Variables;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use klapz_api::extra::EXTRA_TYPE;
use klapz_api::item::ITEM_TYPE;
use klapz_api::GlobalId;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{database_id, with_shared_fragments, GqlExtra, GqlItem, GqlTrackRef};
use crate::items::{MediaEntry, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION};
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/library/metadata/{ids}/extras", get(list).post(add))
}

/// Whether a lookup asks for extras: `includeExtras=1`, and no other value.
pub fn requested(pairs: &[(String, String)]) -> bool {
    pairs
        .iter()
        .any(|(key, value)| key == "includeExtras" && value == "1")
}

/// An item's extras inside its lookup.
#[derive(Debug, Serialize)]
pub struct ExtrasEntry {
    size: u32,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<ExtraEntry>,
}

impl ExtrasEntry {
    /// The item's extras, when the query asked for them.
    pub(crate) fn embedded(item: &GqlItem, language: Language) -> Option<Self> {
        let metadata: Vec<ExtraEntry> = item
            .extras
            .as_ref()?
            .nodes()
            .map(|extra| ExtraEntry::from_graphql(extra, &item.guid, language))
            .collect();
        Some(Self {
            size: u32::try_from(metadata.len()).unwrap_or(u32::MAX),
            metadata,
        })
    }
}

/// The key of a film's or a series' first trailer.
pub(crate) fn primary_extra_key(item: &GqlItem) -> Option<String> {
    let extra = item.primary_extra.as_ref()?;
    Some(format!("/library/metadata/{}", database_id(&extra.id)))
}

/// One extra, as a `clip`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraEntry {
    rating_key: String,
    key: String,
    guid: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_sort: Option<String>,
    /// Always there, empty when the extra has none.
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    year: Option<i32>,
    /// The work it comes with, when that is one of Plex's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    primary_guid: Option<String>,
    subtype: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    originally_available_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    added_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<i64>,
    extra_type: u8,
    #[serde(rename = "Media", skip_serializing_if = "Vec::is_empty")]
    media: Vec<MediaEntry>,
}

/// The reference's name and number for a kind, as the official API spells it.
fn plex_kind(kind: &str) -> (&'static str, u8) {
    match kind {
        "TRAILER" => ("trailer", 1),
        "DELETED_SCENE" => ("deletedScene", 2),
        "INTERVIEW" => ("interview", 3),
        "MUSIC_VIDEO" => ("musicVideo", 4),
        "BEHIND_THE_SCENES" => ("behindTheScenes", 5),
        "SCENE" => ("sceneOrSample", 6),
        "LIVE_MUSIC_VIDEO" => ("liveMusicVideo", 7),
        "LYRIC_MUSIC_VIDEO" => ("lyricMusicVideo", 8),
        "CONCERT" => ("concert", 9),
        "FEATURETTE" => ("featurette", 10),
        "SHORT" => ("short", 11),
        _ => ("other", 12),
    }
}

impl ExtraEntry {
    fn from_graphql(extra: &GqlExtra, owner_guid: &str, language: Language) -> Self {
        let id = extra.database_id();
        let (subtype, extra_type) = plex_kind(&extra.kind);
        Self {
            rating_key: id.to_string(),
            key: format!("/library/metadata/{id}"),
            guid: extra.guid.clone(),
            kind: "clip",
            title: extra.title.clone(),
            title_sort: extra
                .title_sort
                .clone()
                .filter(|sort| Some(sort) != extra.title.as_ref()),
            summary: extra.summary.clone().unwrap_or_default(),
            index: extra.position,
            year: extra.year,
            // Measured on two provider trailers of films matched by Plex's agent, and absent
            // from the local extras of an unmatched film.
            primary_guid: owner_guid
                .starts_with("plex://")
                .then(|| owner_guid.to_owned()),
            subtype,
            // The media's, as an item's: the reference publishes it where the extra's own
            // row has none, on 63 of 264 trailers compared.
            duration: extra
                .versions
                .first()
                .and_then(|version| version.duration_ms)
                .or(extra.duration_ms),
            originally_available_at: extra.released_on.clone(),
            added_at: extra.added_at,
            updated_at: extra.updated_at,
            extra_type,
            media: extra
                .versions
                .iter()
                .map(|version| {
                    let mut media = MediaEntry::from_graphql(version, language);
                    let streamed: Vec<Option<&String>> = version
                        .files
                        .iter()
                        .map(|file| file.stream_url.as_ref())
                        .collect();
                    if streamed.iter().any(Option::is_some) {
                        // A provider's version carries its own flag rather than its file's,
                        // and its protocol with it.
                        let optimized = version.optimized_for_streaming;
                        media.optimized_for_streaming = optimized.then_some(1);
                        media.protocol = optimized.then(|| version.container.clone()).flatten();
                        media.premium = Some(true);
                        for (part, url) in media.part.iter_mut().zip(streamed) {
                            if let Some(url) = url {
                                part.key.clone_from(url);
                            }
                        }
                    }
                    media
                })
                .collect(),
        }
    }
}

/// `GET /library/metadata/{ids}/extras`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtrasContainer {
    size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<ExtraEntry>,
}

static LIST: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexExtras($item: ID!, $extra: ID!, $extras: Boolean!) {
  item: node(id: $item) {
    ... on Item { __typename guid }
    ...Extras
  }
  extra: node(id: $extra) { id }
}
",
    )
});

#[derive(Debug, Deserialize)]
struct ListData {
    item: Option<GqlItem>,
    extra: Option<GqlTrackRef>,
}

/// The item a path names, with its extras when asked: `404` when it names none the caller
/// reads, `400` when it names an extra.
async fn item_of(state: &PlexState, ids: &str, extras: bool) -> Result<GqlItem, ApiError> {
    let key: i64 = ids.trim().parse().map_err(|_| ApiError::NotFound)?;
    let variables = Variables::from_json(json!({
        "item": GlobalId::encode(ITEM_TYPE, key),
        "extra": GlobalId::encode(EXTRA_TYPE, key),
        "extras": extras,
    }));
    let data: ListData = gql::execute(&state.schema, &LIST, variables).await?;
    match (data.item, data.extra) {
        (Some(item), _) if !item.typename.is_empty() => Ok(item),
        (_, Some(_)) => Err(ApiError::BadRequest),
        _ => Err(ApiError::NotFound),
    }
}

/// The window the client asked for: both parameters, or none.
fn window(pairs: &[(String, String)]) -> Option<(usize, usize)> {
    let value = |name: &str| {
        pairs
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| value.parse().ok())
    };
    Some((
        value("X-Plex-Container-Start")?,
        value("X-Plex-Container-Size")?,
    ))
}

async fn list(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<ExtrasContainer>, ApiError> {
    let item = item_of(&state, &ids, true).await?;
    let language = Language::of(&client);
    let mut metadata: Vec<ExtraEntry> = item
        .extras
        .unwrap_or_default()
        .nodes()
        .map(|extra| ExtraEntry::from_graphql(extra, &item.guid, language))
        .collect();
    let total = metadata.len();
    let paged = window(&pairs).map(|(start, size)| {
        let start = start.min(total);
        metadata = metadata.drain(start..).take(size).collect();
        start
    });
    let body = ExtrasContainer {
        size: u32::try_from(metadata.len()).unwrap_or(u32::MAX),
        total_size: paged.map(|_| total as u64),
        offset: paged.map(|start| start as u64),
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        metadata,
    };
    let response = PlexResponse::new(body, format);
    Ok(match paged {
        Some(start) => response.paged(start as u64, total as u64),
        None => response,
    })
}

/// `POST /library/metadata/{ids}/extras`: refused once the request is one the reference would
/// have tried, since what it names is a file or an address to fetch.
async fn add(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> impl IntoResponse {
    if let Err(refusal) = item_of(&state, &ids, false).await {
        return refusal;
    }
    let given = |name: &str| pairs.iter().any(|(key, _)| key == name);
    if !given("url") || !given("title") {
        return ApiError::BadRequest;
    }
    ApiError::Forbidden
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_carry_the_reference_names_and_numbers() {
        assert_eq!(plex_kind("TRAILER"), ("trailer", 1));
        assert_eq!(plex_kind("BEHIND_THE_SCENES"), ("behindTheScenes", 5));
        assert_eq!(plex_kind("SCENE"), ("sceneOrSample", 6));
        assert_eq!(plex_kind("FEATURETTE"), ("featurette", 10));
        assert_eq!(plex_kind("OTHER"), ("other", 12));
    }

    #[test]
    fn extras_are_asked_for_by_one_value_only() {
        let pairs = |value: &str| vec![("includeExtras".to_owned(), value.to_owned())];
        assert!(requested(&pairs("1")));
        assert!(!requested(&pairs("0")));
        assert!(!requested(&pairs("true")));
        assert!(!requested(&[]));
    }
}
