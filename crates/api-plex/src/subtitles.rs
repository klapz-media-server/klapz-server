//! Subtitle files: `/library/streams/{streamId}.{ext}`, bound onto `MediaStream.sidecar`,
//! `setSubtitleOffset` and `deleteSidecar`; and `/library/metadata/{ids}/subtitles`, the
//! search of online subtitles.
//!
//! Measured on the reference's test libraries, with sidecars put beside a film and taken away:
//!
//! - `GET` serves a sidecar's file as it is, at `/library/streams/{id}` or with an extension
//!   that is its format, in any case (`.srt`, `.SRT`, `.` alone): another one is a `404`, and
//!   so is an id that is no number. A track inside the media is a `501`, whatever its kind.
//!   `format` converts nothing, `encoding` (any value) answers UTF-8, and a single `Range` is
//!   honored. The reference types it `text/html`; here it is `text/plain`, so that a subtitle
//!   holding markup is never run as a page of this server's origin.
//! - `PUT ?offset=` keeps the caller's offset in milliseconds, a 32-bit integer, `+` allowed,
//!   on any subtitle: a lookup then publishes it as `offset`, and `0` removes it. Without
//!   `offset` nothing changes. An extension, an unknown id, a value that is no integer are
//!   `400`s. An audio or video track answers `200` and publishes nothing.
//! - `DELETE` removes a sidecar's file from disk and its track. An extension or an id that is
//!   no number is a `400`, an unknown id a `404`, a track inside the media a `403`.

use async_graphql::Variables;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use klapz_api::item::ITEM_TYPE;
use klapz_api::media::{STREAM_TYPE, VERSION_TYPE};
use klapz_api::GlobalId;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::items::{LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION};
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route(
            "/library/streams/{streamId}",
            get(read_stream).put(set_offset).delete(delete_stream),
        )
        .route("/library/metadata/{ids}/subtitles", get(search))
}

/// The `{streamId}.{ext}` segment: an id of digits, then whatever follows its first dot.
#[derive(Debug, PartialEq, Eq)]
struct Segment<'a> {
    id: i64,
    /// Present, possibly empty, when the segment has a dot.
    extension: Option<&'a str>,
}

impl<'a> Segment<'a> {
    fn parse(segment: &'a str) -> Option<Self> {
        let (id, extension) = match segment.split_once('.') {
            Some((id, extension)) => (id, Some(extension)),
            None => (segment, None),
        };
        // Digits only, as the reference's route pattern reads them: no sign, no space.
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        Some(Self {
            id: id.parse().ok()?,
            extension,
        })
    }

    fn track(&self) -> String {
        GlobalId::encode(STREAM_TYPE, self.id)
    }
}

