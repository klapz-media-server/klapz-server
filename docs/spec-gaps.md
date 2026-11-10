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
| `GET /library/sections/{id}/{filter}` | `200` | The choices of one filter — `genre`, `year`, `decade`, `contentRating`, `director`, `actor`, `writer`, `producer`, `country`, `studio`, `resolution` — each with the listing URL that applies it. The paths `includeMeta` points a client at. **Implemented**; see *Filter choices* below. |
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
| `videoResolution` | A class reached by **either dimension**, widest first: `8k` 7680 or 4320, `4k` 2880 or 1540, `2.7k` 2802 wide, `2k` 2048 wide, `1080` 1902 or 1070, `720` 1252 or 704, `576` 1016 or 560, `480` 848 or 462, else `sd`. The height is first capped at the width, so a portrait 810×1440 is `720`. Identical on 99,439 of 99,439 versions across eight libraries; the width-only rule it replaces missed 1.5%. See *Resolution* below. |
| `videoFrameRate` | Only three standards have labels — 23.976/24 → `24p`, 25 → `PAL`, 29.97/30 → `NTSC`. Anything else is **omitted**: a 50 fps file carries no `videoFrameRate` at all. |
| Rating precision | `rating` and `audienceRating` are published to one decimal; a stored 7.924 reports 7.9. Verified over 280 values with no exception. |
| `Media.audioProfile` / `videoProfile` | Not columns. They come from the matching stream's `profile`, which lives inside the packed `extra_data` JSON on `media_streams` (`ma:profile`). |
| `Media.audioCodec` composition | Composed from the codec *and* that profile: the column stores `dca` for all 12,087 DTS tracks, and the server reports `dca-ma` for the ones whose profile is `ma` (DTS-HD Master Audio). One missing value, three symptoms — `audioProfile`, `videoProfile` and this. |
| `Stream` presence | Included by `/library/metadata/{ids}`, omitted by `/library/sections/{id}/all`. |
| Jump-bar buckets | `/library/sections/{id}/firstCharacter?type=` groups by the first character of the sort title, **folded** — 594 films under `C` where a raw first character gives 593, because `Ç` folds to `C`. Ligatures do not fold and get their own bucket: `Œ` is a letter there, with two films. Everything not starting with a letter collapses into `#`. The `key` is percent-encoded (`%23`, `%C5%92`); the count attribute is `size`, not `count`, unlike every other `Directory` in the surface. |
| Filter choices | `/library/sections/{id}/{filter}` is a `secondary` view like the jump bar: `title2` names it in English whatever the language (`By Genre`, `By Starring Actor`…), `content` is `secondary` except on the actor list, which has none, and a series library adds `nocache`. A genre entry carries `type="genre"`, a person a `thumb`, a country neither. Years and decades run newest first (`key="2020"`, `title="2020s"`), everything else by name. `actor` lists only actors **billed first, second or third** on some item: 10,075 of the reference's 10,251, the rest stale, and none of the 102,000 billed lower. Encodings differ per list, measured over every value of a film library: a content rating's `key` and `fastKey` are percent-encoded once; a studio's `key` is encoded **twice** (`10.80 Films` → `10%252E80%2520Films`) while its `fastKey` is raw (`?studio=10.80 Films`); and the encoding keeps only letters, digits, `-` and `_`, so `.` becomes `%2E`. |
| Search-result attribute order | `/search` puts the library attributes (`allowSync`, `librarySectionID`, `librarySectionTitle`, `librarySectionUUID`, `personal`, `sourceTitle`) **before** `ratingKey`; `/library/metadata/{ids}` puts the same ones after `titleSort`. Order is observable in XML, so the two shapes are distinct types. |
| Boolean stream flags | `default` and `forced` are sent only when true, never as `0`. |

### Tracks: what the analysis measured, and how it is labelled

The reference keeps its media analysis in a packed JSON column, `extra_data`, keyed by the
component that wrote it (`ma:` analysis, `ln:` loudness, `pv:` markers, `at:` downloaded
subtitles). Measured over the whole library, each `ma:` and `ln:` key becomes an attribute of
the same name, with a type per key that does not follow the value: `streamIdentifier` and
`DOVIVersion` are strings, loudness values are strings with two decimals, `embeddedInVideo`
is `"1"`, `Part.hasThumbnail` is `"1"` beside boolean neighbours, `Media.optimizedForStreaming`
is a number beside a boolean on the part. They come in **ASCII order of the key**, capitals
first (`DOVI…`, `HDR10PlusPresent`, then `anamorphic`…), between `languageCode` and
`displayTitle`. `requiredBandwidths`, the `pv:` markers and the `at:` keys are not published
on a lookup and are not imported.

| Attribute | Rule |
| --- | --- |
| `language` | ICU's display name of the stored tag in the client's language: standard name, region by its short name (`Anglais (R.-U.)`, `Englisch (UK)`), region dropped when it is the language's likely one (`fr-FR` and `pt-BR` read `Français`, `Portugais`), first letter capitalised where the locale's menus capitalise (`Anglais`, but `English`, and an unknown `qad` reads `Qad` in French only). Identical on 2,516 tracks × 3 languages. `mis` and `und` are the exceptions, `Uncoded language` and `Undetermined language` in every language. |
| `languageTag`, `languageCode` | The stored tag unchanged; its ISO 639-2/T code (`fra`, `deu`, `ron`), empty for a private-use code. |
| `Media.audioProfile`, `videoProfile` | The profile of the audio track the version's codec and channel count describe — the first matching both, else the default, else the first (243,112 of 243,159) — and of the first video track. Repeated on the part. |
| `Media.audioCodec` | `dca-ma` for DTS-HD Master Audio; no other pair is composed, not `dca` with `hra` or `es`. |
| Video `displayTitle` | A resolution word from both dimensions, not `videoResolution`'s ladder (960×720 is `720`, there is no `576`): `4K`, `1080`, `720`, `480` with `p` or `i`, else `SD`; then `HDR10`, `HDR10+`, `HLG`, `DoVi/` in front of the base layer's, or `DoVi` alone. |
| Audio `displayTitle` | `Language Traits (Codec Channels + Atmos)`: DTS named by its profile whatever the codec (`DTS-HD MA`, `DTS-ES`, unknown `DCA`), AAC by its (`HE-AAC`, `AAC Main`), channels from the layout without `(side)`, `Audiodescription` for audio description, `Inconnu` without a language. |
| Subtitle `displayTitle` | `Language Forced SDH`, the words from the client's catalog (`Forcé`, `SME`). |
| `extendedDisplayTitle` | The track's title, then in parentheses what the title does not already say, word by word: words split on spaces, `-`, `@`, `+`, `/`, commas and brackets but not on `.` or `:` (`.1080p.` says nothing, `DTS:X` is not `DTS`), `AC-3` read as `AC3`. The language is said by its name in itself (`Français`, not `French`) or by the whole stored tag as a word (`FR` for `fr`, not for `fr-FR`); a trait by its English word (`Forced`, `SDH`, `CC`). A title written empty opens the label with a space: ` (1080p HEVC Main)`. 100% of audio, 99.9% of video and 99.4% of subtitle labels identical over 4,473 tracks in three languages. |

