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
| `tools/gen_language_names.py` | Language and region names from CLDR 39, the data of the ICU 69 the reference ships. Generates `crates/api-plex/src/language_names.rs`. |
| `tools/capture_scan_samples.py` | ffprobe's output for sample files of the reference's library, with the rows the reference stored for them: the fixtures `crates/db/tests/scan_samples.rs` replays through the scan and compares column for column. |
| `tools/compare_items.py` | Field-by-field diff of our listings and lookups against the reference, aligned by `ratingKey` so ordering and field fidelity stay separate questions. |
| `docs/endpoint-probe.md` | Which spec `GET` operations the reference server answers. **Generated**: `PLEX_TOKEN=... python tools/probe_endpoints.py`. |
| `files/` | A complete Plex Media Server install for Windows (233 MB) — binaries, bundled web app, plug-in framework, transcoder profiles. For reverse engineering. |
| `db/` | A real populated Plex library: `com.plexapp.plugins.library.db` (2.4 GB) and its blobs (5.8 GB). Contains the owner's personal data. |
| `docs/jellyfin/openapi.json` | The reference Jellyfin's (12.4.0) own OpenAPI document, 364 operations. Refreshed by `python tools/gen_jellyfin_endpoints.py --fetch`. |
| `docs/jellyfin/ENDPOINTS.md` | Tracker of the Jellyfin operations served below `/jellyfin`. **Generated**: `python tools/gen_jellyfin_endpoints.py`, from the routes `crates/api-jellyfin` declares. |
| `docs/jellyfin/spec-gaps.md` | What the Jellyfin surface measured on the reference, and where it knowingly differs. Hand-verified. |
| `tools/capture_jellyfin.py` | Captures the reference Jellyfin (`JELLYFIN_USER`, `JELLYFIN_PASSWORD`), or our own surface with `--out`; `--all-items` every file's lookups and `PlaybackInfo` and every series' seasons and episodes, `--fixtures` the scrubbed fixtures of `crates/api-jellyfin/tests`. `tools/compare_jellyfin.py` sets the two side by side file by file, field by field, every difference either explained (its reason in `docs/jellyfin/spec-gaps.md`) or reported. |
| `tools/capture_jellyfin_samples.py` | Gives each scan sample our scanner's report and the reference Jellyfin's answer about its file: the fixtures `crates/server/tests/jellyfin_samples.rs` replays through storage, the official API and the binding. |
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

- **Server only** for now. No client, and no stand-in for `plex.tv`: pairing talks to the real
  one.
