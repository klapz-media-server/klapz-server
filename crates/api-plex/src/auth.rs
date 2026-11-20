//! Role enforcement.
//!
//! Accepting a credential is the HTTP layer's job and lives in the `server` crate, because
//! the official GraphQL API and this adapter share it. What belongs here is the role check,
//! expressed as extractor types so it appears in handler signatures where the compiler can
//! see it.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use klapz_core::Principal;

use crate::error::ApiError;

/// An authenticated caller: a user.
///
/// Taking this in a handler is what proves authentication ran: the extractor fails if the
/// middleware did not attach a principal, so a route cannot accidentally be left open.
#[derive(Debug, Clone)]
pub struct Caller(pub Principal);

impl<S: Sync> FromRequestParts<S> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Principal>()
            .cloned()
            .map(Self)
            .ok_or(ApiError::Unauthorized)
    }
}

/// An authenticated owner of the server.
///
/// A distinct type rather than a field on [`Caller`], so that a handler needing an owner
/// cannot forget the check: omitting it would mean naming a different type.
#[derive(Debug, Clone)]
pub struct AdminCaller(pub Principal);

impl<S: Sync> FromRequestParts<S> for AdminCaller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Caller(principal) = Caller::from_request_parts(parts, state).await?;
        if principal.is_owner() {
            Ok(Self(principal))
        } else {
            // Not 403: the caller is authenticated but not entitled, and saying so would
            // confirm the resource exists.
            Err(ApiError::NotFound)
        }
    }
}
