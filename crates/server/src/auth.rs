//! HTTP authentication, shared by every surface.
//!
//! Lives here rather than in a binding because the official GraphQL API and the Plex adapter
//! both need it, and neither should depend on the other. The middleware resolves the request's
//! credential to the user it belongs to — a [`Principal`] — and attaches it; each surface then
//! enforces roles in its own idiom. There is no caller that is not a user.
//!
//! Three credentials, tried in this order, the first that opens something winning:
//!
//! - a Plex token, `X-Plex-Token`, issued by plex.tv to the account a user paired;
//! - a session as `Authorization: Bearer`, what a script or an API client sends after
//!   `signIn`;
//! - the same session in its cookie, what a browser sends.
//!
//! The Jellyfin surface, below its own base path, takes Jellyfin tokens and nothing else: only
//! a user connected through the Jellyfin extension reaches it, whatever else it holds. Its
//! clients carry the token in their own headers, which `klapz-api-jellyfin` reads.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use klapz_core::access::{
    connection_uris_for_host, token_digest, user_principal, AccessRepository, Extension,
};
use klapz_core::account::SessionKey;
use klapz_core::{Principal, PublishedServer};

use crate::remote_tokens::RemoteTokens;

/// The header and query parameter carrying a Plex token.
///
/// Both forms are required for compatibility: the artwork and stream URLs a server hands out
/// embed the token in the query string, and the bundled Plex web client passes every
/// `X-Plex-*` value that way rather than as headers.
const TOKEN_KEY: &str = "X-Plex-Token";

/// Paths served without authentication.
///
/// Kept deliberately short. `/identity` must be reachable with no credential at all: clients
/// call it to decide whether a host is a Plex server before they hold any token. `/` is not
/// public, as on the reference: its `401` is what sends Plex Web to sign in, and a `200` has it
/// carry on anonymously into refusals it can only present as "not authorized".
const PUBLIC_PATHS: &[&str] = &["/identity", crate::admin::PATH];

/// Who may use the server: the users, the tokens issued to them or to the accounts they paired,
/// and the sessions they open.
#[derive(Debug)]
pub struct Access {
    users: Arc<dyn AccessRepository>,
    remote: Option<Arc<RemoteTokens>>,
    sessions: Option<Arc<SessionKey>>,
}

impl Access {
    pub fn new(users: Arc<dyn AccessRepository>) -> Self {
        Self {
            users,
            remote: None,
            sessions: None,
        }
    }

    /// Accepts the sessions this key signed.
    #[must_use]
    pub fn with_sessions(mut self, key: Arc<SessionKey>) -> Self {
        self.sessions = Some(key);
        self
    }

    /// Adds the tokens an account service issued, which is what a client that found this
    /// server through that service presents.
    #[must_use]
    pub fn with_remote_tokens(mut self, remote: RemoteTokens) -> Self {
        self.remote = Some(Arc::new(remote));
        self
    }

    /// The active user a session names, with its libraries. The signature is checked by the
    /// key, in constant time.
    async fn session_of(&self, token: &str) -> Option<Principal> {
        let user = self
            .sessions
            .as_ref()?
            .verify(token, klapz_core::now_epoch())?;
        let found = match self.users.user(user).await {
            Ok(found) => found.filter(|user| user.active)?,
            Err(error) => {
                tracing::warn!(%error, "could not look a session's user up");
                return None;
            }
        };
        match self.users.permissions(found.id).await {
            Ok(permissions) => Some(user_principal(found, &permissions)),
            Err(error) => {
                tracing::warn!(%error, "could not read a session's permissions");
                None
            }
        }
    }

    /// Who a Plex token belongs to. Only plex.tv issues them: it says whose account a token is,
    /// and the user paired with that account is the caller. A storage failure refuses the token
    /// rather than the server.
    async fn plex_token_of(&self, token: &str) -> Option<Principal> {
        let Some(account) = self.remote.as_ref()?.account_of(token).await else {
            tracing::debug!("refused a token no account service attributes");
            return None;
        };
        let grant = self
            .users
            .grant_for_account(Extension::Plex, &account)
            .await;
        if matches!(grant, Ok(None)) {
            tracing::debug!(%account, "refused the token of an account not paired here");
        }
        match grant {
            Ok(grant) => grant.map(klapz_core::access::Grant::into_principal),
            Err(error) => {
                tracing::warn!(%error, "could not look a token up");
                None
            }
        }
    }