- **Scanning is being brought in, split as Plex splits it.** Walking libraries, deciding what
  changed, matching and storing belong to the server, as they run inside Plex Media Server;
  reading a file's content belongs to `open-plex-scanner`, a separate binary, as Plex leaves it
  to `Plex Media Scanner`, its Fingerprinter and its Transcoder. The endpoints of matching and
  of analysis on demand stay `deferred` in `docs/ENDPOINTS.md` until they exist. How Plex does
  each part is in `docs/scanner/` (`discovery.md`, `matching.md`, `analysis.md`).
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
- **Jellyfin is a second adapter, `api-jellyfin`, held to the same rule.** It depends on
  `api` (and `core`'s types) only, runs every query in process through
  `open_plex_api::in_process` as `api-plex` does, and holds no domain rule: whether a player
  reads a file as it is is `core::stream::reads_as_is`, on the official API as
  `MediaFile.readableAsIs`. It is mounted below `/jellyfin`, Jellyfin's own notion of a base
  URL, because Jellyfin routes without regard to case and its paths would meet Plex's; below it
  the server's authentication takes Jellyfin tokens and nothing else. Jellyfin ids are a wire
  encoding of global ids (`api-jellyfin::id`). What it measured and where it differs is in
  `docs/jellyfin/spec-gaps.md`.
- **Layers: `db` → `core` → `api` (GraphQL) → `api-plex` (adapter).** Dependency direction makes
  it compiler-enforced: `core` declares the storage ports and knows nothing of HTTP or GraphQL,
  `db` implements them, `api` depends only on `core`, and `api-plex` depends on `api` — never on
  `db` or the ports.
- **`transcoder` is to media what `api` is to data.** It speaks in images, sizes and formats and
  knows nothing of HTTP or Plex, and `api-plex` binds Plex's routes onto it: parameter names,
  statuses and Plex's quirks stay in the binding, which URLs may be fetched stays in the engine.
  Built this way so it can one day run on its own.
- **No service layer above the ports.** There was one; it forwarded calls and nothing else, so
  it was deleted. Permission filtering did not bring it back: the rules are in `core::access`
  and the GraphQL schema applies them.
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
- **A derived index has one write path.** `item_title_word`, `item_title_key` and
  `item_folded_title` (the voice search's) are maintained by `db::write::insert_items` and
  `update_item`, and there is no other way to insert an item; `tag_word` likewise by
  `db::write::insert_tags`. Leaving it to each
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
- **Streams are started on the official API, and read at addresses of their own.**
  `startStream` (Relay: an input with `clientMutationId`, a payload with `userErrors`) reaches
  the media engine through the `core::stream::StreamEngine` port; `server::streams` implements
  it over the transcoder and serves `/streams/{id}/…`, opened by a secret per stream rather than
  the account's token, because a player hands those URLs to components that never see the
  account. Direct play gets a file address the same way. The stream is identified by the
  playback session it records, which `stopStream` names.
- **A viewing counted is written once, with the state it explains.** The history has no write
  path of its own: `WatchRepository::record_viewings` writes the watch state and the entries in
  one transaction, and `core::watch` is its only caller.
- **A session and its stream end together, through `core::lifecycle`.** Whatever ends one — the
  player stopping, `stopStream` or Plex's `/stop`, a replacing start, an owner's
  termination, the expiry sweep, an idle stream, a stream read whose session is gone, the server
  stopping — stops the process, forgets the stream's addresses and secret, lets the directory
  go and deletes the row, under a per-key lock (`SessionLocks`, shared by the schema, the stream
  routes and the sweep). An automatic end checks its reason again under the lock. Every sign of
  life counts for both; a read of `/streams/{id}/…` counts at most every 30 seconds.
- **Library files are reached through one port, `core::files::LibraryFiles`.** A root is a
  string its backend understands and everything below it a `RelativePath`, which cannot climb
  out of its root; the scan never builds a filesystem path. `server::files::LocalFiles` is the
  one backend today, so that a library on a share or in object storage later is a new backend,
  not a new scan. What a tool reads is a `FileLocation` the backend hands out.
- **A scan runs in the server, one per library, in the background.** `core::scan` walks with
  the reference's skip rule (a directory whose media files kept their names, sizes and
  modification times is not looked at again), reuses a known path, groups new files by the
  measured naming rules (`scan::layout`), analyzes them through `MediaAnalyzer` — the scanner
  binary's deep analysis, which reads every packet, falling back to the headers alone when it
  cannot finish, two at a time, ten minutes at most each — and writes through `ScanRepository`, whose
  items go through `db::write` like every other. What is gone is removed, unless its whole root
  cannot be reached: an unmounted disk is not an empty library. `BackgroundScans` keeps one scan
  per library and cancels between files.
- **A library's roots are set on the command line only** (`open-plex-server library add`),
  because no endpoint accepts a path: creating a library is not on the API yet.
- **HTTP authentication lives in `server`**, because both surfaces share it. It attaches a
  `Principal`, which is always a user; each surface then enforces roles in its own idiom
  (`Caller`/`AdminCaller` extractors on the Plex side, request data on the GraphQL side).

### GraphQL conventions

- **Relay-compliant.** Every identifiable type implements `Node` and carries an opaque,
  type-qualified global `id` (base64 of `Type:key`), refetchable through a single `node(id:)`
  field. The schema publishes no other identifier: the compatibility layer reads the number
  Plex publishes by decoding `id` with the same codec (`open_plex_api::GlobalId`), and builds
  the global id from an incoming Plex number to query with.
- **Lists are cursor connections** (`edges`/`node`/`cursor`, `pageInfo`), never bare arrays.
- **Every field, argument and enum value carries a description.** Enforced by a test that walks
  introspection, not by review.
- **DataLoaders for anything resolved per parent.** `Library.roots` batches: ten libraries cost
  one `WHERE section_id IN (…)` query, verified against the SQL log.
- Values exceeding 32 bits are exposed as `String`, since GraphQL's `Int` only guarantees 32
  bits — `contentChangedAt`, a counter rather than a time, routinely exceeds it.
- **Points in time are `DateTime`** (`@specifiedBy` the andimarek date-time spec): RFC 3339,
  written in UTC with `Z` to the second, read with any offset. Storage, `core` and `db` keep
  Unix seconds as integers, converted in `api` (`date_time`); a release date is its day's
  midnight UTC. The Plex adapter reads them back into Plex's epochs and `YYYY-MM-DD`.
- **Database through an ORM** so the engine can be swapped: **SeaORM** (one API across
  PostgreSQL / MySQL / SQLite, built-in migrations, natively async). SQLite is one backend among
  several, not the design.
- **Transcoding video is process supervision.** Plex does not decode video itself, it executes
  `ffmpeg`. open-plex does the same: no libav bindings, no video decoding inside the server's
  address space. Images are the exception, by the repo owner's decision: `/photo/:/transcode`
  converts in process with the `image` crate, memory-safe decoders on a format allowlist, because
  two processes per thumbnail cost more than the work.
- **The scanner is a command line over one file, and keeps nothing.** `open-plex-scanner
  <command> [options] -- <file>`: `probe [--deep]`, `hashes`, `loudness`, `fingerprint`,
  `subtitles`, `bif`, `chapter-thumbs`, `artwork`. It prints one JSON report on standard output
  (`open_plex_scanner::report`, the same types the server reads back, versioned), writes the
  files it makes into an `--out` directory the server created and owns, creates no directory
  and overwrites nothing, and exits `3` when the media is at fault, `2` on bad usage. It
  decodes nothing itself — ffprobe and ffmpeg-plex do, as its children — reaches no network
  and holds no state: what to redo, and where results go, is the server's. The file comes after
  `--` and reaches the tools as `file:…`, so no name is read as an option or a protocol.

### Access rights

- **Accounts and permissions are our own design.** We do not reproduce Plex's sharing model —
  not `library_section_permissions`, not its account table, not its home/restricted flags. The
  importer copies no accounts, which also keeps the 283 real accounts (with names and e-mail
  addresses) in the reference database untouched.
- **Every request is a user's.** There is no operator, no admin token and no trusted network:
  a caller is a Plex token resolved through `user_extension`, or a session, and anything else
  is nobody. What used to be the operator's is a server role, `OWNER`, on a user row.
- **Three tables, one per concern.** `users` (id, a unique lower-case `username`, an optional
  unique `email`, a server `role` — `owner` or `user` — and dates), `library_permission` (a
  `viewer` or `admin` role for one user on one library) and `user_extension` (one user
  connected through one protocol, `plex` for now, with its external identifier and a digest of
  its token).
- **An owner manages the server.** It reaches every library as their admin
  (`LibraryAccess::All`, whatever permissions it holds), manages users, their roles, permissions,
  activation and extensions, reads everyone's history and sessions, terminates sessions and
  edits everything. Library permissions shape what a `USER` reaches, as before. `setUserRole`
  grants or revokes `OWNER`, for owners only.
- **The built-in `admin` account always exists.** At startup, before serving, the server makes
  sure a user named `admin` exists, is active and is an owner (`core::account::ensure_admin`,
  called from `main`). Created, it takes `OPENPLEX_ADMIN_PASSWORD`, or a password drawn from the
  OS RNG that is printed on standard output once and goes nowhere else, not even the logs;
  existing, it takes `OPENPLEX_ADMIN_PASSWORD` when that is set and keeps its own otherwise. A
  configured password the policy refuses stops the server. `admin` has no address, and it cannot
  be demoted, deactivated or deleted: those mutations answer `BUILT_IN_ACCOUNT`.
- **Rights are enforced in the schema.** The transport attaches a `Principal`, and the resolvers
  that lead to a library's content check it; every Plex query runs as the request's caller
  (`gql::run_as_caller`). A request with no caller sees nothing: forgetting to attach one fails
  closed. What a user was not given answers as if it did not exist.
- **A username is 3 to 32 of `a-z`, `0-9`, `.`, `_`, `-`**, starting with a letter or a digit,
  stored in lower case so that two users cannot differ by case alone. It holds no `@`, so a
  login is unambiguous: with an `@` it is an address, without one a username.
- **A user can sign in with a password, and waits for an owner first.** `signUp` (a username, an
  optional address, a password) creates an inactive user with an Argon2id hash
  (`user_password`, one row per user); `activateUser` / `deactivateUser` are the owners'. An
  inactive user opens nothing, whatever it presents: its Plex token and its sessions stop at
  once. A user an owner creates (`createUser`) is active and has no password. `signIn` takes a
  username or an address, answers `INVALID_CREDENTIALS` alike for an unknown account and a wrong
  password (a decoy hash keeps the timing the same), tells `INACTIVE` only with the right
  password, and limits failures to five per identifier per fifteen minutes.
- **A session is a signed JWT in an `HttpOnly; SameSite=Lax` cookie**, `open_plex_session`,
  HS256 with `OPENPLEX_SESSION_SECRET` (32 bytes at least; without it a key is drawn per process
  and sessions end with it), `Secure` unless `OPENPLEX_INSECURE_COOKIES=1`. Nothing stores it:
  the transport checks the signature and expiry, then the user's current state, on every
  request. A script or an API client reads the session from the `Set-Cookie` of `signIn` and
  sends it as `Authorization: Bearer <session>`, on every surface; the payload carries no token,
  so a page's scripts never see one. Precedence is the Plex token, then the bearer, then the
  cookie. `signOut` clears the cookie; a copied session stays valid until it expires (30 days)
  unless its user is shut out. The tools sign in as `admin` this way
  (`tools/open_plex_session.py`, with `OPENPLEX_ADMIN_PASSWORD`).
- **`/graphql` admits nobody**, so a browser can sign in before it is anyone. The request goes
  through with no principal, which every resolver already answers as "nothing exists";
  `nobody_reads_nothing_from_any_entry_point` keeps it that way. Every other route still
  answers `401`.
- **Jellyfin is an extension too.** `user_extension` holds a `jellyfin` connection beside a
  `plex` one, switched on by an owner (`connectExtension(JELLYFIN)`, or `/admin`). Jellyfin's
  clients sign in with the user's own password (`signInThroughExtension`, checked and throttled
  as `signIn`), each device keeping a token of its own in `extension_device`; `signOutOfExtension`
  forgets the caller's device, and switching the connection off or deleting the user forgets
  them all. Plex's clients do not sign in with a password (`NO_PASSWORD_SIGN_IN`). Every Jellyfin
  user sees one server id, derived from the machine identifier, and none is an administrator to
  Jellyfin's clients.
- **A user pairs its own server.** `pairExtension`, `moveExtension` and `unpairExtension` are
  open to owners and to the user they name, so a client can offer pairing in the user's own
  settings; issuing and revoking the token (`connectExtension`, `disableExtension`) stays the
  owners'. `viewer` says who is asking (its `role`, and the `User`), which is how a client
  decides what to offer.
- **`/admin` is the owners' page.** Users, their roles, their libraries, their Plex connection;
  served without a token, holding no data, signing in through `signIn` and driven entirely by
  `/graphql` with the session cookie. That endpoint accepts JSON only, because the cookie is
  `SameSite=Lax` and a page of the same site (another port, another subdomain) could otherwise
  post a form to it as whoever is signed in.
- **Watch state is the viewer's and the work's.** Every viewer is a user, and its state is keyed
  by its user id: the operator's viewer 0 is gone, deleted by migration `m0030` with everything
  else it owned. A request with no caller has no watch state at all. Rows are keyed by `guid`,
  because the reference keeps one state for every copy of a work. History is per viewer too,
  but by item, the copy watched: a user reads and deletes its own, an owner everyone's.
- **Each Plex user has its own server.** A token resolves to a user, and the Plex surface
  answers with that user's machine identifier, on `/identity` too when a token comes with the
  request. One process and one address: the separation is in what each token sees. Session
  identifiers are chosen by clients, so they are stored under a per-user name.
