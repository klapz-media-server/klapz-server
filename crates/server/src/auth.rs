//! HTTP token authentication, shared by every surface.
//!
//! Lives here rather than in a binding because the official GraphQL API and the Plex adapter
//! both need it, and neither should depend on the other. The middleware resolves the token
//! to a [`Principal`] and attaches it to the request; each surface then enforces scope in
//! its own idiom.
//!
//! A request with no valid token is still admitted when it comes from a network the operator
//! trusts, which is Plex's `allowedNetworks`: the one way a client with no plex.tv account,
//! Plex Web among them, can talk to the server at all. It is an authentication bypass by
//! design, so it is off unless configured, and it trusts the socket's address alone.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use ipnet::IpNet;
use open_plex_core::access::{connection_uris_for_host, token_digest, AccessRepository, Extension};
use open_plex_core::{Accounts, Principal};

use crate::remote_tokens::RemoteTokens;

/// The header and query parameter carrying the token.
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

/// Who may use the server: the accepted tokens, and the networks trusted without one.
#[derive(Debug, Clone, Default)]
pub struct Access {
    accounts: Accounts,
    trusted_networks: Vec<IpNet>,
    users: Option<Arc<dyn AccessRepository>>,
    remote: Option<Arc<RemoteTokens>>,
}

impl Access {
    pub const fn new(accounts: Accounts, trusted_networks: Vec<IpNet>) -> Self {
        Self {
            accounts,
            trusted_networks,
            users: None,
            remote: None,
        }
    }

    /// Adds the users whose tokens are stored, on top of the operator's.
    #[must_use]
    pub fn with_users(mut self, users: Arc<dyn AccessRepository>) -> Self {
        self.users = Some(users);
        self
    }

    /// Adds the tokens an account service issued, which is what a client that found this
    /// server through that service presents.
    #[must_use]
    pub fn with_remote_tokens(mut self, remote: RemoteTokens) -> Self {
        self.remote = Some(Arc::new(remote));
        self
    }

    /// Who a token belongs to. A storage failure refuses the token rather than the server.
    async fn authenticate(&self, token: &str) -> Option<Principal> {
        if let Some(operator) = self.accounts.authenticate(token) {
            return Some(operator.clone());
        }
        let users = self.users.as_ref()?;
        let grant = match users.grant(Extension::Plex, &token_digest(token)).await {
            Ok(None) => {
                let Some(account) = self.remote.as_ref()?.account_of(token).await else {
                    tracing::debug!("refused a token no account service attributes");
                    return None;
                };
                let grant = users.grant_for_account(Extension::Plex, &account).await;
                if matches!(grant, Ok(None)) {
                    tracing::debug!(%account, "refused the token of an account not paired here");
                }
                grant
            }
            found => found,
        };
        match grant {
            Ok(grant) => grant.map(open_plex_core::access::Grant::into_principal),
            Err(error) => {
                tracing::warn!(%error, "could not look a token up");
                None
            }
        }
    }

    /// Who a request naming no account is, from the address it was sent to.
    async fn visitor_at(&self, host: &str) -> Option<Principal> {
        let uris = connection_uris_for_host(host);
        match self
            .users
            .as_ref()?
            .connection_at(Extension::Plex, &uris)
            .await
        {
            Ok(connection) => connection.map(|found| Principal::visitor_of(found.external_id)),
            Err(error) => {
                tracing::warn!(%error, "could not look an address up");
                None
            }
        }
    }

    /// Whether a peer address falls in a trusted network.
    ///
    /// Reduced to its canonical form first: a dual-stack socket reports an IPv4 client as
    /// `::ffff:127.0.0.1`, which `127.0.0.1/32` would otherwise fail to match.
    fn trusts(&self, peer: IpAddr) -> bool {
        let peer = peer.to_canonical();
        self.trusted_networks
            .iter()
            .any(|network| network.contains(&peer))
    }
}

/// Reads a comma-separated list of networks, each an address or a CIDR range.
///
/// # Errors
///
/// Fails on the first entry that is neither, naming it: a typo in a security setting must
/// stop the server rather than leave it quietly trusting less, or more, than intended.
pub fn parse_networks(list: &str) -> Result<Vec<IpNet>, String> {
    list.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<IpNet>()
                .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| format!("`{entry}` is not an address or a network"))
        })
        .collect()
}

/// The transcoder process calling back. It holds no token: these routes answer only to the
/// unguessable id of a session it was started for.
fn is_transcoder_callback(path: &str) -> bool {
    path.starts_with("/video/:/transcode/session/")
}

/// A stream's playlists and segments, which players fetch without a token, as on the reference.
/// The session's name opens them, and an unknown one answers `404`.
fn is_stream_segment(path: &str) -> bool {
    path.starts_with("/video/:/transcode/universal/session/")
}