Where ours differ, all measured:

- **Downloaded subtitles** read `(Français SRT OpenSubtitles)` on the reference, which names
  the provider from the `at:` keys; we import none of them and write `Externe`. A sidecar
  subtitle also carries `key` and `file` there, and no `index`; we omit `index` too, and the
  `key` waits for a `/library/streams/{id}` route.
- **`zh-Hant`** reads `Chinois (Taïwan)`: ICU completes a script into its likely region, which
  the tables here do not carry.
- **XML writes `frameRate="25.0"`** where the reference writes `25.000`, the stored text: the
  XML is derived from the JSON value, which is the number `25.0` on both sides.
- **A loudness of `-0.00`** reads `0.00`: storage keeps the value, not the sign of a zero.
- **The 1080 threshold** of the video label lies between 1,041 and 1,072 lines (1424×1040 is
  `720`, 1424×1072 is `1080`); 1,060 is used.

### Filter choices: where ours differ

Compared over whole lists on a film and a series library: years, decades, countries and a
series library's genres are identical as sets; genres miss one. The rest:

- **Always windowed**, at 500, with `totalSize` and `offset`, as `/all` is: the reference
  returns 10,251 actors in one response.
- **Derived from the tags, not from the reference's cache**, like the listings: 4,503 directors
  against 4,581 and 3,195 studios against 3,253, with a few dozen on either side only, and 42
  of its 49 content ratings.
- **No `None` rating.** The reference ends the list with `None`, which selects the 3,762 films
  without a rating; the content-rating filter here cannot express absence, so the entry would
  select nothing.
- **No placeholder portrait.** An actor without a photo gets `/:/resources/actor-icon.png?t=…`
  on the reference (850 of them), nothing here. 134 more show another photo of the same person.
- **Order follows the folded byte order**, where the reference collates: studios match it
  case-insensitively, people and genres differ in places, as the listings do.

#### Resolution

The list, the filter and the label share the ladder of `videoResolution`, and differ in two
measured ways. The **list** names the classes some version of the library falls in, widest
first, by `key` and `title` only — no `fastKey` — and a series library lists its episodes'
whatever `type` says. The **filter** applies the ladder to the frame as stored, without the
label's cap on the height, and `resolution=2k` is not the `2k` label: it takes every frame
1,072 to 1,440 lines tall that no wider class claims, 1080p included, which is why it held 3,416
films labelled `1080`. Over eight libraries, every class of every library selects exactly the
reference's items, and every list is the reference's. The one approximation is in the list:
it counts a class by the frame as stored, so a portrait video could list a class one step
off, a case 71 versions of 99,911 present.

### Watch state

Measured on the reference's own test library, never on the owner's: each rule below is a
sequence of calls whose effect was read back after each one.

- **The state belongs to the work.** It is kept per `guid`: the same film in four libraries
  — French, original language, and both 4K — shows one `viewCount` and one `lastViewedAt`.
  Marking one copy watched marks them all.
- **A position under a minute is not kept** (`MinimumProgressTime`, 60,000 ms), whatever the
  state reported, and does not overwrite one already kept. From a minute to 90% of the
  runtime (`LibraryVideoPlayedThreshold`), `viewOffset` and `lastViewedAt` follow every report,
  `playing`, `paused` and `stopped` alike.
- **Crossing 90% counts a viewing**, clears the offset, and is compared in whole seconds:
  5,428,410 ms of a 6,031,567 ms film counts, though a hair short of 90% in milliseconds. It
  has to be a crossing **within one playback**: the report before it, under the same session
  (or, without one, the same client), fell short of 90%. A playback whose first report is past
  90% counts nothing and keeps nothing; a second report past 90% in the same playback answers
  `complete` without counting again. A playback is remembered for five minutes without a report.
- **Without `duration`** the timeline keeps nothing.
- **`/:/scrobble`** counts once more on every call and clears the offset; on an episode it
  counts on its season and series too, whose own counts are of episodes watched one by one —
  marking a whole season or series counts its episodes, not it. **`/:/unscrobble`** forgets an
  episode alone, a season with its episodes, a series with all of it; a season forgotten leaves
  its series' count. **`/:/progress`** keeps any position past a minute, 95% included, and never
  counts a viewing.
- **The timeline answers** `playbackState` `ignore`, `progress` or `complete`, then `skipCount`
  (always 0 here), `viewCount` and `viewOffset`, `-1` for none. A report under a minute reads
  `progress` from a player naming its session and `ignore` from one that does not — one earlier
  probe read `ignore` with a session, and the majority is what is reproduced.
- **On an item**, `viewCount` (once watched), `viewOffset` and `lastViewedAt` come after the
  ratings and before `year`; `viewedLeafCount`, the caller's watched episodes, is on every series
  and season, zero included.

Where ours differ:

- **`skipCount` is never kept.** The reference counts skips we have no event for; it is
  absent on items and 0 in the timeline's answer.
