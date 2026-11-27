---
paths:
  - "crates/core/src/search.rs"
  - "crates/core/src/closest.rs"
  - "crates/core/src/completion.rs"
  - "crates/core/src/credited.rs"
  - "crates/core/src/similar.rs"
  - "crates/core/src/recent.rs"
  - "crates/core/src/page.rs"
  - "crates/db/src/item_order.rs"
  - "crates/db/src/closest_store.rs"
  - "crates/db/src/completion.rs"
  - "crates/db/src/metadata_repository.rs"
  - "crates/db/tests/search.rs"
  - "crates/db/tests/sorting.rs"
  - "crates/api/src/search.rs"
  - "crates/api/src/closest.rs"
  - "crates/api/src/filter.rs"
  - "crates/api/src/item.rs"
  - "crates/api-plex/src/listing.rs"
  - "crates/api-plex/src/search.rs"
  - "crates/api-plex/src/hub_search.rs"
  - "crates/api-plex/src/voice_search.rs"
  - "crates/api-plex/src/autocomplete.rs"
  - "crates/api-plex/src/hubs.rs"
  - "crates/api-plex/src/home.rs"
  - "crates/api-plex/src/meta.rs"
  - "crates/api-plex/src/menus.rs"
  - "crates/api-plex/src/categories.rs"
  - "crates/api-plex/src/related.rs"
  - "crates/api-plex/src/postplay.rs"
---

# Listings, filters, search and shelves

- **Top level by default**: a listing is `parent_id IS NULL` (films, series).
  `ItemFilter.depth` widens it (`LEAVES`); naming a `kind` overrides it.
- **Always windowed**, capped at `page::MAX_ITEMS` (500): listings, person pages, play queues.
  Cursors walk a collection; `Window.skip` exists only for Plex's absolute offsets.
- **A filter or sort we cannot honor is refused (`501`)**, never dropped.
- **Order**: absence first, ties in title order; titles compare by folded `sort_key` bytewise
  (not UCA — accepted divergence). Episodes default to `ItemOrder.SERIES`. A random order is a
  shuffle seeded by the ten-minute period (the API takes the seed).
- **Search** ranks every match by the reference's score (title rung of 0.2 plus 1e-4 per point
  of audience rating) in SQL, paged by an exact keyset cursor (score hundredths, rating, sort
  title, id). Matching is word prefix, every word required, any order — no fuzzy matching. An
  empty film or series shelf fills through a person (`core::credited`).
- **Voice search** ranks by the distance it publishes (`core::closest`: sort titles, one article
  dropped, folded letters, bytes), read in length bands from `item_folded_title`; terms over 256
  bytes refused.
- **Autocomplete** uses the phrase-at-a-word rule (`core::completion`), `queryRange` in bytes.
- **Related films** are an approximation by shared genres, actors, directors (`core::similar`).
- Recently added groups episodes by a measured 20-hour batching rule (`core::recent`).
- Measured rules, counts and remaining differences: `docs/plex/spec-gaps.md`; check with
  `tools/compare_items.py`.
