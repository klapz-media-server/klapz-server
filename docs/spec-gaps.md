# Spec gaps

Where `docs/openapi.json` and the reference server (1.43.5.11029) disagree. Every entry here
was verified by hand against `127.0.0.1:32400`, not inferred from the automated probe.

The spec is treated as authoritative for request shapes; this file records the exceptions. When
an operation 404s under the automated probe, the first assumption is a wrong path parameter, not
a missing route — see the caveat in `docs/endpoint-probe.md`.

## Missing from the spec

Endpoints the server answers that the spec does not declare at all. Both are essential to
library navigation, so the spec cannot be the only source for routing.

| Endpoint | Verified | Purpose |
| --- | --- | --- |
| `GET /library/metadata/{id}/children` | `200` | Direct children: seasons of a show, albums of an artist. The main navigation step. **Implemented**: zero differing and zero extra fields against the reference on seven parents across two libraries (see *Opening a series* below for the one deliberate omission). |
| `GET /library/metadata/{id}/grandchildren` | `200` | Two levels down: all episodes of a show, all tracks of an artist. **Implemented**, same measurement. |
| `GET /library/sections` | `200` | The library list — the entry point of all navigation, and what the bundled web client calls. The spec declares only the `/library/sections/all` alias. |
| `GET /search?query=` | `200` | Global search across libraries. The spec declares only `/hubs/search`. **Implemented**, so the spec-coverage test cannot be what guards it — nothing the spec never mentions can be. |
| `GET /library/sections/{id}/firstCharacter?type=` | `200` | The A–Z jump bar. Singular, and absent from the spec in that form (see *Wrong in the spec*). **Implemented**, with bucket sets and counts identical to the reference on four libraries. |

Both carry containers the listing endpoints do not; they are described under *Opening a
series* below.

## Wrong in the spec

| Spec declares | Server answers | Note |
| --- | --- | --- |
| `GET /library/sections/{sectionId}/firstCharacters` | `GET /library/sections/{sectionId}/firstCharacter` | Singular. The plural form 404s in every variant tried, with and without `type`. The singular form requires `type` (e.g. `type=1`), and 404s without it. |
| `GET /identity` under the global `user_token` security | Answers `200` with no token at all | The spec applies its global security requirement to `/identity`, but a real server serves it unauthenticated — necessarily so, since clients call it to discover whether a host is a Plex server before holding any token. |
| `X-Plex-Client-Identifier` `required: true` on the transcode `decision`, `start` and `subtitles` operations, `POST /:/timeline` and `POST /downloadQueue/{queueId}/add` | Not required: `decision` and `subtitles` answer `200` without it, as a header, as a query parameter, or at all | What the decision actually refuses (`400`) is a request with no resolvable client profile: an `X-Plex-Platform` it knows (matched case-insensitively, untrimmed) or an `X-Plex-Client-Profile-Name`. Probed on a real film; `start`, `timeline` and `downloadQueue` were not probed because they create state. |

## Aliases

Distinct spec paths that are the same endpoint. Verified byte-identical on the reference
server, so they must share a handler rather than be implemented twice.

| Canonical | Alias | Note |
| --- | --- | --- |
| `GET /library/sections` | `GET /library/sections/all` | Identical `MediaContainer`, identical `Directory` children. Only the alias is in the spec. |

## Requires parameters the spec does not mark required

| Endpoint | Required in practice |
| --- | --- |
| `GET /library/sections/{sectionId}/firstCharacter` | `type` |

## Not a gap, but easy to misread

- `GET /:/websocket/notifications` returns `404` to a plain `GET`: it expects a WebSocket
  upgrade. The route exists.
- `/{transcodeType}/:/transcode/universal/*` takes an enumerated segment (`video`, `music`), not
  a numeric id. Substituting a number produces a 404 that says nothing.
- Endpoints keyed on `collectionId`, `playlistId`, `partId`, `streamId`, `mediaId`, `updatedAt`
  and `changestamp` need real identifiers from the library; a synthesized `1` 404s on a route
  that exists.

