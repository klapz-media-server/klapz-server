# open-plex

A media server reimplementing the Plex Media Server API, in Rust. The goal is **API
compatibility**: clients and tools that talk to a PMS must be able to talk to open-plex
unmodified.

Everything in this repository is written in English — code, comments, documentation. Discussion
with the repo owner happens in French.

## Reference material

| Path | Contents |
| --- | --- |
| `docs/openapi.json` | **Official** Plex Media Server 1.2.3 spec — 207 paths, 260 operations, 64 schemas. Source of truth for request shapes. |
| `docs/ENDPOINTS.md` | Implementation tracker for all 260 operations, by domain and status. **Generated**: `python tools/gen_endpoints.py`. |
| `docs/plex-db-schema.md` | The real Plex library schema, as a reference only. **Generated**: `python tools/dump_plex_schema.py`. |
| `docs/wire-format.md` | Authoritative per-field JSON type map and element tables, from live captures. **Generated**: `PLEX_TOKEN=... python tools/capture_plex.py`. |
| `docs/spec-gaps.md` | Where the spec and the reference server disagree: omissions, wrong paths, aliases, and every undocumented behavior we reproduce. Hand-verified. |
| `tools/compare_items.py` | Field-by-field diff of our listings and lookups against the reference, aligned by `ratingKey` so ordering and field fidelity stay separate questions. |
| `docs/endpoint-probe.md` | Which spec `GET` operations the reference server answers. **Generated**: `PLEX_TOKEN=... python tools/probe_endpoints.py`. |
| `files/` | A complete Plex Media Server install for Windows (233 MB) — binaries, bundled web app, plug-in framework, transcoder profiles. For reverse engineering. |
| `db/` | A real populated Plex library: `com.plexapp.plugins.library.db` (2.4 GB) and its blobs (5.8 GB). Contains the owner's personal data. |
| `.claude/skills/rust/` | The project's Rust conventions. Load before writing Rust. |

Three finds inside `files/` worth knowing about:

- `files/Resources/com.plexapp.plugins.library.db` (318 KB) is the **pristine template database**
  Plex deploys on a fresh install. Same schema, no data — the right thing to read for schema
  questions instead of opening the 2.4 GB one.
- `files/Resources/Profiles/` holds the per-client transcoding profiles as XML: the definitive
  source for how Plex makes transcode decisions.
- The bundled Plex Web App is a client that exercises the API, so it reveals real attribute names
  and call patterns.

The populated database under `db/` is treated read-only, and nothing derived from it is committed
without scrubbing tokens, machine identifiers and personal paths.

## Settled decisions

Each of these comes from an explicit instruction by the repo owner. They are not renegotiated
without them saying so.

### Scope

- **Server only** for now. No client, no `plex.tv` shim.
- **The file scanner is out of scope.** On Plex it is a separate binary (`Plex Media Scanner`).
  Endpoints that only drive it — scanning, matching, media analysis — are marked `deferred` in
  `docs/ENDPOINTS.md` (37 of 260 operations).
- **No feature behind a paywall.** No Plex Pass, no Plex Ninja: everything Plex reserves for a
  subscription (hardware transcoding, sync, skip intro, Live TV/DVR, loudness analysis…) ships in
  the base version. In code this means no `if subscription` branch, no `premium` flag, no
  entitlement check. When a client reads a subscription field to decide whether to show a
  feature, return the value that unlocks it.

### Architecture

- **GraphQL at `/graphql` is the official API.** The Plex REST surface is a compatibility
  adapter, and it has **no data source other than the GraphQL schema** — every Plex handler
  executes a query against it in-process. That is the point: the official API can never become
  a second-class citizen, because nothing can be exposed through Plex that GraphQL does not
  already serve.
- **Layers: `db` → `core` → `api` (GraphQL) → `api-plex` (adapter).** Dependency direction makes
  it compiler-enforced: `core` declares the storage ports and knows nothing of HTTP or GraphQL,
  `db` implements them, `api` depends only on `core`, and `api-plex` depends on `api` — never on
  `db` or the ports.
