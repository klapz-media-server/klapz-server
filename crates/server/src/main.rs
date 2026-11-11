//! open-plex server binary: configuration, composition, and mounting the bindings.
//!
//! Also carries the one-off `import` subcommand, which seeds our schema from a real Plex
//! database. Keeping it here rather than in a separate crate avoids a crate whose only job
//! is to call one function.

mod admin;
mod auth;
mod callbacks;
mod cors;
mod graphql;
mod remote_tokens;
mod streams;
mod web;

use std::future::IntoFuture as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderValue, StatusCode};
use open_plex_api::{build_schema, ApiContext};
use open_plex_api_plex::PlexState;
use open_plex_core::access::{publish_link, AccessRepository, AccountDirectory, Extension};
use open_plex_core::account::SessionKey;
use open_plex_core::lifecycle::SessionLocks;
use open_plex_core::metadata::MetadataRepository;
use open_plex_core::play_queue::PlayQueueRepository;
use open_plex_core::playback::PlaybackRepository;
use open_plex_core::{
    Accounts, Credential, LibraryRepository, Scope, ServerCapabilities, ServerIdentity,
};
use open_plex_db::{
    import, SqlAccessRepository, SqlLibraryRepository, SqlMetadataRepository,
    SqlPlayQueueRepository, SqlPlaybackRepository, SqlWatchRepository,
};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Where our own database lives. `rwc` creates it on first run.
const DEFAULT_DATABASE_URL: &str = "sqlite://open-plex.db?mode=rwc";
/// How often expired playback sessions and idle streams are ended. Reads already skip them.
const EXPIRY_SWEEP: Duration = Duration::from_secs(30);
/// How long a stopping server waits for its connections once every session has ended: a
/// player still reading a file can hold one open.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

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
    let metadata: Arc<dyn MetadataRepository> =
        Arc::new(SqlMetadataRepository::new(connection.clone()));
    let directory: Arc<dyn AccountDirectory> =
        Arc::new(open_plex_api_plex::plex_tv::PlexTv::new()?);
    let transcoder = Arc::new(transcoder());
    transcoder.remove_leftover_sessions();
    let streams = Arc::new(streams::Streams::new(Arc::clone(&transcoder)));
    let locks = Arc::new(SessionLocks::default());
    let sessions = streams::Sessions {
        playback: Arc::clone(&playback),
        streams: Arc::clone(&streams),
        locks: Arc::clone(&locks),
    };
    clear_stale_sessions(&sessions).await?;
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity(), open_plex_core::now_epoch()),
        library,
        metadata,
        playback: Arc::clone(&playback),
        play_queues: Arc::clone(&play_queues),
        playlists: Arc::new(open_plex_db::SqlPlaylistRepository::new(connection.clone())),
        collections: Arc::new(open_plex_db::SqlCollectionRepository::new(
            connection.clone(),
        )),
        access: Arc::clone(&users),
        directory: Arc::clone(&directory),
        watch: Arc::new(SqlWatchRepository::new(connection)),
        streams: Arc::clone(&streams) as Arc<dyn open_plex_core::stream::StreamEngine>,
        session_locks: locks,
    });
    tokio::spawn(publish_paired_servers(
        Arc::clone(&users),
        Arc::clone(&directory),
    ));
    let accounts = accounts();
    let (session_key, sign_in) = sign_in()?;
    let access = Arc::new(
        auth::Access::new(accounts.clone(), trusted_networks()?)
            .with_users(users)
            .with_remote_tokens(remote_tokens::RemoteTokens::new(directory))
            .with_sessions(session_key),
    );

    // The Plex surface is the base and GraphQL is merged into it, which inverts the
    // relationship the architecture describes. The reason is mechanical: Plex has paths whose
    // segments literally start with a colon (`/:/prefs`, `/photo/:/transcode`), Axum rejects
    // those unless the router has `without_v07_checks`, and `merge` re-validates whatever is
    // merged *in* with the default rules. So the router holding those paths cannot be the one
    // merged in. GraphQL has no such paths and merges cleanly either way.
    let mut plex_state = PlexState::new(identity(), schema.clone(), accounts);
    plex_state.transcoder = Arc::clone(&transcoder);
    let sweep = expire_playback(sessions.clone(), play_queues, Arc::clone(&transcoder));
    tokio::spawn(sweep);
    let app = open_plex_api_plex::router(plex_state)
        .merge(graphql::router(schema, sign_in))
        .merge(admin::router())
        .merge(streams::router(sessions.clone()))
        .merge(callbacks::router(Arc::clone(&transcoder)));
    let app = with_web_client(app)
        // One token middleware for every surface, so neither can be left unauthenticated by
        // omission.
        .layer(axum::middleware::from_fn_with_state(
            access,
            auth::require_token,
        ))
        .layer(cors::layer())
        .layer((
            TraceLayer::new_for_http().make_span_with(request_span),
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
    let listener = listen(&bind).await?;
    tracing::info!(address = %bind, "open-plex listening");
    serve_until_signal(listener, app, sessions).await?;
    transcoder.remove_leftover_sessions();
    Ok(())
}

/// Serves until Ctrl-C, then ends every session before giving the connections a moment.
async fn serve_until_signal(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    sessions: streams::Sessions,
) -> std::io::Result<()> {
    let ended = Arc::new(tokio::sync::Notify::new());
    // With the peer's address attached, which is what a trusted network is checked against.
    let serving = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(end_on_signal(sessions, Arc::clone(&ended)));
    tokio::select! {
        served = serving.into_future() => served,
        () = async {
            ended.notified().await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => {
            tracing::info!("stopped without waiting for the remaining connections");
            Ok(())
        }
    }
}

/// Waits for Ctrl-C, then ends every session before the server stops listening: no process,
/// directory or row outlives the server that started it.
async fn end_on_signal(sessions: streams::Sessions, ended: Arc<tokio::sync::Notify>) {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::warn!(%error, "cannot listen for Ctrl-C: sessions end at the next start");
        std::future::pending::<()>().await;
    }
    tracing::info!("stopping: ending every playback session");
    if let Err(error) = sessions.lifecycle().end_all().await {
        tracing::warn!(%error, "could not delete the playback sessions");
    }
    ended.notify_one();
}

/// The kernel grows a connection's send buffer to megabytes on a fast path, and a segment that
/// fits in it seems sent at once: a stream the server adapts is judged by how long its segments
/// take to send. Inherited by every accepted connection. 256 KiB still carries 20 Mbps at
/// 100 ms of round trip.
const SEND_BUFFER_BYTES: u32 = 256 * 1024;

async fn listen(bind: &str) -> std::io::Result<tokio::net::TcpListener> {
    let address = tokio::net::lookup_host(bind).await?.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "no address to listen on")
    })?;
    let socket = if address.is_ipv4() {
        tokio::net::TcpSocket::new_v4()?
    } else {
        tokio::net::TcpSocket::new_v6()?
    };
    // As `TcpListener::bind` does. Not on Windows, where it would let two servers share a port.
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
    socket.set_send_buffer_size(SEND_BUFFER_BYTES)?;
    socket.bind(address)?;
    socket.listen(1024)
}