pub async fn require_token(
    State(access): State<Arc<Access>>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    // The web client is public on the reference too: it has to load before it can sign in.
    if crate::web::is_client_path(path) || is_transcoder_callback(path) || is_stream_segment(path) {
        return next.run(request).await;
    }
    let presented = token_from(&request);
    let by_token = match presented {
        Some(presented) => access.authenticate(&presented).await,
        None => None,
    };
    // Public, but a caller who says who it is sees its own server's identifier, and one that
    // does not, the identifier of the server published at the address it used.
    if PUBLIC_PATHS.contains(&request.uri().path()) {
        let principal = if by_token.is_some() {
            by_token
        } else {
            // Read first: a request's body cannot be held across an await.
            let host = request
                .headers()
                .get(header::HOST)
                .and_then(|host| host.to_str().ok())
                .map(str::to_owned);
            match host {
                Some(host) => access.visitor_at(&host).await,
                None => None,
            }
        };
        if let Some(principal) = principal {
            request.extensions_mut().insert(principal);
        }
        return next.run(request).await;
    }

    // A token that does not match still lets a trusted peer in: a client that once signed in
    // elsewhere keeps sending its stale one, and refusing it would refuse the peer we trust.
    let Some(principal) = by_token.or_else(|| trusted_peer(&access, &request)) else {
        return unauthorized();
    };

    // Logged without the URI: the token travels in the query string, so logging the full
    // target would write credentials to disk.
    tracing::debug!(principal = %principal.name, path = request.uri().path(), "authenticated");

    request.extensions_mut().insert(principal);
    next.run(request).await
}

/// The principal of a request admitted for where it comes from.
///
/// The address is the socket's, never `X-Forwarded-For` or `X-Real-IP`: those are written by
/// the client, and `X-Forwarded-For: 127.0.0.1` would otherwise grant anyone access.
fn trusted_peer(access: &Access, request: &Request) -> Option<Principal> {
    let ConnectInfo(peer) = request.extensions().get::<ConnectInfo<SocketAddr>>()?;
    access
        .trusts(peer.ip())
        .then(|| Principal::operator("trusted network"))
}