- **No service layer above the ports.** There was one; it forwarded calls and nothing else, so
  it was deleted. Reintroduce it when real domain logic exists — permission filtering is the
  obvious first candidate — not before.
- **A listing covers a library's top level by default.** `parent_id IS NULL` — the films of a
  film library, the series of a series library — because a series library keeps its seasons and
  episodes in the same section and listing all three flat is never what a client wants.
  `ItemFilter.depth` widens it, and naming a `kind` overrides it (asking for episodes and
  getting nothing would be obtuse).
- **A filter we cannot honor is refused, never dropped.** The adapter knows the filter list a
  real server advertises (`Meta/Type/Filter`); one of those it cannot translate answers `501`,
  because honoring the window and discarding the filter would return the whole library and let
  the client present it as the result. Parameters *not* on that list stay tolerated, as the
  reference tolerates them. The rule generalizes: silence is for what we were never asked, not
  for what we cannot do.
- **A derived index has one write path.** `item_title_word` is maintained by
  `db::write::insert_items`, and there is no other way to insert an item. Leaving it to each
  caller is how a search quietly stops finding things — found the easy way, by a test fixture
  that inserted items directly and then matched nothing.
- **A path the server publishes is not a path the server accepts.** `MediaFile.path` is
  exposed; every endpoint that touches the filesystem still resolves a file from its id. There
  is no route that takes a path, and there must not be — see `security.md`.
- **The official API publishes measurements; the adapter publishes display forms.** Bitrates in
  bits per second, frame rates as measured (23.976), aspect ratios unsnapped, ratings at full
  precision. Plex's kilobits, `24p`, `2.35` and one-decimal ratings are derived in `api-plex`.
  Rule of thumb: if a value is rounded, labelled, or snapped to a ladder, it is presentation.
- **Every collection port takes a `Window` and returns a `Page`.** Storage is never asked for
  everything: the window becomes `WHERE`/`ORDER BY`/`LIMIT`, and pages over-fetch by one row to
  learn whether a further page exists without a second counting query. Page size is capped at
  `page::MAX_ITEMS`, because a client-controlled count with no ceiling is a denial of service.
  The cap is why `/library/sections/{id}/all` **always** windows, where a real server returns a
  whole library unpaged — a deliberate deviation, and the response says where the window sits
  (`totalSize`, `offset`, headers) so `size` never claims more than the body holds.
- **`Window.skip` exists for Plex, and only for Plex.** Clients page by absolute offset, which
  no cursor can express. Cursors stay the way to walk a collection; the offset is how a caller
  lands in the middle of one, and it carries the cost of traversing what it discards.
- **No domain logic in a binding.** A binding translates: parse the request, query, shape the
  response. Review test — if a handler holds a condition expressing a domain rule, it is in the
  wrong place. Presentation decisions (listing order, wire encodings) *are* the binding's job.
- **HTTP token authentication lives in `server`**, because both surfaces share it. It attaches a
  `Principal`; each surface then enforces scope in its own idiom (`Caller`/`AdminCaller`
  extractors on the Plex side, request data on the GraphQL side).

### GraphQL conventions

- **Relay-compliant.** Every identifiable type implements `Node` and carries an opaque,
  type-qualified global `id` (base64 of `Type:key`), refetchable through a single `node(id:)`
  field. The server-local number stays available as `databaseId` for the compatibility layer.
- **Lists are cursor connections** (`edges`/`node`/`cursor`, `pageInfo`), never bare arrays.
- **Every field, argument and enum value carries a description.** Enforced by a test that walks
  introspection, not by review.
- **DataLoaders for anything resolved per parent.** `Library.roots` batches: ten libraries cost
  one `WHERE section_id IN (…)` query, verified against the SQL log.
- Values exceeding 32 bits are exposed as `String`, since GraphQL's `Int` only guarantees 32
  bits — `contentChangedAt` routinely exceeds it.
