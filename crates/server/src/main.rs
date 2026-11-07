//! open-plex server binary: configuration, composition, and mounting the bindings.
//!
//! Also carries the one-off `import` subcommand, which seeds our schema from a real Plex
//! database. Keeping it here rather than in a separate crate avoids a crate whose only job
//! is to call one function.

mod auth;
mod graphql;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderValue, StatusCode};
use open_plex_api::{build_schema, ApiContext};
use open_plex_api_plex::PlexState;
use open_plex_core::metadata::MetadataRepository;
use open_plex_core::{
    Accounts, Credential, LibraryRepository, Scope, ServerCapabilities, ServerIdentity,
};
use open_plex_db::{import, SqlLibraryRepository, SqlMetadataRepository};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Where our own database lives. `rwc` creates it on first run.
const DEFAULT_DATABASE_URL: &str = "sqlite://open-plex.db?mode=rwc";
const DEFAULT_BIND: &str = "127.0.0.1:32500";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "open_plex_server=info,open_plex_api_plex=info,open_plex_db=info".into()
            }),
        )
        .init();

    let database_url =
        std::env::var("OPENPLEX_DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
    let connection = open_plex_db::connect(&database_url).await?;

    // `import <path>` seeds the database and exits; anything else starts the server.
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() == Some("import") {
        let source = arguments
            .next()
            .ok_or("usage: open-plex-server import <plex.db>")?;
        let summary = import::import_library(&connection, &PathBuf::from(source)).await?;
        tracing::info!(
            sections = summary.sections,
            roots = summary.roots,
            items = summary.items,
            media = summary.media,
            files = summary.files,
            streams = summary.streams,
            people = summary.tags.people,
            tags = summary.tags.tags,
            item_tags = summary.tags.item_tags,
            external_ids = summary.tags.external_ids,
            ratings = summary.tags.ratings,
            orphans = summary.orphans,
            "import complete"
        );
        return Ok(());
    }

    let library: Arc<dyn LibraryRepository> =
        Arc::new(SqlLibraryRepository::new(connection.clone()));
    let metadata: Arc<dyn MetadataRepository> = Arc::new(SqlMetadataRepository::new(connection));
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity(), open_plex_core::now_epoch()),
        library,
        metadata,
    });
    let accounts = Arc::new(accounts());

    // The Plex surface is the base and GraphQL is merged into it, which inverts the
    // relationship the architecture describes. The reason is mechanical: Plex has paths whose
    // segments literally start with a colon (`/:/prefs`, `/photo/:/transcode`), Axum rejects
    // those unless the router has `without_v07_checks`, and `merge` re-validates whatever is
    // merged *in* with the default rules. So the router holding those paths cannot be the one
    // merged in. GraphQL has no such paths and merges cleanly either way.
    let app = open_plex_api_plex::router(PlexState::new(
        identity(),
        schema.clone(),
        (*accounts).clone(),
    ))
    .merge(graphql::router(schema))
    // One token middleware for every surface, so neither can be left unauthenticated by
    // omission.
    .layer(axum::middleware::from_fn_with_state(
        Arc::clone(&accounts),
        auth::require_token,
    ))
    .layer((
        TraceLayer::new_for_http(),
        TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(30)),
        // A real server sends both of these on every response, errors included, so they
        // belong in a layer rather than in the response wrapper.
        SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("x-plex-protocol"),
            HeaderValue::from_static("1.0"),
        ),
        SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ),
    ));

    let bind = std::env::var("OPENPLEX_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(address = %bind, "open-plex listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Reads the accepted credentials from the environment.
///
/// No token configured means no authenticated route is reachable, which is the safe failure:
/// the alternative — serving the library to anyone while the operator believes it is
/// configured — is how a self-hosted server ends up public.
fn accounts() -> Accounts {
    match std::env::var("OPENPLEX_TOKEN") {
        Ok(token) if !token.is_empty() => {
            Accounts::new(vec![Credential::new(token, "owner", Scope::Admin)])
        }
        _ => {
            tracing::warn!(
                "OPENPLEX_TOKEN is not set: every authenticated endpoint will answer 401"
            );
            Accounts::default()
        }
    }
}

/// The identity the server announces.
///
/// The machine identifier must be stable across restarts for clients to keep recognizing the
/// server, so it comes from configuration rather than being generated per boot. Persisting a
/// generated one belongs with the settings work.
fn identity() -> ServerIdentity {
    let machine_identifier = std::env::var("OPENPLEX_MACHINE_ID")
        .unwrap_or_else(|_| "0000000000000000000000000000000000000000".to_owned());
    ServerIdentity::new(machine_identifier, env!("CARGO_PKG_VERSION"))
}
