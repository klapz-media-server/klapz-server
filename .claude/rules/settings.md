---
paths:
  - "crates/core/src/settings.rs"
  - "crates/db/src/settings_repository.rs"
  - "crates/api/src/settings.rs"
  - "crates/api/src/library.rs"
  - "crates/api/tests/server_settings.rs"
---

# Server settings

- Every setting is declared in `settings::DEFINITIONS`: key, kind (`Choice`, bounded `Integer`, perhaps
  `optional` — empty for none, `Boolean`, `Address`), default, description. A test holds keys to the grammar and defaults to
  their kind.
- Key grammar: dotted lower-case words, at least two, each `[a-z][a-z0-9_]*`, ≤ 96 chars, the
  machinery first (`transcoder.h264.preset`); `general.*` is the server's as a whole
  (`general.public_url`).
- Undeclared keys are refused (`UNKNOWN_SETTING`), never stored. A stored value the setting no
  longer accepts gives way to the default.
- Owners only: `Server.settings(namespace:)` is empty for others; `updateServerSettings` is
  all-or-nothing, ≤ 100 per change.
- A setting declared `per_library` may also hold a value for one library (`library_setting`),
  which that library follows instead of the server's; a null in `updateLibrarySettings` makes it
  follow the server again. Owners only (`Library.settings`, `isInherited`, `serverValue`); a
  server-only setting is refused there (`SERVER_ONLY`).
- `analysis.*` switches and sizes the analysis' tasks (see the scan rule).
- Read at every use, so a change applies without restart. A child process gets the engine's own
  type, never the stored text.
- What is needed before the database is readable (bind address, database URL, secrets,
  directories, binaries) stays in `KLAPZ_*` environment variables (`docs/configuration.md`).