- **Database through an ORM** so the engine can be swapped: **SeaORM** (one API across
  PostgreSQL / MySQL / SQLite, built-in migrations, natively async). SQLite is one backend among
  several, not the design.
- **Transcoding is process supervision.** Plex does not decode video itself, it executes
  `ffmpeg`. open-plex does the same: no libav bindings, no decoding inside the server's address
  space.

### Access rights

- **Accounts and permissions are our own design.** We do not reproduce Plex's sharing model —
  not `library_section_permissions`, not its account table, not its home/restricted flags. The
  importer copies no accounts, which also keeps the 283 real accounts (with names and e-mail
  addresses) in the reference database untouched.

### Deliberate divergences from Plex

Places where we knowingly answer differently, each decided rather than overlooked.

- **Search finds the right items, in the wrong order.** `GET /search` matches exactly as the
  reference does — a word prefix over title, sort title and original title — and then differs:
  the reference ranks the matches and cuts them off, we return them in title order. Identical
  sets for a distinctive term (`dune`: 21 both sides), materially more for a common one
  (`hell`: 43 against 439). Ranking needs a score, and ordering by a score conflicts with
  keyset cursors, which resume from a stored column.
- **`Meta` lists what we honor, not what Plex offers.** `includeMeta=1` publishes 11 filters and
  2 sorts for a film library where the reference lists 27 and 9: a menu entry answering `501`
  would be worse than an absent one. A test keeps the list in step with `listing::parse`.
- **"Related" films are an approximation.** We found nothing in the reference's database stating
  which films are alike, so `core::similar` ranks by shared genres, actors and directors: a close list,
  not the same one (101 of 180 shown items on 30 films). The director and actor shelves around
  it follow the measured rule and match on 59 of 60 films.
- **The search field finds no people yet.** `GET /library/search`, what Plex Web's search field
  calls, answers films, series and episodes; the people and collections the reference mixes in
  are absent, and `searchTypes=people` is tolerated rather than refused, since Plex Web sends it
  with every search.
- **Range filter operators are unreachable.** `year>=2020` travels with a raw `>`, which is not
  a legal URI character: `hyper` rejects the request line with `400` before any handler runs,
  and the reference rejects the percent-encoded form in turn. The equality filters work; the
  parser handles the operators and is tested, for whenever something can deliver them.
- **Listing order need not be byte-identical.** Plex collates titles in Unicode — numeric
  (`6 Days` < `007 Spectre` < `10 000`) and UCA punctuation order (`[` before `#`). We compare
  the folded `sort_key` column byte-wise. Same items, alphabetical and stable, different
  sequence: 299/500 positions match on films, 438/500 on series, median displacement 0. Close
  enough; a UCA dependency plus a re-import of 1.1M rows is not worth the remainder.
- **File paths are published**, on both surfaces, as Plex does: `MediaFile.path` with a derived
  `MediaFile.filename` in GraphQL, `Part.file` in the Plex layer. This is reference data — it
  makes the library auditable from outside. It changes nothing about input: playback addresses
  a file by `id`, and **no endpoint accepts a path from a client**. A path we hand out and a
  path handed back to us are different things.
- **A listing always windows.** Plex returns a whole library unpaged when the client sends no
  container parameters; `page::MAX_ITEMS` forbids that. The response states `totalSize` and
  `offset` and sends the paging headers, so `size` never claims more than the body holds.
- **`myPlexSubscription` reports true** while `myPlex` reports false: no paywall, and no
  plex.tv link to pretend about.
- **An image is fetched from Plex's hosts only.** The reference's `/photo/:/transcode` fetches
  any URL it is handed, its own loopback included; that is an SSRF hole for any holder of a
  token. We fetch over TLS from `plex.tv` and its subdomains, on every redirect hop, which is
  all an account avatar needs, and answer `403` for the rest.