- **Pairing ties one user's server to one plex.tv account.** An owner, or the user itself,
  gives a code from `plex.tv/claim` and an address; `extension_account` keeps the account, its token
  and the address, which is republished at startup and hourly. plex.tv is reached through the
  `AccountDirectory` port, implemented in `api-plex::plex_tv`. `/identity` with no token shows
  the server published at the address the request was sent to, when only one is (the transport
  attaches a `PublishedServer`, which opens nothing). CORS admits Plex's domains only, because a
  browser here carries its user's session and Plex Web its token in every URL: a page allowed to
  read the answers could read whatever that user can. A client that found the server
  through plex.tv presents a plex.tv token, which only plex.tv can attribute: the answer is
  remembered five minutes, a refusal included, and new tokens are looked up at most 60 times a
  minute, so a stranger cannot use the server to flood plex.tv. The account's token is stored
  in clear, as Plex's own `Preferences.xml` does, because the server must present it.

### Deliberate divergences from Plex

Places where we knowingly answer differently, each decided rather than overlooked.

- **Search ranks every match; the reference ranks a pool.** `/hubs/search` and
  `/library/search` order by the reference's own score — a rung of two tenths for how well
  the title answers, a ten-thousandth per point of audience rating (9,123 of 9,151 captured
  scores reproduced exactly) — computed in SQL, and the official API's `search` pages it by a
  keyset cursor of exact values: the score's hundredths, the stored rating, the sort title,
  the id. The reference ranks only a pool it draws first, in title order library by library,
  so a common term loses its best titles there (`le` at `limit=10`: nine films opening with a
  digit, none of the 0.73s); ours ranks them all. Same score on 4,657 of 4,873 shared items;
  matching stays the word prefix, every word required in any order, where the reference's is
  loose (`dune` finds `d'une`). `GET /search` ranks nothing on the reference either: the
  first 15 films, series and episodes in title order, as we answer it.
- **`Meta` lists what we honor, not what Plex offers.** `includeMeta=1` publishes 18 filters and
  9 sorts for a film library where the reference lists 27 and 9: a menu entry answering `501`
  would be worse than an absent one. A test keeps the list in step with `listing::parse`, and
  `/library/sections/{id}/filters` and `/sorts` are served from the same tables.
- **"Related" films are an approximation.** We found nothing in the reference's database stating
  which films are alike, so `core::similar` ranks by shared genres, actors and directors: a close list,
  not the same one (101 of 180 shown items on 30 films). The director and actor shelves around
  it follow the measured rule and match on 59 of 60 films.
- **A person's page lists at most 500 items.** The reference lists everything a tag carries
  unpaged — 4,699 films for Action. Cut by `page::MAX_ITEMS`, the answer states `totalSize`
  and `offset`; whole, it is the reference's attribute for attribute.
- **The search field lists people, genres and collections, never found loosely.**
  `GET /library/search`, what Plex Web's search field calls, groups films, series, episodes,
  actors, directors, genres and collections, cut to `limit` as a whole, and honors
  `searchTypes` as measured (`people` drawn from the libraries the other values name). The
  reference's fuzzy matches — `comedie` finding actors named *Comer* — are absent, and
  `otherVideos`, which brings film genres there, brings nothing.
- **Search shelves fill an empty film or series shelf through a person.** `GET /hubs/search`
  answers the reference's seventeen shelves, collections included, and for `tom hanks` lists Tom
  Hanks' films with `reason="actor"` by the measured rule (`core::credited`, `searchCredits` on
  the official API): the shelf's first actor and director, their newest credits in one library,
  ranked. Same set and scores as the reference on the film shelf for all eleven names measured
  at `limit=3`; the person chosen differs where the reference's own pool of people leaves the
  best-credited one out. `sectionId` narrows nothing, as on the reference.
- **Voice search ranks by the distance it publishes.** `GET /hubs/search/voice` measures
  every title as the reference does — sort title and term alike, one article dropped, its
  own set of letters folded, bytes counted — but the reference picks its `limit` titles by
  another distance, on the raw title with case kept, and can skip a closer one (`dune`: *Cube*
  at 2 rather than *Dude* at 1). Ours are the closest by the distance shown: on 63 terms, never
  farther, the same distance on every item both list. Ties follow the folded title, and a
  term past 256 bytes is refused.
- **Range filter operators need their `>` encoded.** `year>=2020` travels with a raw `>`, which
  is not a legal URI character: `hyper` rejects the request line with `400` before any handler
  runs. `year%3E=2020`, the `>` alone encoded, reaches the parser and works; the reference
  rejects the fully encoded `year%3E%3D2020`, and so do we. The shelves we publish link to
  their lists in that form.
- **Listing order need not be byte-identical.** Plex collates titles in Unicode — numeric
  (`6 Days` < `007 Spectre` < `10 000`) and UCA punctuation order (`[` before `#`). We compare
  the folded `sort_key` column byte-wise. Same items, alphabetical and stable, different
  sequence: 299/500 positions match on films, 438/500 on series, median displacement 0. Close
  enough; a UCA dependency plus a re-import of 1.1M rows is not worth the remainder.
- **A random listing keeps one order for ten minutes.** The reference reshuffles on every
  request, so paging meets items twice; ours is a shuffle seeded by the clock's ten-minute
  period, and the official API takes the seed as an argument. A sort we cannot honor
  (`unviewedLeafCount`, the series order of episodes, a list of fields) answers `501`.
- **A play queue holds at most 500 entries.** The reference copies a whole library into one;
  every window here is capped by `page::MAX_ITEMS`, and so is what an addition may bring. Queues
  are deleted after 7 days, besides being replaced by their player's next one of the same type
  as on the reference; a queue created without `X-Plex-Client-Identifier`, which the reference
  refuses `400`, is made and replaces nothing. `onDeck=1` is not honored.
- **Quality follows a fixed ladder.** Under `maxVideoBitrate` we re-encode along the sizes and
  bitrates measured on the reference, whose own values vary per film. It copies the video only
  when the limit covers a peak its analysis stored; we have no analysis and compare with the
  file's average, so we copy more often.
- **Automatic quality adapts by a rule of our own.** `autoAdjustQuality=1` is the official
  API's `SERVER_ADAPTIVE`: the reference's decision and single representation, then lower
  bitrates at the same picture size, chosen from the server's timing of its own deliveries and
  moved to by restarting the encoder under the same URLs. The reference holds such a mechanism,
  but no test here fed it, so its rule is unknown. `CLIENT_ADAPTIVE`, where the player chooses
  among the copy and the encodes, exists on the official API only. Both hold restarts back so
  that a player or a link hovering between two rungs does not restart the process back and
  forth; the rules and their values are in `docs/spec-gaps.md`.
- **`/:/timeline` with `state=stopped` tears the conversion down**, as the reference does: its
  process would otherwise run on for five minutes holding a session slot. `/stop` ends the
  session with its conversion, where the reference keeps listing it until the timeline stops. The
  `TranscodeSession`
  it reports carries the reference's full field set, filled from the live conversion
  (`transcoder::session_status`): decisions, codecs, `progress`, `speed`, `maxOffsetAvailable`.
- **A chunk whose process has ended is refused after 5 seconds, not at once.** The repo owner's
  decision, so that a player does not retry a missing chunk in a tight loop. The reference
  answered `404` within 0.1 s when its process had died; a running process could not be measured
  there. A running process keeps its full 20 seconds, and a session that is unknown or stopped
  answers `404` at once, as on the reference.
- **Each user's server holds its own history.** The reference's administrator reads every
  account's viewing history; on the Plex surface a token reads its own viewer's, published as
  `accountID` 1, the owner of the server it sees. An owner reads everyone's on the official
  API (`viewingHistory`). An entry names the item watched and copies nothing of it, so it goes
  with its item; no device is recorded.
- **Playback sessions are stored, with automatic expiry.** The reference keeps them in memory.
  Ours are rows in `playback_session`, exposed on the official API; every sign of life pushes
  `expires_at` forward 300 seconds, reads skip what has expired, a sweep every 30 seconds ends it
  with its stream, and startup and Ctrl-C clear everything; the sweep also removes transcode
  directories no session owns, which a process outliving a killed server leaves behind. A direct-play stream lives as long as
  its session, no longer six hours.
- **An owner terminates a session on the official API first** (`terminatePlaybackSession`,
  owners only); `/status/sessions/terminate` binds onto it, by `Session.id`, which is now the
  player's session identifier as on the reference. The stream stops at once; the row stays,
  listed nowhere, until the player stops or one more lifetime passes, so that its next timeline
  carries `terminationCode` 2006 and the reason; a start under its key is refused meanwhile. The
  reference refuses terminate `401` without a subscription: the success path follows its binary
  and Plex Web, not a capture.