- **`/:/progress` with a key that is no number** is a `404` here; the reference fails with a
  `500`.
- **History is not imported.** The reference keeps 458,724 watch rows for 283 accounts, and
  the importer copies no account, so nothing ties them to a user here.

### Continue Watching

Measured on the reference's test series, given for the purpose five episodes over two seasons
(copies of a real one, removed since), and fourteen watch patterns:

- **A series' episode** is the one in progress if any; else the episode after the one most
  recently watched — not the first unwatched: S5E3 then S5E1 gives S5E2, S5E1 then S5E3 gives
  S5E4 — the next season's first after a season's last; and nothing once the series' last
  episode is the most recently watched, earlier ones unwatched or not. A season marked watched
  at once leads to the next season. Specials stand outside the sequence (unmeasured).
- **A film** is listed while it has a position, a rewatch included.
- **The window** is `OnDeckWindow`, 16 weeks since the work was last played; a series counts as
  played when any of its episodes was. The list is ordered by that date, newest first, and an
  episode never played carries its series' date as `lastViewedAt` — `includedAt` repeats it on
  `/hubs/continueWatching`.
- **Every copy is listed**: a film started in French and original-language libraries appears
  twice, as the series behind an episode does.
- **The endpoints**: `/hubs/continueWatching` (shelf `continueWatching`, style `hero`, both
  kinds) and its `/items`; `/hubs/home/continueWatching` (started only) and `/hubs/home/onDeck`
  (next episodes only); a library's shelf in `/hubs/sections/{id}` and its `/items`. Items are
  lookup-shaped, the library on each.

Where ours differ:

- **History is what the reference keeps after `/:/unscrobble`**: a series forgotten entirely
  still offers its first episode there, from views it remembers. Here a forgotten series
  leaves the shelf.
- **Season premieres past the window** (`OnDeckIncludePremieres`) are not offered: the window
  bounds what is read.
- **`/library/onDeck` and `/library/sections/{id}/onDeck`** still answer `501`. The reference
  serves them unwindowed for films, back to 2019 in its library; nothing in Plex Web calls them.
- **`OnDeckLimit`** (40 series) is not applied; the window bounds the list instead.

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
verified against the reference on 11 of 12 probes with exact totals; `resolution`, identical on
every class of eight libraries (see *Resolution*); and the six
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

### A library's shelves

`GET /hubs/sections/{id}` answers up to eight shelves on the reference. Measured on a film
library: continue watching, recently released, recently added, recent playlists (only with
`includeLibraryPlaylists=1`), top films of a genre, of a person, top unwatched, recently
watched; on a series library, recently added is a `mixed` shelf of seasons, episodes and
series. `count` defaults to six, `count=abc` too; `excludeFields=summary` drops the synopsis
from every item; an empty shelf is still listed, as continue watching is when nothing is in
progress.

Shelf titles are written by the server, in the client's `X-Plex-Language`, from gettext
catalogs shipped with it (`Resources/locale/*/LC_MESSAGES/plex.mo`): `Récemment ajouté dans
VF - Films` in French. `tools/gen_translations.py` extracts the strings we write from all 32
catalogs; a regional tag falls back to its language (`fr-CA` reads `fr`), anything else to
English. Season names come from the same catalogs: a season with no stored title is `Saison 3`
in French, `Staffel 3` in German, `Season 3` with no language, and season zero the language's
word for specials — on every surface that names a season, listings and `parentTitle`
included. A stored title is never translated.

On a series library, recently added groups new episodes. Rule, reproduced on 200 of 200
entries across four series libraries:

- per series, walk its episodes newest first; an episode joins the current batch when it was
  added within 20 hours of the batch's **oldest** episode, so a batch chains;
- a batch spanning several seasons is shown as the series, several episodes of one season as
  the season, a lone episode as itself;
- entries are ordered by their batch's newest addition, and carry it as `addedAt` — a season
  is dated by its newest episode, not by its own row;
- an item appears once, at its newest batch.

The window is not a round guess: every value between 1156 and 1215 minutes yields the same
200 entries, and nothing outside does. Series, seasons and episodes on this shelf come in the
lookup's form (tags with ids, `Guid`, `Rating`), films in the listing's — measured on both.

An episode with no content rating of its own carries its series': 14 of 15 such episodes on
the reference, the fifteenth a rating edited after the snapshot.

Recently added, against the reference on section 4: the same twelve films in the same order,
every shelf attribute identical, no field differing. On sections 5, 7, 19 and 20: the same 48
entries in the same order; the field differences left are titles and credits edited on the
live server since the snapshot.

The other shelves are a library listing each, and say which: their `key` is the listing,
`/library/sections/4/all?unwatched=1&audienceRating>=7.0&sort=audienceRating:desc` for top
unwatched films. Captured, with each list's head as the shelf unless it is `random`:

| Shelf | Library | List behind it | `random` |
| --- | --- | --- | --- |
| `recentlyreleased` | films | `sort=originallyAvailableAt:desc&originallyAvailableAt>=-1y` | |
| `topunwatched` | films | `unwatched=1&audienceRating>=7.0&sort=audienceRating:desc` | yes |
| `recentlyviewed` | films | `sort=lastViewedAt:desc&unwatched=0&viewOffset=0` | |
| `recentlyaired` | series | `type=4&sort=originallyAvailableAt:desc` | |
| `startwatching` | series | `type=2&viewedOrInProgressLeafCount=0` | yes |
| `toprated` | series | `type=2&unwatchedLeaves=1&audienceRating>=7.0&sort=audienceRating:desc` | yes |
| `recentlyviewed` | series | `sort=lastViewedAt:desc&unwatched=0&viewOffset=0&type=4` | |

A `random` shelf is a sample of its list, not its head: the six top unwatched films are none
of the list's first sixty. It holds still across requests. Its items are in a lookup's form
without `Guid` or `Rating`, which only recently added carries.

We serve these seven, filling each from its own `key` through the listing's parser, so a
shelf cannot show what its list does not hold. Against the reference: recently released films
identical in items and order; recently aired episodes the same six, two of them swapped within
a date (*Ordering a listing*, the ties); every shelf attribute identical in value and order.