    /// Who a Jellyfin token belongs to: the user of the device it was given to.
    async fn jellyfin_token_of(&self, token: &str) -> Option<Principal> {
        match self
            .users
            .grant(Extension::Jellyfin, &token_digest(token))
            .await
        {
            Ok(grant) => grant.map(klapz_core::access::Grant::into_principal),
            Err(error) => {
                tracing::warn!(%error, "could not look a Jellyfin token up");
                None
            }
        }
    }

    /// The server published at the address a request naming no account was sent to.
    async fn published_at(&self, host: &str) -> Option<PublishedServer> {
        let uris = connection_uris_for_host(host);
        match self.users.connection_at(Extension::Plex, &uris).await {
            Ok(connection) => connection.map(|found| PublishedServer(found.external_id)),
            Err(error) => {
                tracing::warn!(%error, "could not look an address up");
                None
            }
        }
    }

    /// The user behind the first of the request's credentials that opens something.
    async fn caller_of(&self, credentials: Credentials) -> Option<Principal> {
        if let Some(token) = credentials.plex_token {
            if let Some(principal) = self.plex_token_of(&token).await {
                return Some(principal);
            }
        }
        for session in [credentials.bearer, credentials.cookie]
            .into_iter()
            .flatten()
        {
            if let Some(principal) = self.session_of(&session).await {
                return Some(principal);
            }
        }
        None
    }
}

/// What a request presents, read before anything is awaited: a request's body cannot be held
/// across an await.
#[derive(Default)]
struct Credentials {
    plex_token: Option<String>,
    bearer: Option<String>,
    cookie: Option<String>,
}

impl Credentials {
    fn of(request: &Request) -> Self {
        Self {
            plex_token: token_from(request),
            bearer: bearer_token(request).map(str::to_owned),
            cookie: session_cookie(request).map(str::to_owned),
        }
    }
}

/// The transcoder process calling back. It holds no token: these routes answer only on
/// loopback and to the unguessable id of a session it was started for (`callbacks`).
fn is_transcoder_callback(path: &str) -> bool {
    path.starts_with("/transcoder/")
}

/// A stream at an address of our own, which carries a secret of its own instead of a token.
fn is_native_stream(path: &str) -> bool {
    path.starts_with("/streams/")
}

pub async fn require_token(
    State(access): State<Arc<Access>>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if let Some(jellyfin_path) = klapz_api_jellyfin::surface_path(path) {
        let public = klapz_api_jellyfin::auth::is_public(jellyfin_path);
        let token = klapz_api_jellyfin::auth::token(request.headers(), request.uri().query());
        let caller = match token {
            Some(token) => access.jellyfin_token_of(&token).await,
            None => None,
        };
        return match caller {
            Some(principal) => {
                request.extensions_mut().insert(principal);
                next.run(request).await
            }
            None if public => next.run(request).await,
            // Bare, as the reference refuses.
            None => StatusCode::UNAUTHORIZED.into_response(),
        };
    }
    // The web client is public on the reference too: it has to load before it can sign in.
    if crate::web::is_client_path(path) || is_transcoder_callback(path) || is_native_stream(path) {
        return next.run(request).await;
    }
    let caller = access.caller_of(Credentials::of(&request)).await;

    // Public, but a caller who says who it is sees its own server's identifier, and one that
    // does not, the identifier of the server published at the address it used.
    if PUBLIC_PATHS.contains(&request.uri().path()) {
        if let Some(principal) = caller {
            request.extensions_mut().insert(principal);
        } else {
            let host = request
                .headers()
                .get(header::HOST)
                .and_then(|host| host.to_str().ok())
                .map(str::to_owned);
            if let Some(host) = host {
                if let Some(published) = access.published_at(&host).await {
                    request.extensions_mut().insert(published);
                }
            }
        }
        return next.run(request).await;
    }

    let Some(principal) = caller else {
        // Signing in happens on the GraphQL endpoint, before the caller is anyone. It goes
        // through with no principal, which the schema answers as nobody: it sees nothing.
        if request.uri().path() == "/graphql" {
            return next.run(request).await;
        }
        return unauthorized();
    };

    // Logged without the URI: the token travels in the query string, so logging the full
    // target would write credentials to disk.
    tracing::debug!(principal = %principal.name, path = request.uri().path(), "authenticated");

    request.extensions_mut().insert(principal);
    next.run(request).await
}

