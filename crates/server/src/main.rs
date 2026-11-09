//! open-plex server binary: configuration, composition, and mounting the bindings.
//!
//! Also carries the one-off `import` subcommand, which seeds our schema from a real Plex
//! database. Keeping it here rather than in a separate crate avoids a crate whose only job
//! is to call one function.

mod admin;
mod auth;
mod graphql;
mod remote_tokens;
mod web;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderValue, StatusCode};
use open_plex_api::{build_schema, ApiContext};
use open_plex_api_plex::PlexState;
use open_plex_core::access::{publish_link, AccessRepository, AccountDirectory, Extension};
use open_plex_core::metadata::MetadataRepository;
use open_plex_core::play_queue::PlayQueueRepository;
use open_plex_core::playback::PlaybackRepository;
use open_plex_core::{
    Accounts, Credential, LibraryRepository, Scope, ServerCapabilities, ServerIdentity,
};
use open_plex_db::{
    import, SqlAccessRepository, SqlLibraryRepository, SqlMetadataRepository,
    SqlPlayQueueRepository, SqlPlaybackRepository,
};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Where our own database lives. `rwc` creates it on first run.
const DEFAULT_DATABASE_URL: &str = "sqlite://open-plex.db?mode=rwc";
/// How often expired playback sessions are removed. Reads already skip them.
const EXPIRY_SWEEP: std::time::Duration = std::time::Duration::from_secs(30);