Where ours differ:

- **A `random` shelf is our shuffle of its list**, the one a random listing keeps for ten
  minutes, salted per shelf so two shelves over overlapping lists do not sample alike. The
  reference's sample and how long it keeps it are not known.
- **The `key` carries `%3E` for `>`**: a raw `>` is refused by this server's HTTP layer before
  any handler runs (*Range filter operators*), and a client follows the key as given.
- **Not served**: a genre's top films (`movie.genre`, `tv.moreingenre`), a person's
  (`movie.by.actor.or.director`) and `tv.rediscover`. Each picks a genre, a person or a period
  by a rule not measured yet.

### The list behind "recently added"

`GET /hubs/home/recentlyAdded`, which the spec names only in examples, is where a series
library's "recently added" shelf leads. Measured:

- `type=1` lists films by date added, **ties by arrival** (ascending id, 62 of 62 ties), where
  the library's own `sort=addedAt:desc` ties by title; `totalSize` counts the libraries' films.
- `type=2` lists a series library's entries grouped as the shelf groups them, **at most 50**.
- `sectionID` narrows to one library; without it, or with one that is not a number (`5,7`),
  every library of the kind, merged by date. A library of the other kind, an unknown one, or a
  `type` other than 1 and 2 answers an empty container, never an error.
- Paging as everywhere: `totalSize` and `offset`, and their headers, only when both container
  parameters are sent.

Ours match on all of it: the same entries in the same order with a window or without, the
same container and headers on the empty cases. With no window, a film list holds at most 500
films and then says where its window sits, as `/all` does (*A listing always windows*). Served
on the official API by the root `recentlyAdded(kind:, library:)`.

### Transcoding an image

`GET /photo/:/transcode` needs `url`, `width` and `height`; a missing or non-numeric one answers
`400`, and so does `chromaSubsampling`, with any value. Rules measured on a 1708×640 JPEG:

- by default the image fits inside the box, with `minSize=1` it covers it, and the scale never
  exceeds 1 without `upscale=1`; sides are rounded (`300x100` gives 267×100);
- a zero in either dimension makes the box 256×256;
- a side that rounds to nothing, a negative dimension that decides the scale, or a result as
  large as 13344×5000 answers `500`.

The encoding is less obvious than the spec says. With no `format`, the source's is kept. With
`jpg`, the output is JPEG unless the source has transparency, in which case it stays PNG. And
both of those are labelled `image/jpeg`, even when the bytes are PNG; only an explicit `png` or
`ppm` changes the content type. `quality` of 1 to 127 is honored, anything else is the default.
Images are cached with `Cache-Control: max-age=259200`, the one exception to `no-cache`.

An unknown `format` is echoed by the reference into the content type (`image/gif` over PNG
bytes); we treat it as absent rather than build a header from a client's string.

The reference fetches `url` from anywhere it can reach, its own loopback included. We fetch
over TLS from `plex.tv` and its subdomains only, on every redirect hop, and answer `403`
otherwise. A source that cannot be fetched is `404` on both. Library artwork (a relative `url`)
and the effects (`blur`, `opacity`, `saturation`, `background`, `blendColor`, `rotate=0`) are
not built and answer `501`.

Against the reference on the avatar Plex Web requests: identical status, content type, cache
header, dimensions and pixel format across seven size and format combinations; the bytes
differ, the encoders being different. Converted with the `image` crate, a JPEG comes out 20 to 60%
larger than the reference's at the same quality, and a PNG about 10%.

### Playing a file

**Direct Play.** With `hasMDE=1&directPlay=1` the reference answers `decision="directplay"` for
everything we tried, MPEG-4 in AVI and HEVC in MKV included: `hasMDE` means the client has judged
for itself. The file is then fetched from the part's `key`. The middle segment of that URL is a
change stamp and the last a cosmetic name; we read neither.

**Direct Stream.** With `directPlay=0`, measured with `/decision` on one file of each of 16
container and codec combinations, for Plex Web over HLS: H.264 is copied, AAC and MP3 are copied,
other audio is converted to AAC. Any other video is converted to H.264, which we refuse with
`501`. `generalDecisionCode` is 1001 in all these cases, and
`Part.decision` is `transcode` even when both tracks are copied.

The command line is the reference's, from its log, and a test holds it to that. Two differences,
neither visible to a client:

- it reads the source from a per-session URL rather than `/library/parts/…?X-Plex-Token=…`, which
  keeps a token out of the process list;
- `-segment_list_size` is 100000 where the reference uses 5. A remux runs so far ahead of its
  announcements that the first to arrive already listed segment 5 onward, and the earlier ones
  were never announced. The usual `0` for "unlimited" makes this build exit without writing.

The reference lists every segment up front, 8 seconds each. A remux is cut on the file's own key
frames (9, 9, 6, 9 s on one file), so our playlist is `EVENT` and grows, starting once three
segments exist. `offset` seeks with `-ss` before the input; the captured DASH command has it, with
0, and no capture has another value. The `audio` bitrate of a conversion is 256k, the value in the
captured remuxes.

### DASH

Plex Web in Chrome asks for `protocol=dash`, with `directStreamAudio=0`, which converts the audio
even when it is AAC, and `subtitles=burn`. The reference starts the process and answers its own
manifest before the process has read anything, which is how both were captured although the
file is absent: `type="static"`, a `SegmentTemplate` of `duration="5"`,
`session/{id}/$RepresentationID$/header` and `$Number$.m4s` from 0. Video and audio are separate
streams in MP4 (`segments-video`, `segments-audio`).

The process keeps its chunks on the 5-second grid even when copying video: 494 chunks for 2467
seconds, cut 6, 6, 3 around the file's key frames. So a chunk's number says where it is, and a
predicted manifest is sound. Its files count from 1 and the manifest from 0. The one thing only
the process knows is the codec string, so we read it from the manifest it writes, and answer
`start.mpd` once that exists, about 0.7 seconds later.