## Behaviors the spec does not describe

Observed on the reference server and reproduced in the implementation. None of these are
derivable from the spec or from the database schema alone.

| Behavior | Detail |
| --- | --- |
| Section listing order | By media type (movie, show, artist, photo) then by title — not by id. |
| Location order within a section | Newest root first, id breaking ties. Inferred from the three sections in the reference library with several distinguishable roots, so treat it as a hypothesis rather than a settled rule. |
| `hidden` source | Not a column: it lives inside the packed `user_fields` JSON blob as `pr:hidden`, and is absent entirely on sections that were never hidden. |
| `contentChangedAt` source | The `changed_at` column, a monotonic counter unrelated to `updated_at`. Values exceed 32 bits. |
| Listing depth | `/library/sections/{id}/all` lists a library's **top level** — the films of a film library, the series of a series library. A series library holds its seasons and episodes in the same section (1,980 shows, 5,157 seasons, 79,518 episodes in section 5), and `totalSize` counts only the 1,980. The predicate is `parent_id IS NULL`, which is exactly the shows. An explicit `type=` overrides it: `type=4` returns episodes. |
| Paging activation | The window is applied only when **both** `X-Plex-Container-Start` and `X-Plex-Container-Size` are present. With either alone, or neither, the server returns the entire library in one response, omits `totalSize` and `offset`, reports the total as `size`, and sends no `X-Plex-Container-*` response headers. |
| Item element name in XML | Named after the item's `type`, not after the JSON key: `<Video>` for a film or episode, `<Directory>` for a series or season, inside the same response. JSON puts all of them in one `Metadata` array. |
| `key` suffix | The item's own path for something playable (`/library/metadata/100`), the children path for something browsed into (`/library/metadata/200/children`). |
| Library attributes placement | A listing states `librarySectionID`/`Title`/`UUID` once on the container and **not** on the items; a lookup (`/library/metadata/{ids}`) repeats them on each item and adds `librarySectionKey`. |
| `titleSort` omission | Sent only when it differs from `title`. The database stores one for every row. |
| Item `duration` | The **media's** duration, not the metadata row's, wherever the item has media: an item whose row says 6,060,000 ms reports 6,050,048. The row's value comes from the matched release, the media's from the file. |
| `bitrate` unit | Storage holds bits per second; the API publishes kilobits, rounded — 11,285,835 becomes 11,286 and a 63 bit/s subtitle track becomes 0. Applies to `Media` and `Stream` alike. |
| `aspectRatio` snapping | The measurement is snapped to the nearest of exactly six cinema ratios: 1.33, 1.66, 1.78, 1.85, 2.2, 2.35, clamped at both ends. Verified over 720 media items, which report no other value; 1.5019 comes back as 1.66 and 2.0084 as 1.85, which rules out 1.5 and 2.0 being on the ladder. |
| `videoResolution` | Derived from the **width**, with wide tolerances: 1796×800 and 1920×1080 both report `1080`, 3232×1600 reports `4k`. Height only separates the standard-definition labels (720×576 is `576`, 720×404 is `sd`). |
| `videoFrameRate` | Only three standards have labels — 23.976/24 → `24p`, 25 → `PAL`, 29.97/30 → `NTSC`. Anything else is **omitted**: a 50 fps file carries no `videoFrameRate` at all. |
| Rating precision | `rating` and `audienceRating` are published to one decimal; a stored 7.924 reports 7.9. Verified over 280 values with no exception. |
| `Media.audioProfile` / `videoProfile` | Not columns. They come from the matching stream's `profile`, which lives inside the packed `extra_data` JSON on `media_streams` (`ma:profile`). |
| `Media.audioCodec` composition | Composed from the codec *and* that profile: the column stores `dca` for all 12,087 DTS tracks, and the server reports `dca-ma` for the ones whose profile is `ma` (DTS-HD Master Audio). One missing value, three symptoms — `audioProfile`, `videoProfile` and this. |
| `Stream` presence | Included by `/library/metadata/{ids}`, omitted by `/library/sections/{id}/all`. |
| Jump-bar buckets | `/library/sections/{id}/firstCharacter?type=` groups by the first character of the sort title, **folded** — 594 films under `C` where a raw first character gives 593, because `Ç` folds to `C`. Ligatures do not fold and get their own bucket: `Œ` is a letter there, with two films. Everything not starting with a letter collapses into `#`. The `key` is percent-encoded (`%23`, `%C5%92`); the count attribute is `size`, not `count`, unlike every other `Directory` in the surface. |
| Search-result attribute order | `/search` puts the library attributes (`allowSync`, `librarySectionID`, `librarySectionTitle`, `librarySectionUUID`, `personal`, `sourceTitle`) **before** `ratingKey`; `/library/metadata/{ids}` puts the same ones after `titleSort`. Order is observable in XML, so the two shapes are distinct types. |
| Boolean stream flags | `default` and `forced` are sent only when true, never as `0`. |

