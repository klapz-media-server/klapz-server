//! Plex API binding.
//!
//! Translation only: parse the request, run a query against the official GraphQL API, shape
//! the response. Domain rules belong in `core`, and this crate depends on neither
//! `open-plex-db` nor the repository ports — if a handler
//! needs data, the official GraphQL API must expose it. Converting media is the other thing
//! it binds, onto `open-plex-transcoder`: bytes in and out, which is not a data source.

pub mod activities;
pub mod auth;
pub mod capabilities;
pub mod children;
pub mod client;
pub mod container;
pub mod error;
pub mod gql;
pub mod gql_items;
pub mod hubs;
pub mod items;
pub mod language;
pub mod listing;
pub mod meta;
pub mod photo;
mod play_queues;
mod playback;
pub mod prefs;
pub mod providers;
mod related;
pub mod response;
mod search;
pub mod sessions;
pub mod stubs;
mod timeline;
pub mod translations;
pub mod updater;
pub mod xml;

pub mod general;
mod library;

use std::sync::Arc;

use axum::Router;
use open_plex_api::ApiSchema;
use open_plex_core::{Accounts, ServerIdentity};
use open_plex_transcoder::Transcoder;

/// What the adapter needs from the rest of the application.
///
/// Cloned per request, so everything in it is cheap to clone. Note what is absent: no
/// repository and no database handle. The adapter's only data source is the GraphQL schema,
/// which is what guarantees it cannot expose anything the official API does not.
#[derive(Clone)]
pub struct PlexState {
    pub identity: Arc<ServerIdentity>,
    pub schema: ApiSchema,
    pub accounts: Arc<Accounts>,
    pub transcoder: Arc<Transcoder>,
}

/// Written by hand because `async_graphql::Schema` has no `Debug`, and the workspace lints
/// require one on public types. The schema would be noise in a log anyway.
impl std::fmt::Debug for PlexState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlexState")
            .field("identity", &self.identity)
            .field("schema", &"<graphql schema>")
            .field("accounts", &self.accounts)
            .field("transcoder", &self.transcoder)
            .finish()
    }
}

impl PlexState {
    pub fn new(identity: ServerIdentity, schema: ApiSchema, accounts: Accounts) -> Self {
        Self {
            identity: Arc::new(identity),
            schema,
            accounts: Arc::new(accounts),
            transcoder: Arc::default(),
        }
    }
}

/// The adapter's routes.
///
/// Authentication is applied by the caller: `server` wraps every HTTP surface with one
/// token middleware, so this router assumes a principal has already been attached and the
/// `Caller` extractors simply read it.
pub fn router(state: PlexState) -> Router {
    let routes = Router::new()
        .merge(general::router())
        .merge(library::router())
        .merge(providers::router())
        .merge(sessions::router())
        .merge(updater::router())
        .merge(hubs::router())
        .merge(activities::router())
        .merge(search::router())
        .merge(play_queues::router())
        .merge(related::router())
        // Plex has paths whose segments literally start with a colon — `/:/prefs`,
        // `/:/timeline`, `/photo/:/transcode`. Axum 0.8 rejects those by default as a guard
        // against its own older `:param` syntax.
        //
        // Placed *after* the merges and applied to this one router on purpose: `merge` does
        // not carry the setting over, and routes added through it are re-validated with the
        // default rules. Our own parameters use the `{name}` form, which is unaffected.
        .without_v07_checks();

    let routes = timeline::add_routes(playback::add_routes(photo::add_routes(prefs::add_routes(
        routes,
    ))));

    // Added last so a real handler registered above always wins over its stub.
    stubs::add_routes(routes)
        .with_state(state)
        .layer(axum::middleware::from_fn(gql::run_as_caller))
}