/// Reads the token from the header, falling back to the query string.
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
    use open_plex_core::{Credential, Scope};
    use tower::ServiceExt;

    use std::net::SocketAddr;

    use axum::extract::ConnectInfo;

    use super::{parse_networks, require_token, Access, Accounts, Arc, StatusCode};

    const TOKEN: &str = "owner-token";

    /// Echoes the authenticated principal's name, or `anonymous` on a public route.
    async fn whoami(principal: Option<Extension<open_plex_core::Principal>>) -> String {
        principal.map_or_else(
            || "anonymous".to_owned(),
            |Extension(principal)| principal.name,
        )
    }

    fn app() -> Router {
        app_trusting("")
    }

    fn app_trusting(networks: &str) -> Router {
        let access = Arc::new(Access::new(
            Accounts::new(vec![Credential::new(TOKEN, "owner", Scope::Admin)]),
            parse_networks(networks).unwrap(),
        ));
        Router::new()
            // Plex's paths have segments starting with a colon.
            .without_v07_checks()
            .route("/library/sections", get(whoami))
            .route("/identity", get(whoami))
            .route("/", get(whoami))
            .route(
                "/video/:/transcode/universal/session/s1/0/header",
                get(whoami),
            )
            .route("/video/:/transcode/universal/start.mpd", get(whoami))
            .layer(axum::middleware::from_fn_with_state(access, require_token))
    }

    /// A request as it arrives from `peer`, with no token, through a router trusting
    /// `networks`.
    async fn call_from(networks: &str, peer: &str, forwarded_for: Option<&str>) -> StatusCode {
        let mut request = Request::builder().uri("/library/sections");
        if let Some(forwarded) = forwarded_for {
            request = request.header("X-Forwarded-For", forwarded);
        }
        let mut request = request.body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        app_trusting(networks)
            .oneshot(request)
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn admits_a_peer_from_a_trusted_network_without_a_token() {
        assert_eq!(
            call_from("127.0.0.1/32", "127.0.0.1:51000", None).await,
            StatusCode::OK
        );
        assert_eq!(
            call_from("::1/128", "[::1]:51000", None).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn trusts_nobody_by_default() {
        assert_eq!(
            call_from("", "127.0.0.1:51000", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn refuses_a_peer_outside_the_trusted_networks() {
        assert_eq!(
            call_from("127.0.0.1/32", "192.168.1.20:51000", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn never_trusts_a_forwarded_address() {
        // The header is the client's word for where it comes from; the socket is the fact.
        assert_eq!(
            call_from("127.0.0.1/32", "192.168.1.20:51000", Some("127.0.0.1")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn matches_an_ipv4_peer_seen_through_a_dual_stack_socket() {
        assert_eq!(
            call_from("127.0.0.1/32", "[::ffff:127.0.0.1]:51000", None).await,
            StatusCode::OK
        );
    }

    #[test]
    fn reads_addresses_and_ranges_and_refuses_anything_else() {
        assert_eq!(parse_networks("127.0.0.1/32, ::1").unwrap().len(), 2);
        assert!(parse_networks("").unwrap().is_empty());
        assert!(parse_networks("127.0.0.1/33").is_err());
        assert!(parse_networks("localhost").is_err());
    }

    async fn call(uri: &str, header: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::builder().uri(uri);
        if let Some(token) = header {
            request = request.header("X-Plex-Token", token);
        }
        let response = app()
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

    #[tokio::test]
    async fn accepts_the_token_in_a_header() {
        let (status, body) = call("/library/sections", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "owner");
    }

    #[tokio::test]
    async fn accepts_the_token_in_the_query_string() {
        // Artwork and stream URLs a server hands out carry it this way, and the bundled web
        // client passes every X-Plex-* value as a query parameter.
        let (status, body) = call(&format!("/library/sections?X-Plex-Token={TOKEN}"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "owner");
    }

    #[tokio::test]
    async fn prefers_the_header_over_the_query_string() {
        // The header is the form that does not leak into logs or `Referer`.
        let (status, body) = call("/library/sections?X-Plex-Token=wrong", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "owner");
    }

    #[tokio::test]
    async fn refuses_a_missing_or_unknown_token() {
        assert_eq!(
            call("/library/sections", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call("/library/sections", Some("nope")).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn refuses_a_truncated_token() {
        // The case a non-constant-time comparison would expose one byte at a time.
        let truncated = &TOKEN[..TOKEN.len() - 1];
        assert_eq!(
            call("/library/sections", Some(truncated)).await.0,
            StatusCode::UNAUTHORIZED
        );
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

    /// A router knowing one user, paired with the account behind the stub directory's token.
    async fn app_with_paired_user(enabled: bool) -> Router {
        use open_plex_core::access::{AccessRepository, Extension as Protocol, RemoteAccount};
        use open_plex_core::testing::{InMemoryAccess, StubDirectory};

        let users = Arc::new(InMemoryAccess::default());
        let user = users.create_user("a@example.org", 0).await.unwrap().id;
        users
            .connect_extension(user, Protocol::Plex, "server", "digest", 0)
            .await
            .unwrap();
        let account = RemoteAccount {
            id: "linked".to_owned(),
            name: "alice".to_owned(),
            token: StubDirectory::ACCOUNT_TOKEN.to_owned(),
        };
        users
            .link_account(user, Protocol::Plex, &account, "https://a.example.org", 0)
            .await
            .unwrap();
        if !enabled {
            users
                .disable_extension(user, Protocol::Plex, 0)
                .await
                .unwrap();
        }
        let access = Arc::new(
            Access::new(Accounts::default(), Vec::new())
                .with_users(users)
                .with_remote_tokens(crate::remote_tokens::RemoteTokens::new(Arc::new(
                    StubDirectory::default(),
                ))),
        );
        Router::new()
            .route("/library/sections", get(whoami))
            .route("/identity", get(whoami))
            .layer(axum::middleware::from_fn_with_state(access, require_token))
    }

    async fn identity_asked_at(app: Router, host: &str) -> String {
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/identity")
                    .header("Host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        String::from_utf8_lossy(&body).into_owned()
    }

    #[tokio::test]
    async fn the_address_used_says_whose_identifier_a_stranger_sees() {
        assert_eq!(
            identity_asked_at(app_with_paired_user(true).await, "A.example.org").await,
            "visitor"
        );
        assert_eq!(
            identity_asked_at(app_with_paired_user(true).await, "b.example.org").await,
            "anonymous"
        );
        assert_eq!(
            identity_asked_at(app_with_paired_user(false).await, "a.example.org").await,
            "anonymous"
        );
    }

    async fn status_with(app: Router, token: &str) -> (StatusCode, String) {
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/library/sections")
                    .header("X-Plex-Token", token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn a_token_of_a_paired_account_acts_as_its_user() {
        let (status, name) = status_with(
            app_with_paired_user(true).await,
            open_plex_core::testing::StubDirectory::ACCOUNT_TOKEN,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(name, "a@example.org");
    }

    #[tokio::test]
    async fn a_token_the_service_does_not_know_is_refused() {
        let (status, _) = status_with(app_with_paired_user(true).await, "someone-else").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_switched_off_connection_refuses_its_accounts_token() {
        let (status, _) = status_with(
            app_with_paired_user(false).await,
            open_plex_core::testing::StubDirectory::ACCOUNT_TOKEN,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_streams_segments_need_no_token_but_starting_one_does() {
        let segment = "/video/:/transcode/universal/session/s1/0/header";
        assert_eq!(call(segment, None).await.0, StatusCode::OK);
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
        let (status, body) = call("/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "anonymous");
    }
}
