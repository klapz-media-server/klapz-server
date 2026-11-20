//! Running the schema in process, as whoever made the request being handled.
//!
//! Every binding — Plex's, Jellyfin's — reads its data by executing queries against this
//! schema, and has no other source: a binding cannot publish anything the official API does
//! not expose. The caller is held for the whole request rather than passed to each query, so
//! that no query can be written that forgets it: the schema shows nothing to a request without
//! one.

use std::future::Future;

use async_graphql::Request;
use klapz_core::{Principal, PublishedServer};
use serde::de::DeserializeOwned;

use crate::ApiSchema;

/// Who a request is from, and for a request from nobody, which server its address shows.
#[derive(Debug, Clone, Default)]
pub struct RequestOrigin {
    pub caller: Option<Principal>,
    pub published: Option<PublishedServer>,
}

tokio::task_local! {
    static ORIGIN: RequestOrigin;
}

/// Runs a request's handling with `origin` as the one every query of it runs as.
pub async fn scope<F: Future>(origin: RequestOrigin, handling: F) -> F::Output {
    ORIGIN.scope(origin, handling).await
}

/// The caller of the request being handled, if anyone.
pub fn caller() -> Option<Principal> {
    ORIGIN
        .try_with(|origin| origin.caller.clone())
        .ok()
        .flatten()
}

/// The server published at the address a request from nobody was sent to.
pub fn published() -> Option<PublishedServer> {
    ORIGIN
        .try_with(|origin| origin.published.clone())
        .ok()
        .flatten()
}

#[derive(Debug, thiserror::Error)]
pub enum InProcessError {
    /// A binding's query is the binding's own, never the client's: an error here is a defect
    /// in the binding, which the client must not be told about.
    #[error("the official API rejected a binding's query: {0}")]
    Rejected(String),
    #[error("the official API answered a binding's query in an unexpected shape")]
    Shape(#[from] async_graphql::DeserializerError),
}

/// Executes a request as the current caller and deserializes its `data` into `T`.
///
/// The request's query is always one of the binding's own constants; no binding builds one
/// from request data, and none may.
///
/// # Errors
///
/// [`InProcessError::Rejected`] when the query fails, [`InProcessError::Shape`] when its
/// result does not match `T`.
pub async fn execute<T: DeserializeOwned>(
    schema: &ApiSchema,
    request: Request,
) -> Result<T, InProcessError> {
    let request = match caller() {
        Some(caller) => request.data(caller),
        None => request,
    };
    let response = schema.execute(request).await;
    if !response.errors.is_empty() {
        let joined = response
            .errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(InProcessError::Rejected(joined));
    }
    Ok(async_graphql::from_value(response.data)?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn the_caller_is_the_one_of_the_scope_and_nobody_outside_it() {
        assert!(caller().is_none());
        let owner = klapz_core::testing::owner();
        let seen = scope(
            RequestOrigin {
                caller: Some(owner.clone()),
                published: Some(PublishedServer("server".to_owned())),
            },
            async { (caller().map(|caller| caller.user), published()) },
        )
        .await;
        assert_eq!(
            seen,
            (Some(owner.user), Some(PublishedServer("server".to_owned())))
        );
        assert!(caller().is_none());
    }
}
