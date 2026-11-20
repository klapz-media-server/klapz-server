# Database (SeaORM)

## Contents

- [Why an ORM here](#why-an-orm-here)
- [Crate layout](#crate-layout)
- [The db/core boundary](#the-dbcore-boundary)
- [Staying backend-agnostic](#staying-backend-agnostic)
- [Migrations](#migrations)
- [Our schema, not Plex's](#our-schema-not-plexs)
- [Connection handling](#connection-handling)
- [Testing](#testing)

## Why an ORM here

The engine must be swappable: SQLite is one backend, not the design. SeaORM gives one API over
PostgreSQL, MySQL and SQLite, with built-in migrations, and it is async end to end — so no
`spawn_blocking` around queries, unlike a `rusqlite`-based design.

The tradeoff accepted along with it: a query builder is more verbose than raw SQL, and complex
queries read worse than their SQL equivalent. That is the price of portability, and it is paid
knowingly. Where a query genuinely cannot be expressed, see
[Staying backend-agnostic](#staying-backend-agnostic) — but reach for that last, not first.

## Crate layout

```
crates/db/src/
  entity/        SeaORM entities, one module per table
  migration/     versioned migrations
  query/         query functions grouped by domain, returning core-shaped data
  lib.rs         public surface: connection setup and the query modules
```

Entities are `pub(crate)` or at most `pub` within `db`, never re-exported from it. The moment an
entity type appears in `core`'s signatures, the layering is gone: `core` would then depend on
table structure, and a schema change would ripple through the domain and into the bindings.

## The db/core boundary

`db` returns data shaped for the domain, not rows. A query function is the translation point.

```rust
// crates/db/src/query/library.rs

/// Loads the sections a given account may see.
///
/// Permission filtering happens in the query rather than after it: a forgotten filter is
/// invisible in review, while a missing join fails loudly in tests.
pub async fn sections_for(
    db: &DatabaseConnection,
    account: AccountId,
) -> Result<Vec<LibrarySection>, DbError> {
    // ...
}
```

`LibrarySection` here is a domain type, not `entity::library_sections::Model`. The mapping is
written once, in `db`, and that is the only place that knows about columns.

This is also what keeps `core` testable without a database: its functions take domain types, so
unit tests construct them directly.

## Staying backend-agnostic

The point of the ORM is lost the moment backend-specific SQL creeps in. Three rules keep it:

**No raw SQL on the normal path.** Use the entity API and `sea_query`. If a query needs raw SQL,
it goes behind a function that dispatches per backend, and that function is the documented
exception — not a pattern to copy.

**No backend-specific column types.** No PostgreSQL arrays, no MySQL `ENUM`, no SQLite type
affinity tricks. Domain enums are stored as small integers or short strings and converted in the
entity definition.

**No reliance on backend behaviors.** Notably: SQLite is permissive about types and has no real
`ALTER COLUMN`; PostgreSQL is strict and case-sensitive about quoted identifiers; collation and
case-insensitive sorting differ across all three. Sorting matters here — the Plex API sorts by
`titleSort` — so sort explicitly rather than relying on a default collation that will not match
between engines.

Run the test suite against at least two backends in CI. A single-backend suite does not prove
portability, it only postpones discovering the lack of it.

## Migrations

Migrations are versioned, forward-only, and live in `crates/db/src/migration/`. Each has an
`up` and a `down`; `down` is written even when nobody expects to use it, because writing it is
how you notice a migration is not reversible.

Never edit a migration that has shipped. Add a new one. An edited migration produces two
different schemas bearing the same version, which is the hardest class of data bug to diagnose.

Since the schema is ours and the project is young, prefer getting the shape right over
accumulating corrective migrations — but once a migration is in a release, it is frozen.
Nothing has shipped yet, so an unreleased migration may still be rewritten in place: the local
`open-plex.db` then has to be deleted and imported again (`tools/reimport.py` keeps what was made
here).

## Measuring queries

A claim about performance or batching is proven from the SQL log and `EXPLAIN QUERY PLAN`, and
the number goes into the migration or the commit: "320 ms per page without the index, 0.5 ms with
it"; "50 films and their media chain in 6 queries, not 357". Time it on the real library — a
copy of `open-plex.db` — since the test fixtures never show a slow query.

SQLite traps met here:

- the default `LIKE` is case-insensitive, so it cannot use a binary index for a prefix. Filter
  on a folded column (`sort_key`) as an explicit range, `>= prefix AND < upper`: search went
  from 20 ms to 0.043 ms, with no wildcard to escape;
- filtering on a folded column while ordering on a raw one forces a scan: one folded column
  serves both;
- `range` is reserved in MySQL, `index` everywhere: neither is a column name.

## Reading Plex's database

- **Never default a required column.** `unwrap_or(0)` on a column an edited `SELECT` no longer
  returned imported `hidden` and `content_changed_at` as zero for every row; propagating the
  error showed `ColumnNotFound` at once. A defaulting combinator on required data is an
  `unwrap()` that lies.
- **Qualify every column in a join.** Plex's `tags` has a `metadata_item_id` of its own, so a
  bare one is ambiguous next to `taggings`.
- **A synthetic Plex database** for a test needs the `naturalsort` collation registered and
  `user_fields` set to `''` (not null), as the importer expects them.
- Plex's `extra_data` values are all JSON strings, and `-1` stands for an unknown year: it is
  not imported.

## Our schema, not Plex's

`docs/plex-db-schema.md` documents the real Plex schema as a *reference*. It is there to learn
how Plex models entities and which columns feed which API attributes — not to be reproduced.

What that reference shows us we do **not** want:

- packed key/value columns (`extra_data`, `user_fields`) that cannot be queried or indexed;
- denormalized display columns (`tags_genre`, `tags_star`) duplicating what `taggings` models;
- `datetime` storage when the API speaks epoch seconds, forcing conversions in both directions;
- a column named `index`, which is a reserved SQL word.

What we do keep is the *modeling*, which is sound: an item hierarchy through `parent_id`, media
decomposed into item → part → stream, and tags normalized through a join table.

Conversions between our storage representation and the Plex wire format belong in the binding,
never in `db`. If `db` starts producing epoch seconds because Plex wants them, the next binding
inherits a Plex decision for no reason.

## Connection handling

One pool, created at startup, carried in the application state. SeaORM's `DatabaseConnection` is
already a pool and is cheap to clone — do not wrap it in an `Arc<Mutex<…>>`, which would
serialize every query through one lock.

Size the pool deliberately. For SQLite, concurrent writers are the constraint: enable WAL mode
and keep a single writer, because the default rejects concurrent writes with a busy error under
load rather than queuing.

Set statement timeouts where the backend supports them. A query with no bound is a resource limit
missing (see `security.md`).

## Testing

Query functions are tested against a real database, not a mock. A mock of a query builder tests
the mock.

SQLite in memory is the fast path for the test suite: schema created by running the migrations,
which also means every test run exercises them. Seed with fixtures derived from the reference
database — scrubbed of personal data before being committed.

`core` needs no database in its own tests, because `db` hands it domain types. That is the
practical payoff of the boundary: the layer with the most logic is the cheapest to test.