### Where an `X-Plex-*` value is read from

The spec says every `X-Plex-*` header may also travel as a query parameter, and nothing more.
Measured through `X-Plex-Platform` on the transcode decision, the one value whose effect is
observable without creating state:

| Probe | Reference | Rule |
| --- | --- | --- |
| header `NoSuch`, query `Chrome` | refused | the header wins |
| empty header, query `Chrome` | refused | an empty header still shadows the query |
| header `x-plex-platform: Chrome` | accepted | header names are case-insensitive |
| query `x-plex-platform=Chrome` | refused | query names are **not** |
| query `NoSuch` then `Chrome` | accepted | a repeated parameter takes its last value |
| query `%20Chrome%20` | refused | values are not trimmed |

`ClientIdentity` in `api-plex` applies the same rules to the whole family, and decodes a header
as UTF-8 with an ISO-8859-1 fallback, which the spec states the server attempts.

### Opening a series

`/children` and `/grandchildren` are the navigation steps below a library listing, and their
containers are shaped by what is being opened rather than by the library. Measured on seven
parents across the French and English series libraries.

| Behavior | Detail |
| --- | --- |
| `/children` below a show | The container describes the show: `key` (its bare rating key, as a string), `parentIndex`, `parentTitle`, `parentYear`, `summary` (omitted when empty), `title1` = library title, `title2` = show title, `viewGroup="season"`, `viewMode=65593` — one more than the grid every other listing reports. Plus `nocache=1`. |
| `/children` below a season | The container names both levels above: `parentIndex`/`parentTitle`/`parentYear` for the season, and `grandparentContentRating`, `grandparentRatingKey`, `grandparentStudio`, `grandparentTitle` for the show. `title1` is the **show** title and `title2` the season's display title; `parentTitle` is the season's *stored* title, which is empty for nearly every season. `grandparentRatingKey` is a **number** in JSON where every item's `ratingKey` is a string. `viewGroup="episode"`, `viewMode=65592`, no `summary`. |
| `/grandchildren` | A flat list: nothing about the series, only `art`, `title1` and `viewGroup` for the library, as `/all` reports them, plus `nocache=1`. Below a **season** it lists the season's own episodes — the reference reads "grandchildren" as "the leaves", not literally two levels down. |
| Leaf nodes | `/children` on a film or an episode is a `400`. `/grandchildren` on one is a `200` with an empty container (`size="0"`, no `nocache`) that still names the library. An unknown key is `404` on both. |
| Paging | Same as a listing: `totalSize`/`offset` and the two headers appear only when both container parameters are sent. We always window, as everywhere. |
| The "All episodes" row | Unpaged, `/children` below a show opens with a `Directory` that has no `ratingKey` — `key="/library/metadata/{id}/allLeaves"`, `title="All episodes"`, `leafCount`, `viewedLeafCount`, `thumb` — and `size` counts it (3 for two seasons) while `totalSize` does not (2). It disappears when a window is sent. **Deliberately withheld** until `/allLeaves` has a handler: that route still answers `501`, and advertising a row a client cannot open is the same mistake as publishing `Part.key` without playback. Our `size` therefore counts what the body holds. |
| Season title | The source stores no title for 9,732 of 9,736 seasons; the reference names them at read time as `Season N`, in **English regardless of the library's language** (a French library's seasons read `Season 1` too), and `Specials` for season zero. A stored title wins — *The Haunting of Bly Manor* is season 2 of *The Haunting*. The derivation happens in the Plex adapter: the official API keeps `title` null and exposes `number`. |
| Season `titleSort` | Sent when a sort title is stored, and the stored title is compared, not the derived one: a season stored as (`NULL`, `Season 1`) reports both `title="Season 1"` and `titleSort="Season 1"`, where a film with equal values omits the second. |
| Season `summary` | An empty string when the source holds none, where every other kind omits the attribute. |
| Season `year` / `parentYear` | One or the other, never both: the season's own year when it has one, otherwise the show's as `parentYear`. |
| Parent attributes on items | A season carries `parentGuid`, `parentStudio`, `parentTitle`, `parentIndex` (the show's own index) from the show; an episode carries `parentGuid`, `parentTitle` (the season's display title) and `grandparentGuid`. These appear in lookups too, so `/library/metadata/{ids}` gained them with this work. |
| Episode attribute order | `grandparentKey` and `parentKey` come **before** the `librarySection*` trio in a lookup, where a film puts the trio right after `titleSort`. The earlier ordering had the trio first. |