- **The Plex surface announces a Plex version, not ours.** `/identity`, `GET /` and
  `/media/providers` report `1.43.5.11029-6bf8af3f0`, the reference server every capture comes
  from: Plex Web refuses a server older than 1.18.3, and `0.1.0` reads as one. Clients that
  enable features by version then expect exactly the behavior we copied. The official API keeps
  open-plex's own version (`api-plex::general::PLEX_VERSION`).

### Data model

- The real Plex database is a **reference, not a schema to clone**. We use it to understand how
  Plex models its entities and which columns feed which response attributes, then design our own
  schema. What `docs/plex-db-schema.md` shows we do not want: packed key/value columns
  (`extra_data`, `user_fields`), denormalized display columns (`tags_genre`), `datetime` storage
  where the API speaks epoch seconds, and a column named `index` — a reserved SQL word.

### Code quality

Full detail in `.claude/skills/rust/SKILL.md`. The non-negotiables:

- **All code and documentation in English** — identifiers, comments, doc comments, error
  messages, test names, logs.
- **Light comments**, explaining **why** or **how**, never **what**. Do not explain everything
  every time.
- The code must not read as generated: no single-caller abstractions, no empty doc comments, no
  `unwrap()` outside tests, precise names.
- **Security is a first-class concern** — see `.claude/skills/rust/references/security.md`. The
  three classes that matter: path traversal on file serving, `ffmpeg` argument injection, and
  token leakage (Plex carries tokens in query strings).

### Testing

- **Low-level unit tests** on functions, colocated in the file under test.
- **Endpoint coverage tests** verifying each route answers *correctly*, not just that it exists.
  A test walks `docs/openapi.json` and asserts no expected operation returns `404`.
- Fixtures are real captured responses, never invented data — the only thing that proves
  compatibility.
- **Field completeness against the reference database**: for each entity on the read path,
  enumerate the columns actually populated in the real library and check each either maps to an
  attribute we emit or is deliberately excluded with a reason. This is how we find compatibility
  gaps that no hand-written fixture would reveal.

### Closing procedure

No Rust-touching task is finished until these five steps have passed, in order:

1. `cargo fmt --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`, then fix **every** warning. An
   `#[allow]` is admissible only with its justification in a comment.
3. `cargo test --all-features`. If a test fails, **fix the code, not the test** — unless the test
   is wrong, and then say explicitly how.
4. Verify no `unwrap()`, `expect()` or `panic!` was added outside `#[cfg(test)]`.
5. Summarize the commands run and their actual results, failures included.

## Running it

```sh
# Seed our schema from a real Plex database (idempotent; reads the source read-only).
cargo run -p open-plex-server -- import db/com.plexapp.plugins.library.db

# Serve. Without OPENPLEX_TOKEN every authenticated endpoint answers 401 by design.
OPENPLEX_TOKEN=dev-token cargo run -p open-plex-server
```

| Variable | Default | Purpose |
| --- | --- | --- |
| `OPENPLEX_DATABASE_URL` | `sqlite://open-plex.db?mode=rwc` | Any SeaORM backend; migrations run on connect. |
| `OPENPLEX_TOKEN` | *(none)* | The accepted admin token. Unset means no authenticated route is reachable — the safe failure. |
| `OPENPLEX_ALLOWED_NETWORKS` | *(none)* | Comma-separated addresses or CIDR ranges admitted **without a token**, with admin rights — Plex's `allowedNetworks`, and the only way Plex Web gets in without plex.tv. Checked against the socket address, never `X-Forwarded-For`. A malformed entry stops the server. For local use: `127.0.0.1/32,::1/128`. |
| `OPENPLEX_MACHINE_ID` | placeholder | Must be stable across restarts or clients stop recognizing the server. |
| `OPENPLEX_BIND` | `127.0.0.1:32500` | Listen address. Deliberately not 32400, so it can run beside the reference server. |
| `OPENPLEX_WEB_DIR` | `web` | Plex Web, served without a token at `/web` as the reference serves it. Not versioned: it is Plex's code. Absent means `/web` is not served. |