- **The transcoder's callbacks are the transcoder's, and carry no token.**
  `/transcoder/{session}/{run}/…` (`source`, `manifest`, `progress`) is how the process reads its
  source and reports its segments, served by `server::callbacks` straight over the transcoder:
  no Plex route is involved, where the reference's process calls back under
  `/video/:/transcode/session/`. Those routes bypass token authentication and answer only on
  loopback, and to the unguessable id of a running session.
- **A stream lives at an address of its own.** `start.mpd` and `start.m3u8` start the stream
  through the official API's `startStream` and answer `307` to `/streams/{id}/manifest.mpd` or
  `master.m3u8`, whose URLs all carry the stream's own secret (`?token=`, not the account's): the
  Plex binding builds no manifest. The reference serves its segments under the session's name,
  without a token.
- **Server settings are refused.** `/:/prefs` answers `403`, the repo owner's decision, so that
  Plex Web offers none of its server settings screens, which drive machinery this server does
  not have; `allowChannelAccess` is `0`, which removes Plugins. Editing items is unaffected.
  Dashboard, Alerts, Conversions and Manage › Libraries stay: Plex Web shows them for any server
  plex.tv lists as the viewer's own (`docs/spec-gaps.md`).
- **`/library/metadata/{id}/userState` exists here and not on the reference.** It belongs to
  plex.tv's Discover provider; Plex Web also sends it to the server for an item without a Plex
  GUID, and it answers in that provider's shape, always unwatched.
- **File paths are published**, on both surfaces, as Plex does: `MediaFile.path` with a derived
  `MediaFile.filename` in GraphQL, `Part.file` in the Plex layer. This is reference data — it
  makes the library auditable from outside. It changes nothing about input: playback addresses
  a file by `id`, and **no endpoint accepts a path from a client**. A path we hand out and a
  path handed back to us are different things.
- **A listing always windows.** Plex returns a whole library unpaged when the client sends no
  container parameters; `page::MAX_ITEMS` forbids that. The response states `totalSize` and
  `offset` and sends the paging headers, so `size` never claims more than the body holds.
- **`ownerFeatures` announces all 209 features of a Plex Pass server**, by name and GUID, captured
  from one (`api-plex/src/owner_features.txt`). Plex Web opens a screen on the strength of them,
  so some screens open onto endpoints that still answer `501`.
- **`myPlexSubscription` reports true** whatever the account: no paywall. `myPlex` and the
  attributes around it describe the caller's own server: paired or not.
- **An image is fetched from Plex's hosts only.** The reference's `/photo/:/transcode` fetches
  any URL it is handed, its own loopback included; that is an SSRF hole for any holder of a
  token. We fetch over TLS from `plex.tv` and its subdomains, on every redirect hop, which is
  all an account avatar needs, and answer `403` for the rest.
- **The Plex surface announces a Plex version, not ours.** `/identity`, `GET /` and
  `/media/providers` report `1.43.5.11029-6bf8af3f0`, the reference server every capture comes
  from: Plex Web refuses a server older than 1.18.3, and `0.1.0` reads as one. Clients that
  enable features by version then expect exactly the behavior we copied. The official API keeps
  open-plex's own version (`api-plex::general::PLEX_VERSION`).
- **A playlist is its viewer's, and its ids are its own.** One user's, an owner's included, as
  watch state is; another's answers `404`, and an entry whose library its owner may no longer
  read is hidden. Ids start at 1 rather than sharing the items' numbering, so a film's id is not a
  playlist and `PUT /playlists/{a film's id}` renames nothing, where the reference renames the
  film. `POST /playlists/upload` imports `.m3u` files from a path the client names and stays
  refused (`403`): no endpoint accepts a path from a client.
- **A collection is named by its own title, and listed apart.** The reference names a
  collection on its items by the tag it shares across libraries, whose case, guid and summary
  can differ from the collection's; we publish the collection's own. `includeCollections=1`
  mixes nothing into `/all`: collections are listed by `type=18` and `/collections`. A
  collection made here is numbered past the highest item, where a later import could bring an
  item of that number (`docs/spec-gaps.md`).
- **Live TV answers as a server with nothing set up.** There is no grabber, tuner or guide
  provider, so every Live TV, DVR, guide and recording list is empty and every one of them by
  id a `404`, as on the reference without a DVR. Where the reference lists what it has — the
  grabbers it ships, plex.tv's guide countries and languages — we list nothing, since none of
  it can be used here; where it refuses recordings `401` for want of a subscription, we answer
  the empty list (`docs/spec-gaps.md`).
- **Intro and credits markers are everyone's, and their shape is inferred.** The reference
  publishes the markers its analysis found only to a subscriber, so their element follows the
  spec's example and the bookmarks it does publish (`docs/spec-gaps.md`); we publish them to
  every caller who reads the item, and detect none: they are imported, or set on the official
  API by a library's editors. The Plex marker routes write the caller's bookmarks only — a
  `type` other than `bookmark` is a `400`, as is a bookmark on a series, which the reference
  keeps.

- **A rating is from 0 to 10.** The reference keeps whatever `/:/rate` is sent — 11, 100,
  `inf` — and nulls the rating on a negative other than `-1`; we refuse all of these `400`, as
  the spec bounds `rating` to ten (`docs/spec-gaps.md`).
- **Extras are listed to everyone.** The reference hides the trailers its metadata provider
  supplied from an account without a subscription — every extra of the reference library — and
  names a film's first trailer only by `hasPremiumPrimaryExtra`. We list them on `/extras` and in
  `Extras`, each as the reference looks it up, and name the first trailer as `primaryExtraKey`
  on films as on series. The provider's address is published as imported and answers `404`
  here; `POST /library/metadata/{ids}/extras`, which fetches a URL the client names, is refused
  `403` (`docs/spec-gaps.md`).

- **A backdrop's colors are read from Plex's hosts or from storage, nothing else.** The
  reference's `/services/ultrablur/colors` and `/image` fetch any `url`, its own loopback
  included; ours fetch over TLS from `plex.tv` hosts only (`403` otherwise), answer an item's
  `/library/metadata/{id}/art` with the colors its analysis stored, since no artwork is served,
  and `404` any other local address. The algorithm is the reference's, from its binary, exact on
  PNGs; JPEGs decode differently in `zune-jpeg`, so 34 of 150 photos match exactly. A color the
  reference's `sscanf` cannot read draws stack garbage there and is a `400` here; our noise is
  seeded, so an image never changes (`docs/spec-gaps.md`).
- **A subtitle file is served as UTF-8 plain text.** The reference sends a sidecar's bytes as
  written, typed `text/html`, and converts to UTF-8 only when `encoding` is given; ours is
  always UTF-8 (a file that is not is read as Windows-1252), `text/plain` with `nosniff`,
  because subtitle markup served as a page of this origin would run with the viewer's session.
  A sidecar is read and deleted only within its library and in its media's directory or below,
  never the media itself; subtitles the reference downloaded into its blob storage were not
  imported and answer `404` (`docs/spec-gaps.md`).
- **The search of online subtitles finds nothing.** The reference asks OpenSubtitles through
  Plex's service; there is no provider here, so `/library/metadata/{ids}/subtitles` answers the
  reference's own empty result after its checks, and downloading a result has no route.
- **Nothing is popular in a user's libraries.** The reference's "Popular in Your Libraries"
  counts the plays of the server's other users; each user's server here receives no one
  else's, so `/hubs/popularInLibraries` answers the `404` the reference gives an empty or
  disabled shelf, and `/hubs` leaves it out, as the reference does.
- **Who watched an item is answered, where the reference answers `404`.**
  `/library/metadata/{id}/users/top` is `404` on every film, series, season and episode of the
  reference, though its binary counts each account's viewings of the work; ours answers the
  spec's `Account` list with the caller alone, `accountID` 1 on its own server, through
  `viewers` on the official API, where an owner sees every user (`docs/spec-gaps.md`).
