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
use open_plex_api::ApiSchema;
use open_plex_core::Principal;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

tokio::task_local! {
    /// Who the request being handled is from. Task-local rather than passed along so that no
    /// query can be written that forgets it: the schema shows nothing to a request without one.
    static CALLER: Option<Principal>;
}

/// Makes the authenticated caller, if any, the one every query of this request runs as.
pub async fn run_as_caller(request: axum::extract::Request, next: Next) -> Response {
    let caller = request.extensions().get::<Principal>().cloned();
    CALLER.scope(caller, next.run(request)).await
}

fn caller() -> Option<Principal> {
    CALLER.try_with(Clone::clone).ok().flatten()
}

/// The machine identifier this request's caller knows the server by: each user has its own.
pub fn machine_identifier(server: &str) -> String {
    caller()
        .and_then(|caller| caller.server_identifier)
        .unwrap_or_else(|| server.to_owned())
}

/// The name a session is kept under by the transcoder, which is shared by every user.
pub fn session_key(client_key: &str) -> String {
    caller().map_or_else(
        || client_key.to_owned(),
        |caller| caller.session_key(client_key),
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
    let mut request = Request::new(query).variables(variables);
    if let Some(caller) = caller() {
        request = request.data(caller);
    }
    let response = schema.execute(request).await;

    if !response.errors.is_empty() {
        let joined = response
            .errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ApiError::Internal(Box::new(GraphQlFailure(joined))));
    }

    async_graphql::from_value(response.data).map_err(|error| ApiError::Internal(Box::new(error)))
}

#[derive(Debug, thiserror::Error)]
#[error("the official API rejected an adapter query: {0}")]
pub struct GraphQlFailure(String);
