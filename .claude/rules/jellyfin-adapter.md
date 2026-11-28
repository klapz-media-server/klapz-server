---
paths:
  - "crates/api-jellyfin/**"
  - "crates/server/tests/jellyfin_samples.rs"
  - "docs/jellyfin/**"
  - "tools/*jellyfin*.py"
---

# The Jellyfin adapter (`crates/api-jellyfin`)

Held to the Plex adapter's rule: depends on `api` and `core`'s types only, runs every query in
process through `klapz_api::in_process` (`gql.rs`), holds no domain rule. Whether a player reads
a file as it is comes from `MediaFile.readableAsIs` (`core::stream::reads_as_is`), not from a
condition here.

- Mounted below `/jellyfin` (Jellyfin routes ignore case and would meet Plex's). Below it the
  server authenticates Jellyfin tokens only.
- Ids are a wire encoding of global ids (`id.rs`).
- A user switches its own Jellyfin connection on (`connectExtension(JELLYFIN)`); clients sign in
  with the user's own password through `signInThroughExtension`, each device keeping a token in
  `extension_device`. Every user sees one server id derived from the machine identifier, and none
  is an administrator to Jellyfin's clients.
- A teletext stream's pages share one `index`: list the first page only.

## Measuring

Reference Jellyfin at `http://localhost:8096` (`JELLYFIN_USER`, `JELLYFIN_PASSWORD`; load the
`reference-servers` skill). `tools/capture_jellyfin.py` captures it (or us with `--out`,
`--fixtures` for `tests/fixtures`), `tools/compare_jellyfin.py` diffs field by field: every
difference is either explained in `docs/jellyfin/spec-gaps.md` or a bug. After adding routes run
`python tools/gen_jellyfin_endpoints.py` (`docs/jellyfin/ENDPOINTS.md`).
