---
paths:
  - "crates/core/src/watch.rs"
  - "crates/core/src/history.rs"
  - "crates/core/src/user_rating.rs"
  - "crates/core/src/playlist.rs"
  - "crates/core/src/play_queue.rs"
  - "crates/core/src/collection.rs"
  - "crates/core/src/marker.rs"
  - "crates/core/src/extra.rs"
  - "crates/core/src/edit.rs"
  - "crates/db/src/watch_repository.rs"
  - "crates/db/src/playlist_repository.rs"
  - "crates/db/src/play_queue_repository.rs"
  - "crates/db/src/collection_repository.rs"
  - "crates/db/src/marker_store.rs"
  - "crates/api/src/watch.rs"
  - "crates/api/src/history.rs"
  - "crates/api/src/user_rating.rs"
  - "crates/api/src/favorite.rs"
  - "crates/api/src/playlist.rs"
  - "crates/api/src/play_queue.rs"
  - "crates/api/src/collection.rs"
  - "crates/api/src/marker.rs"
  - "crates/api/src/edit.rs"
  - "crates/api-plex/src/scrobble.rs"
  - "crates/api-plex/src/history.rs"
  - "crates/api-plex/src/rate.rs"
  - "crates/api-plex/src/playlists.rs"
  - "crates/api-plex/src/play_queues.rs"
  - "crates/api-plex/src/collections.rs"
  - "crates/api-plex/src/markers.rs"
  - "crates/api-plex/src/edit.rs"
  - "crates/api-plex/src/user_state.rs"
---

# Viewer state, lists and edits

- **Watch state, ratings and favorites are the viewer's and the work's**: keyed by user id and
  `guid`, so every copy of a film is watched at once. No caller, no state. Rules for positions,
  viewings and parents' counts are in `core::watch`.
- **A viewing is written once, with the state it explains**: `WatchRepository::record_viewings`
  only, called only from `core::watch` (a playback crossing 90%, a mark watched). Unwatching
  leaves history alone. History is per viewer and per item (the copy watched); a user reads its
  own, an owner everyone's.
- **Ratings** are 0–10 (anything else `400`), per viewer and work, timestamped so an older
  change — a clearing included — is ignored.
- **Playlists** are their viewer's alone (another's is `404`), ids start at 1 apart from items';
  positions halve on a move. Smart ones keep `Library.items`' own input as their rule.
- **Play queues**: one per player and type, rebuilt from their kept source
  (`play_queue_source`) by `core::play_queue`'s rules; at most 500 entries; deleted after 7 days.
- **Collections** are a library's: read by its readers, changed by its admins and owners; named
  by their own title; listed by `type=18` and `/collections`, never mixed into `/all`.
- **Markers**: the work's (intro, credits) for library admins and owners, bookmarks for their
  keeper only; published to every reader. No detection yet.
- **Edits** (`updateMovie`…): library admins and owners, per-field locks in `item_field_lock`; a
  title edit goes through `db::write::update_item` so the indexes follow.
- Measured behaviors of each Plex route: `docs/plex/spec-gaps.md`.