- **`/library/tags` lists what the caller's libraries carry.** The reference lists every row of
  its `tags` table of a type, unpaged; ours leaves out a tag nothing readable carries, pages by
  500, answers `type=2` (collections, which have no tag here) `501` and its internal types
  (labels, networks, guids…) empty (`docs/spec-gaps.md`).
- **An item's tree tells its times as stored.** The reference's `/library/metadata/{id}/tree`
  shifts `addedAt` and `refreshedAt` by its system's UTC offset (an hour earlier, measured in
  Paris in November), unlike every other endpoint; ours are unshifted. A series' eight
  preferences, which it writes for an account holding a state of the series, stand at their
  defaults since none is kept here, the language choices named in French as the reference's
  system names them. A trailer has no tree (`404`): it is not looked up by number here.
- **Nothing has sonic neighbors.** Both `nearest` routes answer the reference's `403` for what
  is not an analysed track, music libraries included, which the reference could not show.
- **A transcoder fallback is always refused.** On the reference it restarts a hardware or HEVC
  conversion in software and H.264 (its binary and the spec say so); every conversion here
  already decodes in software and encodes H.264, so `fallBackStream` answers `NO_FALLBACK` and
  `/universal/fallback` the `412` the reference gives its own software conversions. Its success
  could not be measured: the reference has hardware transcoding off (`docs/spec-gaps.md`).
- **A scan request names no path, and `force` rescans nothing.** `path=` on
  `/library/sections/{id}/refresh`, a partial scan of a directory the client names, answers `403`:
  no endpoint takes a path. `force=1` on the reference refreshes every item's metadata and still
  skips unchanged directories; without an agent, it changes nothing here. `force` on the
  official API's `scanLibrary` analyzes every file again. New items carry `local://{id}` and the
  title and year their names give, as the reference's do before an agent runs; a renamed file is
  a new item, where the reference finds it again by size and hash.
- **A file's name gives its one untagged audio track a French dub.** The repo owner's rule: when
  a file has a single audio track and it carries no language, a whole word of the file's name
  sets one — `VFQ` `fr-CA`, `VFF`, `VFI`, `FRENCH` and `TRUEFRENCH` `fr-FR`, `VFQ` winning over the others —
  where the reference leaves the track without a language. A tag, or a second audio track,
  turns it off (`db::scan_repository::analysis::dub_language`).
- **A scanned track's rate is the one measured over its packets.** The repo owner's decision:
  the scan stores the deep analysis' rates and durations, where the reference mostly publishes
  the declared ones (33 of 38 samples), its deep analysis overwritten by a later normal one.
  The headers' rates stand only when the deep analysis cannot finish
  (`docs/scanner/analysis.md`, §2).
- **A category's tile shows no item's art.** The reference draws each genre's tile from the
  background art of its best-rated item not already shown; this server keeps no artwork, so
  every tile names the image the reference uses for a genre without art,
  `category-art/animation.png`, with the reference's own color (`docs/spec-gaps.md`).

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

# Serve. The first start creates the `admin` account; without OPENPLEX_ADMIN_PASSWORD its
# generated password is printed once on standard output.
OPENPLEX_ADMIN_PASSWORD='a long passphrase' cargo run -p open-plex-server
```

| Variable | Default | Purpose |
| --- | --- | --- |
| `OPENPLEX_DATABASE_URL` | `sqlite://open-plex.db?mode=rwc` | Any SeaORM backend; migrations run on connect. |
| `OPENPLEX_ADMIN_PASSWORD` | *(none)* | The password of the built-in `admin` owner, set at every start. Unset, a new `admin` gets a generated one, printed once on standard output, and an existing one keeps its own. Outside the password policy (8 to 256 characters), the server refuses to start. The tools sign in with it. |
| `OPENPLEX_SESSION_SECRET` | *(drawn per process)* | The HS256 key sessions are signed with, 32 bytes at least. Without it, sessions end when the server stops. |
| `OPENPLEX_INSECURE_COOKIES` | *(off)* | `1` drops `Secure` from the session cookie, for plain HTTP on a local network. |
| `OPENPLEX_MACHINE_ID` | placeholder | Must be stable across restarts or clients stop recognizing the server. |
| `OPENPLEX_BIND` | `127.0.0.1:32500` | Listen address. Deliberately not 32400, so it can run beside the reference server. |
| `OPENPLEX_JELLYFIN_WEB_DIR` | *(none)* | Jellyfin Web, served without a token at `/jellyfin/web`: a Jellyfin install's `jellyfin-web` directory. Not versioned: it is Jellyfin's code. |
| `OPENPLEX_WEB_DIR` | `web` | Plex Web, served without a token at `/web` as the reference serves it. Not versioned: it is Plex's code. Absent means `/web` is not served. |
| `OPENPLEX_TRANSCODER` | `ffmpeg` beside the server's executable, else on `PATH` | The patched ffmpeg that Direct Stream runs; its command lines use options a stock build lacks. Plex's sources rebuilt with its configuration, codecs included: `github.com/Maxou44/ffmpeg-plex` releases, kept locally under `.work/ffmpeg-plex/<version>/`. A stream whose player chooses its rung needs `absolute_segments`, which cuts and numbers its segments on the media's timeline and its audio where each key frame is shown: v8.1.3-13 onward. `files/Plex Transcoder.exe` also works for the rest, with the codecs below. |
| `OPENPLEX_TRANSCODER_FONTS` | `Resources/Fonts` beside the transcoder | The fonts burned-in subtitles are drawn with: a Plex install's, `files/Resources/Fonts` here. Without them, a burn answers `501`. |
| `OPENPLEX_FFPROBE` | `OPENPLEX_TRANSCODER -probe` | The ffprobe the scanner reads files with. ffmpeg-plex carries its own, run as `ffmpeg -probe` (first argument), from v8.1.3-15; with an older release or Plex's binary, a stock ffprobe. |
| `OPENPLEX_SCANNER` | `open-plex-scanner` beside the server's executable, else on `PATH` | The scanner binary a library scan runs on each new or changed file. |
| `OPENPLEX_TRANSCODER_CODECS` | *(none)* | Plex's separately distributed codecs (`%LOCALAPPDATA%/Plex Media Server/Codecs/<build>`), which Plex's own binary needs. The rebuilt one has them built in and needs none. |

`.github/workflows/package.yml` builds the server and the scanner for Linux (x86_64, arm64),
Windows and macOS (arm64, x86_64), packages each with the ffmpeg-plex release it pins by
version and checksum and `packaging/README.txt`, scans a generated file with the package as a
test, and publishes the archives as a release on a `v*` tag. Laid side by side, the three
programs find each other with no variable set.

The imported library's files live on another machine, so nothing imported can be played here.
`python tools/plex_tunnel.py --origin http://LAN-IP:32500` opens a Cloudflare quick tunnel and
publishes its new address to plex.tv through `moveExtension`, signed in as `admin`, with the
server running; it refuses a loopback origin, where the transcoder's callbacks answer without a
token.
`open-plex-server library add --kind movie|show --title TITLE ROOT...` creates a library over
directories of this machine, and `open-plex-server scan ID [--force]` scans it without serving
(a running server scans on `scanLibrary` or `/library/sections/{id}/refresh`); this supersedes
`python tools/seed_test_library.py DIRECTORY`, which writes a `Test` library straight to the
tables; `python tools/capture_transcoder.py` studies the transcoder alone.
To import again into a database the server has written to, `python tools/reimport.py`, server
stopped: the importer replaces every library, and with them the history, track choices,
locks and collections made here and the seeded libraries; the script backs the database up,
keeps those, imports, seeds again and puts back whatever still has its item.

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
- ✅ **filter choices**, `/library/sections/{id}/{filter}`: genres, countries, people, years,
  decades, content ratings and studios a library's items hold, each with the listing that
  applies it (`Library.tags`, `years`, `fieldValues` on the official API); the starring list
  is the actors billed in the first three positions, as measured;
- ✅ **resolution**: `videoResolution`, `resolution=` and its value list from one measured ladder
  (`api-plex::resolution`), over a frame-range filter on the official API (`ItemFilterInput.frame`):
  identical to the reference on 99,439 versions and every class of eight libraries;
