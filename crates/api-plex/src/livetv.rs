//! Live TV and DVR: tuners, the guide, recordings and live sessions.
//!
//! This server has no grabber to drive a tuner and no guide provider, so every list here is
//! empty and every DVR, device, recording, session or guide it could be asked about does not
//! exist. The answers are the reference's when it has no DVR: an empty `MediaContainer` for
//! a list, `404` for a thing it does not know, `400` for a lookup missing the parameter that
//! names it. There is no data model behind them, which is why they live in the binding.
//!
//! Nothing here is refused for want of a subscription: the reference does refuse some of
//! these (`401` on `/media/subscriptions`), and that refusal is what this server does not
//! copy.

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::Router;
use serde::Serialize;

use crate::auth::{AdminCaller, Caller};
use crate::error::ApiError;
use crate::response::{PlexResponse, ResponseFormat, CONTAINER_START};
use crate::PlexState;

type Pairs = Query<Vec<(String, String)>>;

pub fn router() -> Router<PlexState> {
    dvrs()
        .merge(guide())
        .merge(grabbers())
        .merge(subscriptions())
}

fn dvrs() -> Router<PlexState> {
    Router::new()
        .route("/livetv/dvrs", get(nothing_listed).post(create_dvr))
        .route("/livetv/dvrs/{dvrId}", get(absent).delete(absent_to_admin))
        .route("/livetv/dvrs/{dvrId}/channels/{channel}/tune", post(absent))
        .route(
            "/livetv/dvrs/{dvrId}/devices/{deviceId}",
            put(absent_to_admin).delete(absent_to_admin),
        )
        .route(
            "/livetv/dvrs/{dvrId}/lineups",
            put(absent_to_admin).delete(absent_to_admin),
        )
        .route("/livetv/dvrs/{dvrId}/prefs", put(absent_to_admin))
        .route(
            "/livetv/dvrs/{dvrId}/reloadGuide",
            post(absent_to_admin).delete(absent_to_admin),
        )
        .route("/livetv/sessions", get(nothing_listed))
        .route("/livetv/sessions/{sessionId}", get(absent))
        .route(
            "/livetv/sessions/{sessionId}/{consumerId}/index.m3u8",
            get(absent),
        )
        .route(
            "/livetv/sessions/{sessionId}/{consumerId}/{segmentId}",
            get(absent),
        )
}

fn guide() -> Router<PlexState> {
    Router::new()
        .route("/livetv/epg/countries", get(nothing_listed))
        .route("/livetv/epg/languages", get(nothing_listed))
        // Under a guide provider's identifier, and there is none: an unknown provider is a
        // `404` on the reference.
        .route(
            "/livetv/epg/countries/{country}/{epgId}/lineups",
            get(absent),
        )
        .route(
            "/livetv/epg/countries/{country}/{epgId}/regions",
            get(absent),
        )
        .route(
            "/livetv/epg/countries/{country}/{epgId}/regions/{region}/lineups",
            get(absent),
        )
        .route(
            "/livetv/epg/channelmap",
            get(|_: Caller, Query(pairs): Pairs| async move {
                unknown(&pairs, &["device", "lineup"])
            }),
        )
        .route(
            "/livetv/epg/channels",
            get(|_: Caller, Query(pairs): Pairs| async move { unknown(&pairs, &["lineup"]) }),
        )
        .route(
            "/livetv/epg/lineupchannels",
            get(|_: Caller, Query(pairs): Pairs| async move { unknown(&pairs, &["lineup"]) }),
        )
        .route(
            "/livetv/epg/lineup",
            get(|_: Caller, Query(pairs): Pairs| async move {
                unknown(&pairs, &["device", "lineupGroup"])
            }),
        )
}

fn grabbers() -> Router<PlexState> {
    Router::new()
        .route("/media/grabbers", get(nothing_listed))
        .route(
            "/media/grabbers/devices",
            get(nothing_listed_to_admin).post(add_device),
        )
        .route("/media/grabbers/devices/discover", post(discover))
        .route(
            "/media/grabbers/devices/{deviceId}",
            get(absent_to_admin)
                .put(absent_to_admin)
                .delete(absent_to_admin),
        )
        .route("/media/grabbers/devices/{deviceId}/channelmap", put(absent))
        .route(
            "/media/grabbers/devices/{deviceId}/channels",
            get(absent_to_admin),
        )
        .route(
            "/media/grabbers/devices/{deviceId}/prefs",
            put(absent_to_admin),
        )
        .route(
            "/media/grabbers/devices/{deviceId}/scan",
            post(absent).delete(absent),
        )
        .route(
            "/media/grabbers/devices/{deviceId}/thumb/{version}",
            get(absent_to_admin),
        )
        .route("/media/grabbers/operations/{operationId}", delete(absent))
}

