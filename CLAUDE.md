# klapz

A media server reimplementing the Plex Media Server API, in Rust. The goal is **API
compatibility**: clients and tools that talk to a PMS must talk to klapz unmodified. A second
adapter serves the Jellyfin API.

## Language

**Everything in this repository is written in English**: code, identifiers, comments, logs,
error messages, test names, commit messages, documentation, and every file written for agents
(`CLAUDE.md`, `.claude/rules/`, `.claude/skills/`). Whatever language the conversation is in.

## Where the knowledge is

Rules for a domain live in `.claude/rules/*.md`, each scoped by `paths:` to the code it governs
and loaded when those files are touched. This file holds only what applies everywhere. When you
settle something in a domain, write it in that domain's rule, tersely; keep measurements and
history in `docs/`.

| Rule | Domain |
| --- | --- |
| `graphql-api` | `crates/api`: Relay conventions, scalars, rights, schema regeneration |
| `plex-adapter` | `crates/api-plex`: endpoint workflow, wire rules settled by capture, deliberate answers |
| `jellyfin-adapter` | `crates/api-jellyfin`: surface, ids, measuring against the reference |
| `storage` | `crates/db`: migrations, schema design, single write paths, importer |
| `access` | accounts, authentication, sessions, CORS, pairing |
| `playback` | streams, playback sessions, their lifecycle |
| `transcoder` | `crates/transcoder`: ffmpeg supervision, images, backdrops, sidecars |
| `scan` | `crates/scanner`, `core::scan`: libraries, roots, walking, analysis |
| `metadata` | `crates/agents`, source metadata, artwork |
| `search-and-listing` | listings, filters, sorts, search, shelves |
| `viewer-state` | watch state, history, ratings, playlists, play queues, collections, markers, edits |
| `settings` | `core::settings`, server settings |
| `languages` | `core::language`: the one language type, its tags, its tables |

Skills: `rust` (load before writing any Rust), `reference-servers` (load before any capture or
comparison against a reference server).

Reference material is split by surface: everything measured on or taken from Plex is in
`docs/plex/`, everything about Jellyfin in `docs/jellyfin/`. Never mix the two in one file.

| Plex (`docs/plex/`) | Contents |
| --- | --- |
| `openapi.json` | Official PMS 1.2.3 spec: source of truth for request shapes, wrong in places. |
| `spec-gaps.md` | Where the Plex spec and the reference PMS disagree, every measured behavior and deliberate divergence. Hand-verified. |
| `wire-format.md` | Per-field JSON types from live captures. Generated (`tools/capture_plex.py`). |
| `ENDPOINTS.md` | Status of all 260 Plex operations. Generated (`tools/gen_endpoints.py`). |
| `endpoint-probe.md` | Which spec `GET` operations the reference answers. Generated (`tools/probe_endpoints.py`). |
| `scanner/` | How Plex discovers (`discovery.md`), matches (`matching.md`) and analyzes (`analysis.md`) files. |
| `db-schema.md` | Plex's library schema, as a reference only. Generated (`tools/dump_plex_schema.py`). |

| Jellyfin (`docs/jellyfin/`) | Contents |
| --- | --- |
| `openapi.json` | The reference Jellyfin's (12.4.0) own OpenAPI document. |
| `spec-gaps.md` | What the Jellyfin surface measured on the reference, and where it deliberately differs. Hand-verified. |
| `ENDPOINTS.md` | Jellyfin operations served below `/jellyfin`. Generated (`tools/gen_jellyfin_endpoints.py`). |

| klapz | Contents |
| --- | --- |
| `docs/schema.graphql`, `docs/introspection.json` | The official API. Generated. |
| `docs/configuration.md` | Running, importing, every `KLAPZ_*` variable, packaging. |

| Not versioned | Contents |
| --- | --- |
| `files/` | A Plex install for reverse engineering: `Resources/com.plexapp.plugins.library.db` (empty template schema), `Resources/Profiles/` (transcode profiles), the bundled Plex Web. |
| `db/` | A real Plex library database. Personal data: read-only, nothing derived committed unscrubbed. |

## Reference servers

Plex at `http://127.0.0.1:32400` (`PLEX_TOKEN`), Jellyfin at `http://localhost:8096`
(`JELLYFIN_USER`, `JELLYFIN_PASSWORD`). They are the arbiter of every behavioral question:
**capture first, then implement**; the spec alone cannot drive this project. Credentials come
from the environment and are never committed; `captures/` is gitignored.

## Settled decisions

Each comes from the repo owner and is not renegotiated without them.

### Scope

- Server only. No client, no stand-in for `plex.tv`: pairing talks to the real one.
- **No paywall.** Everything Plex reserves for a subscription ships. No `if subscription`
  branch, no premium flag, no entitlement check; a subscription field a client reads answers
  the value that unlocks the feature.

### Architecture