- ✅ **track analysis**: the media analysis Plex packs into `extra_data` imported into typed
  columns (`media_stream`, `media_part`) and two tables (`stream_dolby_vision`,
  `stream_loudness`), exposed on the official API (`MediaStream.video`, `.audio`,
  `.subtitle`, `.traits`; `MediaVersion.primaryVideo`/`primaryAudio`), and published on the
  Plex surface with the labels clients show — `language` named by ICU's rules from CLDR 39
  (`tools/gen_language_names.py`), `displayTitle` and `extendedDisplayTitle` measured on
  4,473 tracks in three languages: no differing field on video or audio tracks;
- ✅ **listing order**: the reference's nine film sorts and their series and episode variants,
  over nullable and computed values — absence first, ties in title order, a version's extreme
  by direction, all measured on whole libraries (`docs/spec-gaps.md`). Cursors carry several
  keys, one of which may be absent (`item_order`); a sort we cannot honor answers `501`, like
  a filter;
- ✅ **filters and shelves on watch state**: `unwatched`, `inProgress`, `unwatchedLeaves` on
  `/all` by the rules measured on the reference's test libraries (`ItemFilter.watched`,
  `inProgress`, `allEpisodesWatched` on the official API, read for the caller), and the
  library shelves that are listings — recently released, top unwatched, recently watched,
  recently aired, start watching, top rated — each filled from the `key` it publishes;
- ✅ **choosing tracks and burning subtitles in**: `PUT /library/parts/{id}` keeps each viewer's
  audio and subtitle per file (`chooseTracks`, `MediaFile.selectedAudio`/`selectedSubtitle` on
  the official API), forced subtitles in the audio's language by default, and `subtitles=burn`
  draws the chosen text subtitle into a re-encode with the reference's own `inlineass` filter;
- ✅ **`/hubs/home/recentlyAdded`**, the list behind a series library's shelf: films or grouped
  series entries, one library or all of a kind (`recentlyAdded(kind:)` on the official API);
- ✅ **watch state**: per viewer and per work (`guid`), so every copy of a film is watched at
  once; positions, viewings and the parents' counts by the rules measured on the reference
  (`core::watch`), on the official API (`Item.watchState`, `stats.watchedEpisodeCount`,
  `markWatched`, `markUnwatched`, `reportViewing`, `keepWatchPosition`) and on the Plex surface
  (`/:/scrobble`, `/:/unscrobble`, `/:/progress`, the timeline's `playbackState`, and
  `viewCount`/`viewOffset`/`lastViewedAt`/`viewedLeafCount` on items);
- ✅ **continue watching**: films in progress and each series' next episode, by the rules
  measured on fourteen watch patterns (`Watching::continue_watching`), on the official API
  (`continueWatching`, each entry with its reason) and on the Plex surface
  (`/hubs/continueWatching`, `/hubs/home/continueWatching`, `/hubs/home/onDeck`, and every
  library's shelf);
- ✅ **viewing history**: one entry per viewing counted — a playback crossing 90%, a film or
  episode marked watched — written with the watch state by `record_viewings`, its one write
  path, and left alone by unwatching; on the official API (`viewingHistory`, a user's own and
  an owner everyone's, `deleteHistoryEntry` with `userErrors`) and on the Plex surface
  (`/status/sessions/history/all`, its lookup and `DELETE`), with the filters, sorts and
  refusals measured on the reference's test libraries;
- ✅ the drawn shelves — top of a genre, of a person — drawn as measured on 150 of the
  reference's draws, and rediscover;
- ✅ **native streams**: `startStream` and `stopStream` on the official API — direct play,
  direct stream or transcode, decided from the file and the player's limits — read at
  `/streams/{id}/…?token=…`, and Plex's `start.mpd`/`start.m3u8` redirecting there;
- ✅ **the end of a session**: every end path converges on `core::lifecycle` (stream and row
  together, per-key locks, the sweep re-checking under them), reads of a native stream keep its
  session alive, Ctrl-C ends everything; `terminatePlaybackSession` and
  `/status/sessions/terminate`, the session's `terminatedAt` and `terminationReason`, listed by
  `playbackSessions(status: TERMINATED)`, and the timeline's `terminationCode`, and
  `/status/sessions/background`, always empty;
- ✅ **editing films, series and episodes**: `updateMovie`, `updateShow`, `updateEpisode` on the
  official API, for a library's admins and the owners, with per-field locks (`lock`,
  `unlock`, `lockedFields`) kept in `item_field_lock` and imported from Plex's `user_fields`;
  a title edit rewrites the sort title, its key and the title-word index through
  `db::write::update_item`, the one write path. On the Plex surface, `PUT
  /library/sections/{id}/all` with `.value`/`.locked`, the spec's `PUT /library/metadata/{ids}`
  with bare fields as well, and `Field` elements on lookups, by the rules measured on the
  reference's test libraries (`docs/spec-gaps.md`);
- ✅ **Live TV, DVR and music answer empty**: the 52 Live TV, DVR, guide and recording
  operations as the reference answers them without a DVR (`api-plex::livetv`), and music as
  held nowhere — `ARTIST`, `ALBUM` and `TRACK` on the official API keep nothing, so music
  `type`s on listings, filter choices and `/search` answer empty, a music library is listed
  and empty, and `/library/sections/{id}/albums` is its empty view;
- ✅ **person pages**: `/library/people/{id}` and `/media`, where `id` is a tag id (one
  person in one role, or any genre or country), as the reference keys them. On the official
  API, `Tag.items` and `Person.items` with `itemCount`, and `Person.credits`:
  a series counts for a credit on one of its episodes, hidden libraries are left out,
  libraries come by title with films by title and series by id, and a tag or a person exists
  for a caller only when its libraries credit it. Same order as the reference on sixteen
  pages; the library holds no biography to import (`docs/spec-gaps.md`);
- ✅ a series library's **"more in" a genre**: drawn evenly among the first four genres heading
  the series the viewer watched last, each series giving its first genre — the four that came
  up, and only those, in 260 draws over three of the reference's libraries
  (`docs/spec-gaps.md`).
- ✅ **search by shelves**, `/hubs/search`: the seventeen shelves of the reference, items
  through `search` and people and genres through `searchTags` on the official API, one entry per
  library carrying the tag, over a word index of tag names (`tag_word`, written by
  `db::write::insert_tags` only); identical to the reference entry for entry on the people
  shelves of six terms (`docs/spec-gaps.md`).
- ✅ **playlists**: the viewer's own, of chosen items or smart, kept in tables of our own
  (`playlist`, `playlist_entry`) with positions a move halves; on the official API (`Playlist`,
  `PlaylistEntry`, `playlists`, `createPlaylist`, `addToPlaylist`, `movePlaylistEntry`,
  `removeFromPlaylist`, `clearPlaylist`, `updatePlaylist`, `replacePlaylistRule`,
  `deletePlaylist`), a smart playlist keeping `Library.items`' own input as its rule; on the Plex
  surface the 16 `/playlists` operations as measured on the reference's test libraries, and
  `POST /playQueues?playlistID=` to play one. Upload stays refused, the optimizer's generators
  answer `404`, and `composite` names an image not served yet (`docs/spec-gaps.md`).
- ✅ **play queues, edited**: one queue per player and type, built from a source the queue keeps
  (`play_queue_source`) and edited by the reference's rules in `core::play_queue` — the "up
  next" area that `playQueueLastAddedItemID` ends, `next=1`, moves with the reference's reading
  of `after`, removal, clearing, reset, and shuffling that rebuilds from the source and renews
  every entry but the one playing; `continuous` on an episode and `repeat` on the window. On the
  official API (`addToPlayQueue`, `movePlayQueueEntry`, `removeFromPlayQueue`,
  `clearPlayQueue`, `resetPlayQueue`, `shufflePlayQueue`, `unshufflePlayQueue`,
  `PlayQueue.lastAddedEntry`, `kind`, `clientIdentifier`) and on the Plex surface (the nine
  `/playQueues` operations), replayed against the reference with the same trace
  (`docs/spec-gaps.md`).
- ✅ **collections**: a library's, read by whoever reads the library and changed by its admins
  and the owners; imported from Plex's type-18 items and collection tags (2,295 collections,
  4,586 entries; the 4 smart ones list a filter we cannot express) into tables of our own
  (`collection`, `collection_entry`, `collection_word`), ordered by release date, title or a
  custom order. On the official API `Collection`, `Library.collections`, `collections`, each
  item's `collections`, `ItemFilterInput.collections`, and `createCollection`,
  `updateCollection`, `deleteCollection`, `addToCollection`, `removeFromCollection`,
  `moveCollectionItem`, `clearCollection`, a smart collection keeping `Library.items`' own
  input. On the Plex surface the eight spec operations, `/children`, lookups and `type=18`,
  `Collection` tags, `collection=` and its choices, the search shelf, prefs and deletion, as
  measured on the reference's test libraries; composites answer `404`, as they did there
  (`docs/spec-gaps.md`).
