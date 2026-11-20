//! JSON as the reference sends it.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

pub const JSON: &str = "application/json; charset=utf-8";

/// A `200` carrying `value`, typed as the reference types every JSON answer.
pub fn json<T: Serialize>(value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => (
            [(header::CONTENT_TYPE, HeaderValue::from_static(JSON))],
            body,
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "could not serialize a Jellyfin answer");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// What the reference answers an action that returns nothing.
pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}