Still missing on these items, all pre-existing gaps: artwork (`thumb`, `art`, `parentThumb`,
`grandparentArt`, `Image`, `UltraBlurColors`), tags (`Director`, `Writer`, `Role`), `parentSlug`
and `grandparentSlug`, `audienceRatingImage`, `chapterSource`, `Part.key`, `videoProfile`, and
the per-stream detail (`languageTag`, `displayTitle`, `bitDepth`, …). One divergence surfaced by
the comparison and not yet fixed: `Stream.language` publishes the tag (`en`) where the
reference publishes the name (`English`), with `languageTag` and `languageCode` beside it.

### Filtering on a listing

`/library/sections/{id}/all` accepts filter parameters the spec does not describe. Measured on
the reference server against section 4 (9,667 films):

| Parameter | Result | Semantics |
| --- | --- | --- |
| `title=avatar` | 4 items | **Prefix** match, case-insensitive. `title=atar` returns nothing, so it is not a substring search. Matches our GraphQL `titlePrefix` exactly. |
| `year=2022` | 488 items | Exact year. |
| `studio=Netflix` | 12 items | Exact studio name. |
| `genre=4025` | 2,416 items | By tag id or by name — see *Tag filters* below. |
| `title~=avatar` | 9,667 items | Silently ignored: the reference drops an operator it does not recognize rather than failing. |

The field list is not guesswork: the server publishes it per library under
`Meta/Type/Filter` (fetch `/library/sections/{id}/all?includeMeta=1`), 27 filters and 9 sorts
for a film library.

**Implemented**: `title`, `year` (with `>=`/`<=`), `decade`, `contentRating`, `studio`,
`resolution`, verified against the reference on 11 of 12 probes with exact totals; and the six
tag filters, `genre`, `country`, `director`, `writer`, `actor` and `producer`.

**Refused with `501`**, not silently dropped: every other field on the published list, plus the
operators (`!=`) and the comma-OR form outside the tag filters, which `ItemFilterInput`
cannot express. Dropping one would answer with the
whole library and let the client present it as the filtered result — an ignored filter is worse
than a rejected one. Parameters that are *not* on the list (`includeMeta`, `X-Plex-Product`,
nonsense) stay tolerated, exactly as the reference tolerates them.

#### Tag filters: by id or by name

`genre`, `country`, `director`, `writer`, `actor` and `producer` select by tag. The spec
documents none of the value grammar; all of it measured on the reference, section 6 (7,167
films):