/// The session cookie's value, if the request carries one.
fn session_cookie(request: &Request) -> Option<&str> {
    request
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == klapz_api::SESSION_COOKIE)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

/// The session an `Authorization: Bearer` header carries. The scheme is case-insensitive.
fn bearer_token(request: &Request) -> Option<&str> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// Reads the Plex token from the header, falling back to the query string.
///
/// Header first because it is the form that does not end up in logs or `Referer`.
fn token_from(request: &Request) -> Option<String> {
    if let Some(value) = request.headers().get(TOKEN_KEY) {
        if let Ok(token) = value.to_str() {
            return Some(token.to_owned());
        }
    }

    let query = request.uri().query()?;
    form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == TOKEN_KEY)
        .map(|(_, value)| value.into_owned())
}

/// The minimal HTML body a real Plex server returns, content negotiation ignored.
///
/// Duplicating the shape here rather than reusing the adapter's would risk the two drifting;
/// this is the one response the middleware produces, and the adapter's tests pin the bytes.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/html"))],
        "<html><head><title>Unauthorized</title></head><body>\
         <h1>401 Unauthorized</h1></body></html>",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::{Extension, Router};
    use klapz_core::access::{
        token_digest, AccessRepository, Extension as Protocol, NewUser, RemoteAccount, ServerRole,
    };
    use klapz_core::account::SessionKey;
    use klapz_core::testing::{InMemoryAccess, StubDirectory};
    use klapz_core::{Principal, PublishedServer};
    use tower::ServiceExt;

    use super::{require_token, Access, Arc, StatusCode};

    /// The plex.tv token of the account `alice`, an active owner, paired.
    const TOKEN: &str = StubDirectory::ACCOUNT_TOKEN;

    /// Echoes the caller's username, the server a stranger sees, or `anonymous`.
    async fn whoami(
        principal: Option<Extension<Principal>>,
        published: Option<Extension<PublishedServer>>,
    ) -> String {
        match (principal, published) {
            (Some(Extension(principal)), _) => principal.name,
            (None, Some(Extension(PublishedServer(server)))) => format!("visitor of {server}"),
            (None, None) => "anonymous".to_owned(),
        }
    }

    struct Fixture {
        app: Router,
        key: Arc<SessionKey>,
        alice: klapz_core::UserId,
        bob: klapz_core::UserId,
    }

    /// `alice`, an active owner connected through Plex and paired with the stub directory's
    /// account when `paired`, its connection switched off unless `enabled`; and `bob`, who
    /// signed up and waits.
    async fn fixture(paired: bool, enabled: bool) -> Fixture {
        let users = Arc::new(InMemoryAccess::default());
        let alice = users
            .create_user(
                NewUser {
                    username: "alice",
                    email: Some("alice@example.org"),
                    role: ServerRole::Owner,
                    active: true,
                    password_hash: None,
                },
                0,
            )
            .await
            .unwrap()
            .id;
        let bob = users
            .create_user(
                NewUser {
                    username: "bob",
                    email: None,
                    role: ServerRole::User,
                    active: false,
                    password_hash: Some("$argon2id$x"),
                },
                0,
            )
            .await
            .unwrap()
            .id;
        users
            .connect_extension(alice, Protocol::Plex, "server", 0)
            .await
            .unwrap();
        if paired {
            let account = RemoteAccount {
                id: "linked".to_owned(),
                name: "alice".to_owned(),
                token: StubDirectory::ACCOUNT_TOKEN.to_owned(),
            };
            users
                .link_account(alice, Protocol::Plex, &account, "https://a.example.org", 0)
                .await
                .unwrap();
        }
        if !enabled {
            users
                .disable_extension(alice, Protocol::Plex, 0)
                .await
                .unwrap();
        }
        let key = Arc::new(SessionKey::generate());
        let access = Arc::new(
            Access::new(users)
                .with_remote_tokens(crate::remote_tokens::RemoteTokens::new(Arc::new(
                    StubDirectory::default(),
                )))
                .with_sessions(Arc::clone(&key)),
        );
        let app = Router::new()
            // Plex's paths have segments starting with a colon.
            .without_v07_checks()
            .route("/library/sections", get(whoami))
            .route("/identity", get(whoami))
            .route("/graphql", get(whoami))
            .route("/", get(whoami))
            .route("/streams/s1/0/header", get(whoami))
            .route("/video/:/transcode/universal/start.mpd", get(whoami))
            .layer(axum::middleware::from_fn_with_state(access, require_token));
        Fixture {
            app,
            key,
            alice,
            bob,
        }
    }

    async fn app() -> Router {
        fixture(true, true).await.app
    }

    async fn send(app: Router, request: axum::http::request::Builder) -> (StatusCode, String) {
        let response = app
            .oneshot(request.body(Body::empty()).expect("request builds"))
            .await
            .expect("router responds");
        let status = response.status();
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .expect("body")
            .to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    async fn call(uri: &str, token: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::builder().uri(uri);
        if let Some(token) = token {
            request = request.header("X-Plex-Token", token);
        }
        send(app().await, request).await
    }

    #[tokio::test]
    async fn accepts_a_plex_token_in_a_header() {
        assert_eq!(
            call("/library/sections", Some(TOKEN)).await,
            (StatusCode::OK, "alice".to_owned())
        );
    }

    #[tokio::test]
    async fn accepts_a_plex_token_in_the_query_string() {
        // Artwork and stream URLs a server hands out carry it this way, and the bundled web
        // client passes every X-Plex-* value as a query parameter.
        let (status, body) = call(&format!("/library/sections?X-Plex-Token={TOKEN}"), None).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "alice"));
    }

    #[tokio::test]
    async fn prefers_the_header_over_the_query_string() {
        // The header is the form that does not leak into logs or `Referer`.
        let (status, body) = call("/library/sections?X-Plex-Token=wrong", Some(TOKEN)).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "alice"));
    }

    #[tokio::test]
    async fn refuses_a_missing_unknown_or_truncated_token() {
        let truncated = &TOKEN[..TOKEN.len() - 1];
        for token in [None, Some("nope"), Some(truncated)] {
            assert_eq!(
                call("/library/sections", token).await.0,
                StatusCode::UNAUTHORIZED,
                "{token:?}"
            );
        }
    }

    #[tokio::test]
    async fn answers_rejections_with_the_reference_html_body() {
        let (status, body) = call("/library/sections", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            body,
            "<html><head><title>Unauthorized</title></head><body><h1>401 Unauthorized</h1></body></html>"
        );
        assert_eq!(body.len(), 91);
    }

    #[tokio::test]
    async fn the_address_used_says_whose_identifier_a_stranger_sees() {
        let asked_at = |app: Router, host: &'static str| {
            send(
                app,
                Request::builder().uri("/identity").header("Host", host),
            )
        };
        assert_eq!(
            asked_at(fixture(true, true).await.app, "A.example.org")
                .await
                .1,
            "visitor of server"
        );
        assert_eq!(
            asked_at(fixture(true, true).await.app, "b.example.org")
                .await
                .1,
            "anonymous"
        );
        assert_eq!(
            asked_at(fixture(true, false).await.app, "a.example.org")
                .await
                .1,
            "anonymous"
        );
    }

    #[tokio::test]
    async fn a_token_of_a_paired_account_acts_as_its_user() {
        let (status, name) = send(
            fixture(true, true).await.app,
            Request::builder()
                .uri("/library/sections")
                .header("X-Plex-Token", StubDirectory::ACCOUNT_TOKEN),
        )
        .await;
        assert_eq!((status, name.as_str()), (StatusCode::OK, "alice"));
    }

    #[tokio::test]
    async fn a_switched_off_connection_refuses_its_accounts_token() {
        let (status, _) = send(
            fixture(true, false).await.app,
            Request::builder()
                .uri("/library/sections")
                .header("X-Plex-Token", TOKEN),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_token_of_an_account_paired_with_nobody_opens_nothing() {
        let (status, _) = send(
            fixture(false, true).await.app,
            Request::builder()
                .uri("/library/sections")
                .header("X-Plex-Token", TOKEN),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "an account paired with nobody"
        );
    }

    #[tokio::test]
    async fn a_streams_segments_need_no_token_but_starting_one_does() {
        // The stream's own secret is checked by its routes.
        assert_eq!(call("/streams/s1/0/header", None).await.0, StatusCode::OK);
        assert_eq!(
            call("/video/:/transcode/universal/start.mpd", None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn the_capabilities_need_a_token() {
        // Measured on the reference, whose answer is what makes Plex Web ask for a sign-in.
        assert_eq!(call("/", None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call("/", Some(TOKEN)).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn lets_identity_through_unauthenticated() {
        // Clients call it to decide whether a host is a Plex server at all, before they hold
        // any token.
        assert_eq!(
            call("/identity", None).await,
            (StatusCode::OK, "anonymous".to_owned())
        );
    }

    fn cookie(session: &str) -> String {
        format!("theme=dark; {}={session}", klapz_api::SESSION_COOKIE)
    }

    #[tokio::test]
    async fn a_session_acts_as_its_user_in_a_cookie_or_as_a_bearer() {
        let fixture = fixture(false, true).await;
        let session = fixture
            .key
            .issue(fixture.alice, klapz_core::now_epoch())
            .unwrap();
        for request in [
            Request::builder().header("Cookie", cookie(&session)),
            Request::builder().header("Authorization", format!("Bearer {session}")),
            Request::builder().header("Authorization", format!("bearer {session}")),
        ] {
            assert_eq!(
                send(fixture.app.clone(), request.uri("/library/sections")).await,
                (StatusCode::OK, "alice".to_owned())
            );
        }
    }

    #[tokio::test]
    async fn the_session_of_an_inactive_user_or_an_altered_one_opens_nothing() {
        let fixture = fixture(false, true).await;
        let now = klapz_core::now_epoch();
        let waiting = fixture.key.issue(fixture.bob, now).unwrap();
        let altered = format!("{}x", fixture.key.issue(fixture.alice, now).unwrap());
        let foreign = SessionKey::generate().issue(fixture.alice, now).unwrap();
        for session in [&waiting, &altered, &foreign] {
            for request in [
                Request::builder().header("Cookie", cookie(session)),
                Request::builder().header("Authorization", format!("Bearer {session}")),
            ] {
                let (status, _) = send(fixture.app.clone(), request.uri("/library/sections")).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{session}");
            }
        }
    }

    #[tokio::test]
    async fn a_plex_token_is_not_a_bearer_session() {
        let (status, _) = send(
            app().await,
            Request::builder()
                .uri("/library/sections")
                .header("Authorization", format!("Bearer {TOKEN}")),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_stale_token_falls_back_to_the_session() {
        let fixture = fixture(false, true).await;
        let session = fixture
            .key
            .issue(fixture.alice, klapz_core::now_epoch())
            .unwrap();
        let (status, name) = send(
            fixture.app,
            Request::builder()
                .uri("/library/sections")
                .header("X-Plex-Token", "stale")
                .header("Cookie", cookie(&session)),
        )
        .await;
        assert_eq!((status, name.as_str()), (StatusCode::OK, "alice"));
    }

    #[tokio::test]
    async fn the_graphql_endpoint_lets_nobody_through_to_sign_in_and_nothing_else_does() {
        assert_eq!(
            call("/graphql", None).await,
            (StatusCode::OK, "anonymous".to_owned())
        );
        assert_eq!(
            call("/library/sections", None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    /// The Jellyfin token of `alice`'s device.
    const DEVICE_TOKEN: &str = "alice-jellyfin-device";

    /// `alice` as in [`fixture`], connected through Jellyfin as well when `jellyfin` and
    /// signed in from a device, then switched off unless `enabled`; and the Jellyfin routes.
    async fn jellyfin_fixture(jellyfin: bool, enabled: bool) -> Fixture {
        let users = Arc::new(InMemoryAccess::default());
        let alice = users
            .create_user(
                NewUser {
                    username: "alice",
                    email: None,
                    role: ServerRole::User,
                    active: true,
                    password_hash: None,
                },
                0,
            )
            .await
            .unwrap()
            .id;
        users
            .connect_extension(alice, Protocol::Plex, "server", 0)
            .await
            .unwrap();
        if jellyfin {
            users
                .connect_extension(alice, Protocol::Jellyfin, "jf", 0)
                .await
                .unwrap();
            let device = klapz_core::access::ExtensionDevice {
                id: "d1".to_owned(),
                name: "Firefox".to_owned(),
                client: "Jellyfin Web".to_owned(),
                client_version: "10.11.0".to_owned(),
            };
            users
                .issue_device_token(
                    alice,
                    Protocol::Jellyfin,
                    &device,
                    &token_digest(DEVICE_TOKEN),
                    0,
                )
                .await
                .unwrap();
            if !enabled {
                users
                    .disable_extension(alice, Protocol::Jellyfin, 0)
                    .await
                    .unwrap();
            }
        }
        let key = Arc::new(SessionKey::generate());
        let access = Arc::new(Access::new(users).with_sessions(Arc::clone(&key)));
        let app = Router::new()
            .route("/jellyfin/System/Info", get(whoami))
            .route("/jellyfin/System/Info/Public", get(whoami))
            .route("/library/sections", get(whoami))
            .layer(axum::middleware::from_fn_with_state(access, require_token));
        Fixture {
            app,
            key,
            alice,
            bob: alice,
        }
    }

    fn jellyfin_header(token: &str) -> String {
        format!(r#"MediaBrowser Client="Jellyfin Web", DeviceId="d1", Token="{token}""#)
    }

    #[tokio::test]
    async fn a_jellyfin_token_acts_as_its_user_on_the_jellyfin_surface() {
        let app = jellyfin_fixture(true, true).await.app;
        for request in [
            Request::builder().header("Authorization", jellyfin_header(DEVICE_TOKEN)),
            Request::builder().header("X-Emby-Token", DEVICE_TOKEN),
        ] {
            assert_eq!(
                send(app.clone(), request.uri("/jellyfin/System/Info")).await,
                (StatusCode::OK, "alice".to_owned())
            );
        }
        assert_eq!(
            send(
                app,
                Request::builder().uri(format!("/jellyfin/System/Info?ApiKey={DEVICE_TOKEN}"))
            )
            .await,
            (StatusCode::OK, "alice".to_owned())
        );
    }

    #[tokio::test]
    async fn the_jellyfin_surface_refuses_nobody_with_a_bare_401_but_its_public_paths() {
        let app = jellyfin_fixture(true, true).await.app;
        let (status, body) =
            send(app.clone(), Request::builder().uri("/jellyfin/System/Info")).await;
        assert_eq!((status, body.as_str()), (StatusCode::UNAUTHORIZED, ""));
        assert_eq!(
            send(app, Request::builder().uri("/jellyfin/System/Info/Public")).await,
            (StatusCode::OK, "anonymous".to_owned())
        );
    }

    #[tokio::test]
    async fn refuses_a_user_without_a_jellyfin_extension() {
        // A Plex connection, a session or a Plex token open nothing on the Jellyfin surface.
        let fixture = jellyfin_fixture(false, true).await;
        let session = fixture
            .key
            .issue(fixture.alice, klapz_core::now_epoch())
            .unwrap();
        for request in [
            Request::builder().header("Authorization", jellyfin_header(DEVICE_TOKEN)),
            Request::builder().header("Authorization", jellyfin_header(TOKEN)),
            Request::builder().header("X-Plex-Token", TOKEN),
            Request::builder().header("Authorization", format!("Bearer {session}")),
            Request::builder().header("Cookie", cookie(&session)),
        ] {
            let (status, _) = send(fixture.app.clone(), request.uri("/jellyfin/System/Info")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn a_disabled_jellyfin_extension_refuses_its_tokens() {
        let (status, _) = send(
            jellyfin_fixture(true, false).await.app,
            Request::builder()
                .uri("/jellyfin/System/Info")
                .header("Authorization", jellyfin_header(DEVICE_TOKEN)),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_jellyfin_token_opens_nothing_on_the_plex_surface() {
        let app = jellyfin_fixture(true, true).await.app;
        for request in [
            Request::builder().header("X-Plex-Token", DEVICE_TOKEN),
            Request::builder().header("Authorization", jellyfin_header(DEVICE_TOKEN)),
        ] {
            let (status, _) = send(app.clone(), request.uri("/library/sections")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }
}