A seek needs no new request from the player: it simply asks for a chunk far from where the
process is. The running process is then replaced by one started with `-ss {chunk × 5}
-skip_to_segment {chunk + 1}`, the option the reference's own command carries as `1`. Run directly
on a file, `-ss 600 -skip_to_segment 121` wrote `chunk-stream0-00121.m4s` from 597 seconds on, so
timestamps and numbering hold. We could not observe the reference doing this, its job dying at
once for lack of a file, so when it decides to restart is ours: a chunk that is neither written,
nor being written, nor within 30 seconds of what the process has reached. Chunks already written
stay on disk and are served without restarting anything. A chunk is written as `….m4s.tmp` and
renamed when whole.

### Lowering the quality

`maxVideoBitrate`, in kilobits per second, is what Plex Web's quality menu sends; the same limit
repeats inside `X-Plex-Client-Profile-Extra`, which we do not read. Measured over the bitrates
that menu offers, on six H.264 films (not every bitrate on each), the reference then re-encodes to H.264 and converts the
audio to AAC, always in stereo here, and the decision reports the output's size and bitrates.

The command differs from a copy's: no `-noaccurate_seek`, `-ss` only for a real offset, `-fps_mode
cfr` only without one, the AAC encoder `aac` instead of `aac_mf`, and a scale and pixel format
filter. Segment length follows the output width, and with it the x264 options and the key frame
interval: 8 seconds up to 720 wide, 5 up to 1280, 1 above. The manifest announces that duration,
`maxSegmentDuration` twice it, and the fixed codecs `avc1.640028` and `mp4a.40.2` at once, without
waiting for the process. A started offset of 600 gives `-skip_to_segment 121` at 5 seconds and
`601` at 1, the numbering our seek already used.

This is approximate in two places, and both are deliberate:

- The reference copies the video only when the limit covers the peak bandwidth its analysis
  stored per file (`requiredBandwidths`): a film averaging 1100 kbps with a peak of 3964 is
  re-encoded at 3000 and copied at 4000. We hold no such analysis and compare with the file's
  average, so we copy more often.
- Resolution, bitrates and CRF vary from film to film on the reference. `plan::LADDER` holds the
  values it produced for 1920-wide films: 3 Mbps, which it sends to 720 or 1280 wide depending on
  the film, is 1280 here. We never enlarge, and never exceed the file's bitrate.

A start that differs from the session running under the same id replaces it, should a player
change quality without changing session; we have not observed which Plex Web does.
Audio is converted whenever the video is re-encoded, `directStreamAudio=1` included,
which no capture covers. Other codecs than H.264 still answer `501`.

`subtitles=burn` with a selected subtitle forces a video transcode on the reference. Nothing here
records a viewer's choice of subtitle, so none is ever selected and the video is copied.

### Play queues

`POST /playQueues` needs `type` and `uri` (`400`), and `type` otherwise changes nothing: `audio` or
`bogus` on a film gives the same queue. The URI is
`server://{machine}/com.plexapp.plugins.library{path}`. The path names a film or an episode (one
entry, `continuous` changes nothing), a series (every episode), a season, a list of keys, or a
library listing with its filters and sort. A URI that names nothing, another server or an unknown
item is still a `200`: an empty queue, without the selected attributes or `playQueueTotalCount`.
`key` selects the entry to play first, `shuffle=1` randomizes and selects the first.
`playQueueSourceURI` restates the source as `library://x/item/…` for one item and
`library://x/directory/…` otherwise.

The window, measured on a queue of 22 centred on position 4: `window=N` (20 by default) shows N
entries on each side plus the centre. `includeBefore=0` or `includeAfter=0` drops that side **and
the centre**; both together are `400`. `window=0` shows nothing, and so does a `center` that is
not in this queue. Entries come in the form of a lookup, `playQueueItemID` first.

A queue holds at most 500 entries, where the reference took all 9,667 films of a library: every
window here is capped. Queues are deleted after 7 days, a figure of ours. `PUT` answers `501`, and
editing a queue is not served.

### Playback sessions are stored

None of the reference's 71 tables holds a session in progress: only what outlives one, the play
queues and the viewing history. Ours are stored in `playback_session`, by the repo owner's
decision, and exposed by `playbackSessions` on the official API, which `/status/sessions` reads.

A session expires by itself, 300 seconds after its last sign of life (a `ping`, a playlist or a
segment fetched), the same limit as the transcoder's idle processes. An expired session is no
longer read at once, and a sweep every 30 seconds deletes the row and stops the process. At
startup every row is deleted: no process from an earlier run is still alive.

`/status/sessions` cannot be held against the reference, whose files are not on this machine: its
attributes are what Plex Web reads. `Player.state` is what `/:/timeline` last reported. Direct Play
creates no session: only the timeline could reveal one, and we do not open one from it.

### One server per user

Nothing here comes from the reference, whose sharing goes through plex.tv. A user token is a
Plex token: `X-Plex-Token`, in a header or the query string. The Plex surface then answers with
that user's `machineIdentifier` on `/`, `/identity` and `/media/providers`, lists only the
libraries it was given, and answers `404` for anything else. A library's `viewer` browses and
plays; its `admin` also gets past the check on `PUT /library/sections/{id}/prefs`. `/status/sessions`
and `/updater/*` stay with the operator, who keeps the server's own identifier.

### Automatic quality

Plex Web offers "Convert automatically" when the server's `streamingBrainABRVersion` is at least 3.
It then sends `directPlay=0`, `directStream=0` and `autoAdjustQuality=1`. Measured on two files the
reference can really read, added to it as libraries of their own (a 1920x1080 film and a 1280x720
episode):

- the decision gains `abr: true`, right after `size`;
- the video is always encoded, even when it could be copied and fits the limit;
- the audio is always AAC;
- segments last 3 seconds at every picture size, where a fixed quality cuts by width (8 s up to
  720, 1 s above 1280; at 1280 we keep the 5 s logged for other films, this file got 3 s);
- the bitrate is not capped by the source's: 9 Mbps for a 3.2 Mbps file;
- without a limit, a 1920-wide source gets 20000k at CRF 16, and a 1280-wide one 9000k at CRF 16,
  its ceiling for any limit of 10 Mbps or more. Below that the rungs are in `plan.rs`.