const DEFAULT_BIND: &str = "127.0.0.1:32500";
/// How often paired servers announce their address. The reference does it when it starts; the
/// repeat is ours, and is what retries an announcement that failed.
const PUBLISH_INTERVAL: Duration = Duration::from_secs(3600);

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
        return run_import(&connection, &PathBuf::from(source)).await;
    }

    let library: Arc<dyn LibraryRepository> =
        Arc::new(SqlLibraryRepository::new(connection.clone()));
    let play_queues: Arc<dyn PlayQueueRepository> =
        Arc::new(SqlPlayQueueRepository::new(connection.clone()));
    let playback: Arc<dyn PlaybackRepository> =
        Arc::new(SqlPlaybackRepository::new(connection.clone()));
    let users: Arc<dyn AccessRepository> = Arc::new(SqlAccessRepository::new(connection.clone()));
    let metadata: Arc<dyn MetadataRepository> = Arc::new(SqlMetadataRepository::new(connection));
    let directory: Arc<dyn AccountDirectory> =
        Arc::new(open_plex_api_plex::plex_tv::PlexTv::new()?);
    // No process from before this one is still running, so no session from before is either.
    let cleared = playback.delete_all().await?;
    if cleared > 0 {
        tracing::info!(
            cleared,
            "dropped the playback sessions a previous run left behind"
        );
    }
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity(), open_plex_core::now_epoch()),
        library,
        metadata,
        playback: Arc::clone(&playback),
        play_queues: Arc::clone(&play_queues),
        access: Arc::clone(&users),
        directory: Arc::clone(&directory),
    });
    tokio::spawn(publish_paired_servers(
        Arc::clone(&users),
        Arc::clone(&directory),
    ));
    let accounts = accounts();
    let access = Arc::new(
        auth::Access::new(accounts.clone(), trusted_networks()?)
            .with_users(users)
            .with_remote_tokens(remote_tokens::RemoteTokens::new(directory)),
    );

    // The Plex surface is the base and GraphQL is merged into it, which inverts the
    // relationship the architecture describes. The reason is mechanical: Plex has paths whose
    // segments literally start with a colon (`/:/prefs`, `/photo/:/transcode`), Axum rejects
    // those unless the router has `without_v07_checks`, and `merge` re-validates whatever is
    // merged *in* with the default rules. So the router holding those paths cannot be the one
    // merged in. GraphQL has no such paths and merges cleanly either way.
    let mut plex_state = PlexState::new(identity(), schema.clone(), accounts);
    let transcoder = Arc::new(transcoder());
    transcoder.remove_leftover_sessions();
    plex_state.transcoder = Arc::clone(&transcoder);
    tokio::spawn(expire_playback(
        Arc::clone(&playback),
        Arc::clone(&play_queues),
        transcoder,
    ));
    let mut app = open_plex_api_plex::router(plex_state)
        .merge(graphql::router(schema))
        .merge(admin::router());
    let web_dir = web::directory();
    if let Some(client) = web::router(&web_dir) {
        tracing::info!(directory = %web_dir.display(), "serving the web client at /web");
        app = app.merge(client);
    } else {
        tracing::info!(directory = %web_dir.display(), "no web client found, /web is not served");
    }
    let app = app
        // One token middleware for every surface, so neither can be left unauthenticated by
        // omission.
        .layer(axum::middleware::from_fn_with_state(
            access,
            auth::require_token,
        ))
        .layer((
            TraceLayer::new_for_http(),
            TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(30)),
            // A real server sends both of these on every response, errors included, so they
            // belong in a layer rather than in the response wrapper. Only an image says
            // otherwise about caching, so that one yields to the handler.
            SetResponseHeaderLayer::overriding(
                header::HeaderName::from_static("x-plex-protocol"),
                HeaderValue::from_static("1.0"),
            ),
            SetResponseHeaderLayer::if_not_present(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            ),
        ));

    let bind = std::env::var("OPENPLEX_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(address = %bind, "open-plex listening");
    // With the peer's address attached, which is what a trusted network is checked against.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn run_import(
    connection: &open_plex_db::DatabaseConnection,
    source: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let summary = import::import_library(connection, source).await?;
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

/// The networks admitted without a token, from `OPENPLEX_ALLOWED_NETWORKS`.
///
/// None unless configured. Each one is an authentication bypass, so the server says so when
/// it starts with any.
fn trusted_networks() -> Result<Vec<ipnet::IpNet>, String> {
    let networks =
        auth::parse_networks(&std::env::var("OPENPLEX_ALLOWED_NETWORKS").unwrap_or_default())
            .map_err(|error| format!("OPENPLEX_ALLOWED_NETWORKS: {error}"))?;
    if !networks.is_empty() {
        let list: Vec<String> = networks.iter().map(ToString::to_string).collect();
        tracing::warn!(
            networks = %list.join(","),
            "requests from these networks are admitted without a token"
        );
    }
    Ok(networks)
}

/// `OPENPLEX_TRANSCODER` is the patched ffmpeg to run; `OPENPLEX_TRANSCODER_CODECS` where its
/// separately distributed codecs are, for Plex's own build.
fn transcoder() -> open_plex_transcoder::Transcoder {
    let bind = std::env::var("OPENPLEX_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let port = bind.rsplit(':').next().unwrap_or("32500");
    open_plex_transcoder::Transcoder::new(open_plex_transcoder::Config {
        binary: std::env::var_os("OPENPLEX_TRANSCODER")
            .map_or_else(|| "ffmpeg".into(), std::path::PathBuf::from),
        external_libs: std::env::var_os("OPENPLEX_TRANSCODER_CODECS").map(std::path::PathBuf::from),
        callback_base: format!("http://127.0.0.1:{port}"),
    })
}

/// Removes what a vanished player left behind: its session, its process and old queues.
async fn expire_playback(
    playback: Arc<dyn PlaybackRepository>,
    play_queues: Arc<dyn PlayQueueRepository>,
    transcoder: Arc<open_plex_transcoder::Transcoder>,
) {
    let mut ticks = tokio::time::interval(EXPIRY_SWEEP);
    loop {
        ticks.tick().await;
        transcoder.stop_idle_sessions();
        match playback.delete_expired(open_plex_core::now_epoch()).await {
            Ok(0) => {}
            Ok(expired) => tracing::info!(expired, "expired playback sessions"),
            Err(error) => tracing::warn!(%error, "could not expire playback sessions"),
        }
        let cutoff =
            open_plex_core::now_epoch() - open_plex_core::play_queue::QUEUE_RETENTION_SECONDS;
        if let Err(error) = play_queues.delete_created_before(cutoff).await {
            tracing::warn!(%error, "could not remove old play queues");
        }
    }
}

/// Keeps every paired server's address current on the account service, from startup on.
async fn publish_paired_servers(
    users: Arc<dyn AccessRepository>,
    directory: Arc<dyn AccountDirectory>,
) {
    let mut ticks = tokio::time::interval(PUBLISH_INTERVAL);
    loop {
        ticks.tick().await;
        if let Err(error) = publish_all(users.as_ref(), directory.as_ref()).await {
            tracing::warn!(%error, "could not walk the paired servers");
        }
    }
}

async fn publish_all(
    users: &dyn AccessRepository,
    directory: &dyn AccountDirectory,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut after = None;
    loop {
        let window = open_plex_core::page::Window::by_id(
            after,
            None,
            Some(open_plex_core::page::MAX_ITEMS),
            None,
        )?;
        let page = users.users(window).await?;
        for user in &page.items {
            let link = users.account_link(user.id, Extension::Plex).await?;
            let Some(link) = link.filter(|link| link.enabled) else {
                continue;
            };
            let now = open_plex_core::now_epoch();
            if let Err(error) = publish_link(users, directory, &link, now).await {
                tracing::warn!(%error, user = user.id.get(), "could not publish a server");
            }
        }
        match page.items.last() {
            Some(last) if page.has_next => after = Some(last.id.get()),
            _ => return Ok(()),
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
