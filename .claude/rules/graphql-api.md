---
paths:
  - "crates/api/**"
  - "docs/schema.graphql"
  - "docs/introspection.json"
---

# The official API (`crates/api`)

GraphQL at `/graphql` is the only data source of both adapters. A field missing here is a
field neither Plex nor Jellyfin can serve: add it here first. `api` depends on `core` only.

## Conventions, enforced

- **Relay.** Every identifiable type implements `Node` with an opaque global id (base64 of
  `Type:key`, `GlobalId`), refetchable by `node(id:)`. No other identifier is published.
- **Lists are connections** (`edges`/`node`/`cursor`, `pageInfo`), never bare arrays. Windows
  come from `schema::window_for` / `id_window`, capped by `page::MAX_ITEMS`.
- **Link entities, not ids**: `HistoryEntry.library: Library`, not `libraryId`. Only a delete
  payload's `deleted…Id` is an id.
- **What describes an entity lives on it** (`PlaybackSession.terminatedAt`), not in a root field
  or a type of its own.
- **Mutations are Relay**: an input with `clientMutationId`, a payload with `userErrors` and a
  typed error code enum (`LibraryErrorCode`…). Expected failures are `userErrors`, not errors.
- **Every field, argument and enum value has a description**: `tests/schema_contract.rs` walks
  introspection.
- **Per-parent resolution goes through a DataLoader** (`loader.rs`): ten libraries cost one
  `IN (…)` query.
- **Scalars**: `Int64` (decimal string, `int64.rs`) for anything past 32 bits — sizes,
  `contentChangedAt`, microseconds; durations in ms and bitrates stay `Int`. `DateTime` is
  RFC 3339 UTC `Z` to the second (`date_time.rs`); storage keeps Unix seconds; a release date is
  its day's midnight UTC.
- **Measurements, not display forms**: bits per second, 23.976, unsnapped aspect ratios, full
  precision ratings. Rounding, labels and ladders are the adapters'.

## Rights

The resolvers that lead to a library's content check the `Principal` against `core::access`. No
principal sees nothing (`/graphql` admits anonymous requests so a browser can sign in;
`nobody_reads_nothing_from_any_entry_point` guards it). What a user was not given answers as if
it did not exist. Owner-only arguments are marked `OWNER_ONLY`.

## After any schema change

```sh
cargo run -p klapz-api --example sdl > docs/schema.graphql
cargo run -p klapz-api --example introspect > docs/introspection.json
```