/// No process from before this one is still running, so no session from before is either.
async fn clear_stale_sessions(
    sessions: &streams::Sessions,
) -> Result<(), Box<dyn std::error::Error>> {
    let cleared = sessions.lifecycle().end_all().await?;
    if cleared > 0 {
        tracing::info!(
            cleared,
            "dropped the playback sessions a previous run left behind"
        );
    }
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
        markers = summary.markers.markers,
        collections = summary.collections.collections,
        collection_entries = summary.collections.entries,
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

/// The key sessions are signed with, for the transport to check them, and the sign-in
/// machinery around it, for the schema.
fn sign_in() -> Result<(Arc<SessionKey>, Arc<open_plex_api::Sessions>), String> {
    let key = Arc::new(session_key()?);
    let sessions = open_plex_api::Sessions::new(Arc::clone(&key), secure_cookies());
    Ok((key, Arc::new(sessions)))
}

/// What sessions are signed with, from `OPENPLEX_SESSION_SECRET`.
///
/// Without one, a key is drawn for this process: signing in works, but every session ends when
/// the server stops. A secret too short to sign with stops the server instead of being used.
fn session_key() -> Result<SessionKey, String> {
    match std::env::var("OPENPLEX_SESSION_SECRET") {
        Ok(secret) if !secret.is_empty() => {
            SessionKey::from_secret(secret.as_bytes()).ok_or_else(|| {
                format!(
                    "OPENPLEX_SESSION_SECRET: at least {} bytes are needed",
                    SessionKey::MIN_SECRET_BYTES
                )
            })
        }
        _ => {
            tracing::warn!(
                "OPENPLEX_SESSION_SECRET is not set: sessions will not survive a restart"
            );
            Ok(SessionKey::generate())
        }
    }
}

/// Whether the session cookie is for HTTPS only. On unless `OPENPLEX_INSECURE_COOKIES=1`, for a
/// server reached over plain HTTP on a local network; browsers count `http://localhost` as
/// secure either way.
fn secure_cookies() -> bool {
    let insecure = std::env::var("OPENPLEX_INSECURE_COOKIES").is_ok_and(|value| value == "1");
    if insecure {
        tracing::warn!("OPENPLEX_INSECURE_COOKIES=1: session cookies also travel over plain HTTP");
    }
    !insecure
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

/// A request's trace: the method and the path, never the URI. Plex clients carry the token in
/// the query string, and tower-http's default span would write it to the log.
fn request_span(request: &axum::extract::Request) -> tracing::Span {
    tracing::debug_span!(
        "request",
        method = %request.method(),
        path = request.uri().path(),
    )
}

/// `OPENPLEX_TRANSCODER` is the patched ffmpeg to run; `OPENPLEX_TRANSCODER_CODECS` where its
/// separately distributed codecs are, for Plex's own build; `OPENPLEX_TRANSCODER_FONTS` the
/// fonts subtitles are burned in with, by default `Resources/Fonts` beside the binary, where a
/// Plex install keeps them.
/// Plex Web at `/web`, when its directory holds it.
fn with_web_client(app: axum::Router) -> axum::Router {
    let web_dir = web::directory();
    if let Some(client) = web::router(&web_dir) {
        tracing::info!(directory = %web_dir.display(), "serving the web client at /web");
        app.merge(client)
    } else {
        tracing::info!(directory = %web_dir.display(), "no web client found, /web is not served");
        app
    }
}

fn transcoder() -> open_plex_transcoder::Transcoder {
    let bind = std::env::var("OPENPLEX_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let port = bind.rsplit(':').next().unwrap_or("32500");
    let binary: std::path::PathBuf = std::env::var_os("OPENPLEX_TRANSCODER")
        .map_or_else(|| "ffmpeg".into(), std::path::PathBuf::from);
    let fonts = std::env::var_os("OPENPLEX_TRANSCODER_FONTS")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            binary
                .parent()
                .map(|directory| directory.join("Resources").join("Fonts"))
        })
        // Absolute, as the process runs in its session's directory.
        .and_then(|fonts| std::fs::canonicalize(fonts).ok());
    if fonts.is_none() {
        tracing::warn!("no subtitle fonts found: subtitles cannot be burned in");
    }
    open_plex_transcoder::Transcoder::new(open_plex_transcoder::Config {
        binary,
        external_libs: std::env::var_os("OPENPLEX_TRANSCODER_CODECS").map(std::path::PathBuf::from),
        callback_base: format!("http://127.0.0.1:{port}"),
        fonts,
        // One per port, so that two servers on one machine never empty each other's.
        sessions_directory: std::env::temp_dir().join(format!("open-plex-transcode-{port}")),
    })
}

/// Removes what a vanished player left behind: its session with its stream, and old queues;
/// and what a server killed outright left on disk.
async fn expire_playback(
    sessions: streams::Sessions,
    play_queues: Arc<dyn PlayQueueRepository>,
    transcoder: Arc<open_plex_transcoder::Transcoder>,
) {
    let mut ticks = tokio::time::interval(EXPIRY_SWEEP);
    loop {
        ticks.tick().await;
        transcoder.remove_orphan_directories().await;
        match sessions
            .lifecycle()
            .sweep(open_plex_core::now_epoch())
            .await
        {
            Ok(swept) if swept == open_plex_core::lifecycle::Swept::default() => {}
            Ok(swept) => tracing::info!(
                expired = swept.expired,
                idle = swept.idle,
                "ended playback sessions"
            ),
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
