//! Running the official API from the Jellyfin binding, as the request's caller.

use async_graphql::{Request, Variables};
use axum::middleware::Next;
use axum::response::Response;
use klapz_api::in_process::{self, RequestOrigin};
use klapz_api::ApiSchema;
use klapz_core::Principal;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::error::JellyfinError;

/// Makes the authenticated caller, if any, the one every query of this request runs as.
pub async fn run_as_caller(request: axum::extract::Request, next: Next) -> Response {
    let origin = RequestOrigin {
        caller: request.extensions().get::<Principal>().cloned(),
        published: None,
    };
    in_process::scope(origin, next.run(request)).await
}

/// Runs one of this crate's own queries with JSON variables.
pub async fn query<T: DeserializeOwned>(
    schema: &ApiSchema,
    query: &str,
    variables: serde_json::Value,
) -> Result<T, JellyfinError> {
    Ok(in_process::execute(
        schema,
        Request::new(query).variables(Variables::from_json(variables)),
    )
    .await?)
}

/// Runs a prepared request: one that carries more than the caller, such as what signing in
/// needs.
pub async fn run<T: DeserializeOwned>(
    schema: &ApiSchema,
    request: Request,
) -> Result<T, JellyfinError> {
    Ok(in_process::execute(schema, request).await?)
}

/// The caller, as the official API knows it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewer {
    pub id: String,
    pub username: String,
    pub has_password: bool,
}

const VIEWER: &str = "{ viewer { user { id username hasPassword } } }";

#[derive(Deserialize)]
struct ViewerData {
    viewer: ViewerUser,
}

#[derive(Deserialize)]
struct ViewerUser {
    user: Option<Viewer>,
}

/// Who the request is from; [`JellyfinError::Unauthorized`] for nobody, which the server's
/// authentication lets through only to the public routes.
pub async fn viewer(schema: &ApiSchema) -> Result<Viewer, JellyfinError> {
    let data: ViewerData = query(schema, VIEWER, serde_json::Value::Null).await?;
    data.viewer.user.ok_or(JellyfinError::Unauthorized)
}

impl Viewer {
    /// The caller's Jellyfin user id.
    pub fn jellyfin_id(&self) -> String {
        crate::id::of_global(&self.id).unwrap_or_default()
    }

    /// Whether a user id a request names is the caller's. Each user sees its own view of the
    /// server; another's answers as if it did not exist.
    ///
    /// # Errors
    ///
    /// [`JellyfinError::BadRequest`] for what is no GUID, [`JellyfinError::NotFound`] for
    /// another user.
    pub fn require_self(&self, user_id: Option<&str>) -> Result<(), JellyfinError> {
        let Some(user_id) = user_id else {
            return Ok(());
        };
        let named = crate::id::global_of_type(user_id, klapz_api::access::USER_TYPE)?;
        if named == self.id {
            Ok(())
        } else {
            Err(JellyfinError::NotFound)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn alice() -> Viewer {
        Viewer {
            id: klapz_api::GlobalId::encode("User", 3),
            username: "alice".to_owned(),
            has_password: true,
        }
    }

    #[test]
    fn a_request_may_name_its_own_user_or_none() {
        let alice = alice();
        assert!(alice.require_self(None).is_ok());
        assert!(alice.require_self(Some(&alice.jellyfin_id())).is_ok());
        assert!(alice
            .require_self(Some(&crate::id::dashed(&alice.jellyfin_id())))
            .is_ok());
    }

    #[test]
    fn another_user_is_not_found_and_garbage_is_refused() {
        let alice = alice();
        let bob = crate::id::of("User", 4).unwrap();
        assert!(matches!(
            alice.require_self(Some(&bob)),
            Err(JellyfinError::NotFound)
        ));
        let library = crate::id::of("Library", 3).unwrap();
        assert!(matches!(
            alice.require_self(Some(&library)),
            Err(JellyfinError::NotFound)
        ));
        assert!(matches!(
            alice.require_self(Some("me")),
            Err(JellyfinError::BadRequest(_))
        ));
    }
}