| Probe | Reference | Rule |
| --- | --- | --- |
| `genre=38` | 1,220 | by tag id; ids are global, so a genre id from another library works |
| `genre=Action`, `genre=action`, `genre=ACTION` | 1,220 each | by name, ASCII case folded |
| `genre=COMÉDIE` (section 4) | 0 | only ASCII case: `É` does not fold to `é` |
| `genre=Comedie`, `genre=Act` | 0 | exact: no accent folding, no prefix |
| `genre=%2038`, `genre=38.0` | 0 | a value that is not an integer is a name, and no genre has that one |
| `genre=038` | 1,220 | an integer is an id, leading zeros and all |
| `genre=38,198` | 1,629 | a comma is an `OR` |
| `genre=38,Crime` | 1,629 | ids and names mix inside one list |
| `genre=38&genre=198` | 404 | a repeated parameter is an `AND` |
| `genre!=38` | 5,947 | negation, which we refuse with `501` |
| `genre=` | 0 | an empty name matches nothing, rather than everything |

The element and the filter disagree on one name: the cast is `<Role>` and filters with
`actor=`. A person credited in two roles is two tags — Luc Besson is writer `473529` and
producer `475888` — so `producer=473529` finds nothing.

**Implemented** exactly, totals identical to the snapshot on 16 probes across the six kinds
and both library types. The live reference returns 3–5% more on some probes: it has refreshed
metadata since the snapshot was taken (Snowpiercer has four genres there and none in the
snapshot), which no import of the snapshot can reproduce.

Two deviations of ours. A tag filter listing more than 50 values answers `400`: the reference
has no such ceiling, and we need one since each value becomes a bound parameter. And a name is
compared with `LOWER` on both sides, which folds exactly what the engine folds: ASCII only on
`SQLite`, as the reference does, but all of Unicode on PostgreSQL, where `genre=COMÉDIE` would
find Comédie. Matching the reference to the letter on every engine was not judged worth a
stored column.

`collection` stays refused with `501`. In Plex a collection is an item of its own, with its own
endpoints and artwork, and its tag form is unreliable even on the reference: 40% of the films
tagged with a collection do not list it.

#### `title=` is a word-prefix match

Not a column prefix. On the 488 films from 2022, `title=le` returns 75: every item having a
word beginning with `le` in its **title, sort title or original title**, accents folded. A
prefix on the sort title finds 45 of those 75.

The tokenizer splits on **whitespace only**, keeping punctuation inside the word. Each half of
that is observable:

| Probe | Reference | Why |
| --- | --- | --- |
| `title=white` misses `Black/White` | not found | the word is `black/white`; `white` is not a prefix of it |
| `title=black` finds it | found | it *is* a prefix |
| `title=nixon` finds `Frost / Nixon` | found | the spaces make `nixon` its own word |
| `title=amour` misses `#Amour et déconnexion` | not found | the word is `#amour` |
| `title=%23amour` finds it | found | leading punctuation is part of the word |
| `title=soiree` finds `# Pire Soirée` | found | accents fold |

Since the match is a prefix, only *leading* punctuation hides a word; trailing punctuation is
absorbed. Served by the `item_title_word` table (migration `m0004`), 736,105 rows covering
160,157 of 171,018 items — the rest are untitled seasons and episodes, named at read time —
queried as a range scan.

One residual divergence: we return 1,840 for `title=le` where the reference returns 1,839. The
extra item is `Corps et âme`, matched through the Hungarian original title `Testről és
lélekről` — our fold maps `ő` to `o`, giving the word `lelekrol`, and the reference does not.
A strict superset by one item in 1,839, and a superset is the harmless direction for a search.

#### `studio=` is a substring match

Not an exact name: `studio=Films` returns 992 of 9,667 films, which is every studio with
"Films" anywhere in it — not the 14 whose name begins with it. `studio=Netflix` returns 12, the
sum of `Netflix` (8), `Netflix Studios` (2), `Netflix International Pictures` (1) and
`Netflix Worldwide Entertainment` (1).