**The quality never changes during a session here, and it did not on the reference either.** Four
attempts to provoke a change left the bitrate alone, with one job and one `Bandwidth` entry:
downloads throttled to 2.5 and 3 Mbps for up to 215 seconds, over loopback and the machine's own
address, then with a real playback clock and `state=buffering`, with `location=lan` and `wan`. Its
binary does hold a mechanism (`Client buffer has dropped; switching to measurement`, `Reducing
bandwidth of %s by %d to %d`), so the condition exists and is unknown; every connection here was
local. Plex Web's timelines carry no bandwidth, so nothing the client sends explains it.

A timeline for a running conversion also answers `Bandwidths` and `TranscodeSession` (`size` 2),
fixed quality and copies included. `Bandwidth.bandwidth` is the rounded-up sum of 2.1 times the
bitrate of each copied track and 1.05 times that of each encoded one, and `resolution` follows the
width of what is sent. Plex Web shows it as the current quality, once `TranscodeSession.key` ends
with the session it started. We send the attributes we can state and leave out `progress`, `speed`
and the codec fields. After `stopped` our session is closed, so neither is sent; the reference
still sends them until `/stop`.

### Refusing a chunk

A request for a DASH header, a DASH chunk or an HLS segment waits for the file, up to 20 seconds
while a process is running. Once the session's process has ended it is held for 5 seconds before
the `404`, and is served if the file appears meanwhile. A session that is unknown or stopped
answers `404` at once, a wait already under way included. Measured on the reference, an unknown
session and a dead process both answer `404` within 0.1 s; it cannot read its files here, so a
chunk that is merely late could not be timed. The hold is a deliberate choice against retry loops.
Playlists and manifests are not held.

### Timelines

The spec declares `POST /:/timeline`. The reference also answers `GET`, which is what Plex Web
sends, and `PUT`. Measured in the order the checks apply, on a film at `time=0` (which left its
watch state unchanged) and on a key of no item:

| Request | Answer |
| --- | --- |
| `state` absent, or not `playing`, `paused`, `buffering` or `stopped` | `400`, even for an item that does not exist |
| `time` present and not an integer | `400`, likewise |
| no `ratingKey`, or one that is not a number | `200`, `playbackState="ignore"` |
| numeric `ratingKey` of no item | `404` |
| a real film, with or without `time` | `200`, `{"size":0,"playbackState":"ignore"}` |

Plex Web's bundle also reads `progress` (it moves its copy of `viewOffset`) and `complete` (it
marks the item watched). Neither was captured: provoking them would write a position to the
owner's account on the reference. We store no watch state, so we always answer `ignore`, and the
position lives only on the session.

A timeline names its playback through `X-Plex-Session-Identifier`, the bundle's
`playbackSessionID`. The `session` of the transcode URLs is another value, its
`transcodeSessionID`, seen to differ between requests carrying the same identifier. A session
therefore stores both: a report applies to every unexpired session of the playback, and `stopped`
closes them. Stopping the transcoder stays with `/video/:/transcode/universal/stop` and the idle
limit.

### A library's settings

`GET /library/sections/{id}/prefs` lists 17 settings for a film library and 18 for a series, nearly
all about scanning and matching. Plex Web draws its dialog from whatever the list holds and looks
up only two by name, `hidden` and `includeAdultContent`, so those are what we publish, at their
defaults: identical to the reference in English, French and German. The labels come from its
catalogs. Unlike `/:/prefs`, every `default` and `value` here is a JSON string, a boolean included.
`PUT` answers `501`.

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

### What a listing says it can do

`includeMeta=1` on `/library/sections/{id}/all` adds a `Meta` block before the items, which Plex
Web builds its filter and sort menus from: one `Type` per item type the library lists, each with
its `Filter`s and `Sort`s, then `FieldType`. Without the parameter, or with `0`, it is absent. The
requested `type` is the active one, the first by default. The active `Sort` carries `active` and
`activeDirection`: `sort=addedAt:desc` marks it `desc`, `sort=addedAt` marks it `asc`.

We publish the filters and sorts a listing honors, not the reference's whole list: 11 of 27
filters and all 9 sorts for a film library, 9 of 14 and 7 of 8 for a series, 1 of a season's 2
sorts, 2 of 17 and 6 of 7 for an episode. Two tests hold every filter and every sort offered to
being one `listing::parse` accepts. What we do publish has the reference's attributes, wording
and relative order on every type compared. Left out: the `folder` type, which no route serves,
and the advanced filters' `Field` and `FieldType`.

A filter's `key` (`/library/sections/4/genre`) is the list of its values, and those routes do not
exist yet: they answer `404`.

### Filtering on what was watched

The reference's filters on watch state, measured on its test libraries with four films
(untouched, watched, in progress, watched then restarted) and a series through six states:

- **`unwatched=1`** keeps what was never watched to the end: a film in progress is unwatched,
  a restarted one is not. **Any other value**, `0`, `true` and the empty string included,
  keeps the watched ones; `unwatched!=1` too.
- **`inProgress=1`** keeps what has a position, rewatches included; any other value, what has
  none.
- **A series counts its own viewings**: one whose only watched episode was unmarked since
  stays watched (`viewCount` 1, `viewedLeafCount` 0), so `unwatched=1` leaves it out. A series
  is never `inProgress`.
- **`unwatchedLeaves=1`** keeps the series with an episode not watched to the end; any other
  value, those with every episode watched.
- Two of them combine with `AND`; the same one twice, `unwatched=1&unwatched=0`, is empty.

Ours match on every case above, against our own library with the same files and states, and
each reads the caller's own state. A caller with no watch state has watched nothing. The same
flag twice answers `501`, which one flag cannot express.

The shelves also filter on fields the reference does not advertise, which we honor for the
values they use and refuse otherwise: `audienceRating>=`, `originallyAvailableAt>=` (a date,
or one counted back from today: `-1y`, `-6w`, `-30d`), `viewOffset=0` and
`viewedOrInProgressLeafCount=0`.

