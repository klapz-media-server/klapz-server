---
paths:
  - "crates/db/**"
  - "tools/reimport.py"
  - "tools/dump_plex_schema.py"
  - "docs/plex/db-schema.md"
---

# Storage (`crates/db`)

Implements the ports `core` declares (`*Repository` traits). SeaORM over any backend: SQLite is
one engine among several, so no SQLite-only SQL.

- **Migrations** are `migration/mNNNN_name.rs`, registered at the end of the list in
  `migration.rs`; take the next number, never edit a shipped one.
- **Our schema, not Plex's.** `docs/plex/db-schema.md` is a reference. No packed key/value
  columns (`extra_data`), no denormalized display columns (`tags_genre`), epoch seconds rather
  than `datetime`, no column named `index` or `key` (reserved in MySQL).
- **Derived indexes have one write path.** Items only through `write::insert_items` and
  `write::update_item` (they maintain `item_title_word`, `item_title_key`,
  `item_folded_title`), tags only through `write::insert_tags` (`tag_word`). A test fixture that
  inserts rows directly then matches nothing.
- **Windows**: every collection query takes a `Window` and returns a `Page`, translated into
  `WHERE`/`ORDER BY`/`LIMIT`, over-fetching one row to know whether a next page exists. Sort keys
  with absent values go through `item_order`. Titles compare by the folded `sort_key`, bytewise.
- **Watch state and ratings are keyed by `guid`** (the work, every copy at once); history by
  item. Viewings are written only by `WatchRepository::record_viewings`, with the state, in one
  transaction.
- **Settings rows** (`server_setting`) hold only a value an owner chose, canonical; deleting the
  row restores the default.

## The importer (`import/`)

Reads a real Plex database (`db/`, not versioned, personal data) read-only, idempotently. It
copies no accounts. Nothing derived from it is committed without scrubbing tokens, machine ids
and personal paths. For schema questions read the empty template
`files/Resources/com.plexapp.plugins.library.db`, not the 2.4 GB one. Re-importing over a used
database: `python tools/reimport.py`, server stopped.

Field completeness: for each entity on the read path, every populated column of the reference
either maps to something we emit or is excluded with a reason.
