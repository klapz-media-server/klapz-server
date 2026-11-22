//! klapz server binary: configuration, composition, and mounting the bindings.
//!
//! Also carries the one-off `import` subcommand, which seeds our schema from a real Plex
//! database. Keeping it here rather than in a separate crate avoids a crate whose only job
//! is to call one function.

mod admin;
mod analyzer;
mod artwork;
mod auth;
mod callbacks;
mod commands;
mod cors;
mod files;
mod graphql;
mod remote_tokens;
mod streams;
#[cfg(windows)]
mod tray;
mod web;

use std::future::IntoFuture as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderValue, StatusCode};
use klapz_api::{build_schema, ApiContext};
use klapz_api_plex::PlexState;
use klapz_core::access::{publish_link, AccessRepository, AccountDirectory, Extension};
use klapz_core::account::{ensure_admin, AdminAccount, AdminAccountError, SessionKey};
use klapz_core::lifecycle::SessionLocks;
use klapz_core::metadata::MetadataRepository;
use klapz_core::play_queue::PlayQueueRepository;
use klapz_core::playback::PlaybackRepository;
use klapz_core::scan::BackgroundScans;
use klapz_core::{LibraryRepository, ServerCapabilities, ServerIdentity};
use klapz_db::{
    import, SqlAccessRepository, SqlLibraryRepository, SqlMetadataRepository,
    SqlPlayQueueRepository, SqlPlaybackRepository, SqlWatchRepository,
};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Where our own database lives. `rwc` creates it on first run.
const DEFAULT_DATABASE_URL: &str = "sqlite://klapz.db?mode=rwc";
/// How often expired playback sessions and idle streams are ended. Reads already skip them.
const EXPIRY_SWEEP: Duration = Duration::from_secs(30);
/// How long a stopping server waits for its connections once every session has ended: a
/// player still reading a file can hold one open.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

const DEFAULT_BIND: &str = "127.0.0.1:32500";
/// How often paired servers announce their address. The reference does it when it starts; the
/// repeat is ours, and is what retries an announcement that failed.
const PUBLISH_INTERVAL: Duration = Duration::from_secs(3600);

/// The database URL as it may be logged: an `SQLite` file by its absolute path, since a relative
/// one depends on the directory the server was started from, and any other URL without the
/// password it may carry.
fn database_shown(url: &str) -> String {
    if let Some(file) = url.strip_prefix("sqlite:") {
        let file = file.trim_start_matches("//");
        let file = file.split_once('?').map_or(file, |(path, _)| path);
        return std::path::absolute(file).map_or_else(
            |_| format!("sqlite:{file}"),
            |path| format!("sqlite:{}", path.display()),
        );
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => {
            let user = rest[..at].split(':').next().unwrap_or_default();
            format!("{scheme}://{user}:***{}", &rest[at..])
        }
        None => url.to_owned(),
    }
}

/// Source metadata as the scans and the API use it: TMDB and TVDB where they have a key, and
/// the images kept in `images`.
fn source_metadata(
    connection: &klapz_db::DatabaseConnection,
    images: &std::path::Path,
) -> Result<
    (
        Arc<klapz_transcoder::artwork::ArtworkStore>,
        klapz_core::source_metadata::Matching,
    ),
    klapz_core::source_metadata::AgentError,
> {
    let credentials = klapz_agents::ProviderCredentials::from_env();
    log_metadata_providers(&credentials);
    tracing::info!(directory = %images.display(), "images kept");
    let store = Arc::new(klapz_transcoder::artwork::ArtworkStore::new(
        images.to_path_buf(),
    ));
    let matching = klapz_core::source_metadata::Matching {
        repository: Arc::new(klapz_db::SqlSourceMetadataRepository::new(
            connection.clone(),
        )),
        sources: commands::metadata_sources(credentials)?,
        artwork: Some(Arc::new(artwork::StoreFetcher(Arc::clone(&store)))),
    };
    Ok((store, matching))
}