`ffmpeg` and `ffprobe` must be on `PATH` for `/photo/:/transcode`; without them it answers `500`.

## Current milestone

**Milestone 1 — skeleton and conformance first.** Done:

- ✅ a Cargo workspace split along the layers, with GraphQL as the official API and Plex as an
  adapter over it;
- ✅ a Relay-compliant GraphQL schema at `/graphql` (`Node`, global ids, connections,
  DataLoaders, fully documented);
- ✅ the dual XML/JSON `MediaContainer` serializer (JSON canonical, XML derived from it);
- ✅ token authentication: middleware validates and attaches the principal, typed extractors
  (`Caller`, `AdminCaller`) enforce scope in handler signatures;
- ✅ `SeaORM` storage with migrations, plus an importer that seeds our schema from a real Plex
  database;
- ✅ `/identity` and `/library/sections` (+ its `/all` alias) verified byte-identical to the
  reference server, served through the GraphQL schema;
- ✅ routes declared for the remaining non-deferred operations, answering `501` (206
  operations across 167 paths, generated, with a test asserting no declared operation 404s);
- ✅ `GET /` capabilities, with every optional feature reported as available;
- ✅ `/library/sections/{id}/all` and `/library/metadata/{ids}` — library listings and item
  lookups, verified field-by-field against the reference server over real items
  (`tools/compare_items.py`): zero differing fields and zero extra fields on shared items;
- ✅ **library search** — the `title`/`year`/`decade`/`contentRating`/`studio`/`resolution`
  filters on `/all`, the `/library/sections/{id}/firstCharacter` jump bar, and `/search` across
  every library; backed by a title-word index (`item_title_word`, 736,105 rows) and a `search`
  root field on the official API;
- ✅ `/library/metadata/{id}/children` and `/grandchildren` — opening a series: seasons, a
  season's episodes, and every episode of a show as one list, through a `grandchildren`
  port and `Show.episodes` on the official API; zero differing fields against the reference
  on seven parents across two libraries;
- ✅ the `X-Plex-*` client-identity extractor (`ClientIdentity`): header or query, with the
  precedence, case and repetition rules measured on the reference. It never rejects —
  the spec marks `X-Plex-Client-Identifier` required, the reference does not enforce it.
  No handler takes it yet; transcoding and timelines will be its first consumers;
- ✅ `docs/ENDPOINTS.md` kept current as work lands, including the endpoints served outside
  the spec (`OFF_SPEC` in `tools/gen_endpoints.py`).

Milestone 1 is complete.

Then: iterate endpoint by endpoint, following `docs/ENDPOINTS.md`.

Since then:

- ✅ **tags, people and references** — genres, countries, directors, writers, cast and
  producers, imported with Plex's own tag ids; the people credited rebuilt into a table of
  their own, grouped by provider key; external identifiers and ratings in tables of their own.
  On the official API every item type exposes the same `tags` object, as cursor
  connections: a series lists its own credits then its episodes' (with `episodeCount`), a
  season and an episode inherit their series' genres and countries. On the Plex surface:
  full tags, `Guid` and `Rating` on `/library/metadata/{ids}`, the listing form on `/all`,
  `/children` and `/grandchildren`, and the six tag filters on `/all`, by id or by name.
  Measured rules and the one divergence (listings derived from the tags, not from the
  reference's stale cache) are in `docs/spec-gaps.md`.

Next, as the repo owner asked: the library shelves `/hubs/sections/{id}` still leaves out.

- ✅ a series library's **recently added**: new episodes grouped into their season or series
  by a measured 20-hour batching rule (`core::recent`, `Library.recentlyAdded` on the official
  API), identical in order to the reference on four libraries; seasons named in the client's
  language from Plex's own catalogs, and an episode without a rating carrying its series';
- ⬜ **recently released**, which needs ordering by release date — the nullable-column sort
  in *Open questions*;
- ⬜ the shelves that need watch state — continue watching with content, recently watched,
  top unwatched, top of a genre or of a person — once accounts and `view_state` exist.

## Reference server

A live Plex server runs at `http://127.0.0.1:32400`. It is the arbiter for every behavioral
question: capture first, then implement. Token comes from the environment (`PLEX_TOKEN`) and is
never committed; `captures/` is gitignored because it holds library content.

**The spec alone cannot drive this project.** Verified deviations so far, all in
`docs/spec-gaps.md`: `GET /library/sections` and `GET /library/metadata/{id}/children` and
`/grandchildren` are missing from the spec entirely, `firstCharacters` should be
`firstCharacter`, and `/identity` is public despite the declared global security. When the
automated probe reports a 404, suspect a wrong path parameter before concluding a route is
absent.

## Settled by capture

- **Content negotiation**: JSON if and only if `Accept` contains `application/json`; XML
  otherwise. `*/*` yields XML. Request `Content-Type` is ignored, and there is **no `?format=`
  override**. XML content type is exactly `text/xml;charset=utf-8`.
- **Errors**: minimal HTML (`text/html`), ignoring content negotiation entirely. No
  `MediaContainer`, no error code. Clients branch on the status alone.
- **Booleans**: `0`/`1` in XML, but JSON is per-field — `allowSync` is `true` while
  `hasPremiumPrimaryExtra` is `"1"` and `hidden` is `0`, all within the same response.
- **Scalar types are per-field**: `ratingKey` is a string, `Media.id` a number.
- **Attribute order is not alphabetical**, so `serde_json` needs `preserve_order`.
- **Pagination**: the server returns `X-Plex-Container-Start` and
  `X-Plex-Container-Total-Size` only — no `X-Plex-Container-Size` response header.
- **Pagination is activated by both parameters together**: the window applies only when
  `X-Plex-Container-Start` *and* `X-Plex-Container-Size` are present. Either alone is ignored.
- Every response carries `X-Plex-Protocol: 1.0` and `Cache-Control: no-cache`.
- **`X-Plex-*` values** come from the header or the query parameter of the same name. A header
  wins even when empty; header names are case-insensitive, query names are not; a repeated
  query parameter takes its last value; nothing is trimmed.
- **The XML element of a listing item is named after its `type`**, not after the JSON key:
  `<Video>` for a film or episode, `<Directory>` for a series or season, in the same response.
  The one place the two formats are not isomorphic.
- The remaining derived wire rules — item `duration` from the media, `titleSort` omitted when
  equal to `title`, the six-value aspect-ratio ladder, `videoResolution` from the width,
  `videoFrameRate` only for three standards and omitted otherwise — are in `docs/spec-gaps.md`,
  each with the measurement it came from.

## Open questions

- **Stream profiles are not imported.** `Media.audioProfile`, `videoProfile` and the
  `audioCodec` composition (`dca` + profile `ma` → `dca-ma`) all come from one value packed
  inside `media_streams.extra_data`. Unpacking packed columns into real ones at import is
  exactly the pattern we want; it is simply not done yet.
- Default database engine for development (SQLite for now).
- Whether any client depends on attribute ordering within an element.
- **Permission filtering is not implemented**: any valid token sees every library.
  `library_section_permissions` exists in the Plex schema and we ignore it. This is the domain
  logic that justifies a service layer above the ports.
- The access model (accounts, rights) is unspecified, and `view_state` cannot be written until
  it exists.
- **Sorting is still two columns.** `sort=` honors `titleSort` and `addedAt`; the reference
  advertises nine, and the other seven (`originallyAvailableAt`, `rating`, `audienceRating`,
  `duration`, `lastViewedAt`, `mediaHeight`, `random`) are all backed by nullable columns.
  Ordering by one needs the cursor to encode absence and the null position to be pinned. Note
  the inconsistency to resolve: an unsupported `sort=` falls back to title, while an
  unsupported filter answers `501`.
- The ports are one trait because there is one aggregate. Splitting them is additive, and
  should happen when metadata and playback arrive — not pre-emptively.