### Ordering a listing

The reference offers nine sorts on a film library (`includeMeta=1`). Measured over the whole of
two libraries, 9,667 films and 1,980 series, in both directions:

- **Absence sorts first**: a film with no rating leads an ascending list by rating and ends a
  descending one, as `NULL` does on its SQLite. We state the position in every `ORDER BY`, since
  another engine puts `NULL` last.
- **Ties follow the title, ascending, whichever the direction**: 9,406 of 9,567 tied pairs by
  rating, 1,065 of 1,080 by date added. The rest is the collation (*Sort collation*). The title
  sort itself breaks ties by id, in its own direction.
- **An item with several versions ranks by the smallest ascending and the largest descending**,
  for `duration` and `mediaHeight`: a 1038- and a 1040-line film is 1038 one way and 1040 the
  other, as if joined to each version and kept at its first row. `episode.addedAt` does the same
  over a series' episodes: its first ascending, its latest descending (0 of 1,979 pairs out of
  order either way, 561 and 585 under the other reading).
- **`duration` reads the versions**, not the item: two films with no version duration lead the
  ascending list although they display one.
- **The field is case-sensitive and the direction is `desc` exactly**: `Rating:desc` is a `500`
  there, `rating:DESC` ascends.

Against the reference, with the items whose value changed on the live server since the snapshot
set aside: the sequence of values is identical for every sort and direction compared, `duration`
lands every film in the same position, `addedAt` 99.7%, `episode.addedAt` 99.7 and 99.9%. The
positions left differ within ties, by collation — 45.6% for ratings on films, where 5,434 unrated
ones tie.

Where ours differ:

- **A sort we cannot honor answers `501`**, as a filter does: `unviewedLeafCount`, the series and
  season orders of seasons and episodes (`show.titleSort,index…`), and a list of fields, which
  is how the reference states tiebreakers. Its `year`, `viewCount` or `title` work there and are
  refused here; an unknown field is a `500` there.
- **`random` keeps one order for ten minutes.** The reference reshuffles on every request, so a
  client paging a random list meets some items twice and never sees others. Ours is a shuffle
  seeded by the clock's ten-minute period, so the pages of one browse agree; the official API
  takes the seed as an argument.
- **`lastViewedAt` is the caller's own**, like all watch state; a visitor's list is in title order.

### The shelves under an item

`GET /library/metadata/{id}/related` is a `Hub` list, in the reference's order: collections, films
like this one, the director, the actors, `external.*`. Seasons and episodes have none, an unknown
item is `404`, and `count` (default 6) sizes every shelf.

Measured on 150 films of one library:

- an actor gets a shelf only among the **first two billed**, a director only as the **first
  credited**, and only with **five or more other films** in the library. All of those have
  one; below that, two actors with three or four other films do, out of 38 first- and
  second-billed actors with that many, which the snapshot being older may explain;
- the shelf holds those films by title, and leads to `/all?actor=…&id!=…`, so `id!=` is honored
  on a listing;
- a `movie.same.actor` shelf is numbered from 0 in the order the shelves appear.

Against the reference on 60 other films: the same person shelves on 59, attributes identical on
all 41 compared, the same films in the same order on 35. The other 6 all come down to title
collation, which moved a film across the cut in two of them. The field differences on the 233 items compared are items edited since the snapshot, checked column
by column against it; two `contentRatingAge` were not.

**Similar items are an approximation.** We found no column or table stating them.
`metadata_item_clusters` is empty, `metadata_relations` has no row for a film that has the shelf,
and the `similar` tags (type 305) are what the `external.*` shelf holds (120 of 120 items on 25
films) and only 2 of 18 of this one, which a film without any still gets. Ranking by shared tags gives a recognizably close list, not the same
one: a shared genre counts one, any shared actor one, any shared director two, ties to the most
recent. That finds 101 of the 180 items shown on 30 films, the best of fifteen weightings tried.
The shelf exists on 40 of the 60 films here against 36 on the reference; why the reference omits
it from a film with genres and actors is not known.

`/library/metadata/{id}/similar` answers a bare container: `size`, `identifier` and the media tag
attributes, with none of a listing's.

### The search field calls `/library/search`

Plex Web's search field does not use `/hubs/search` or `/search`. It calls the
`universalsearch` feature from `/media/providers`, `GET /library/search?query=&limit=&searchTypes=
&includeMetadata=1&filterPeople=1`, which the spec does not declare. The answer is a list of
`SearchResult`, each a `score` around one `Metadata` (or one `Directory`, for a person or a
collection, per library holding it).

Measured: `query` is required (`400`; empty is an empty container); `limit` defaults to 10, a
non-number is `400` and zero `500` (a `400` here); `searchTypes` defaults to `movies,tv`, where
`movies` is films and `tv` is series and episodes, and an unknown value yields nothing;
`sectionId` changes nothing. Items come in the listing's form, naming `librarySectionTitle`;
only a series adds `librarySectionID` and `librarySectionKey`. A film with no synopsis carries
`summary=""`, where a listing omits it.

Differences that remain, each knowingly:

- **Scores.** The reference's are finer and fuzzy: `zzzqqq` matches *Dragon Ball Z*. We keep the
  word-prefix match and use three of its values: 0.93 for a title equal to the term, 0.73 for
  one starting with it, 0.33 otherwise, ranking the first 100 matches of each of films and
  series, then of episodes. Same top results for a title someone types in full.
- **People and collections are absent.** `searchTypes=people` is accepted and finds nothing,
  against the rule that what we cannot honor is refused: Plex Web sends it with every search,
  and a `501` would take the whole field down. Tag names have no word index to search yet, and
  collections are not imported.

Against the reference on eight terms and 89 shared items, what still differs is items edited
since the snapshot, the stream profiles (*Open questions*), and the `Field` lock list on two
series, which we do not serve.

A term of several words needs them all, here as on `title=` and `/search`. The reference also
wants them in order (`bad breaking` finds nothing); our word index holds no positions, so a
reversed term finds a little more.

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