/// Which metadata providers have a key, and whose: never the key itself.
fn log_metadata_providers(credentials: &klapz_agents::ProviderCredentials) {
    let shown = |source: Option<klapz_agents::KeySource>| match source {
        Some(klapz_agents::KeySource::BuiltIn) => "built-in key",
        Some(klapz_agents::KeySource::Environment) => "key from the environment",
        None => "no key, not consulted",
    };
    tracing::info!(
        tmdb = shown(
            credentials
                .tmdb
                .as_ref()
                .map(klapz_agents::TmdbCredentials::source)
        ),
        tvdb = shown(
            credentials
                .tvdb
                .as_ref()
                .map(klapz_agents::TvdbCredentials::source)
        ),
        tvdb_pin = credentials
            .tvdb
            .as_ref()
            .is_some_and(|tvdb| tvdb.pin().is_some()),
        "metadata providers"
    );
}

/// The `.env` file read at startup.
struct EnvFile {
    path: PathBuf,
    /// Its variables the environment already set, whose values in the file were ignored.
    shadowed: Vec<String>,
}

/// Reads the `.env` beside the executable or, under `cargo run`, the nearest one above the
/// crate, for what the environment does not already set.
fn load_env_file() -> Result<Option<EnvFile>, dotenvy::Error> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(".env")));
    // Cargo sets it for the program it runs, never for an installed binary.
    let above = || {
        std::env::var_os("CARGO_MANIFEST_DIR").and_then(|dir| {
            PathBuf::from(dir)
                .ancestors()
                .map(|dir| dir.join(".env"))
                .find(|file| file.is_file())
        })
    };
    let Some(file) = beside.filter(|file| file.is_file()).or_else(above) else {
        return Ok(None);
    };
    let mut shadowed = Vec::new();
    for variable in dotenvy::from_path_iter(&file)? {
        let (name, _) = variable?;
        if std::env::var_os(&name).is_some() {
            shadowed.push(name);
        }
    }
    dotenvy::from_path(&file)?;
    Ok(Some(EnvFile {
        path: file,
        shadowed,
    }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Before the runtime starts: changing the environment is only sound with one thread.
    let env_file = load_env_file()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "klapz_server=info,klapz_core=info,klapz_api=info,klapz_api_plex=info,klapz_db=info".into()
            }),
        )
        .init();
    if let Some(file) = env_file {
        tracing::info!(file = %file.path.display(), "read settings from a .env file");
        for variable in file.shadowed {
            tracing::warn!(%variable, "set in the environment, which wins over the .env file");
        }
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(start())
}