const SIDECAR: &str = r"
query PlexSidecar($id: ID!) {
  node(id: $id) {
    ... on MediaStream {
      codec
      subtitle { format }
      sidecar { text }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct SidecarData {
    node: Option<SidecarTrack>,
}

#[derive(Debug, Deserialize)]
struct SidecarTrack {
    codec: Option<String>,
    subtitle: Option<SidecarFormat>,
    sidecar: Option<SidecarText>,
}

#[derive(Debug, Deserialize)]
struct SidecarFormat {
    format: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SidecarText {
    text: Option<String>,
}

impl SidecarTrack {
    /// What the extension must name: the sidecar's format, else its codec.
    fn format(&self) -> Option<&str> {
        self.subtitle
            .as_ref()
            .and_then(|subtitle| subtitle.format.as_deref())
            .or(self.codec.as_deref())
    }
}

/// `GET /library/streams/{streamId}.{ext}`.
async fn read_stream(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(segment): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let segment = Segment::parse(&segment).ok_or(ApiError::NotFound)?;
    let variables = Variables::from_json(json!({ "id": segment.track() }));
    let data: SidecarData = gql::execute(&state.schema, SIDECAR, variables).await?;
    let track = data.node.ok_or(ApiError::NotFound)?;
    let sidecar = track.sidecar.as_ref().ok_or(ApiError::NotImplemented)?;
    if let Some(extension) = segment.extension.filter(|extension| !extension.is_empty()) {
        if !track
            .format()
            .is_some_and(|format| format.eq_ignore_ascii_case(extension))
        {
            return Err(ApiError::NotFound);
        }
    }
    let text = sidecar.text.as_deref().ok_or(ApiError::NotFound)?;
    Ok(ranged(text.as_bytes().to_vec(), &headers))
}

/// The body whole, or the one range a `Range` header asks for. Anything else in the header —
/// several ranges, units other than bytes, a range past the end — is ignored, as a server may.
fn ranged(body: Vec<u8>, headers: &HeaderMap) -> Response {
    let total = body.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| byte_range(value, total));
    let mut response = match range {
        Some((start, end)) => {
            let mut response =
                (StatusCode::PARTIAL_CONTENT, body[start..=end].to_vec()).into_response();
            if let Ok(value) = HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            response
        }
        None => (StatusCode::OK, body).into_response(),
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// The first and last byte of one satisfiable range of a body of `total` bytes.
fn byte_range(header: &str, total: usize) -> Option<(usize, usize)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || total == 0 {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let last = total - 1;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let length: usize = suffix.parse().ok()?;
            (total.checked_sub(length.min(total))?, last)
        }
        (start, "") => (start.parse().ok()?, last),
        (start, end) => (start.parse().ok()?, end.parse::<usize>().ok()?.min(last)),
    };
    (start <= end && start <= last).then_some((start, end))
}

const SET_OFFSET: &str = r"
mutation PlexSubtitleOffset($input: SetSubtitleOffsetInput!) {
  setSubtitleOffset(input: $input) { userErrors { code } }
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetOffsetData {
    set_subtitle_offset: Refusals,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Refusals {
    user_errors: Vec<UserError>,
}

#[derive(Debug, Deserialize)]
struct UserError {
    code: String,
}

impl Refusals {
    fn code(&self) -> Option<&str> {
        self.user_errors.first().map(|error| error.code.as_str())
    }
}

#[derive(Debug, Deserialize)]
struct OffsetParams {
    offset: Option<String>,
}

const TRACK_EXISTS: &str = r"
query PlexTrackExists($id: ID!) {
  node(id: $id) { ... on MediaStream { kind } }
}
";

#[derive(Debug, Deserialize)]
struct TrackExistsData {
    node: Option<serde::de::IgnoredAny>,
}

/// `PUT /library/streams/{streamId}?offset=`.
async fn set_offset(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(segment): Path<String>,
    Query(params): Query<OffsetParams>,
) -> Result<impl IntoResponse, ApiError> {
    let segment = Segment::parse(&segment)
        .filter(|segment| segment.extension.is_none())
        .ok_or(ApiError::BadRequest)?;
    let Some(offset) = params.offset else {
        // The reference looks the track up even when there is nothing to change.
        let variables = Variables::from_json(json!({ "id": segment.track() }));
        let data: TrackExistsData = gql::execute(&state.schema, TRACK_EXISTS, variables).await?;
        return match data.node {
            Some(_) => Ok(empty_ok()),
            None => Err(ApiError::BadRequest),
        };
    };
    let offset: i32 = offset.parse().map_err(|_| ApiError::BadRequest)?;
    let variables = Variables::from_json(json!({
        "input": { "trackId": segment.track(), "offsetMs": offset }
    }));
    let data: SetOffsetData = gql::execute(&state.schema, SET_OFFSET, variables).await?;
    match data.set_subtitle_offset.code() {
        // Kept nowhere, as nothing publishes it: the reference answers the same.
        None | Some("NOT_A_SUBTITLE") => Ok(empty_ok()),
        Some(_) => Err(ApiError::BadRequest),
    }
}

const DELETE_SIDECAR: &str = r"
mutation PlexDeleteSidecar($input: DeleteSidecarInput!) {
  deleteSidecar(input: $input) { userErrors { code } }
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteData {
    delete_sidecar: Refusals,
}

/// `DELETE /library/streams/{streamId}`.
async fn delete_stream(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(segment): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let segment = Segment::parse(&segment)
        .filter(|segment| segment.extension.is_none())
        .ok_or(ApiError::BadRequest)?;
    let variables = Variables::from_json(json!({ "input": { "trackId": segment.track() } }));
    let data: DeleteData = gql::execute(&state.schema, DELETE_SIDECAR, variables).await?;
    match data.delete_sidecar.code() {
        None => Ok(empty_ok()),
        Some("TRACK_NOT_FOUND") => Err(ApiError::NotFound),
        Some(_) => Err(ApiError::Forbidden),
    }
}

fn empty_ok() -> impl IntoResponse {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")])
}

const ITEM_MEDIA: &str = r"
query PlexSubtitleSearchItem($id: ID!) {
  node(id: $id) {
    ... on Movie { versions { id } library { viewerRole } }
    ... on Episode { versions { id } library { viewerRole } }
  }
}
";

#[derive(Debug, Deserialize)]
struct ItemMediaData {
    node: Option<ItemMedia>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ItemMedia {
    versions: Option<Vec<VersionRef>>,
    library: Option<LibraryRole>,
}

#[derive(Debug, Deserialize)]
struct VersionRef {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryRole {
    viewer_role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    language: Option<String>,
    #[serde(rename = "mediaItemID")]
    media_item_id: Option<String>,
}

/// The empty answer of a search that found nothing, as the reference writes it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubtitleResults {
    size: u32,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
}

/// `GET /library/metadata/{ids}/subtitles`: the subtitles online providers hold for a film or
/// an episode.
///
/// The reference asks a subtitle provider through Plex's own service and lists what it finds
/// as tracks to download. There is no provider here, so the answer is the one the reference
/// gives when its provider finds nothing — after the same checks: a language of two letters,
/// an optional region (`fr`, `fr-FR`), where the reference fails a three-letter code with a
/// `500` and this a `400`; one film or episode, and a version of it for `mediaItemID`.
async fn search(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(params): Query<SearchParams>,
    format: ResponseFormat,
) -> Result<impl IntoResponse, ApiError> {
    let language = params.language.as_deref().ok_or(ApiError::BadRequest)?;
    if !is_provider_language(language) {
        return Err(ApiError::BadRequest);
    }
    let id: i64 = ids.parse().map_err(|_| ApiError::NotFound)?;
    let variables = Variables::from_json(json!({ "id": GlobalId::encode(ITEM_TYPE, id) }));
    let data: ItemMediaData = gql::execute(&state.schema, ITEM_MEDIA, variables).await?;
    let item = data.node.unwrap_or_default();
    let versions = item.versions.ok_or(ApiError::NotFound)?;
    if let Some(version) = params.media_item_id.as_deref() {
        let version: i64 = version.parse().map_err(|_| ApiError::NotFound)?;
        let wanted = GlobalId::encode(VERSION_TYPE, version);
        if !versions.iter().any(|held| held.id == wanted) {
            return Err(ApiError::NotFound);
        }
    }
    // Adding subtitles changes the library: its admins' and the owners'.
    let role = item.library.and_then(|library| library.viewer_role);
    if role.as_deref() != Some("ADMIN") {
        return Err(ApiError::Forbidden);
    }
    let results = SubtitleResults {
        size: 0,
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
    };
    Ok(PlexResponse::new(results, format))
}

/// A language as the reference's provider takes it: two letters, then optionally a region.
fn is_provider_language(language: &str) -> bool {
    let mut parts = language.split(['-', '_']);
    let primary = parts.next().unwrap_or_default();
    let region = parts.next();
    primary.len() == 2
        && primary.bytes().all(|byte| byte.is_ascii_alphabetic())
        && region.is_none_or(|region| {
            !region.is_empty() && region.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        && parts.next().is_none()
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn reads_the_id_and_what_follows_its_first_dot() {
        assert_eq!(
            Segment::parse("12265847"),
            Some(Segment {
                id: 12_265_847,
                extension: None
            })
        );
        assert_eq!(Segment::parse("012265847.srt").unwrap().id, 12_265_847);
        assert_eq!(Segment::parse("1.").unwrap().extension, Some(""));
        assert_eq!(
            Segment::parse("1.srt.srt").unwrap().extension,
            Some("srt.srt")
        );
        assert_eq!(Segment::parse("abc.srt"), None);
        assert_eq!(Segment::parse("-1"), None);
        assert_eq!(Segment::parse(".srt"), None);
        assert_eq!(Segment::parse("99999999999999999999"), None);
    }

    #[test]
    fn honors_one_satisfiable_byte_range() {
        assert_eq!(byte_range("bytes=0-9", 1361), Some((0, 9)));
        assert_eq!(byte_range("bytes=100-", 1361), Some((100, 1360)));
        assert_eq!(byte_range("bytes=-10", 1361), Some((1351, 1360)));
        assert_eq!(byte_range("bytes=1300-5000", 1361), Some((1300, 1360)));
        assert_eq!(byte_range("bytes=-5000", 1361), Some((0, 1360)));
        assert_eq!(byte_range("bytes=2000-", 1361), None);
        assert_eq!(byte_range("bytes=9-0", 1361), None);
        assert_eq!(byte_range("bytes=0-1,5-6", 1361), None);
        assert_eq!(byte_range("items=0-1", 1361), None);
        assert_eq!(byte_range("bytes=0-0", 0), None);
    }

    #[test]
    fn takes_the_languages_the_provider_takes() {
        for language in ["fr", "zu", "fr-FR", "pt_BR", "EN"] {
            assert!(is_provider_language(language), "{language}");
        }
        for language in ["", "fra", "French", "f", "fr-", "fr-FR-x", "f1"] {
            assert!(!is_provider_language(language), "{language}");
        }
    }
}
