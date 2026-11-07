# Schema proposal: metadata and media

Design note for the next migration — the one that carries the library contents. **Not
implemented yet**; this exists to be reviewed before 2 million rows are committed to a shape.

Derived from the real Plex schema in `docs/plex-db-schema.md` and from measurements against the
reference library: 204,388 metadata items, 244,078 media, 244,092 parts, 947,346 streams.

## What Plex gets right, and we keep

**A single self-referencing item table with a type discriminator.** `movie`, `show`, `season`,
`episode`, `artist`, `album`, `track` all live in one table with `parent_id`. The API mirrors
this exactly — `parentRatingKey`, `grandparentRatingKey`, `/children`, `/grandchildren` — so
splitting into per-type tables would mean reassembling the hierarchy on every read.

**The item → media → part → stream chain.** One item can have several *versions* (a 1080p and a
4K cut), a version can span several *files* (stacked discs), and a file carries several *tracks*
(video, audio, subtitles).

Two of those levels are rarely multi-valued, and the honest figures matter. Among the items we
import, only 8 movies and 88 episodes have more than one version, and 244,064 of 244,092 media
have exactly one file. The 27,230 items with three versions are *extras*, which are out of
scope — an earlier draft of this note quoted that as "14% of items" and was wrong, because the
statistic was computed over the whole table and attributed to the subset.

Both levels stay anyway, for the same reason: the Plex surface exposes `<Media>` and `<Part>`
with their own identities, and collapsing either would make the items that do use them
unrepresentable. Streams, by contrast, are genuinely many: 773,829 of them across 157,414
files.

**Watch state keyed on `(account, guid)` rather than on the item.** This looks odd until you see
the consequence: the same film exists in up to six libraries here (French, English, 3D, UHD
variants sharing a `Multi` folder), and keying on `guid` means watching it once marks it watched
everywhere. Keying on the item id would give the same person six independent progress bars for
one film. We keep Plex's choice deliberately.

**Normalized tags through a join table, with an ordering index and a role text.** `taggings`
carries `index` (billing order for a cast list) and `text` (the character name). Both are needed
to render a cast list correctly.

## What we reject

| Plex does | Why we do not |
| --- | --- |
| `extra_data` packed key/value blobs on `media_items`, `media_parts`, `media_streams`, `tags`, `taggings`, `metadata_item_settings` | Unqueryable, unindexable, version-fragile. Every field we need becomes a column. |
| `metadata_items.tags_genre`, `tags_director`, `tags_star` as comma-joined strings | Duplicates what the join table already models, for display speed we do not need. |
| Columns named `index` and `default` | Reserved SQL words. We use `position` and `is_default`. |
| `tags.metadata_item_id` | A tag table pointing at one item contradicts the join table that already exists. Tag values are global; attachment belongs in the join. |
| Everything is a tag: `tag_type` 312 = Thumb, 313 = Art, 314 = `imdb://…`, 315 = collection guid | External identifiers and artwork providers are not tags. They get their own tables. |
| `tag_type` and `stream_type_id` as bare integers | Magic numbers. Stored as text discriminants, mapped at the storage boundary like `library_section.kind` already is. |
| `hints`, `settings`, `proxy_type`, `channel_id`, `begins_at`/`ends_at`, `media_analysis_version` | Scanner and DVR concerns, both out of scope. |
| `display_aspect_ratio` *and* `sample_aspect_ratio` | Keep the display one; derive the other if it is ever needed. |

## Proposed tables

### `metadata_item`

The hierarchy. One row per movie, show, season, episode, artist, album or track.