- ✅ **adaptive bitrate, chosen by the server or by the player**, over HLS and DASH: `quality`
  on `startStream` (`FIXED`, `SERVER_ADAPTIVE`, `CLIENT_ADAPTIVE`). The player's choice lists
  the copy first and encodes cut on the same key frames, on a grid of segments ffmpeg-plex lays
  on the media's timeline so that every rung and every restart agree on where a segment starts;
  the server's moves bitrates under one representation whose codec headers never change
  (`transcoder::adapt` holds both rules).
  Played across rung changes by shaka-player and hls.js without a stop.
- ✅ **intro and credits markers, and bookmarks**: the 217,863 markers the reference's analysis
  stored imported into a table of our own (`item_marker`), the work's and each viewer's
  bookmarks apart by owner; on the official API (`Movie.markers`, `Episode.markers`, `Marker`
  with `node`, `addMarker`, `updateMarker`, `deleteMarker`: the work's for a library's admins
  and the owners, a bookmark for its keeper alone) and on the Plex surface (`Marker` on
  lookups, listings, `/children`, `/grandchildren`, `/allLeaves` and play queues with
  `includeMarkers=1`; the bookmark routes `POST`, `PUT` and `DELETE …/marker` as measured on the
  reference's test libraries). Detection (`…/intro`, `…/credits`) stays deferred.
- ✅ **relevance, people and collections in search**: the reference's score — rung and audience
  rating — computed in SQL over each item's folded whole titles (`item_title_key`, written by
  `db::write` only, migration `m0031`) and served by the official API's `search` (`order`,
  each edge's `score` and `titleMatch`, `episodesBySeries`), paged by an exact keyset cursor;
  films and series found through a person's name by the measured rule (`core::credited`,
  `searchCredits`); `/library/search` with people, genres and collections and `searchTypes`
  honored, `/hubs/search` ranked by the API's score and filling an empty film or series shelf
  through a person, and `/search` cut to 15 per kind in title order, as measured on 53 terms
  (`docs/spec-gaps.md`, *Relevance*).
- ✅ **voice search and autocompletion**: `/hubs/search/voice` ranks every film and series —
  or the kind `type` names — by edit distance to the term, by the measured rule
  (`core::closest`: sort titles, one article, the reference's folded letters, bytes), shelves
  per kind and `limit` across them; on the official API `closestTitles`, read in bands of
  length from an index of folded titles (`item_folded_title`, written by `db::write` alone)
  rather than the whole library. `/library/sections/{id}/autocomplete` completes titles and
  the six tag fields by the reference's phrase-at-a-word rule (`core::completion`), with
  `title.queryRange` in bytes, through `ItemFilterInput.titleCompletion` and
  `Library.tags(completing:)`; the other fields answer `501` (`docs/spec-gaps.md`).
- ✅ **the end of a playback, and the home screen**: `/hubs/metadata/{id}/postplay` — after an
  episode its series' Continue Watching entry, else the library's; after a film the next of
  the first of its collections whose shelf shows it, a shelf per collection, similar films —
  on the official API through `Episode.upNext` and `Movie.upNext(within:)`, which names its
  collection; the same shelves and next film as the reference on 191 and 111 of 200 films, the
  rest snapshot drift. `/hubs` and `/hubs/promoted` with the reference's shelves, an
  episode in progress on deck as on the reference, the playlists played last;
  `/hubs/metadata/{id}/related` and the empty music shelves of `/hubs/metadata/{id}`;
  `/hubs/popularInLibraries` a `404`, as on the reference.
- ✅ **codecs, layouts, menus and common fields**: the values a library's tracks hold
  (`videoCodec`, `audioCodec`, `audioLayout`, `subtitleCodec`), every track of every version,
  as choice lists and as filters on `/all` with a comma `OR` and `!=` (`Library.trackValues`,
  `ItemFilterInput.tracks` on the official API): the 40 lists of ten libraries identical, 780 of
  782 filtered totals; `/filters` and `/sorts`, one type's `Meta` menus; and
  `/library/sections/{id}/common`, what a set of items shares — fields, tags, collections and
  locks, aggregated in storage (`Library.common`, `ItemFilterInput.ids`) — element for element
  the reference's on films, series and episodes (`docs/spec-gaps.md`).
- ✅ **transcoder fallback**: `POST /{transcodeType}/:/transcode/universal/fallback` over the
  official API's `fallBackStream` (`StreamEngine::fall_back`, under the session's lock through
  `core::lifecycle`), with the reference's measured answers: `session` rather than the spec's
  `transcodeSessionId`, any of the four kinds, `400` without it, `404` for an unknown one, `412`
  under its `500` page for a running conversion, `404` for other methods (`docs/spec-gaps.md`).
- ✅ **a viewer's rating**: `PUT /:/rate`, the caller's own rating of a work out of ten — any
  kind, every copy at once, nothing added up from children — kept per viewer and `guid` in a
  table of our own (`user_rating`, migration `m0034`) with the time of the last change, so that
  an older change, a clearing included, is ignored (`core::user_rating`). On the official API
  `Item.userRating` and `rateItem` (`userErrors`), and `ItemFilterInput.userRating` and the
  `USER_RATING` order; on the Plex surface `userRating` to one decimal and `lastRatedAt` wherever
  an item is published, and `userRating` filtering and sorting `/all` by the reference's rules
  (`=-1` for no rating, every comparison leaving the unrated out). Not imported, like the watch
  state (`docs/spec-gaps.md`).
- ✅ **extras**: trailers, deleted scenes, interviews and featurettes, imported from Plex's type-12
  clips and `metadata_relations` (30,141 extras, 85,640 versions, 171,280 tracks; the 70 of kind
  `-1` left out) into tables of our own (`extra`, `extra_media`, `extra_track`, migration
  `m0033`). On the official API `Movie.extras`, `Show.extras`, `Season.extras`,
  `Episode.extras`, `primaryExtra` on films and series, and `Extra` as a node whose versions are
  `MediaVersion`s, their files and tracks found where an item's are (`MediaFile.streamUrl` for a
  provider's stream). On the Plex surface `GET /library/metadata/{ids}/extras`, `Extras` on
  `includeExtras=1` and `primaryExtraKey`, as measured with local extras of every kind on the
  test libraries; the provider's trailers, which the reference lists to no account without a
  subscription, as it looks each up: 230 of 264 compared identical, the rest one `primaryGuid`
  (`docs/spec-gaps.md`).
- ✅ **backdrop colors (UltraBlur)**: the four corner colors the reference's analysis stored,
  imported from `extra_data` into `item_corner_colors` (170,565 items) and published as
  `UltraBlurColors` on every item and as `cornerColors` on the official API; and the reference's
  own algorithm, read from its binary, in the transcoder (`transcoder::backdrop`: a median-cut
  palette per quarter, Okhsl scoring, the gradient over black) — identical on 269 of 269 PNGs,
  and on every JPEG once decoded as libjpeg-turbo decodes it. `/services/ultrablur/colors`
  binds onto it for Plex's hosts and onto storage for an item's art; `/image` draws the
  gradient, pixel-identical without noise, its area capped at 3840×2160 (`docs/spec-gaps.md`).
- ✅ **categories**, `/library/sections/{id}/categories`: a library's genres as tiles, from
  `Library.tags(kind: GENRE)`, each opening `/all?genre=`; whole unless both container
  parameters are sent, other parameters ignored, as on the reference. `blendColor` by the rule
  read from its binary and measured on 335 tiles of ten libraries, all identical — thirteen
  named genres keep a color, the others take a seven-color cycle by tag id — and the
  fallback image for a genre without art as `thumb`; identical to the reference, JSON and XML,
  on a test library without art (`docs/spec-gaps.md`).
