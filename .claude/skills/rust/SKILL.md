---
name: rust
description: Rust conventions for klapz, an Axum server reimplementing the Plex Media Server API. Load this skill before writing or modifying any .rs file in the repo, before adding a dependency to a Cargo.toml, and before designing a module, a domain type or a handler. It covers the DB/Core/binding layering, strong typing through newtypes, error handling, the async/blocking boundary, dual XML/JSON MediaContainer serialization, security, testing and lints — and above all the anti-slop rules that keep the code from reading as machine-generated. Use it whenever the task mentions Rust, Axum, tokio, serde, SeaORM, clippy, Cargo, ffmpeg, or code quality and maintainability, even when no file is named.
---

# Rust in klapz

klapz reimplements the Plex Media Server API: ~260 HTTP operations described by the official
spec in `docs/openapi.json`. The dominant risk in this project is surface area — 260 handlers
written without discipline become an unreadable mass where every bug costs three times over.

This skill exists for a specific reason, stated by the repo owner: the code must not read as
generated. Generated code is recognizable by concrete signs, not by atmosphere — comments that
restate the next line, layers of indirection with a single caller, `String` everywhere the domain
has distinct identities, scattered `unwrap()`, and handlers copy-pasted 260 times instead of one
pattern factored once. The sections below are ordered by their real impact on readability.

## The reviewer test

Before calling a file done, reread it asking one question: **can a Rust developer who doesn't
know Plex say, for every line, why it exists?** A line that fails that question goes. This is
more useful than any style rule, because it catches what lints cannot see.

## Anti-slop: the rules that matter

**Everything is in English** — identifiers, comments, doc comments, error messages, test names,
log messages, and project documentation. The API being reimplemented is English, and a bilingual
repo is unreadable for any contributor: `SectionId`, never `IdSection`.

**Comments are rare, and explain why or how — never what.** The code already says what it does.
Don't explain everything every time: a file where every block carries a comment is as unreadable
as one with none. Comment what would surprise a competent reviewer — a Plex quirk, a
counter-intuitive choice, an external constraint.

```rust
// NO — restates the code, pure noise
// Increment the size counter
size += 1;

// YES — states a constraint you cannot infer from the code
// Plex counts `size` over the children in this page but `totalSize` over the whole
// filtered set. Clients paginate on totalSize and loop forever if both match.
container.size = page.len();
container.total_size = Some(filtered_total);
```

**No abstraction with a single caller.** A trait with one impl, a wrapper that only delegates, a
catch-all `utils` module: each adds a mental hop and returns nothing. Introduce indirection at
the second real need, not in anticipation of the first.

**No empty doc comments.** `/// Creates a new Foo` on `Foo::new` says nothing. Document what
surprises: invariants, units, error conditions, Plex quirks. Otherwise write none.

**Precise names over descriptive ones.** `handle_request`, `process_data`, `DataManager`,
`Helper` carry no information. Name after the domain: `resolve_transcode_decision`, `SectionId`,
`MediaContainer`, `negotiate_format`.

**No `unwrap()` or `expect()` outside tests.** Legitimate exceptions are rare and must carry
their justification: a constant regex compiled once, an invariant proven just above. In those
cases use `expect("...")` with a message explaining *why it cannot happen*, not *what failed*.

**No reflexive `clone()`.** A `clone()` is a decision. If it's there to silence the borrow
checker, the real problem is the data layout or a lifetime — fix that. If it's genuinely needed,
it reads as such in context.

**Delete instead of commenting out.** No dead code in comments, no `#[allow(dead_code)]` on code
kept "for later". Git remembers everything.

**One owner per lifecycle.** When two pieces of code start and stop the same thing, they drift
apart: `main::Sessions` duplicated `streams::StreamRoutes`, and `/stop` drove the transcoder
directly past the official API; merging them into one owner removed several bugs at once.

## Traps this codebase has already met

- **async-graphql's `#[Object]` does not expand a `macro_rules!` call inside its impl block**:
  it sees an opaque token, and the type silently loses those fields. Only the generated SDL
  shows it — regenerate and read it after touching a resolver impl.
- **GraphQL descriptions are not inherited from an interface**: every concrete type documents
  its own fields, which the introspection test enforces.
- **Global ids do not sort.** They are base64 strings; ordering by them compares text, not
  numbers. Decode with `GlobalId` before sorting.
- **Inserting a method above a doc comment or an attribute** (`#[must_use]`) silently re-attaches
  it to the new method. Re-read the file after `cargo fmt` before patching it: the anchors move.