#### Range operators cannot reach us

`year>=2020` is sent by the reference's own clients with a **raw** `>`, which is not a legal URI
character. The reference's HTTP parser accepts it; ours is `hyper`, which rejects the request
line with `400` before any handler runs — confirmed by `GET /identity?year>=2020`, which also
`400`s and reads no query at all. The percent-encoded form `year%3E%3D2020` is in turn rejected
by the reference. So the equality filters work and the range filters are unreachable over this
stack; the parser handles them, and is tested, for the day something can deliver them.

### Tags on an item

A lookup carries every genre, country and credit; a listing — `/all`, `/children`,
`/grandchildren` — carries only the first few, by name alone. Measured over every film, series
and episode of the reference library:

| Element | Listing keeps | Lookup attributes, in order |
| --- | --- | --- |
| `Genre` | 2 | `id`, `filter`, `tag` |
| `Country` | 2 | `id`, `filter`, `tag` |
| `Director` | 3 | `id`, `filter`, `tag`, `tagKey`, `thumb` |
| `Writer` | 2 | `id`, `filter`, `tag`, `tagKey`, `thumb` |
| `Role` | 3 | `id`, `filter`, `tag`, `tagKey`, `role`, `thumb` |
| `Producer` | none | `id`, `filter`, `tag`, `tagKey`, `thumb` |
| `Guid` | none | `id`, as `imdb://tt0293662` |
| `Rating` | none | `image`, `value`, `type` |

They follow `Media` in the order `Genre`, `Country`, `Guid`, `Rating`, then the four credits.
Plex stores identifiers and ratings as tags too (`tag_type` 314 and 316); we keep them in tables
of their own. A rating's `value` is published to **one decimal**, as an item's own ratings are:
TMDB's stored 5.525 comes out as 5.5. `tagKey` and `thumb` are absent when the provider knows
nothing of the person; `filter` is the listing filter selecting the tag (`actor=472942` for a
`Role`).

**`role` is always present on a cast entry in a lookup**, empty when the source names no
character — the source stores an empty string for 22,518 credits, and the reference emits
`role=""` for each. Its first letter is **capitalized when it is ASCII**: 25 of 25 credits the
source stores as `actor` or `self - host` come out as `Actor` and `Self - host`, while
`čarodějobijec`, `министр` and `(uncredited)` come out as stored. Both are presentation, done
in the adapter; the official API publishes the source text.

**The listing comes from a cache, and ours does not.** The reference reads the listing form
from denormalized `tags_*` columns that a metadata refresh rewrites. On 0.5% of items those
columns are stale against the item's own tags — `Wong Kar-wai` cached where the tag now reads
`Wong Kar-Wai`, a country tagged but absent from the cache. We derive the listing from the tags,
the first *n* by billing order, and differ on those items. Measured against the live reference:
1,122 of 1,151 listed items identical; of the 69 differing elements, 3 are this cache and 66
are metadata the live server refreshed after the snapshot.

**A person has one portrait, the reference one per credit.** The reference shows each tag's own
`thumb`, and 964 people carry two across their credits. People live in a table of their own
here, so one portrait is kept: the one most of their credits show. That leaves 6,717 of 3.45
million credits showing a different portrait from the reference's, against 29,094 had the
most recent tag's portrait been kept.

Lookups, over 420 sampled films, series, seasons and episodes: 355 identical; 44 are items
whose live tags no longer match the snapshot's, and the other 21 differ by a credit's portrait
alone. `Guid` identical on 419 and `Rating` on 414, every difference being an item the live
server refreshed after the snapshot.

### Server settings

`GET /:/prefs` on the reference lists 162 settings: transcoder, DLNA, maintenance tasks,
remote access, most of them hidden. Plex Web reads ten — found by searching its bundle for
each of the 162 identifiers — and those ten are what we publish, with the reference's labels,
types, defaults and order. The values are this server's: its name, whether media deletion is
allowed, and `AcceptedEULA` true, since there is no Plex licence to accept and a client reading
`false` stops on an agreement screen. `PublishServerOnPlexOnlineKey` is `false`, the server not
being linked to plex.tv.

