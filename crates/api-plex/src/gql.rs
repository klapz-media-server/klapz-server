//! Executing the official GraphQL API from the Plex adapter.
//!
//! Every Plex endpoint reads its data by running a query against the GraphQL schema. The
//! adapter has no other source, which is the point: a Plex response cannot contain anything
//! the official API does not expose.
//!
//! Execution is in-process (`Schema::execute`), so this costs a resolver call rather than an
//! HTTP round trip. Results are deserialized into typed structs instead of being read as
//! loose JSON, so the adapter keeps compile-time guarantees about the shape it consumes.

use async_graphql::{Request, Variables};
use axum::middleware::Next;
use axum::response::Response;
use klapz_api::in_process::{self, RequestOrigin};
use klapz_api::ApiSchema;
use klapz_core::{Principal, PublishedServer};
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// Makes the authenticated caller, if any, the one every query of this request runs as.
pub async fn run_as_caller(request: axum::extract::Request, next: Next) -> Response {
    let origin = RequestOrigin {
        caller: request.extensions().get::<Principal>().cloned(),
        published: request.extensions().get::<PublishedServer>().cloned(),
    };
    in_process::scope(origin, next.run(request)).await
}

/// The machine identifier this request knows the server by: each user has its own, and a
/// request from nobody sees the one published at the address it used.
pub fn machine_identifier(server: &str) -> String {
    in_process::caller()
        .and_then(|caller| caller.server_identifier)
        .or_else(|| in_process::published().map(|PublishedServer(id)| id))
        .unwrap_or_else(|| server.to_owned())
}

/// The name a session is kept under by the transcoder, which is shared by every user.
pub fn session_key(client_key: &str) -> String {
    in_process::caller().map_or_else(
        || client_key.to_owned(),
        |caller| caller.session_key(client_key),
    )
}

/// The name a client knows a stored session by.
pub fn client_key(stored: &str) -> String {
    in_process::caller().map_or_else(
        || stored.to_owned(),
        |caller| caller.client_key(stored).to_owned(),
    )
}

/// Runs a query and deserializes `data` into `T`.
///
/// `query` is always one of this crate's own constants. It is borrowed rather than
/// `'static` only because the longer queries concatenate a shared fragment set at first
/// use; no caller builds one from request data, and none may.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the query fails or its result does not match `T`.
/// A GraphQL error here is a defect in the adapter's own query, not client input: the
/// client never supplies these queries, so it must not be told what went wrong.
pub async fn execute<T: DeserializeOwned>(
    schema: &ApiSchema,
    query: &str,
    variables: Variables,
) -> Result<T, ApiError> {
    in_process::execute(schema, Request::new(query).variables(variables))
        .await
        .map_err(|error| ApiError::Internal(Box::new(error)))
}

/// The official API's `Locale`, selected as `{ tag }`, read back into the tag Plex publishes as
/// `languageTag` and labels from; for `#[serde(deserialize_with)]`.
pub(crate) mod locale {
    use serde::Deserializer;

    pub fn tag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        klapz_api::language::deserialize(deserializer).map(|tag| tag.to_string())
    }

    pub fn optional_tag<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        klapz_api::language::deserialize_optional(deserializer)
            .map(|tag| tag.map(|tag| tag.to_string()))
    }
}

/// The official API's `DateTime`, read back into the Unix seconds Plex publishes; for
/// `#[serde(deserialize_with)]`.
pub(crate) mod date_time {
    use klapz_core::edit::CalendarDate;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer};

    pub fn seconds<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
        let text = String::deserialize(deserializer)?;
        klapz_api::date_time::parse(&text)
            .ok_or_else(|| D::Error::custom(format!("`{text}` is not a DateTime")))
    }

    pub fn optional_seconds<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<i64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                klapz_api::date_time::parse(&text)
                    .ok_or_else(|| D::Error::custom(format!("`{text}` is not a DateTime")))
            })
            .transpose()
    }

    /// A release date's `DateTime`, its midnight, as the `YYYY-MM-DD` Plex publishes.
    pub fn optional_calendar_date<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        Ok(optional_seconds(deserializer)?
            .and_then(CalendarDate::containing)
            .map(|date| date.to_string()))
    }
}