- **A GraphQL `Int` is 32 bits.** A field backed by an `i64` that can exceed it (a size in bytes,
  a counter, microseconds) is the `Int64` scalar (`api::int64`), never a bare `String`; the
  adapters read it with `klapz_api::int64::deserialize` or `deserialize_optional`.
- **`std::fs::canonicalize` returns verbatim paths on Windows**: `\\?\C:\…`, and a mapped
  network drive becomes `\\?\UNC\server\share\…`. Store and publish the plain form; the
  standard library adds the prefix back itself when a path is long.

## Typing: make illegal states unrepresentable

This is the project's strongest lever. The Plex API juggles a dozen different numeric
identifiers (`ratingKey`, `sectionId`, `mediaId`, `partId`, `streamId`, `playQueueItemId`…) that
are all integers. Passing them as `i64` guarantees they will be swapped one day, and the
compiler will say nothing.

```rust
// NO — four interchangeable integers, a silent bug waiting to happen
fn stream_part(section: i64, rating_key: i64, media: i64, part: i64) -> ...

// YES — swapping them does not compile
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RatingKey(i64);

fn stream_part(section: SectionId, item: RatingKey, media: MediaId, part: PartId) -> ...
```

Newtypes stay cheap: derive what is useful, implement `Deref` only when the type really is a
transparent alias, and expose `fn get(self) -> i64` rather than making the field public.

Same reasoning for domain enums. Plex carries `type=movie|show|season|episode|…` as a string; on
the server it is an `enum` with `#[serde(rename_all = "lowercase")]`. A string admits `"moive"`,
an enum does not.

Prefer borrowing in signatures: `&str` over `String`, `&[T]` over `&Vec<T>`. For a return value
that is sometimes borrowed and sometimes owned, `Cow<'_, str>` avoids an allocation on the hot
path without forcing the caller's hand.

## Errors

Two levels, for two different audiences.

**In the `db` and `core` layers**: `thiserror`, one enum per domain, variants that name the cause
and carry the useful context.

```rust
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("no library section with id {0}")]
    SectionNotFound(SectionId),
    #[error("media part {part} points at a file that is gone: {path}")]
    MissingFile { part: PartId, path: PathBuf },
    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}
```

No `Other(String)` variant: it defeats the point of typing the error. If a case fits no variant,
add the variant.

**At the HTTP boundary, in the binding**: a single `ApiError` type implementing `IntoResponse`,
producing the error shape Plex clients expect with the right status code. `core` errors convert
into it through `From`, which lets handlers use `?` without noise. The binding decides the HTTP
status — `core` has no business knowing that HTTP statuses exist. Details, including the exact
body shape, are in `references/errors.md`.

`anyhow` has no place in this repo's libraries. It is acceptable in a `main` or a dev tool, where
nobody matches on variants.

## Async and blocking

The server runs on Tokio. Database access goes through SeaORM, which is async end to end, so
there is **no** `spawn_blocking` to write for queries. What remains blocking by nature is the
**filesystem** and **child processes**.

Blocking a Tokio worker degrades the whole server under load, and it is a bug you will not see
in development — with a single client everything looks fine. The rule: every blocking call goes
through `tokio::task::spawn_blocking` or the `tokio::fs` equivalent, and that boundary stays
inside the layer that needs it. A handler containing `spawn_blocking` is a sign the layer below
is leaking.

When serving files, never read a media file into memory: Plex clients issue `Range` requests
against multi-gigabyte files. Use a streaming body (`tokio_util::io::ReaderStream`), or
`tower_http::services::ServeFile` where it fits — it already handles `Range`, `ETag` and
conditional requests, which would be absurd to rewrite.

No `block_on` in async code. No `std::sync::Mutex` held across an `.await` — use
`tokio::sync::Mutex` if the guard must cross a suspension point, otherwise keep the lock
synchronous and release it before.

## Dual XML/JSON serialization

This is the most structurally demanding constraint in the repo, and the easiest to get wrong.
The Plex API answers XML by default and JSON on `Accept: application/json` — **both from the same
data model**. In XML, scalars are *attributes* and relations are *child elements*; in JSON the
same relations become arrays under capitalized keys.

Two non-negotiable consequences:

`skip_serializing_if = "Option::is_none"` on every optional field. Plex **omits** absent
attributes; emitting `viewOffset=""` or `"viewOffset": null` diverges from real clients.

Booleans are `0`/`1` in XML, not `true`/`false` — confirmed on real responses. They need a
dedicated type rather than `bool`, because the JSON encoding is not consistently the same.

The full pattern — quick-xml's `@` attribute prefix, `MediaContainer` structure, content
negotiation, `X-Plex-Container-*` headers, and the irregular XML-element-to-JSON-key mapping —
is in `references/serialization.md`. Read it before touching a response type.