### Global search is ranked, ours is not

`GET /search?query=` applies the same match rule as the `title=` filter — a word prefix over
the three title fields, confirmed in both directions (`jouet` finds *Le Nouveau Jouet*, `hell`
finds *3 from Hell*) — and then **ranks the matches and cuts them off**. We return matches in
sort-title order.

For a distinctive term the answers coincide: `dune` gives 21 results on both, identical sets.
For a common one they do not, because the reference is selecting and we are not:

| Term | Reference | Our total matches |
| --- | --: | --: |
| `dune` | 21 | 21 |
| `jouet` | 19 | 27 |
| `genius` | 23 | 50 |
| `avatar` | 37 | 116 |
| `soiree` | 19 | 151 |
| `hell` | 43 | 439 |
| `world` | 45 | 500+ |

Reproducing the cutoff needs a relevance score, and ranking by one conflicts with keyset
cursors: the cursor resumes from the ordering column, so the ordering has to be a stored value.
Left as it is deliberately — the endpoint finds what it should find, in a different order and
with weaker matches included.

`/search` also carries a static `Provider` array of four entries, and its `size` counts the
items **plus** the providers: 37 items and 4 providers are reported as 41.

### Sort collation

**Accepted divergence** — not a bug to fix. The reference server sorts titles with a Unicode
collation that has two properties a byte comparison does not:

- **Numeric ordering.** `1-800-Hot-Nite` < `2 automnes 3 hivers` < … < `6 Days` < `007 Spectre`
  < `10 000`. Digit runs compare as numbers, so `007` (7) sorts after `6` and before `10`.
- **UCA punctuation order.** `[CAM] Le Nouveau Jouet` sorts before `# Pire Soiree`, which is the
  DUCET order of `[` before `#` — the reverse of their byte order.

Measured over the first 500 items of two libraries: the item **sets** match exactly, and 299/500
(films) and 438/500 (series) land in the same position, with a median displacement of 0 and a
worst case of 161. No item is ever missing or duplicated — pagination stays correct, because the
cursor resumes from the same key the ordering uses.

Closing it would mean writing UCA collation keys into the folded `sort_key` column at import,
which costs a Unicode-collation dependency and a re-import of 1.1M rows. Judged not worth it:
the ordering is alphabetical and stable, just not byte-identical to Plex's. Revisit only if a
client turns out to depend on the exact sequence.

## Response-shape gaps

The spec's `MediaContainer` schema declares four properties with `additionalProperties: true`, so
it describes almost nothing about responses. The authoritative per-field type map is
`docs/wire-format.md`, generated from live captures. Notable findings recorded there:

- JSON scalar types are per-field: `ratingKey` is a string, `Media.id` is a number.
- Booleans are inconsistent in JSON: `allowSync` is `true`, `hasPremiumPrimaryExtra` is `"1"`.
- `Meta` and `UltraBlurColors` are JSON objects; every other child is an array.
- XML escapes non-ASCII as numeric character references despite declaring UTF-8.
- Line breaks in an attribute are written `&#xD;` and `&#xA;`, in hex unlike the other
  character references. We do the same, and it matters beyond bytes: a parser turns a raw
  newline inside an attribute into a space, so a synopsis written raw loses its paragraphs.
- An apostrophe in an attribute is written `&#39;` (`Homer&#39;s Odyssey`), though the
  attributes are double-quoted and need no escaping of it. We do the same.
- `/media/providers` writes an empty `Feature` or `Directory` in long form,
  `<Feature type="manage">
</Feature>`, where the same server self-closes an empty
  `Directory` in every other response we have captured (`GET /`, the A–Z bar). We self-close it
  there too: the two forms are the same document to an XML parser, and Plex Web reads JSON.
- Errors are `text/html` and ignore content negotiation entirely.