## Pairing with plex.tv

None of this is in the spec. Measured against the reference and plex.tv.

| Step | Request | Source |
| --- | --- | --- |
| Spend a code | `POST https://plex.tv/api/claim/exchange?token=claim-…` with `X-Plex-Client-Identifier: <machine identifier>` | documentation; works here, the answer has the `<authentication-token>` element |
| Announce the address | `PUT https://servers.plex.tv/api/v2/devices/<machine identifier>?connections[0][uri]=…&httpsEnabled=1&httpsRequired=0&dnsRebindingProtection=0` | reference logs (it also sends `connections[0][prefixLength]`, for a LAN address) |
| Register the server | `POST https://servers.plex.tv/servers.xml?auth_token=…`, `Content-Type: application/xml`, a `<MediaContainer size="N">` holding `<Server name host localAddresses machineIdentifier version date …/>` and one `<Directory>` per library | captured through a relay. `address` and `port` are added once a public mapping exists; `date` is in microseconds |
| Whose token is this | `GET https://plex.tv/api/v2/user` with `X-Plex-Token`, `Accept: application/json` | captured: `id` is a number, `username` and `title` strings |

- The value `/identity` calls `machineIdentifier` is the registry's `ProcessedMachineIdentifier`,
  so the identifier plex.tv knows a server by is the one clients read from it.
- `/users/account.xml` carries the token as an `<authentication-token>` element, which is where
  the documentation says the exchange's answer has it.
- For an owned server, `clients.plex.tv/api/v2/resources` gives `accessToken` equal to the token
  that asked. A client therefore presents a token of its own, which is why tokens are attributed
  by account rather than compared with the one pairing obtained.
- The reference's logs show the address announced around its startups, not on a timer.
- `/api/v2/resources` lists a server with an empty `accessToken` until it has posted to
  `servers.xml`, and a client signed in to that account then has no token to present. The post
  answers `422` when it carries no public `address`, to the reference too, and has its effect
  all the same.
- Cross-origin, the reference answers on every route and before authentication: a `plex.tv`
  origin (http or https, any subdomain) is echoed in `Access-Control-Allow-Origin`, and any other
  origin gets `https://app.plex.tv`, which a browser refuses just the same. `OPTIONS` answers `200`
  with the requested headers mirrored and a `Access-Control-Max-Age` of `1209600`.
- `GET /library/metadata/{id}/userState` is a `404` on the reference, token or no token. Plex's
  Discover provider answers it: `{"MediaContainer":{"identifier":"tv.plex.provider.discover",
  "size":1,"UserState":[{"ratingKey":…,"type":"movie","viewCount":0,"viewOffset":0}]}}`, and
  `404` for an identifier it does not know.
- A new conversion with the `X-Plex-Session-Identifier` of a running one replaces it on the
  reference: the old session's segments answer `404` as soon as the new one has started. Plex Web
  relies on this when changing quality, opening a new `session=` without stopping the last.
- `GET /library/metadata/{id}` marks exactly one audio stream `selected` (219 of 220 files
  measured): the first default on 193, the first track on 13, and on the other 13 the one in the
  account's language. No video stream is marked; subtitles depend on the account. Plex Web reads
  `Stream.find(selected).id` in its player settings and crashes without it.
- A far forward seek over HLS has to restart the remux at the target segment, numbering its
  output from there; only waiting leaves a segment the process was never going to reach, and the
  player stalls. The reference's own log confirms the command: a seek at 600 s on an 8-second grid
  reads `-ss 600 … -segment_time 8 -segment_start_number 75` (75 = 600 / 8), i.e. `-ss N·seconds`
  with `-segment_start_number N`, output numbered from the target. The DASH seek is the same with
  one-based numbering.
- `GET /library/metadata/{id}/allLeaves` lists episodes for a series only. A season answers an
  empty container on the reference, measured on seven seasons of six series, though its
  `/children` lists the same episodes; a film and an episode answer one too. Every such
  container describes the node: `parentTitle`, `parentYear`, `title1`/`title2`, and for a season
  or an episode a `grandparentTitle`, empty for the episode. A series' episodes carry its
  `studio`, which `/grandchildren` leaves out.
- A series' `/children` leads its first page with an `All episodes` `Directory` (`leafCount`,
  `viewedLeafCount`, `key` to `/allLeaves`, `title`), counted in `size` and not in `totalSize`,
  absent from a later page, and in English whatever `X-Plex-Language` says.
- An HLS remux's `index.m3u8` is a fake VOD playlist the reference builds up front from the
  media's duration: `#EXTM3U`, `#EXT-X-TARGETDURATION:10`, `#EXT-X-ALLOW-CACHE:NO`,
  `#EXT-X-MEDIA-SEQUENCE:0`, then one `#EXTINF:10, nodesc` per 10s segment and a rounded
  remainder, ending in `#EXT-X-ENDLIST` at once — no `#EXT-X-VERSION`, no `#EXT-X-PLAYLIST-TYPE`.
  Measured on Soul (6_031_568 ms): 603 full segments and a final `#EXTINF:2`. The actual remux
  cuts on the file's key frames near those boundaries. This is why a player shows the true
  length at once rather than a duration that grows.
- A stream's `header` and chunks are served without a token by the reference, from the LAN too:
  the manifest carries no `X-Plex-Token` and players fetch its relative URLs as they are. An
  unknown session answers `404` with or without a token.
- A client with no token matches the server it reached with its plex.tv entry through
  `/identity`, so that answer must be the paired server's identifier.
- Without a token, `GET /` answers `401` on the reference, from loopback and from the LAN alike,
  where `/identity` answers `200`. Plex Web needs that `401`: a `200` tells it the server admits
  anonymous visitors, so it never offers a sign-in and then stops at "Not authorized".
- With a claimed account, `/` and `/media/providers` both carry, in this order, `myPlex`,
  `myPlexMappingState` (`mapped`), `myPlexSigninState` (`ok`), `myPlexSubscription` and
  `myPlexUsername`. Plex Web treats a server as claimed when `myPlexSigninState` is `ok`.
