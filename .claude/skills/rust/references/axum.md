# Axum

## Contents

- [Router organization](#router-organization)
- [Application state](#application-state)
- [The skeleton route pattern](#the-skeleton-route-pattern)
- [Extractors](#extractors)
- [X-Plex-* headers](#x-plex--headers)
- [Authentication](#authentication)
- [Handler shape](#handler-shape)
- [Tower layers](#tower-layers)

## Router organization

One router per domain, assembled with `merge`. Domains follow the spec's own tags — the same ones
that structure `docs/ENDPOINTS.md`, which keeps an obvious correspondence between the tracker and
the code.

```rust
// crates/api-plex/src/lib.rs
pub fn router(state: PlexState) -> Router {
    Router::new()
        .merge(general::router())
        .merge(library::router())
        .merge(hubs::router())
        .merge(playqueue::router())
        .merge(timeline::router())
        .merge(transcode::router())
        .layer(middleware_stack())
        .with_state(state)
}
```

`merge` rather than `nest`: Plex paths are not usefully hierarchical (`/library/...` sits next to
`/:/timeline` and `/photo/:/transcode`), and `nest` would force artificial prefix splits. Each
sub-router declares its full paths.

Watch the Plex paths containing `:/` — `/:/timeline`, `/photo/:/transcode`,
`/{transcodeType}/:/transcode/universal/start`. The `:` there is a literal character, not a
parameter marker. In Axum 0.8 syntax parameters are written `{name}`, so the literal `:` creates
no conflict. That is one reason not to stay on the older `:param` syntax.

Axum 0.8 still refuses a segment that *starts* with a colon (`/:/prefs`) unless the router calls
`without_v07_checks()`, and `merge` does not carry that setting: routes merged in are
re-validated with the default rules. So the colon routes are added through `add_routes(router)`
functions onto the router that already called it (`api-plex/src/lib.rs`), never merged in.

Traps measured against the reference, each of which cost a debugging round:

- `Redirect::temporary` answers `307`; Plex answers `302`. Write
  `(StatusCode::FOUND, [(header::LOCATION, …)])` by hand.
- A malformed path parameter is rejected by the extractor with `400`; the reference answers
  `404`. Where the status matters, take the segment as `String` and parse it yourself.
- A `{a}.{b}` segment (two parameters in one segment) is unroutable: take the whole segment and
  split it.
- `nest_service("/web", …)` conflicts with a route on the same prefix: nest a small router that
  holds both instead.
- rustfmt wraps method chains past `chain_width` (60): generated routes are a `const` table
  walked by a loop, not a thousand-line `.route()` chain.

## Application state

`core` is exposed to the binding through a state that is cheap to clone, since it is cloned per
request. Everything it holds is therefore behind an `Arc` or already cheap (a connection pool
is).

```rust
#[derive(Clone)]
pub struct PlexState {
    library: Arc<LibraryService>,
    playback: Arc<PlaybackService>,
    identity: ServerIdentity,
}
```

When a handler only needs one piece, implement `FromRef` to extract it directly: the handler's
signature then announces what it touches, which is useful review information.

```rust
impl FromRef<PlexState> for Arc<LibraryService> {
    fn from_ref(state: &PlexState) -> Self {
        Arc::clone(&state.library)
    }
}

async fn sections(State(library): State<Arc<LibraryService>>) -> ...
```

Note what the state holds: only `core` services. No database connection, no SeaORM entity — the
binding does not depend on `db`.

## The skeleton route pattern

223 operations to declare as `501`. The trap is writing 223 functions. One is enough, referenced
as many times as needed: there is no duplication to generate.

```rust
/// Placeholder for routes that exist in the spec but aren't implemented yet.
///
/// Returning 501 rather than 404 is what makes the spec-coverage test meaningful:
/// a missing route fails the test, a known-unimplemented one doesn't.
async fn not_implemented() -> ApiError {
    ApiError::NotImplemented
}

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/tags", get(not_implemented))
        .route("/library/matches", get(not_implemented))
        .route("/library/sections/{section_id}/sorts", get(not_implemented))
}
```

As an endpoint gets implemented, `not_implemented` is replaced by the real handler and the
matching row in `docs/ENDPOINTS.md` moves from `stub` to `partial` or `done`. The diff stays
readable: one line per endpoint.

Do not write a macro to generate these routes. It would save a few characters and make the paths
untraceable by text search — which costs far more over time than the verbosity it avoids.

## Extractors

Extractors turn the request into domain types. They are the boundary where validation happens,
and that is the right place: a handler must never receive an unvalidated value.

For path parameters, use newtypes rather than primitives. A `Deserialize` newtype extracts
directly, and the typing then propagates through the whole handler:

```rust
async fn section(Path(section_id): Path<SectionId>, ...) -> ...
```

For the query string, a dedicated struct per endpoint, with Plex's defaults made explicit.
`serde` applies no default without `#[serde(default)]`, and an absent parameter is not an error
on Plex's side:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionQuery {
    #[serde(default)]
    pub include_details: PlexBool,
    pub sort: Option<SortSpec>,
    #[serde(rename = "type")]
    pub item_type: Option<MetadataType>,
}
```

Write a custom extractor with `FromRequestParts` when it does not consume the body — which is the
case for anything reading headers. Keep them in an `extract/` module of the binding, and have
them return `ApiError` so a rejection produces the same error shape as everything else.

## X-Plex-* headers

Plex clients identify themselves through a family of `X-Plex-*` headers: product, version,
platform, platform version, device, device name, and above all `X-Plex-Client-Identifier`. They
name playback sessions, inform transcode capability decisions, and show up in
`/status/sessions`.

Reading them one by one in every handler would be noise repeated everywhere. A single extractor
gathers them into one type, and handlers ask for it when they need it:

```rust
/// Client identity carried by the `X-Plex-*` header family.
///
/// Only `X-Plex-Client-Identifier` is required: sessions are keyed on it, and a client
/// that omits it cannot be tracked across requests. The rest is advisory metadata that
/// real clients populate inconsistently, so none of it is trusted for decisions.
#[derive(Debug, Clone)]
pub struct ClientIdentity {
    pub client_identifier: ClientId,
    pub product: Option<String>,
    pub version: Option<String>,
    pub platform: Option<String>,
    pub device_name: Option<String>,
}
```

Two compatibility traps. These headers are also accepted **as query parameters** by Plex
(`?X-Plex-Client-Identifier=...`), because some playback contexts cannot set headers; the
extractor therefore checks both, header first. And HTTP header names are case-insensitive: read
them through `HeaderMap`, which handles that, rather than comparing strings.

Pagination arrives as `X-Plex-Container-Start` and `X-Plex-Container-Size`, mirrored by query
parameters of the same name. Same treatment, and **clamp the size** (see `security.md`).

## Authentication

The token arrives in `X-Plex-Token` or as a `X-Plex-Token` query parameter. Both are required:
the artwork and stream URLs the server hands out carry the token in the query string, and clients
replay them verbatim.

Implement authentication as an extractor, not as a global middleware. The reason is in the spec:
scopes vary per operation (`shared user` / `admin`) and a few endpoints are public (`/identity`).
A uniform middleware would force either per-path exemptions — a list to maintain, therefore to
forget — or authorization redone inside every handler.

```rust
// The type in the signature *is* the authorization: a handler that needs admin rights
// takes AdminToken, and no body of code can forget to check it.
async fn delete_section(_: AdminToken, Path(id): Path<SectionId>, ...) -> ...
async fn sections(token: UserToken, ...) -> ...
async fn identity(...) -> ...  // public, no token extractor at all
```

Two distinct types rather than one type with a role field: the latter forces an `if` inside the
handler, and a forgotten `if` is a silent privilege escalation. The former hands the check to the
compiler.

## Handler shape

A handler does three things: it receives already-validated values, it calls `core`, it shapes the
output. No domain logic (see `SKILL.md`, "Layered architecture").

```rust
async fn section_all(
    _: UserToken,
    Path(section_id): Path<SectionId>,
    Query(query): Query<SectionQuery>,
    page: Pagination,
    State(library): State<Arc<LibraryService>>,
    format: ResponseFormat,
) -> Result<PlexResponse<MediaContainer>, ApiError> {
    let listing = library.list_section(section_id, query.into(), page.into()).await?;
    Ok(PlexResponse::new(MediaContainer::from_listing(&listing), format))
}
```

What this signature tells a reader: the rights required, the inputs, the dependencies, the output
format. That is the goal — the signature documents the handler better than a comment would.

`query.into()` and `listing` do the translation between Plex vocabulary and `core` vocabulary.
That is precisely the binding's job, and here it stays visible instead of being diluted.

## Tower layers

Order matters: a layer added with `.layer()` wraps everything declared before it. Layers that
must see all requests, errors included, therefore go last.

```rust
fn middleware_stack() -> impl Layer<Route> + Clone {
    ServiceBuilder::new()
        .layer(TraceLayer::new_for_http().make_span_with(redacted_span))
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(SetResponseHeaderLayer::overriding(
            header::SERVER,
            HeaderValue::from_static("open-plex"),
        ))
}
```

`redacted_span` is not decorative: the token travels in the query string, so `TraceLayer`'s
default configuration would log credentials. See `security.md`, "Logging".

Do not add a permissive `CorsLayer` by reflex. The server serves token-authenticated media; an
`Access-Control-Allow-Origin: *` combined with a token in the query string lets any web page
exfiltrate content. If CORS is needed, restrict it to origins declared in configuration.