- ✅ **a library's leaves**, `GET /library/sections/{id}/allLeaves`: its films or its episodes,
  `/all?type=` the leaves' type as measured on the reference (92 requests, filters and sorts
  included), under `/all`'s container without the library's identity, `title2` or `Meta`; a
  `type` naming no leaf kind is empty. On the official API `ItemFilterInput.depth: LEAVES`
  (a `kind` narrows it rather than replacing it) and `ItemOrder.SERIES`, the episodes' default
  order — series sort title, season, episode, release date, each absent one last — which is
  also `/all?type=4`'s default now, and so autocompletion's and a listing's play queue's: every
  series' episode order identical to the reference on four whole libraries (4,002 series), the
  remaining positions the collation of series titles (`docs/spec-gaps.md`).
- ✅ **subtitle files**: a sidecar's file, read through the media engine from the path its track
  stores and only where a sidecar of its media may be (`transcoder::media::resolve_sidecar`);
  each viewer's offset on a subtitle, per track (`subtitle_offset`, migration `m0036`); and
  deleting a sidecar with its file. On the official API `MediaStream.sidecar` (`path`,
  `filename`, `isDownloaded`, `text`), `MediaStream.offsetMs`, `setSubtitleOffset` and
  `deleteSidecar` (`userErrors`, library admins and owners); on the Plex surface `GET`, `PUT
  ?offset=` and `DELETE /library/streams/{streamId}.{ext}`, `key`, `file` and `offset` on
  `Stream`, and `/library/metadata/{ids}/subtitles` answering the reference's empty search, all
  as measured on the reference's test film with sidecars put beside it (`docs/spec-gaps.md`).
  `tools/seed_test_library.py` records the subtitle files beside each file as sidecars.
- ✅ **every tag of a kind, and who watched an item**: `/library/tags` over the official API's
  `tags(kind:)` — the six kinds by Plex's numbers, every depth, id order, as measured on every
  `type` and on 72,160 tags against the live reference (70,822 identical, the rest portraits
  and snapshot drift), asked tag by tag through the `(tag_id, item_id)` index; tag names with
  an ASCII first letter capitalized in lookups, listings and person pages too, as measured.
  `viewers` on films, series, seasons and episodes (`WatchRepository::viewers`, the binary's
  counting rule: every copy of the work, a series' and a season's episodes), and
  `/library/metadata/{id}/users/top` bound onto it (`docs/spec-gaps.md`).
- ✅ **an item as a tree**, `/library/metadata/{ids}/tree`: the reference's rows nearly as stored
  (`MetadataItem` → `MediaItem` → `MediaPart` → `MediaStream`, a track's analysis under its own
  keys in byte order), one number of the list exactly, a season rooting its own tree and an
  episode the branch from its series, by the rules measured on films, series, seasons, episodes
  and a collection; byte for byte on the test libraries. The three things it needed imported
  (migration `m0037`) are on the official API: `Item.refreshedAt`, `MediaFile.contentHash` and
  `openSubtitlesHash`, `MediaStream.requiredBandwidths`, and `MediaStream.externalUrl` for a
  sidecar. Both `nearest` routes answer as the reference does without music (`docs/spec-gaps.md`).
- 🚧 **the scanner's file half**, `open-plex-scanner`, measured against the reference on the
  test files: the analysis (every track field ffprobe gives, chapters with generic titles
  cleared, MP4 layout, MPEG-TS packet length; `--deep` measures each track's rate over every
  packet, the video's equal to Matroska's `BPS`), the OpenSubtitles hash (identical), the BIF
  (header and index identical, frames re-encoded by a newer libavcodec, 58 dB apart), Chromaprint
  fingerprints in process (9,841 of 9,945 values equal to Plex's Fingerprinter, 0.03 % of bits
  apart), EBU R 128 loudness, subtitle tracks extracted, chapter frames, and poster and backdrop
  frames at a quarter, half and three quarters (720×405 and 1280×720 for 16:9, as the
  reference's) with the covers a file carries; H.264's coded size, reference frames and scaling
  matrix from its sequence header (`trace_headers`), and the deep analysis' rates and duration
  over the time the packets span, as the reference measures them. A scanned file's lookup is the
  reference's field for field on `Media`, `Part` and `Stream` for the three test files and 38
  samples covering every container, codec and layout of the library, rates aside: we keep the
  measured ones where the reference mostly publishes the declared ones (`docs/scanner/analysis.md`, §2).
  **Chapters** too: their times as the reference keeps them (six significant digits) and
  numbered or timestamp titles cleared, identical on the 86 chapters of the samples; stored in
  `item_chapter` with the item's `chapter_source`, imported (519,141) or written by a scan from
  the file that describes the item, on the official API (`Movie.chapters`, `Episode.chapters`,
  `chapterSource`, `Chapter` as a node) and on the Plex surface (`chapterSource`, and `Chapter`
  on a lookup with `includeChapters=1`, as measured; `docs/spec-gaps.md`). Not yet:
  `requiredBandwidths`, credits detection, chapter images stored.
- 🚧 **library scans in the server**: libraries walked through `LibraryFiles` (local disks for
  now), files grouped into films, series, seasons and episodes by the reference's measured
  naming rules (`CleanName`, stacking, versions, the 18 episode patterns, dates, sidecars,
  `.plexignore`), analyzed by the scanner and stored through `SqlScanRepository` in the
  importer's own vocabulary, so a scanned film reads as an imported one on both surfaces (the
  test film's tracks, bitrates and labels as the reference's). On the official API
  `scanLibrary`, `cancelLibraryScan`, `Library.scanning`; on the Plex surface the four
  `/refresh` operations. Not yet: matching and metadata, finding a renamed or moved file again
  by size and hash, adding a version to an item already stored, a multi-episode file on every
  episode it holds, local extras and artwork, generated images, scan activities.
- 🚧 **a Jellyfin surface**, `api-jellyfin` below `/jellyfin`, turned on per user as a
  `jellyfin` extension (`signInThroughExtension`, devices in `extension_device`): discovery,
  sign-in and out, views, listings and lookups with the reference's shapes and track labels,
  latest, resume, next up, seasons and episodes, `PlaybackInfo` deciding through
  `MediaFile.readableAsIs`, direct play and HLS started through `startStream`, progress
  reports, played marks. Jellyfin Web signs in, browses and plays directly and through a
  conversion. Not yet: artwork, favorites, subtitles delivered to the client, remuxing,
  people and genre pages, search hints, the other clients tried (`docs/jellyfin/`).
  On the 35 test files, every field a file determines is the reference's — tracks, media sources,
  chapters, containers, durations, rates, filters — from what ffprobe states as printed, which the
  scan now keeps beside Plex's reading (`MediaStream.stated`, `MediaFile.stated`,
  `MediaFile.sideStreams`, `Chapter.stated`, migration `m0040`; Matroska's languages read by the
  scanner itself), and what remains differs for reasons measured in `docs/jellyfin/spec-gaps.md`
  (*Measured on the 35 files*): metadata and artwork from a provider, the scan's grouping and
  naming rules, what the reference's prober decodes.

## Reference server

A live Plex server runs at `http://127.0.0.1:32400`. It is the arbiter for every behavioral
question: capture first, then implement. For the Jellyfin surface, a reference Jellyfin runs at
`http://localhost:8096`, reached with `JELLYFIN_USER` and `JELLYFIN_PASSWORD` from the
environment, never committed. Token comes from the environment (`PLEX_TOKEN`) and is
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

- Default database engine for development (SQLite for now).
- Whether any client depends on attribute ordering within an element.
- **Plex Web without plex.tv lost its way in with the trusted network.** A browser signed in at
  `/admin` sends its session cookie on the same-origin calls of `/web`, which the transport
  accepts; whether Plex Web goes on without an `X-Plex-Token` of its own is untested.
- A library's `admin` role has little to open yet: changing a library's settings is the only
  route it gates, and that route still answers `501`.
- **A paired server shows `presence=false` on plex.tv.** The reference becomes present through a
  push connection (`/services/pubsub/servers`) we do not open, and what clients do about the
  absence is not settled.
- **One domain for every user leaves `/web` behind.** The owner wants a single published address.
  Clients that come through plex.tv present a token, which names the right server, so that
  works; the Plex Web served from the domain itself starts without a token, and `/identity`
  cannot say whose server it reached when several users share the address. Users of a shared
  address go through `app.plex.tv`.
- The ports are one trait because there is one aggregate. Splitting them is additive, and
  should happen when metadata and playback arrive — not pre-emptively.