## Security

A media server is a more exposed target than it looks: it serves arbitrary files from disk,
builds `ffmpeg` command lines from query parameters, and often faces the internet. The three
failure classes that matter here, worst first:

**Path traversal.** File-serving endpoints receive client-supplied paths. A path never becomes a
disk access without being canonicalized *and* checked to descend from a declared library root.
Do not filter `..` by hand — that is bypassable through encoding, symlinks, `\` on Windows and
UNC paths; resolve, then compare.

**ffmpeg argument injection.** Transcode parameters come from the client. Always invoke `ffmpeg`
through an argument array (`Command::arg`), **never** through a shell, and validate each value
against an allowed set instead of interpolating it.

**Tokens.** Plex carries `X-Plex-Token` both as a header and as a **query parameter** — both must
be accepted for compatibility, which means tokens end up in URLs. Direct consequence: no full URL
ever reaches the logs, and token comparison is constant-time.

Plus the non-negotiable reflexes: `unsafe_code = "forbid"` at the workspace level, exclusively
parameterized SQL (never `format!` into SQL), and explicit caps on everything the client controls
by size (`containerSize`, request bodies, concurrent transcode sessions).

The details — full threat model, authentication and roles, the absence of unauthenticated access, CORS, resource
limits, dependency auditing, data at rest — are in `references/security.md`. Read it before
writing any endpoint that touches disk, `ffmpeg`, authentication or SQL.

## Transcoding is process supervision

Plex does not decode video itself — it **executes `ffmpeg`** (its `Plex Transcoder` is a patched
ffmpeg binary) and supervises the process. klapz does the same, and this is an architectural
decision rather than a detail: no libav bindings, no video decoding inside the server's address
space. Images are the exception: `/photo/:/transcode` uses the `image` crate in process.

What that implies in code: a transcode session is a child process with a lifecycle to honor. You
must handle termination when the client stops playing (otherwise orphaned processes pile up and
saturate the machine), concurrent reads of `stdout`/`stderr` (a full pipe blocks the child), HLS
segment temp directories and their cleanup, and a session identifier that survives client
reconnections.

Corollary of the security section: the command line is built from client parameters. Build it
argument by argument, validated against allowed sets, never by interpolating into a string
handed to a shell.

## No feature behind a paywall

Plex reserves part of its functionality for Plex Pass: hardware transcoding, offline sync, skip
intro, Live TV and DVR, loudness analysis, and more. **klapz has no paid tier** — everything
ships in the base version.

This has a concrete effect on the code, not just on the pitch. Endpoints and attributes that Plex
gates behind a subscription answer unconditionally here: no `if subscription` branch, no
`premium` flag, no entitlement check written "for later". When the spec describes a subscription
field that a client reads to decide whether to show a feature, return the value that unlocks it.

## Layered architecture

The project's structural constraint: **`db` → `core` → bindings**. The Plex API is only one
binding; others will come. If Plex vocabulary seeps into the core, no new binding can be added
without a rewrite.

```
crates/
  db/          SeaORM entities, migrations, data access. Knows nothing of Plex or HTTP.
  core/        domain logic: libraries, items, playback, sessions. Knows nothing of HTTP.
  transcoder/  converting media: images today, video next. Knows nothing of HTTP or Plex.
  api-plex/    binding: routes, MediaContainer, X-Plex-* headers. Knows nothing of db.
  server/      binary: configuration, composition, mounting the bindings.
```

Two rules make the separation real rather than decorative, and both are enforced by the compiler
through the dependencies declared in each `Cargo.toml`:

**A binding never depends on `db`.** If it needs data, `core` must expose it. A
`use klapz_db::…` inside `api-plex` is an architectural defect, not a shortcut.

**`core` knows nothing of HTTP, `MediaContainer`, or Plex vocabulary.** No `StatusCode` in its
signatures, no type named after the spec. `core` talks about `Movie`, `Episode`,
`PlaybackSession`; the binding is what knows a `Movie` serializes as `Video` with `type="movie"`.

**No domain logic in the binding.** A handler parses the request, calls `core`, shapes the
response — often three lines. The reviewer test: if a handler contains a condition expressing a
domain rule (who may read what, when a transcode is required, how progress is recorded), that
condition is in the wrong place. Legitimate conditions in a binding are about the protocol:
content negotiation, pagination, presence of a parameter.

Inside a crate, split by domain rather than by technical layer: `library/`, `transcode/`,
`playqueue/`, `timeline/` say something, whereas `models/`, `handlers/`, `services/`, `utils/`
force you to open three directories to follow one feature.

Declare modules as `foo.rs` rather than `foo/mod.rs`. A crate's public surface is a decision:
`pub` what is part of the contract, `pub(crate)` the rest. A crate where everything is `pub` has
no API — and here, the layer boundary collapses with it.

Axum patterns — extractors, `State`, `X-Plex-*` headers, tower layers, router organization — are
in `references/axum.md`. SeaORM conventions, migrations and backend swapping are in
`references/database.md`.

## Testing

Two levels, catching different things. Details and tooling in `references/testing.md`.

**Unit tests** live in the file they test, under `#[cfg(test)] mod tests`. That keeps logic and
verification within sight, and grants access to private items. It is the right level for pure
logic: path resolution, transcode decisions, identifier conversions, serializing one type in
isolation.