- **Layers: `db` → `core` → `api` (GraphQL) → `api-plex` / `api-jellyfin`.** `core` declares
  the storage ports and knows nothing of HTTP or GraphQL; `db` implements them; `api` depends
  on `core` only; adapters on `api` only, never on `db` or the ports. `transcoder` is to media
  what `api` is to data; `scanner` runs ffprobe/ffmpeg-plex; `server` wires everything and owns
  HTTP authentication.
- **GraphQL at `/graphql` is the official API, and the adapters' only data source.** Every
  adapter handler runs a query in process. Nothing reaches Plex or Jellyfin that GraphQL does
  not serve.
- **No domain logic in a binding**; no service layer above the ports. Domain rules live in
  `core` (`core::access` for rights, applied by the schema).
- **A filter we cannot honor is refused, never dropped** (`501`). Silence is for what we were
  never asked, not for what we cannot do.
- **Every collection port takes a `Window` and returns a `Page`**, capped at `page::MAX_ITEMS`:
  nothing ever asks storage for everything.
- **A derived index has one write path** (`db::write`).
- **No path from a client.** Paths are published (`MediaFile.path`, `Part.file`), never
  accepted: endpoints resolve files from ids. The only exceptions are an owner's library roots
  and `serverDirectories` (see `scan`); neither is a precedent.
- **The official API publishes measurements; adapters publish display forms** (kbps, `24p`,
  snapped aspect ratios, one-decimal ratings).
- **Every action on a media file goes through `klapz-ffmpeg`** (our ffmpeg-plex build, and its
  ffprobe as `klapz-ffmpeg -probe`), run as a child process by `klapz-scanner` or
  `transcoder`. Probing, analysis, hashing, decoding, transcoding, extracting a track, a
  subtitle, an attachment, a cover or a frame: all of it. **The server never opens a media file
  to look inside it**: no reading of headers or bytes to learn anything, no container or codec
  crate (`mp4`, `matroska`, `symphonia`, libav bindings…), no hand-written parser, not even
  "just a few bytes". A hostile file must only ever reach a child process, never the server's
  address space with its database and tokens. A need `klapz-ffmpeg` does not meet is a patch to
  ffmpeg-plex, not code in the server.
  What the server may do with a media file, and nothing more: list it and read its name, size
  and times (`LibraryFiles`), and send its bytes unchanged — direct play and the transcoder's
  source callback (`ServeFile`). Images through the `image` crate (`/photo/:/transcode`) are
  the one standing exception, not a precedent.
- **Every request is a user's.** No operator, admin token or trusted network; no caller sees
  nothing; what a user was not given answers as if it did not exist.
- **Database through SeaORM**; SQLite is one backend, not the design. Our schema is our own
  design: the Plex database is a reference, not a schema to clone.

### Security

First-class (`.claude/skills/rust/references/security.md`). The three classes that matter:
path traversal on file serving, ffmpeg argument injection, token leakage (Plex carries tokens in
query strings: never log them).

## Code quality

Full detail in the `rust` skill. Light comments explaining why or how, never what. No
single-caller abstractions, no empty doc comments, precise names, no `unwrap()`/`expect()`/
`panic!` outside tests. The code must not read as generated.

## Testing

- Unit tests colocated with the code under test.
- Endpoint tests check each route answers *correctly*; `crates/api-plex/tests/spec_coverage.rs`
  asserts no spec operation 404s.
- Fixtures are real captured responses, never invented data.

## Generated files

Never edited by hand; regenerate in the same change, and after a merge regenerate on the merged
tree instead of resolving conflicts:

- `docs/schema.graphql`, `docs/introspection.json`: `cargo run -p klapz-api --example sdl` /
  `--example introspect`, after any schema change;
- stub routes: `python tools/gen_stub_routes.py` then `cargo fmt`, when a stub becomes real;
- `docs/plex/ENDPOINTS.md`: `python tools/gen_endpoints.py`, then diff it;
- `docs/jellyfin/ENDPOINTS.md`: `python tools/gen_jellyfin_endpoints.py`;
- `crates/core/src/language/table.rs`, `crates/api/src/language/values.rs`:
  `python tools/gen_languages.py`;
- `crates/api-plex/src/language_names.rs`: `python tools/gen_language_names.py`.

## Closing procedure

No Rust-touching task is finished until these pass, in order:

1. `cargo fmt --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`, every warning fixed. An
   `#[allow]` only with its justification in a comment.
3. `cargo test --all-features`. A failing test means fix the code, not the test — unless the
   test is wrong, and then say how.
4. No `unwrap()`, `expect()` or `panic!` added outside `#[cfg(test)]`.
5. Summarize the commands run and their actual results, failures included.

## Running it

```sh
cargo run -p klapz-server -- import db/com.plexapp.plugins.library.db   # seed from Plex
KLAPZ_ADMIN_PASSWORD='a long passphrase' cargo run -p klapz-server      # serve on 127.0.0.1:32500
```

Variables, `.env` lookup, library and scan commands, tunnels and packaging:
`docs/configuration.md`.