#[allow(
    clippy::too_many_lines,
    reason = "the composition root: every component is built and wired here, once"
)]
async fn start() -> Result<(), Box<dyn std::error::Error>> {
    let client_origins =
        cors::ClientOrigins::from_setting(std::env::var("KLAPZ_CORS_ORIGINS").ok().as_deref())?;
    let database_url =
        std::env::var("KLAPZ_DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
    tracing::info!(database = %database_shown(&database_url), "opening the database");
    let connection = klapz_db::connect(&database_url).await?;

    // `import`, `library`, `scan` and `match` do their job and exit; anything else starts the
    // server.
    let images = artwork::images_directory(&database_url);
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Some(done) = run_command(&connection, &arguments, &images).await {
        return done;
    }

    let library: Arc<dyn LibraryRepository> =
        Arc::new(SqlLibraryRepository::new(connection.clone()));
    let (artwork_store, matching) = source_metadata(&connection, &images)?;
    let scans = BackgroundScans::new(commands::scanner(&connection, Arc::clone(&library)))
        .with_matching(matching.clone());
    tokio::spawn(check_scanner_tools());
    let play_queues: Arc<dyn PlayQueueRepository> =
        Arc::new(SqlPlayQueueRepository::new(connection.clone()));
    let playback: Arc<dyn PlaybackRepository> =
        Arc::new(SqlPlaybackRepository::new(connection.clone()));
    let users: Arc<dyn AccessRepository> = Arc::new(SqlAccessRepository::new(connection.clone()));
    let admin = ensure_admin_account(users.as_ref()).await?;
    let metadata: Arc<dyn MetadataRepository> =
        Arc::new(SqlMetadataRepository::new(connection.clone()));
    let directory: Arc<dyn AccountDirectory> = Arc::new(klapz_api_plex::plex_tv::PlexTv::new()?);
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
    let images_of_items = Arc::clone(&matching.repository);
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity(), klapz_core::now_epoch()),
        library,
        metadata,
        playback: Arc::clone(&playback),
        play_queues: Arc::clone(&play_queues),
        playlists: Arc::new(klapz_db::SqlPlaylistRepository::new(connection.clone())),
        collections: Arc::new(klapz_db::SqlCollectionRepository::new(connection.clone())),
        access: Arc::clone(&users),
        directory: Arc::clone(&directory),
        watch: Arc::new(SqlWatchRepository::new(connection)),
        streams: Arc::clone(&streams) as Arc<dyn klapz_core::stream::StreamEngine>,
        session_locks: locks,
        scans: Arc::new(scans),
        matching: Some(matching),
    });
    tokio::spawn(publish_paired_servers(
        Arc::clone(&users),
        Arc::clone(&directory),
    ));
    let (session_key, sign_in) = sign_in()?;
    let access = Arc::new(
        auth::Access::new(users)
            .with_remote_tokens(remote_tokens::RemoteTokens::new(directory))
            .with_sessions(session_key),
    );

    // The Plex surface is the base and GraphQL is merged into it, which inverts the
    // relationship the architecture describes. The reason is mechanical: Plex has paths whose
    // segments literally start with a colon (`/:/prefs`, `/photo/:/transcode`), Axum rejects
    // those unless the router has `without_v07_checks`, and `merge` re-validates whatever is
    // merged *in* with the default rules. So the router holding those paths cannot be the one
    // merged in. GraphQL has no such paths and merges cleanly either way.
    let mut plex_state = PlexState::new(identity(), schema.clone());
    plex_state.transcoder = Arc::clone(&transcoder);
    plex_state.artwork = Some(Arc::clone(&artwork_store));
    let reader = artwork::StoreReader {
        store: artwork_store,
        transcoder: Arc::clone(&transcoder),
    };
    let jellyfin_artwork = klapz_api_jellyfin::images::JellyfinArtwork {
        reader: Arc::new(reader.clone()),
        repository: images_of_items,
    };
    let sweep = expire_playback(sessions.clone(), play_queues, Arc::clone(&transcoder));
    tokio::spawn(sweep);
    let app = klapz_api_plex::router(plex_state)
        .merge(jellyfin_surface(&schema, &sign_in, jellyfin_artwork))
        .merge(graphql::router(schema, Arc::clone(&sign_in)))
        .merge(admin::router())
        .merge(streams::router(sessions.clone()))
        .merge(callbacks::router(Arc::clone(&transcoder)))
        .merge(artwork::router(Arc::clone(&reader.store)));
    let (app, web_served) = with_web_client(app);
    // One token middleware for every surface, so neither can be left unauthenticated by
    // omission.
    let app = app.layer(axum::middleware::from_fn_with_state(
        access,
        auth::require_token,
    ));
    // Outside the token's middleware, which would answer a browser `401` first; and only when
    // there is a client to be sent to.
    let app = if web_served {
        app.layer(axum::middleware::from_fn(web::send_navigation_to_client))
    } else {
        app
    };
    if !client_origins.configured().is_empty() {
        tracing::info!(origins = ?client_origins.configured(), "web client origins allowed");
    }
    let app = app.layer(cors::layer(client_origins)).layer((
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

    serve(app, sessions, web_served, admin, sign_in).await?;
    transcoder.remove_leftover_sessions();
    Ok(())
}

/// Listens on `KLAPZ_BIND` and serves until stopped, with the tray icon on Windows: its "Open
/// web app" signs the browser in as `admin` when the web client is served.
async fn serve(
    app: axum::Router,
    sessions: streams::Sessions,
    web_served: bool,
    admin: klapz_core::UserId,
    sign_in: Arc<klapz_api::Sessions>,
) -> std::io::Result<()> {
    let bind = std::env::var("KLAPZ_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let listener = listen(&bind).await?;
    tracing::info!(address = %bind, "klapz listening");
    let quit = Arc::new(tokio::sync::Notify::new());
    #[cfg(windows)]
    let tray = show_tray(web_served.then_some((admin, sign_in)), &listener, &quit);
    #[cfg(not(windows))]
    let _ = (web_served, admin, sign_in);
    let served = serve_until_signal(listener, app, sessions, quit).await;
    #[cfg(windows)]
    if let Some(tray) = tray {
        tray.close();
    }
    served
}

/// The tray icon; `web_app`, the user its "Open web app" signs in, when there is a client.
#[cfg(windows)]
fn show_tray(
    web_app: Option<(klapz_core::UserId, Arc<klapz_api::Sessions>)>,
    listener: &tokio::net::TcpListener,
    quit: &Arc<tokio::sync::Notify>,
) -> Option<tray::Tray> {
    let web_app: Option<Box<dyn Fn() -> String + Send>> = match (web_app, listener.local_addr()) {
        (Some((admin, sign_in)), Ok(address)) => {
            let origin = tray::local_origin(address);
            Some(Box::new(move || {
                format!(
                    "{origin}/web/?ticket={}",
                    sign_in.issue_sign_in_ticket(admin)
                )
            }))
        }
        _ => None,
    };
    tray::Tray::show(tray::Actions {
        web_app,
        quit: Arc::clone(quit),
    })
}

/// Says at startup, once, that scans cannot analyse files, rather than leaving it to the first
/// scan to find out, by asking the scanner the scans will run.
async fn check_scanner_tools() {
    if let Err(error) = analyzer::ScannerProcess::from_env().check().await {
        tracing::error!(%error, "library scans cannot analyse files until this is fixed");
    }
}

/// Serves until Ctrl-C or `quit`, then ends every session before giving the connections a moment.
async fn serve_until_signal(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    sessions: streams::Sessions,
    quit: Arc<tokio::sync::Notify>,
) -> std::io::Result<()> {
    let ended = Arc::new(tokio::sync::Notify::new());
    // With the peer's address attached, which is how the transcoder's callbacks know loopback.
    let serving = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(end_on_signal(sessions, quit, Arc::clone(&ended)));
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

/// Waits for Ctrl-C or `quit` (the tray icon's), then ends every session before the server stops listening: no process,
/// directory or row outlives the server that started it.
async fn end_on_signal(
    sessions: streams::Sessions,
    quit: Arc<tokio::sync::Notify>,
    ended: Arc<tokio::sync::Notify>,
) {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "cannot listen for Ctrl-C: sessions end at the next start");
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        () = ctrl_c => {}
        () = quit.notified() => {}
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

/// A job the command line names, run to its end; `None` when it names none and the server is to
/// start.
async fn run_command(
    connection: &klapz_db::DatabaseConnection,
    arguments: &[String],
    images: &std::path::Path,
) -> Option<Result<(), Box<dyn std::error::Error>>> {
    let (command, rest) = arguments.split_first()?;
    Some(match command.as_str() {
        "import" => match rest.first() {
            Some(source) => run_import(connection, &PathBuf::from(source)).await,
            None => Err("usage: klapz-server import <plex.db>".into()),
        },
        "library" => commands::library(connection, rest).await,
        "scan" => commands::scan(connection, rest, images).await,
        "match" => commands::match_command(connection, rest, images).await,
        _ => return None,
    })
}

async fn run_import(
    connection: &klapz_db::DatabaseConnection,
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
        extras = summary.extras.extras,
        orphans = summary.orphans,
        "import complete"
    );
    Ok(())
}

/// Makes sure the built-in `admin` account exists, an active owner, before anything is served.
///
/// `KLAPZ_ADMIN_PASSWORD` sets its password, at creation and on every start; without it, a
/// new account gets a generated one and an existing one keeps its own. A password the policy
/// refuses stops the server. A generated password goes to standard output, once, and never
/// to the logs, which may be kept or shipped where a password must not go. Its id, for the
/// tray icon to sign a browser in as it.
async fn ensure_admin_account(users: &dyn AccessRepository) -> Result<klapz_core::UserId, String> {
    let configured = std::env::var("KLAPZ_ADMIN_PASSWORD")
        .ok()
        .filter(|password| !password.is_empty());
    match ensure_admin(users, configured.as_deref(), klapz_core::now_epoch()).await {
        Ok(AdminAccount::Created {
            generated_password: Some(password),
        }) => {
            println!(
                "\nklapz created its `admin` account. Its password, shown this once:\n\n    \
                 {password}\n\nSign in with it at /admin. Set KLAPZ_ADMIN_PASSWORD to \
                 choose another.\n"
            );
            tracing::info!("created the admin account, with a generated password");
        }
        Ok(AdminAccount::Created {
            generated_password: None,
        }) => tracing::info!("created the admin account, with KLAPZ_ADMIN_PASSWORD"),
        Ok(AdminAccount::PasswordReplaced) => {
            tracing::info!("the admin account takes KLAPZ_ADMIN_PASSWORD");
        }
        Ok(AdminAccount::Kept) => {}
        Err(error @ AdminAccountError::WeakPassword) => {
            return Err(format!("KLAPZ_ADMIN_PASSWORD: {error}"));
        }
        Err(error) => return Err(format!("could not set the admin account up: {error}")),
    }
    match users.user_named(klapz_core::access::ADMIN_USERNAME).await {
        Ok(Some(admin)) => Ok(admin.id),
        Ok(None) => Err("the admin account vanished as it was set up".to_owned()),
        Err(error) => Err(format!("could not read the admin account: {error}")),
    }
}

/// The key sessions are signed with, for the transport to check them, and the sign-in
/// machinery around it, for the schema.
fn sign_in() -> Result<(Arc<SessionKey>, Arc<klapz_api::Sessions>), String> {
    let key = Arc::new(session_key()?);
    let sessions = klapz_api::Sessions::new(Arc::clone(&key), secure_cookies());
    Ok((key, Arc::new(sessions)))
}

/// What sessions are signed with, from `KLAPZ_SESSION_SECRET`.
///
/// Without one, a key is drawn for this process: signing in works, but every session ends when
/// the server stops. A secret too short to sign with stops the server instead of being used.
fn session_key() -> Result<SessionKey, String> {
    match std::env::var("KLAPZ_SESSION_SECRET") {
        Ok(secret) if !secret.is_empty() => {
            SessionKey::from_secret(secret.as_bytes()).ok_or_else(|| {
                format!(
                    "KLAPZ_SESSION_SECRET: at least {} bytes are needed",
                    SessionKey::MIN_SECRET_BYTES
                )
            })
        }
        _ => {
            tracing::warn!("KLAPZ_SESSION_SECRET is not set: sessions will not survive a restart");
            Ok(SessionKey::generate())
        }
    }
}

/// Whether the session cookie is for HTTPS only. On unless `KLAPZ_INSECURE_COOKIES=1`, for a
/// server reached over plain HTTP on a local network; browsers count `http://localhost` as
/// secure either way.
fn secure_cookies() -> bool {
    let insecure = std::env::var("KLAPZ_INSECURE_COOKIES").is_ok_and(|value| value == "1");
    if insecure {
        tracing::warn!("KLAPZ_INSECURE_COOKIES=1: session cookies also travel over plain HTTP");
    }
    !insecure
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

/// `KLAPZ_TRANSCODER` is the patched ffmpeg to run; `KLAPZ_TRANSCODER_CODECS` where its
/// separately distributed codecs are, for Plex's own build; `KLAPZ_TRANSCODER_FONTS` the
/// fonts subtitles are burned in with, by default `Resources/Fonts` beside the binary, where a
/// Plex install keeps them.
/// Plex Web at `/web`, when its directory holds it, and whether it does.
fn with_web_client(app: axum::Router) -> (axum::Router, bool) {
    let Some(web_dir) = web::directory() else {
        tracing::info!("no web client directory, /web is not served");
        return (app, false);
    };
    if let Some(client) = web::router(&web_dir) {
        tracing::info!(directory = %web_dir.display(), "serving the web client at /web");
        (app.merge(client), true)
    } else {
        tracing::info!(directory = %web_dir.display(), "no web client found, /web is not served");
        (app, false)
    }
}

/// The Jellyfin surface, with Jellyfin Web when there is one to serve.
fn jellyfin_surface(
    schema: &klapz_api::ApiSchema,
    sign_in: &Arc<klapz_api::Sessions>,
    artwork: klapz_api_jellyfin::images::JellyfinArtwork,
) -> axum::Router {
    let mut state =
        klapz_api_jellyfin::JellyfinState::new(schema.clone(), Arc::clone(sign_in), &identity());
    state.web_dir = jellyfin_web_dir();
    state.artwork = Some(artwork);
    klapz_api_jellyfin::router(state)
}

/// Jellyfin Web, served below the Jellyfin surface when `KLAPZ_JELLYFIN_WEB_DIR` names a
/// directory holding it. Not versioned here: it is Jellyfin's code.
fn jellyfin_web_dir() -> Option<PathBuf> {
    let directory = PathBuf::from(std::env::var_os("KLAPZ_JELLYFIN_WEB_DIR")?);
    if directory.join("index.html").is_file() {
        tracing::info!(directory = %directory.display(), "serving Jellyfin Web at /jellyfin/web");
        Some(directory)
    } else {
        tracing::warn!(directory = %directory.display(), "no Jellyfin Web found there");
        None
    }
}

/// `name` in the directory of this server's executable, where the packages lay out what it
/// runs and serves.
pub(crate) fn beside_executable(name: &str) -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(name))
}

/// A program beside this server's executable, where Cargo and the packages put it, or else the
/// one `PATH` finds.
pub(crate) fn bundled(program: &str) -> PathBuf {
    let name = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    beside_executable(&name)
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn transcoder() -> klapz_transcoder::Transcoder {
    let bind = std::env::var("KLAPZ_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let port = bind.rsplit(':').next().unwrap_or("32500");
    let binary = klapz_scanner::transcoder::binary();
    // A bare name is the one `PATH` finds, which may be a stock build without Plex's options.
    tracing::info!(binary = %binary.display(), "transcoder");
    let fonts = std::env::var_os("KLAPZ_TRANSCODER_FONTS")
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
    klapz_transcoder::Transcoder::new(klapz_transcoder::Config {
        binary,
        external_libs: std::env::var_os("KLAPZ_TRANSCODER_CODECS").map(std::path::PathBuf::from),
        callback_base: format!("http://127.0.0.1:{port}"),
        fonts,
        // One per port, so that two servers on one machine never empty each other's.
        sessions_directory: std::env::temp_dir().join(format!("klapz-transcode-{port}")),
    })
}

/// Removes what a vanished player left behind: its session with its stream, and old queues;
/// and what a server killed outright left on disk.
async fn expire_playback(
    sessions: streams::Sessions,
    play_queues: Arc<dyn PlayQueueRepository>,
    transcoder: Arc<klapz_transcoder::Transcoder>,
) {
    let mut ticks = tokio::time::interval(EXPIRY_SWEEP);
    loop {
        ticks.tick().await;
        transcoder.remove_orphan_directories().await;
        match sessions.lifecycle().sweep(klapz_core::now_epoch()).await {
            Ok(swept) if swept == klapz_core::lifecycle::Swept::default() => {}
            Ok(swept) => tracing::info!(
                expired = swept.expired,
                idle = swept.idle,
                "ended playback sessions"
            ),
            Err(error) => tracing::warn!(%error, "could not expire playback sessions"),
        }
        let cutoff = klapz_core::now_epoch() - klapz_core::play_queue::QUEUE_RETENTION_SECONDS;
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
        let window =
            klapz_core::page::Window::by_id(after, None, Some(klapz_core::page::MAX_ITEMS), None)?;
        let page = users.users(window).await?;
        for user in &page.items {
            let link = users.account_link(user.id, Extension::Plex).await?;
            let Some(link) = link.filter(|link| link.enabled) else {
                continue;
            };
            let now = klapz_core::now_epoch();
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
    let machine_identifier = std::env::var("KLAPZ_MACHINE_ID")
        .unwrap_or_else(|_| "0000000000000000000000000000000000000000".to_owned());
    ServerIdentity::new(machine_identifier, env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::database_shown;

    #[test]
    fn shows_a_relative_sqlite_file_by_its_absolute_path() {
        let expected = std::env::current_dir().unwrap().join("klapz.db");
        assert_eq!(
            database_shown("sqlite://klapz.db?mode=rwc"),
            format!("sqlite:{}", expected.display())
        );
    }

    #[test]
    fn hides_the_password_of_a_server_database() {
        assert_eq!(
            database_shown("postgres://klapz:secret@db.local:5432/klapz"),
            "postgres://klapz:***@db.local:5432/klapz"
        );
    }

    #[test]
    fn leaves_a_url_without_credentials_as_it_is() {
        assert_eq!(
            database_shown("postgres://db.local/klapz"),
            "postgres://db.local/klapz"
        );
    }
}