**Endpoint coverage tests** check that each route answers *correctly*, not merely that it exists.
Mount the router and drive real requests with `tower::ServiceExt::oneshot`, without opening a
socket. Two complementary forms:

- a test parameterized by `docs/openapi.json` asserting that no expected operation answers
  `404` — it fails the moment a route from the spec is missing, which is exactly what you want
  during the skeleton phase;
- per implemented endpoint, assertions on the status, the negotiated `Content-Type`, and the
  `MediaContainer` attributes — including the ones that must be **absent**.

Name a test after the expected behavior, not the function under test:
`omits_absent_attributes_in_xml` teaches you something when it breaks; `test_serialize` does not.

Fixtures are real captured responses, never invented data — they are the only thing that proves
compatibility. They live in `tests/fixtures/` and are compared as-is. Scrub tokens and personal
paths before committing them.

Inside tests, `unwrap()` is perfectly fine: a panicking test is a failing test, which is the
intent.

## Lints

Lints are centralized at the workspace level so every crate inherits them, in the root
`Cargo.toml`:

```toml
[workspace.lints.rust]
unsafe_code = "forbid"
missing_debug_implementations = "warn"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
unwrap_used = "warn"
expect_used = "warn"
todo = "warn"
```

and in each crate:

```toml
[lints]
workspace = true
```

`clippy::pedantic` is noisy by design. When a pedantic lint is wrong for our case, silence it
**locally with its reason** (`#[allow(clippy::x, reason = "...")]`), never globally for
convenience — a blanket `allow` is exactly how quality erodes without anyone deciding it.

## Closing procedure

No Rust-touching task is finished until these five steps have passed, in order:

1. `cargo fmt --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`, then fix **every** warning. An
   `#[allow]` is admissible only with its justification in a comment.
3. `cargo test --all-features`. If a test fails, **fix the code, not the test** — unless the test
   itself is wrong, and then say explicitly how.
4. Verify no `unwrap()`, `expect()` or `panic!` was added outside `#[cfg(test)]`:
   `rg -n '\.unwrap\(\)|\.expect\(|panic!\(' --type rust`, then discount occurrences under
   `#[cfg(test)]`.
5. Summarize the commands you ran and their actual results, failures included. A summary claiming
   "everything passes" without the command having run is worse than none.

Judge the test run by cargo's exit code and the `FAILED` lines, not by summing `test result`
lines. A GraphQL change also regenerates `docs/schema.graphql` and `docs/introspection.json`
(`cargo run -p klapz-api --example sdl` and `--example introspect`).

These commands passing is not proof of quality, it is the floor. The reviewer test at the top of
this page remains the real criterion.

## Dependencies

Every dependency is a liability: API surface to learn, compile time, maintenance risk. Before
adding one, check that the standard library or an existing dependency does not already cover the
need.

Versions are declared once in `[workspace.dependencies]` and picked up with `dep.workspace = true`
in each crate, so no version drift is possible.

Enable features explicitly (`default-features = false` where it makes sense): pulling all of
`tokio` with `features = ["full"]` into a crate that only needs `fs` slows everyone's build.

## References

- `references/security.md` — threat model, path traversal, `ffmpeg` execution, tokens and roles,
  no unauthenticated access, resource limits, dependency auditing.
  **Read before any endpoint touching disk, `ffmpeg`, auth or SQL.**
- `references/axum.md` — handlers, extractors, `State`, `X-Plex-*` headers, tower layers, router
  organization, the skeleton route pattern.
- `references/serialization.md` — `MediaContainer`, XML attributes vs elements, content
  negotiation, pagination headers, the absent-field trap.
- `references/database.md` — SeaORM entities, migrations, backend swapping, the `db`/`core`
  boundary, and why the Plex schema must not show through.
- `references/errors.md` — `ApiError`, status mapping, the error body shape Plex clients expect.
- `references/testing.md` — unit tests, spec-driven endpoint coverage, fixtures, `cargo-llvm-cov`.