```sql
CREATE TABLE metadata_item (
  id                 bigint  PRIMARY KEY,
  library_section_id bigint  NOT NULL REFERENCES library_section(id) ON DELETE CASCADE,
  parent_id          bigint  NULL REFERENCES metadata_item(id) ON DELETE CASCADE,
  kind               varchar NOT NULL,   -- movie|show|season|episode|artist|album|track
  guid               varchar NOT NULL,   -- plex://movie/5d77…; never null in practice
  title              varchar NOT NULL,
  title_sort         varchar NULL,       -- NULL when title already sorts correctly
  original_title     varchar NULL,
  summary            text    NULL,
  tagline            varchar NULL,
  studio             varchar NULL,
  content_rating     varchar NULL,
  content_rating_age integer NULL,
  position           integer NULL,       -- episode/track number within its parent
  year               integer NULL,
  released_on        varchar NULL,       -- YYYY-MM-DD, as the API emits it
  duration_ms        bigint  NULL,
  rating             real    NULL,
  audience_rating    real    NULL,
  added_at           bigint  NOT NULL,
  updated_at         bigint  NOT NULL,
  deleted_at         bigint  NULL        -- soft delete, so watch state survives a rescan
);
```

Indexes, each justified by a query the API actually makes:

```sql
-- The listing endpoint: one section, one type, ordered by sort title, paginated.
CREATE INDEX idx_item_listing ON metadata_item (library_section_id, kind, title_sort, id);
-- /children and /grandchildren.
CREATE INDEX idx_item_parent  ON metadata_item (parent_id, position);
-- Joining watch state, and finding the same work across libraries.
CREATE INDEX idx_item_guid    ON metadata_item (guid);
```

`deleted_at` rather than a hard delete: a rescan that briefly loses a file must not destroy
458,724 rows of watch history.

### `media`

A version of an item — one particular encoding.

```sql
CREATE TABLE media (
  id                      bigint  PRIMARY KEY,
  metadata_item_id        bigint  NOT NULL REFERENCES metadata_item(id) ON DELETE CASCADE,
  width                   integer NULL,
  height                  integer NULL,
  aspect_ratio            real    NULL,
  duration_ms             bigint  NULL,
  bitrate                 integer NULL,
  container               varchar NULL,
  video_codec             varchar NULL,
  audio_codec             varchar NULL,
  audio_channels          integer NULL,
  frame_rate              varchar NULL,   -- a string: Plex emits "24p", not a number
  interlaced              boolean NOT NULL DEFAULT false,
  optimized_for_streaming boolean NOT NULL DEFAULT false,
  created_at              bigint  NOT NULL,
  updated_at              bigint  NOT NULL
);
CREATE INDEX idx_media_item ON media (metadata_item_id);
```

### `media_part`

A file.

```sql
CREATE TABLE media_part (
  id          bigint  PRIMARY KEY,
  media_id    bigint  NOT NULL REFERENCES media(id) ON DELETE CASCADE,
  position    integer NOT NULL DEFAULT 0,  -- disc order in a stacked file
  path        varchar NOT NULL,
  size_bytes  bigint  NULL,
  duration_ms bigint  NULL,
  container   varchar NULL,
  created_at  bigint  NOT NULL,
  updated_at  bigint  NOT NULL
);
CREATE INDEX idx_part_media ON media_part (media_id);
```

`path` is the only column in the schema that becomes a filesystem access, so it is the one
`security.md`'s path-traversal rule applies to. Endpoints take a `PartId` and read the path from
here — never the reverse.

### `media_stream`

A track inside a file.

```sql
CREATE TABLE media_stream (
  id              bigint  PRIMARY KEY,
  media_part_id   bigint  NOT NULL REFERENCES media_part(id) ON DELETE CASCADE,
  kind            varchar NOT NULL,   -- video|audio|subtitle
  position        integer NOT NULL,   -- ffmpeg stream index, needed to select a track
  codec           varchar NULL,
  language        varchar NULL,       -- ISO 639, as stored
  channels        integer NULL,
  bitrate         integer NULL,
  is_default      boolean NOT NULL DEFAULT false,
  is_forced       boolean NOT NULL DEFAULT false,
  external_path   varchar NULL        -- sidecar subtitle file, NULL when embedded
);
CREATE INDEX idx_stream_part ON media_stream (media_part_id, kind, position);
```

947,346 rows here, so this index is not optional. `position` keeps its ffmpeg meaning because
that is what the transcoder command line needs.

### `person`, `tag` and `item_tag`

**Implemented** in migration `m0005_tags`, with four corrections to what this section first
proposed.

