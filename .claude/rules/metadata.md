---
paths:
  - "crates/agents/**"
  - "crates/core/src/source_metadata.rs"
  - "crates/core/src/source_metadata/**"
  - "crates/db/src/source_metadata_repository.rs"
  - "crates/db/tests/source_metadata.rs"
  - "crates/api/src/source_metadata.rs"
  - "crates/api/src/artwork.rs"
  - "crates/api/src/theme.rs"
  - "crates/server/src/artwork.rs"
  - "crates/api-plex/src/matching.rs"
  - "crates/api-plex/src/artwork.rs"
  - "crates/api-jellyfin/src/images.rs"
  - "docs/plex/scanner/matching.md"
---

# Source metadata and artwork

- Films match on TMDB, series on TVDB (`klapz-agents`, behind `core::source_metadata::MetadataSource`).
  Credentials are built in (`agents::credentials`, the repo owner's decision); `KLAPZ_TVDB_PIN`
  never is. Log which provider has a key and from where, never the key.
- A work is stored once as a source record in every language; its items show it in their
  library's language, under their field locks (`item_field_lock`). One image per kind per item,
  chosen at matching — not by `X-Plex-Language`.
- Identity: an imported item keeps its `plex://` guid, else `klapz://{kind}/{agent}-{id}`;
  viewers' state follows an item whose identity changes.
- Our match addresses are `{agent}://{id}` (`tmdb://603`), which `/match` takes back.
  `/unmatch` restores `local://{id}`, drops links, identifiers and images, keeps texts.
- Images are kept on disk (`KLAPZ_IMAGES_DIR`, `provider/ab/{sha256(url)}.{ext}`) with their
  BlurHash and backdrop colors. Jellyfin's image routes serve without a token only for the right
  `tag`.
- Matching runs after every scan and through `klapz-server match`.
- A series' theme music is found at matching, as the reference finds it: a `theme-?[0-9]?` file
  beside its episodes (their directory, its parent and grandparent, never the library's root)
  wins over Plex's service's for its TVDB id (`tvthemes.plexapp.com/{tvdb}.mp3`, the one host
  fetched). Either is copied into the artwork store (`themes/ab/{sha256}.{ext}`), as the
  reference copies a local asset into its bundle, so no route serves a library file by its path.
  A local theme may be any format browsers play (`mp3`, `m4a`, `aac`, `flac`, `ogg`, `oga`,
  `opus`, `wav`), the repo owner's decision, where the reference reads `mp3` alone; its type is
  its extension's, nothing reads its content. `Show.theme` on the official API; `/unmatch`
  forgets the service's and keeps a local one (`docs/plex/spec-gaps.md`).
