//! Jellyfin API binding: open-plex as a Jellyfin server, for Jellyfin's own clients.
//!
//! The counterpart of `open-plex-api-plex`, and held to the same rule: translation only. A
//! handler parses the request, runs a query against the official GraphQL API in process, and
//! shapes the answer as the reference Jellyfin server does. This crate depends on neither
//! `open-plex-db` nor the repository ports; what Jellyfin needs and GraphQL lacks is added to
//! the official API first.
//!
//! The surface lives under [`BASE_PATH`], which Jellyfin clients support as a server's base
//! URL: Plex's paths and Jellyfin's would otherwise collide once compared without regard to
//! case, as Jellyfin's are. Only a user connected through the Jellyfin extension reaches it:
//! the server's authentication resolves Jellyfin tokens alone there.

// Jellyfin's user policy is one flat object of forty fields, written as `json!`, whose macro
// recurses once per field.
#![recursion_limit = "256"]

pub mod auth;
mod browse;
mod collections;
mod dto;
mod error;
mod filters;
mod gql;
pub mod id;
mod items;
mod language;
mod listing;
mod playback;
mod playlists;
mod query;
mod response;
mod shows;
mod system;
mod time;
mod track;
mod users;
mod views;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::Uri;
use axum::Router;
use open_plex_api::{ApiSchema, Sessions};
use open_plex_core::ServerIdentity;

pub use error::JellyfinError;

/// Where the Jellyfin surface is mounted: clients are given `http://host:port/jellyfin`.
pub const BASE_PATH: &str = "/jellyfin";

/// The version announced: the reference's, which every capture comes from. Clients enable
/// features by version, and then expect exactly the behavior we copied.
pub const JELLYFIN_VERSION: &str = "12.4.0";

/// The path below [`BASE_PATH`], when a path is the Jellyfin surface's.
pub fn surface_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(BASE_PATH)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

/// What the binding needs from the rest of the application. Cheap to clone: it is cloned per
/// request. No repository and no database handle: the schema is the only data source.
#[derive(Clone)]
pub struct JellyfinState {
    pub schema: ApiSchema,
    /// What signing in needs: the password hashing and the throttle the official API shares.
    pub sessions: Arc<Sessions>,
    pub server: Arc<ServerInfo>,
    /// Jellyfin Web, served at `/web` below the base when set.
    pub web_dir: Option<PathBuf>,
}

impl std::fmt::Debug for JellyfinState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JellyfinState")
            .field("server", &self.server)
            .field("web_dir", &self.web_dir)
            .finish_non_exhaustive()
    }
}

/// How the server presents itself to Jellyfin clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// 32 hexadecimal digits, stable for as long as the machine identifier is.
    pub id: String,
    pub name: String,
}

impl ServerInfo {
    /// Every Jellyfin user shares one server, as on the reference: its id is derived from the
    /// machine identifier, so it lasts as long as that does.
    pub fn of(identity: &ServerIdentity) -> Self {
        use sha2::{Digest, Sha256};
        use std::fmt::Write as _;
        let digest = Sha256::digest(format!("jellyfin:{}", identity.machine_identifier));
        let id = digest
            .iter()
            .take(16)
            .fold(String::with_capacity(32), |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            });
        Self {
            id,
            name: identity.friendly_name.clone(),
        }
    }
}

impl JellyfinState {
    pub fn new(schema: ApiSchema, sessions: Arc<Sessions>, identity: &ServerIdentity) -> Self {
        Self {
            schema,
            sessions,
            server: Arc::new(ServerInfo::of(identity)),
            web_dir: None,
        }
    }
}

/// The surface's routes, mounted at [`BASE_PATH`].
///
/// Authentication is the server's: it attaches the caller a Jellyfin token names, and only
/// that, before these routes run.
pub fn router(state: JellyfinState) -> Router {
    let web_dir = state.web_dir.clone();
    let mut routes = Router::new()
        .merge(system::router())
        .merge(users::router())
        .merge(views::router())
        .merge(items::router())
        .merge(filters::router())
        .merge(playlists::router())
        .merge(collections::router())
        .merge(browse::router())
        .merge(shows::router())
        .merge(playback::router())
        .fallback(not_found)
        .with_state(state)
        .layer(axum::middleware::from_fn(gql::run_as_caller));
    if let Some(directory) = web_dir {
        routes = routes.nest_service("/web", tower_http::services::ServeDir::new(directory));
    }
    let surface = tower::ServiceBuilder::new()
        .map_request(lower_case_path)
        .service(routes);
    Router::new().nest_service(BASE_PATH, surface)
}

async fn not_found() -> JellyfinError {
    JellyfinError::NotFound
}

/// The reference routes without regard to case: `/users/authenticatebyname` reaches the same
/// action as `/Users/AuthenticateByName`. Routes are declared in lower case and paths lowered
/// to meet them; ids are hexadecimal, which reads alike in either case. The web client's files
/// keep their names.
fn lower_case_path(mut request: Request) -> Request {
    let path = request.uri().path();
    if path == "/web" || path.starts_with("/web/") || !path.bytes().any(|b| b.is_ascii_uppercase())
    {
        return request;
    }
    let lowered = match request.uri().query() {
        Some(query) => format!("{}?{query}", path.to_ascii_lowercase()),
        None => path.to_ascii_lowercase(),
    };
    let mut parts = request.uri().clone().into_parts();
    if let Ok(path_and_query) = lowered.parse() {
        parts.path_and_query = Some(path_and_query);
        if let Ok(uri) = Uri::from_parts(parts) {
            *request.uri_mut() = uri;
        }
    }
    request
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_surface_is_what_lies_below_its_base() {
        assert_eq!(surface_path("/jellyfin/System/Info"), Some("/System/Info"));
        assert_eq!(surface_path("/jellyfin"), Some(""));
        assert_eq!(surface_path("/jellyfinx/System"), None);
        assert_eq!(surface_path("/library/sections"), None);
    }

    #[test]
    fn the_server_id_is_32_digits_derived_from_the_machine() {
        let one = ServerInfo::of(&ServerIdentity::new("a".repeat(40), "0.1.0"));
        let again = ServerInfo::of(&ServerIdentity::new("a".repeat(40), "0.1.0"));
        let other = ServerInfo::of(&ServerIdentity::new("b".repeat(40), "0.1.0"));
        assert_eq!(one, again);
        assert_ne!(one.id, other.id);
        assert_eq!(one.id.len(), 32);
        assert!(one.id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        // Not the machine identifier cut short: the Plex surface publishes that one.
        assert_ne!(one.id, "a".repeat(32));
        assert_eq!(one.name, "open-plex");
    }

    #[test]
    fn a_path_is_lowered_with_its_query_kept() {
        let request = Request::builder()
            .uri("/Users/AuthenticateByName?Pw=Secret")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            lower_case_path(request).uri(),
            "/users/authenticatebyname?Pw=Secret"
        );
        let web = Request::builder()
            .uri("/web/Assets/Logo.png")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(lower_case_path(web).uri(), "/web/Assets/Logo.png");
    }
}