```sql
CREATE TABLE person (
  id           bigint  PRIMARY KEY,  -- our own: Plex has no person table
  name         varchar NOT NULL,
  provider_key varchar NULL,         -- the provider's id: what tells homonyms apart
  thumb_url    varchar NULL
);

CREATE TABLE tag (
  id        bigint  PRIMARY KEY,     -- Plex's own id, kept: clients hold it in `filter=genre=38`
  kind      varchar NOT NULL,        -- genre|country|director|writer|actor|producer
  name      varchar NOT NULL,        -- as credited
  person_id bigint  NULL REFERENCES person(id) ON DELETE CASCADE  -- set for a credit
);

CREATE TABLE item_tag (
  item_id  bigint  NOT NULL REFERENCES metadata_item(id) ON DELETE CASCADE,
  tag_id   bigint  NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
  position integer NOT NULL,  -- billing order within the item and the kind
  role     varchar NULL,      -- character name, for actors only
  PRIMARY KEY (item_id, tag_id, position)
);
CREATE INDEX idx_item_tag_tag ON item_tag (tag_id, item_id);
```

The corrections:

- **People in their own table.** A credit stays a tag — Luc Besson the writer and Luc Besson
  the producer are two tags, filtered on separately — but both point at one person, who
  carries the name and the portrait. Plex has no such table; people are rebuilt at import by
  grouping credit tags on the provider key: 255,011 keyed people, plus one person per keyless
  credit tag, 279,255 in all. A keyless credit is not matched by name, since a namesake and the
  same person cannot be told apart and merging two people is the worse mistake.
- **No `UNIQUE (kind, name)`.** The first draft proposed it as what Plex lacks. The data says
  otherwise: every same-name group of person tags in the reference — 715 directors, 1,410
  writers, 11,879 actors, 410 producers — carries a distinct provider key per row. They are
  homonyms, and the constraint would have merged them.
- **A credit keeps its own name.** 949 people are credited under two spellings (`Andy Garcia`,
  `Andy García`), and the reference publishes the tag's. The person takes the most recent.
  The portrait is the person's alone: of the 964 people with two, the one most credits show
  is kept, which leaves 6,717 of 3.45 million credits showing another portrait than the
  reference does — against 29,094 had the most recent been kept.
- **No `collection` and no `studio` kind.** A collection is an item of its own in Plex, with
  its own endpoints and artwork; studio is already a column on the item.

### `item_external_id` and `item_rating`

**Implemented** in migration `m0006_references`. Plex stores both as tags (`tag_type` 314 and
316), the identifier or the badge as the tag and the score in the attachment's text.

```sql
CREATE TABLE item_external_id (
  item_id  bigint  NOT NULL REFERENCES metadata_item(id) ON DELETE CASCADE,
  source   varchar NOT NULL,   -- imdb|tmdb|tvdb, the only three observed
  value    varchar NOT NULL,
  position integer NOT NULL,
  PRIMARY KEY (item_id, source, value)
);
CREATE INDEX idx_external_id_lookup ON item_external_id (source, value);

CREATE TABLE item_rating (
  item_id  bigint  NOT NULL REFERENCES metadata_item(id) ON DELETE CASCADE,
  source   varchar NOT NULL,   -- imdb|rottentomatoes|themoviedb
  kind     varchar NOT NULL,   -- critic|audience, unpacked from the badge's `extra_data`
  value    double  NOT NULL,   -- out of 10, as stored: rounding is presentation
  image    varchar NOT NULL,   -- the badge, `rottentomatoes://image.rating.ripe`
  position integer NOT NULL,
  PRIMARY KEY (item_id, source, kind)
);
```

The identifier key is `(item, source, value)` rather than `(item, source)`: 65 items carry two
identifiers from one source. The API emits these as `<Guid id="imdb://tt8041270"/>`; splitting
source from value makes "find by IMDb id" an indexed lookup instead of a `LIKE`.

Volumes for the 171,018 imported items: 292,549 tags, 3,564,487 attachments, 424,487 external
identifiers and 256,243 ratings. The full import takes 3 min 33 s on the reference library.

### Rejected: a `work` table

An earlier draft introduced `work` + `work_external_id` to make "the same film in several
libraries" a first-class entity. It was dropped, and the reasoning is worth keeping.

Duplication is real and dominant — 78% of movies, 77% of shows and 67% of episodes share a
`guid` with another item. But every use the table would serve is already served by an indexed
`guid` column: shared watch state keys on it, external ids hang off the item, and a
deduplicated listing is a `GROUP BY`. The table would have cost 132,712 rows and an import
resolution step for no new capability — the premature abstraction `SKILL.md` warns about.

The `guid` namespace also argues against it: of 204,388 items, 81% are `plex://`, 16% are
`iva://` (trailers and extras from InternetVideoArchive), 2% `local://`, and the rest legacy
agent strings. Building an entity keyed on a namespace we do not control, where a fifth of the
values are not works in any meaningful sense, is the wrong foundation.