fn subscriptions() -> Router<PlexState> {
    Router::new()
        .route("/media/subscriptions", get(nothing_listed).post(subscribe))
        .route("/media/subscriptions/process", post(process))
        .route("/media/subscriptions/scheduled", get(nothing_listed))
        .route("/media/subscriptions/template", get(template))
        .route(
            "/media/subscriptions/{subscriptionId}",
            get(absent).put(absent).delete(absent),
        )
        .route(
            "/media/subscriptions/{subscriptionId}/move",
            put(absent_to_admin),
        )
}

/// An empty list: `<MediaContainer size="0">`, with `totalSize` and `offset` when the
/// client asked for a window, as on the reference.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NothingListed {
    size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
}

fn value<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn empty_list(pairs: &[(String, String)], format: ResponseFormat) -> PlexResponse<NothingListed> {
    // Both or neither, as everywhere a Plex client pages.
    let start = value(pairs, CONTAINER_START)
        .zip(value(pairs, "X-Plex-Container-Size"))
        .map(|(start, _)| start.parse().unwrap_or(0));
    let body = NothingListed {
        size: 0,
        total_size: start.map(|_| 0),
        offset: start,
    };
    match start {
        Some(start) => PlexResponse::new(body, format).paged(start, 0),
        None => PlexResponse::new(body, format),
    }
}

async fn nothing_listed(
    _caller: Caller,
    Query(pairs): Pairs,
    format: ResponseFormat,
) -> PlexResponse<NothingListed> {
    empty_list(&pairs, format)
}

async fn nothing_listed_to_admin(
    _caller: AdminCaller,
    Query(pairs): Pairs,
    format: ResponseFormat,
) -> PlexResponse<NothingListed> {
    empty_list(&pairs, format)
}

async fn absent(_caller: Caller) -> ApiError {
    ApiError::NotFound
}

async fn absent_to_admin(_caller: AdminCaller) -> ApiError {
    ApiError::NotFound
}

/// A lookup by parameters: `400` when one is missing, `404` otherwise, since nothing it can
/// name exists here. Measured on each of the four guide lookups.
fn unknown(pairs: &[(String, String)], required: &[&str]) -> ApiError {
    if required.iter().all(|name| value(pairs, name).is_some()) {
        ApiError::NotFound
    } else {
        ApiError::BadRequest
    }
}

/// `POST /livetv/dvrs` builds a DVR on a device, and no device exists.
async fn create_dvr(_caller: AdminCaller, Query(pairs): Pairs) -> ApiError {
    unknown(&pairs, &["device"])
}

/// `POST /media/grabbers/devices?uri=…`: no grabber is there to recognize the address, which
/// is the spec's `400` for a device that cannot be added.
async fn add_device(_caller: AdminCaller) -> ApiError {
    ApiError::BadRequest
}

/// `POST /media/subscriptions`: there is no DVR to record with.
async fn subscribe(_caller: Caller) -> ApiError {
    ApiError::BadRequest
}

/// Discovery finds nothing without a grabber; the answer is the device list it would add to.
async fn discover(_caller: AdminCaller, format: ResponseFormat) -> PlexResponse<NothingListed> {
    empty_list(&[], format)
}

/// Processing no subscription is done at once. Acknowledged the way the spec types it: an
/// empty `200`, `text/html`.
async fn process(_caller: Caller) -> impl IntoResponse {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")])
}

/// What can be recorded of an item: nothing, without a DVR. The reference refuses a request
/// with no `guid` with `400`.
async fn template(
    _caller: Caller,
    Query(pairs): Pairs,
    format: ResponseFormat,
) -> Result<PlexResponse<NothingListed>, ApiError> {
    value(&pairs, "guid").ok_or(ApiError::BadRequest)?;
    Ok(empty_list(&[], format))
}
