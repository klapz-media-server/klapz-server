# GraphQL model for library items

Design note. One storage table, several GraphQL types — the mapping layer absorbs the
difference, which is what makes the single-table decision free rather than a compromise.

## Why types rather than one `MetadataItem`

A single polymorphic type would have to expose every field any kind might carry, and clients
would learn by trial which ones are null for which kind. The reference library makes the cost
concrete:

| Field | Movies | Episodes | Shows / Seasons |
| --- | --: | --: | --- |
| `studio` | 61% | 0% | — |
| `tagline` | 47% | 0% | — |
| `rating` | 44% | 0% | — |
| media attached | 100% | 100% | **never** |

`Show` and `Season` carry no media at all: 0 of 4,002 shows and 0 of 9,736 seasons have a
`media_items` row, while all 18,332 movies and all 138,981 episodes do. A `versions` field on
`Show` would be a field that is always empty — worse than absent, because it invites the client
to ask.

So: an interface for what is genuinely common, concrete types for the rest, and navigation that
names what it returns.

## The interface

```graphql
"""
Anything that appears in a library listing.
"""
interface MetadataItem implements Node {
  id: ID!
  databaseId: Int!
  "The library this item belongs to. The same work in two libraries is two items."
  library: Library!
  title: String!
  titleSort: String
  summary: String
  addedAt: Int!
  updatedAt: Int!
}
```

Implementing `Node` on the interface is what lets a client refetch any item through
`node(id:)` without knowing its kind.

## The concrete types

```graphql
type Movie implements MetadataItem & Node {
  # …interface fields
  year: Int
  releasedOn: String          # YYYY-MM-DD
  studio: String
  tagline: String
  contentRating: String
  rating: Float               # critic
  audienceRating: Float
  durationMs: Int
  versions: [MediaVersion!]!  # 14% of items have more than one
}

type Show implements MetadataItem & Node {
  year: Int
  studio: String
  contentRating: String
  seasons(first: Int, after: String, last: Int, before: String): SeasonConnection!
  seasonCount: Int!
  episodeCount: Int!
  # no `versions`: a show is not playable
}

type Season implements MetadataItem & Node {
  number: Int!
  show: Show!
  episodes(first: Int, after: String, last: Int, before: String): EpisodeConnection!
  episodeCount: Int!
}

type Episode implements MetadataItem & Node {
  number: Int!
  season: Season!
  "The show, reachable directly rather than through the season."
  show: Show!
  releasedOn: String
  durationMs: Int
  versions: [MediaVersion!]!
}
```

`Episode.show` is a deliberate convenience: Plex exposes `grandparentTitle` for exactly this,
because a client rendering an episode row needs the show name without a second hop. It costs
one DataLoader, not a second query per row.

## The media chain, renamed

Plex calls these Media, Part and Stream. The names leak its implementation — "part" means
nothing to a client, and it is almost never plural (244,064 of 244,092 media have exactly one).
The official API names them for what they are, and the Plex adapter maps back:

```graphql
"A particular encoding of an item — a 1080p cut and a 4K cut are two versions."
type MediaVersion {
  databaseId: Int!
  width: Int
  height: Int
  videoCodec: String
  audioCodec: String
  audioChannels: Int
  bitrate: Int
  container: String
  durationMs: Int
  frameRate: String          # a string: "24p", not a number
  files: [MediaFile!]!       # usually one; several for a stacked release
}

type MediaFile {
  databaseId: Int!
  sizeBytes: Int
  container: String
  durationMs: Int
  streams: [MediaStream!]!
  # the filesystem path is deliberately NOT exposed
}

type MediaStream {
  databaseId: Int!
  kind: StreamKind!          # VIDEO | AUDIO | SUBTITLE
  codec: String
  language: String
  channels: Int
  isDefault: Boolean!
  isForced: Boolean!
}

enum StreamKind { VIDEO, AUDIO, SUBTITLE }
```

**The path is not a field.** `media_part.path` is the one column that becomes a filesystem
access, and publishing it would hand every client the server's directory layout — and a target
for the path-traversal rules in `security.md`. Playback takes a file id and resolves the path
server-side.

## Browsing a library

```graphql
type Library {
  # …existing fields
  items(
    first: Int, after: String, last: Int, before: String
    sort: ItemSort = TITLE
    direction: SortDirection = ASCENDING
    search: String
  ): MetadataItemConnection!
}

enum ItemSort { TITLE, ADDED_AT, RELEASED_ON, YEAR, RATING }
enum SortDirection { ASCENDING, DESCENDING }
```

`items` is polymorphic: a movie library yields `Movie`, a show library yields `Show`. Clients
use inline fragments, which is also what maps cleanly onto the Plex adapter's `/all` endpoint.

Typed accessors (`library.movies`, `library.shows`) are deliberately **not** proposed. They
would be nicer to type by hand but duplicate every argument, and the kind is already known from
`library.kind`. Add them if real client code proves the fragments painful — not before.

`sort` and `direction` are enums rather than strings because they become SQL identifiers, which
cannot be bound as parameters. An exhaustive match is the only form that is both
injection-proof and verifiable by reading (`security.md`).

## Search

Starts as a portable prefix match on `title` and `title_sort`, served by the existing
`(library_section_id, kind, title_sort, id)` index.

Plex spends 28 of its 84 tables and 1,100,571 rows on FTS4 and spellfix to get
typo-tolerance and relevance ranking. That is the least portable feature in a database: SQLite
uses virtual FTS5 tables, PostgreSQL `tsvector` with GIN, MySQL `FULLTEXT` — three incompatible
dialects, none expressible through SeaORM. Committing to it now means giving up the
engine-swapping the ORM was chosen for, or writing three implementations.

So search lives behind the repository port, and a real full-text implementation can replace the
prefix match per backend later, once we know whether multi-backend is still a goal.

## DataLoaders required

Each of these resolves per parent, so each needs batching or the N+1 problem returns at a scale
where it matters — a page of 50 episodes would otherwise issue 200 queries:

| Loader | Keyed by | Feeds |
| --- | --- | --- |
| `VersionsLoader` | item id | `Movie.versions`, `Episode.versions` |
| `FilesLoader` | version id | `MediaVersion.files` |
| `StreamsLoader` | file id | `MediaFile.streams` |
| `ChildrenCountLoader` | item id | `Show.seasonCount`, `Season.episodeCount` |
| `ItemLoader` | item id | `Season.show`, `Episode.season`, `Episode.show` |
| `LibraryLoader` | section id | `MetadataItem.library` |

## Two things to handle before importing

**Two rows in the reference library have a dangling `parent_id`** — one season and one episode
point at a row that does not exist. A real foreign key will reject them. The importer must
either skip them with a warning or null the parent, and it must say which; silently dropping
rows is how an import becomes untrustworthy.

**Extras are 30,260 items** of `metadata_type = 12` with media attached, and they are not
modeled here. Plex exposes them through `/library/metadata/{id}/extras` rather than in listings.
Out of scope for the first pass, but they exist and the importer needs an explicit decision
rather than an accident.