**Revisit when open-plex has its own metadata agent.** At that point the server needs a work
identity it controls, `plex://` can no longer serve as the key, and the table earns its place.

### `view_state`

Accounts and access rights are **our own design and out of this proposal**: we deliberately do
not reproduce Plex's sharing model (`library_section_permissions`, its account table, its
`home`/`restricted` flags). That design is a separate piece of work, so the table below
references an `account_id` whose owning table it does not define.

Two consequences, both deliberate. The importer copies **no accounts** — which also means the
283 real accounts with names and e-mail addresses in the reference database are never touched.
And watch state cannot be imported either, since it is keyed on Plex account ids that will not
exist here; the structure is in place for our own accounts to fill.

```sql
CREATE TABLE view_state (
  account_id     bigint  NOT NULL,
  guid           varchar NOT NULL,   -- the work, not the item: shared across libraries
  view_count     integer NOT NULL DEFAULT 0,
  view_offset_ms bigint  NULL,       -- NULL when not in progress, never 0 as a sentinel
  last_viewed_at bigint  NULL,
  rating         real    NULL,
  PRIMARY KEY (account_id, guid)
);
```

`view_offset_ms` must stay nullable. The API omits `viewOffset` entirely on an item that was
never started, and an unstarted item is not the same thing as one at offset zero — see
`serialization.md` on absent attributes.

## Consequences to settle before implementing

**Cursors can no longer be plain integers.** `Window` currently carries `after: Option<i64>`,
which works while the only ordering is by id. The listing endpoint sorts by `titleSort`, so a
cursor has to encode the *sort key plus the id* as a tiebreaker, and the SQL becomes
`WHERE (title_sort, id) > (?, ?)`. This is the main change the proposal forces on existing code.

**Sort and filter fields must be typed enums, not strings.** `sort=titleSort:desc` and per-field
filters become SQL identifiers, which cannot be bound as parameters. An exhaustive match over an
enum of sortable columns is the only form that is both injection-proof and verifiable by
reading — see `security.md`.

**Items are duplicated across libraries by design.** One work is up to six rows here, each with
its own id, metadata language and tags. Nothing should deduplicate by path or title; the only
thing shared is `guid`, and the only thing keyed on it is `view_state`.

**Tag type coverage.** The six imported kinds were identified by matching the reference's
output element by element: `1` is `<Genre>`, `4` `<Director>`, `5` `<Writer>`, `6` `<Role>`,
`7` `<Producer>`, `8` `<Country>`; `2` is `<Collection>`, left out on purpose. `314` and `316`
come out as `<Guid>` and `<Rating>`, and live in `item_external_id` and `item_rating`.
The rest — 9, 10, 12, 305, 312, 313, 315, 317–319, 322–325 — never come out as tag elements
on a lookup; some feed `<Image>` and `<CommonSenseMedia>`, and none is identified
individually yet.

**Accounts and rights are ours to design, and must come before `view_state` is usable.** The
table is specified here because the metadata schema has to account for it, but nothing can be
written to it until the access model exists. That model is not Plex's.

**The reference database holds 283 real accounts with names and e-mail addresses.** The importer
touches none of it, and nothing derived from `db/` reaches a fixture, a log or a commit without
scrubbing.
