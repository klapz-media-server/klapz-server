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
use open_plex_api::ApiSchema;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

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
    let response = schema
        .execute(Request::new(query).variables(variables))
        .await;

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
