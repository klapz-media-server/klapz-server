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
| `GET /library/sections/{id}/{filter}` | `200` | The choices of one filter — `genre`, `year`, `decade`, `contentRating`, `director`, `actor`, `writer`, `producer`, `country`, `studio`, `resolution`, `collection`, and `videoCodec`, `audioCodec`, `audioLayout`, `subtitleCodec`, which the spec declares — each with the listing URL that applies it. The paths `includeMeta` points a client at. **Implemented**; see *Filter choices* below. |
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

#### Codecs and channel layouts

`videoCodec`, `audioCodec`, `audioLayout` and `subtitleCodec` list what the tracks hold, and
filter on it. Measured on ten libraries, every value of four of them filtered both ways:

- **Every track of every version counts**, not a version's summary: `videoCodec` lists `mjpeg`,
  which no version names as its codec — a cover picture stored as a second video track — and
  `audioCodec=aac` keeps 1,348 films where 896 versions have AAC as their codec. A layout is an
  audio track's channel layout as the analysis names it (`5.1(side)`, `8 channels
  (FL+FR+FC+LFE+SL+SR+TFL+TFR)`); `subtitleCodec` includes sidecar files and `none`, which the
  analysis writes for 21 tracks it could not identify.
- **The list** is the distinct values in byte order, by `key` and `title` only: no `fastKey`,
  the key percent-encoded once as a content rating's is (`5%2E1%28side%29`). A series library
  lists its episodes' whatever `type` says, as for resolutions.
- **The filter matches exactly**, case included (`H264` keeps nothing). A comma is an `OR`
  (`videoCodec=av1,vc1`, 101 films), a repeated parameter an `AND` of two tracks
  (`videoCodec=h264&videoCodec=mjpeg`, 1), and `!=` keeps the items with no such track, films
  without media included (`subtitleCodec!=srt`, 3,217 = 9,667 − 6,450). An item is judged on
  its own tracks: `type=2&videoCodec=vp9` keeps no series, `type=4` keeps 117 episodes.
- The reference also compares the text (`videoCodec>=h264` keeps 23 films); that answers `501`
  here, as any operator a filter cannot express.

Ours: the 40 lists of ten libraries identical, and 780 of 782 listing totals over every value
of four libraries with `=` and `!=` — the two others three episodes whose sidecar subtitles
the snapshot predates.

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

### A library's categories, `/library/sections/{id}/categories`

The spec calls it "Set section categories" and documents a `GET` with no parameter but the
section. Measured on the ten film and series libraries of the reference and on genres given
to the show of test library 28 (and taken away again):

- **The list is the library's genres**, the entries of `/genre` in the same order (by name),
  each a `Directory` with `thumb`, `type="directory"`, `key="/library/sections/{id}/all?genre={tagId}"`
  and `title`, and nothing else. A library without genres (27 and 28 as they stand) lists
  none. The container is a `secondary` view — `content`, `viewGroup`, `title2=""`,
  `viewMode=65592`, `nocache` on a series library — with a `Meta` block of one `Type`,
  `key` the view itself, `type="directory"`, `title="Categories"`, `active`, written in XML
  in the long form (`<Type …>
</Type>`) although it holds nothing.
- **Whole unless both container parameters are sent**, then `size`, `totalSize`, `offset` and
  the two headers, as on `/filters`. Every other parameter — `type`, `genre`, `year`,
  `unwatched`, `sort`, an unknown one — changes nothing on the reference, and nothing here.
  An unknown section and a non-numeric one are `404`.
- **`blendColor`**, read from the binary and measured on every tile (335 in ten libraries, and
  six sets of genres on library 28): the thirteen genres the reference ships a
  `category-art` image for — `action` `2B717E`, `animation` `347431`, `comedy` `51284C`,
  `crime` `33486E`, `documentary` `884624`, `drama` `846E49`, `history` `37397A`, `horror`
  `7A281A`, `musical` `3B8265`, `romance` `892959`, `science-fiction` `673E8B`, `thriller`
  `57222B`, `western` `704A23` — keep their color, matched in lower case with spaces as
  hyphens (`Science Fiction` and `Science-Fiction` alike). Every other genre takes the next
  of `494616`, `91293D`, `4C561B`, `883E7D`, `8B6122`, `8B4D43`, `23533C` in turn, by
  ascending tag id over the whole library, so a window shows the colors the whole list does.
  Adventure (tag 122) is the lowest such genre in every library measured, which makes it look
  named: on library 28 without it, Fantasy takes `494616`. Whether the reference matches a
  name's case was not measured, since its tags merge names case-insensitively.
- **`thumb`** is `/photo/:/transcode?blendColor=…&width=2560&height=1440&url=…`. The `url` is
  the background art of one of the genre's items: the query in the binary keeps the items with a
  `user_art_url`, those not already shown by a genre of lower id (`metadata_items.id not in
  (…)`), and takes the highest `audience_rating`; 40 of 41 tiles of a film library name
  distinct items, a genre whose items are all taken repeating one. A genre none of whose items
  has art gets `/:/resources/category-art/animation.png` — that file, whatever the genre, as
  library 28's show without art showed, encoded `%2F%3A%2Fresources%2Fcategory-art%2Fanimation%2Epng`.

**Where we differ.** This server keeps no artwork (`user_art_url` is not imported), so every
tile has the fallback the reference gives a genre without art: identical to the reference on
library 28, attribute for attribute in JSON and byte for byte in XML; on a library with art,
the colors, keys, titles and order are the reference's and the `url` is the fallback rather
than an item's art. `/photo/:/transcode` refuses both a relative `url` and `blendColor`
(`501`), so the tile's image does not open here yet. The list is `Library.tags(kind: GENRE)`
on the official API; the color and the image are presentation, chosen in the adapter.

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

### A viewer's rating, `/:/rate`

Measured on the reference's test libraries 27 and 28 — a film, a series, its season and two
episodes — each rating read back on the item and in its row of `metadata_item_settings`, and
every rating cleared again afterwards.

- **The rating is the viewer's and the work's.** It lives beside the watch state, one row per
  account and `guid` with a `rating` column; of the works the reference's owner rated, ten are
  held by several libraries and rated once for all of them. Every kind can be rated — film,
  series, season, episode — and **nothing adds up**: a series rated 4 over episodes rated 8
  says 4, and a series not rated says nothing whatever its episodes are.
- **`rating` is kept as sent and published to one decimal**: 7.26 shows as 7.3, 7.25 as 7.3,
  7.24 as 7.2, 0.04 as 0.0 (the row holds a 32-bit float). XML writes `userRating="8.0"`. Plex
  Web sends 1 to 10 — a star is 2 — and `-1` for no stars.
- **`rating=-1` clears** the rating, `-1.0` alike, and records when (`pv:unratedAt` in the row's
  `extra_data`); both `userRating` and `lastRatedAt` then leave the item. Clearing a rating
  that is not there changes nothing.
- **`ratedAt` dates the change**, now when absent, and **a change older than the last one is
  ignored**, answered `200` all the same: a rating dated before the one held, or before the
  rating was cleared, leaves it as it is; the same second replaces it. Never rated, any date is
  taken, and `lastRatedAt` is that date.
- **Refusals**: `400` without `identifier`, `key` or `rating`, for a `rating` that is no
  number (`abc`, empty, `3,5`), and for a `ratedAt` that is no integer, `0`, or in the future —
  all before anything else, an unknown key included. Then `404` for any `identifier` but
  `com.plexapp.plugins.library` (empty and `tv.plex.provider.metadata` included), and for an
  unknown or non-positive `key`. A `key` that is no number, or a list, fails with a `500`.
  Success is an empty `200` typed `text/html`, on `PUT`, `GET`, `POST` and `DELETE` alike.
- **On an item**, `userRating` follows `audienceRating` and comes before `viewCount` and
  `viewOffset`; `lastRatedAt` follows `lastViewedAt`, before `year`. Both are on lookups,
  listings, `/children`, `/allLeaves`, `/grandchildren`, the recently-added lists, Continue
  Watching, the library shelves and the search shelves — wherever the item is.
- **`userRating` filters and sorts `/all`**, though the `Meta` menus offer neither: the type's
  `Field` list names it (`userRating`, `show.userRating`, `episode.userRating`, subtype
  `rating`), and Plex Web's table view sorts its rating column by it. `=` takes a comma list in
  which `-1` stands for no rating (`=8,-1` lists the episode rated 8 and the three unrated);
  `!=` a comma list; `>=` and `<=` are inclusive, `>>=` and `<<=` strict. **Every comparison but
  that `-1` leaves the unrated out**: `!=8`, `<=8`, `>=-1` and `!=-1` list rated items only.
  Equality is exact: `=8.0` matches 8, `=7.99999` nothing. The sort puts the unrated first
  ascending and last descending, ties in the same order both ways.

Where ours differ:

- **A rating off the scale is refused** with a `400`. The reference keeps 11 and 100 and
  publishes them, takes `0x5` as 5, `1e1` as 10, stores `inf` (published `null`) and `nan`, and
  on a negative other than `-1` nulls the rating while keeping `lastRatedAt`. The spec bounds
  `rating` to 0–10, and so do we.
- **A negative `ratedAt` is a `400`**, like `0`; the reference takes it as a very old change.
- **A `key` that is no number is a `404`**, as for a scrobble, where the reference fails `500`.
- **A second `userRating=` or `!=`, or the same comparison twice, answers `501`** rather than
  being combined; the reference takes `=8&=8`. `userRating=abc` is a `501` where the reference
  fails `500`. The `show.` and `episode.` forms the `Field` list names are not read.
- **Ratings are not imported**, for the reason the watch state is not: the importer copies no
  account. The reference holds 942 ratings for its accounts, all whole numbers from 1 to 10.

### Viewing history

`/status/sessions/history/all`, `/status/sessions/history/{historyId}` and its `DELETE`,
measured on the reference's test libraries 27 and 28 by playing, marking and deleting, each
step read back (its rows in `metadata_item_views`, with a `view_type` column
`docs/plex-db-schema.md` predates):

- **An entry is a viewing counted, and nothing else.** A timeline crossing 90% writes one
  (`viewType` 0), and the reports after it in the same playback none; `/:/scrobble` writes one
  on every call (`viewType` 1), and marking a season writes one per episode, none for the
  season. A playback stopped at 30%, a position kept by `/:/progress`, or a report under the
  minute write nothing. `/:/unscrobble` deletes no entry, and deleting an entry leaves
  `viewCount` as it was.
- **The listing holds what was played**: `viewType` 0 only, unless `type=1` asks for what was
  marked; `type` is that number, not a metadata type (`type=movie` is a `400`, any other
  number an empty list). `viewType` is published only when it is 1.
- **Entries come in the order written** (`historyKey` ascending) unless `sort` says otherwise.
  `sort` takes `viewedAt`, `accountID`, `librarySectionID` and `type`, comma-separated, each
  with `:asc` or `:desc` — `:DESC`, `title`, `metadataItemID` and anything else is a `400`.
- **Filters**: `metadataItemID` takes a film or an episode, or a season or series for its
  episodes, and is a `404` for an item it does not have; `librarySectionID` and `accountID`
  take lists; `viewedAt=`, `viewedAt>=`, `viewedAt<=` and `viewedAt!=` bound the date, in
  seconds — `-1d` is a `400`, `viewedAt>>=` is ignored. A number that is not one is a `400`.
  `deviceID`, `ratingKey` and other names are ignored.
- **Paging**: without both container parameters the whole history comes back with `size`
  alone; with them, `totalSize`, `offset` and the two headers.
- **An entry** carries, in this order: `historyKey`, `key`, `ratingKey`, `librarySectionID` (a
  string), `parentKey`, `grandparentKey`, `title`, `grandparentTitle`, `type`, `thumb`,
  `parentThumb`, `grandparentThumb`, `grandparentArt`, `index`, `parentIndex`,
  `originallyAvailableAt`, `viewedAt`, `accountID`, `deviceID`, `viewType`. The keys and the
  artwork are the item's, read live: an entry whose item is gone has none of them, and a lookup
  by `historyKey` has none of them either. The rest is a copy kept in the row. XML elements are
  `<Video>`.
- **A lookup** is `size="1"` and the entry; one it does not have, or a key that is no number,
  is a `404`. **`DELETE`** answers the empty container `size="0"`, then `404` for the same key.

Where ours differ:

- **Each user's server holds its own history.** The spec has the administrator read every
  account's; here a token reads the history of its own viewer, published as `accountID` 1, the
  account a server's owner has on the reference. `accountID` naming any other answers an empty
  list. An owner's view of everyone's history is on the official API (`viewingHistory`), as is
  its own (`mine: true`), which is what an owner's token reads here. The
  lookup and the `DELETE`, admin-only in the spec, are open to a user for its own entries.
- **The listing always windows**, at most 500 an answer, and always states `totalSize` and
  `offset`, as a library listing does.
- **No `deviceID`**: no device is recorded. **No artwork**, as on items.
- **An entry goes with its item.** Nothing about the item is copied into the row, so the
  title, numbers and date are the item's as it is now, and deleting an item deletes its entries.
- **`viewedAt!=` and one more order after the first** (`librarySectionID,viewedAt:desc`) answer
  `501`; an account or a `type` in `sort` orders nothing here, every entry having the same.
- **History is not imported** from the reference, for the same reason as watch state.

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
  (each series' episode, the one in progress included: Bull's first episode at 78 seconds is
  listed by both); a library's shelf in `/hubs/sections/{id}` and its `/items`. Items are
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
`grandparentArt`, `Image`), tags (`Director`, `Writer`, `Role`), `parentSlug`
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
every class of eight libraries (see *Resolution*); the six
tag filters, `genre`, `country`, `director`, `writer`, `actor` and `producer`; `collection`;
and the four track filters, `videoCodec`, `audioCodec`, `audioLayout` and `subtitleCodec`,
with `=`, a comma `OR` and `!=` (see *Codecs and channel layouts*).

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

### Person pages

`GET /library/people/{personId}` and `/media`. **`personId` is a tag id** — the `id` a lookup
publishes on its `Role`, `Director`, `Writer` or `Producer`, and that `/hubs/search` publishes
on its person entries — not a person: Christopher Nolan the director (`553828`) and the writer
(`553829`) are two pages, listing different films (*Insomnia* on one, *Man of Steel* on the
other). The spec's alternative, the provider's `tagKey`, answers `404`; Plex Web only builds
that form for the plex.tv metadata provider. Any tag answers, a genre (`38`, Action) or a
country as well as a credit; a tag nothing carries any more is a `404`, as is a non-number.

The person is one `Directory` in a container of `size`, `identifier`, `mediaTagPrefix` and
`mediaTagVersion`: `id`, `filter` (`actor=531575`, `director=553828`, `genre=38`), `tag`,
`tagType` (1 genre, 4 director, 5 writer, 6 actor, 7 producer, 8 country), and `tagKey` and
`thumb` when the provider knows the person. Nothing else — no biography, birth or death:
**the reference database holds none.** `tags` has no column for them — of its 292,378 credits,
12,143 carry an `extra_data`, always `{"url":""}`, and none a `tag_value` or `user_art_url` — and Plex Web reads a person's biography from plex.tv's metadata
provider, not from the server. So there is nothing to import, and the official API's `Person`
carries only what the library does: name, provider key, portrait.

`/media` lists what carries the tag, in the listing form, each item naming its library
(`librarySectionTitle`, `librarySectionID`, `librarySectionKey`, after `titleSort`). Measured on
fourteen tags of seven kinds:

| Rule | Measured |
| --- | --- |
| A series is listed for a credit on it **or on one of its episodes**, once | Sammi Hanratty, credited on three episodes of *Pushing Daisies*, lists the series alone; `actor=64685` on `/all` finds nothing, since a tag filter reads an item's own tags |
| **Hidden libraries are left out** | Mia Wasikowska's *Alice* films in the two hidden 3D libraries do not appear; her 17 items come from two libraries |
| Libraries in **title order**, films and series interleaved | `VF - Films`, `VF - Films UHD`, `VF - Series`, `VF - Series UHD`, `VO - Movies`, … on Jason Momoa and Tim Burton |
| Within a film library, by title | DiCaprio's 18 films in `VF - Films` |
| Within a series library, **by id**, whatever the titles | four people, 59 series: *Plus belle la vie, encore plus belle* (457329) before *Plus belle la vie* (491172), *The Night Of* (23161) before *Master of None* (263788), added earlier |
| An actor's page carries `viewGroup="movie"` and `viewMode="65592"`, no other kind's | seven actors, against directors, writers, a producer, a genre, a country and a collection |
| Listing filters apply: `year`, `genre`, `actor`, `title`, `resolution`, `unwatched` | `year=2010` keeps DiCaprio's two *Inception* |
| `sort` and `type` are ignored, as is any unknown parameter | the same order and count under `sort=year:desc`, `type=2`, `type=4` |
| Paging takes both container parameters, adding `totalSize`, `offset` and the two headers | `Start=5&Size=3`: items 5–7 of 41; `Size` alone is ignored |
| Unpaged, everything | 4,699 items for Action, no `totalSize` |

**Implemented** on the official API as `Tag.items` and `Person.items` (every tag of the person),
with `itemCount` beside each, ordered by `ItemSort::Filmography`: libraries in the order the
query lists them, then films by title and series by id. The tag and the person are refetchable
only when an item in a library the caller can see credits them, which is what makes an unknown
tag, a stale one and one a user was not given the same `404`.

Deviations of ours. The list is **capped at 500** (`page::MAX_ITEMS`) where the reference lists
everything: an unpaged answer cut by the cap carries `totalSize` and `offset` so that `size`
never claims more than the body holds, and one that is whole is the reference's attribute for
attribute. Titles compare **byte-wise**, as every listing here does. Two libraries with the
same title would interleave their films, where the reference's order is not known. And as on
`/all`, the container parameters are read from the query only, not from headers.

Against the live reference on eighteen tags — actors with many films, directors, a writer, a
producer, a country, a guest of three episodes, a name with an accent, a stale tag and an
unknown id: the `Directory` identical on all sixteen that exist, both `404`s the same, and the
same order on all sixteen `/media` pages. Of the reference's 350 items we list 313; the 37
others carry the tag on the live server and not in the snapshot imported here, every one
checked. The items themselves are `/all`'s, attribute for attribute and in the same order, with
the three library attributes after `titleSort`; what differs is what differs on `/all` (no
artwork, `Image` or `UltraBlurColors`, the part `key` without its timestamp) and the watch
state of another account.

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
- **The `key` carries `%3E` for `>` and `%3C` for `<`**: a raw one is refused by this server's
  HTTP layer before any handler runs (*Range filter operators*), and a client follows the key
  as given.

A series library's `tv.moreingenre.{s}`, `More in {genre}`, comes after rediscover: a
`random` shelf of `type=2&unwatchedLeaves=1&genre={id}`. Its genre follows the viewer's
history. Measured on 260 draws over three libraries, each against its series ordered by
`lastViewedAt`, the series' own:

- take the series watched last, newest first, and the **first genre of each**, a repeat
  adding nothing, until there are **four**;
- the genre is drawn **evenly** among those four: 41, 37, 37, 34 of 150 in one library
  (Comedy, Fantasy, Animation, Reality, from its five series watched last: Fantasy twice), 19,
  19, 17, 14 of 70 in another, 12, 12, 11, 5 of 40 in the third — Comédie, Drame, Crime,
  Action, where it took seven series to reach a fourth;
- a series counts whether finished or not, at any distance in time: the seventh there was
  last viewed ten months before;
- no shelf when the viewer watched no series there.

Our draw is the shuffle's ten-minute one, salted for the shelf, as the other drawn shelves'.
Two draws in 260 gave no shelf at all on the reference, for a reason not found.

Three more shelves are drawn or dated, measured on 150 draws of a film library (each new
query string is a new draw there, kept for that string: `?nonce=1` answers the same until
the cache turns, as does each `X-Plex-Language`):

| Shelf | List behind it | `random` |
| --- | --- | --- |
| `movie.genre.{s}.{genre}` | `unwatched=1&genre={genre}&audienceRating>=7.0` | |
| `movie.by.actor.or.director.{s}.{person}` | `unwatched=1&director={person}&sort=audienceRating:desc`, or `actor=` | yes |
| `tv.rediscover.{s}` | `type=2&lastViewedAt<=-6w&lastViewedAt>=-5y&unwatchedLeaves=1&viewCount>=1` | yes |

- **The genre is drawn evenly among the library's**: 39 of 40 came up, one-film genres as
  often as the largest; a draw can give an empty shelf, and is still shown.
- **The person is a director or an actor about evenly** (81 and 69), at any billing — the 98th
  credited came up — and with no condition: one whose films were all watched came up too.
- Titles: `Top Movies in {genre}`, `Top Movies by {director}`, `Top Movies with {actor}`.

Ours draw the same way, from the shuffle's ten-minute seed, salted per shelf; which genre or
person comes up is not the reference's, as no draw could be. Rediscover reads the caller's
own history.

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

### A backdrop's colors (UltraBlur)

Plex Web 4.162 never calls `/services/ultrablur/*`: it draws its backdrop itself, a WebGL
blend of four corner colors (fallbacks `#333333`, `#202020`, `#262626`, `#1c1c1c`), from the
item's `UltraBlurColors` or colors it extracts in the browser. Other clients call the routes.

**On items.** The reference's analysis (`UltraBlurProcessor`) stores four colors per item in
`extra_data` (`pv:ultraBlurTopLeft`, `…TopRight`, `…BottomRight`, `…BottomLeft`, six lowercase
hex digits, always the four together) for 18,278 films, 3,992 series, 9,629 seasons, 138,666
episodes and 732 collections. It draws them from the item's `art`, else its parent's or
grandparent's, else its own `thumb` (read in the binary). Lookups, listings and shelves publish
them as `UltraBlurColors`, right after `Media` and `Image` and before `Genre`: an **object** in
JSON, attributes `topLeft`, `topRight`, `bottomRight`, `bottomLeft` in that order. On 160 random
items of four kinds, 154 publish exactly the stored values; the other six differ by the
snapshot's age. Imported into `item_corner_colors` (one row per item, `0xRRGGBB` integers),
published on films, series, seasons and episodes; collections keep theirs unimported.

**`GET /services/ultrablur/colors?url=`.** No `url` is `400`; a `url` the reference cannot read
— an empty one, `garbage`, a library image missing from its bundles — `404`; an image without a
color left after the white filter `500`. It fetches any absolute URL, its own loopback included,
and resolves relative ones against itself. The answer is `{"MediaContainer":{"size":1,
"UltraBlurColors":[{…}]}}`, an **array** here, `<UltraBlurColors …/>` in XML. The algorithm, read
from the binary and reproduced:

1. decode with OpenCV (three channels: alpha dropped, not blended); above 3840×2160 pixels,
   scale down by `sqrt(8294400 / pixels)`, bilinear;
2. cut into four quarters of `width/2` × `height/2` (an odd last row or column is ignored);
3. per quarter, a palette by Leptonica's modified median cut as `quantize.js` runs it — every
   fifth pixel, pixels above 250 on all three channels skipped, 5 bits per channel, 10 colors —
   with two quirks of Plex's port: the second pass counts its colors from one rather than from
   its boxes, and a cut whose upper half is empty walks back to the first non-empty slice without
   reading the counts ahead. A cut that leaves a box empty keeps it, as its center color: why a
   flat image still gets four distinct corners (black gives `0d0202`, `030403`, `0d0202`,
   `030403`);
4. per palette color, Okhsl and Oklab (Ottosson's code, in `float`; the Oklab used for distances
   is of the *encoded* values, not of linear light), and a score: `1.5 × dark + place + 3 ×
   saturated + 3 × apart − 2 × green − 2 × muddy − 2 × washed`, each a band worth 100 inside and
   falling linearly outside — `dark` lightness 0 to 0.4 (0 at 0.7), `place` the palette rank (100
   first, 0 last), `saturated` saturation 0.6 to 1 (0 at 0.2), `apart` the Oklab distance from the
   corner chosen before, 0.05 to 0.1 (0 at 0 and 1; 100 for the first corner), `green` hue 80° to
   120° (±10°), `muddy` the mean of hue 40° to 70° (±10°) and saturation 0.2 to 0.6 (0.1 either
   side) when both are non-zero, `washed` the mean of lightness 0.5 to 1 (0 at 0.3) and saturation
   0 to 0.2 (0 at 0.4) when both are non-zero;
5. the corners in the order top left (lightness capped at 0.2), top right and bottom left (0.4,
   apart from top left), bottom right (0.4, apart from bottom left); the best score wins, the
   earliest on a tie; a saturation strictly between 0.2 and 0.6 becomes 0.6; back to sRGB,
   clamped, truncated, written `{:02x}{:02x}{:02x}`.

Measured on 420 images served to the reference from a local web server: identical on all 269
PNGs (149 of Plex's own graphics, 120 synthetic), and on all 151 JPEGs once given the pixels
libjpeg-turbo decodes. Our JPEG decoder (`zune-jpeg`, through `image`) rounds differently, and a
pixel crossing a 5-bit boundary changes a palette: 34 of 150 photos of people from
`metadata-static.plex.tv` identical, 390 of 604 corners, 532 within 2 per channel, the rest
another palette color (up to 130 apart). Matching would take a libjpeg binding, which the image
policy rules out.

**`GET /services/ultrablur/image`.** A PNG, RGB 8-bit, `Cache-Control: no-cache`.
`width` and `height` default to 1920 and 1080 each on its own; the **area** is bounded at
3840×2160 (4000×100 and 4096×2025 are drawn, 4096×2026 is `400`), a value that is not an unsigned
integer is `400`, zero is `500`. `noise` is `0` or `1` (default), anything else `400`. The four
colors are required unless `url` is given, which draws the colors `/colors` would read (`500` when
it cannot). Each pixel starts black and is blended, per channel in `float` and truncated after
each step, towards bottom left, bottom right, top right and top left in that order, by `1 − d`,
`d` the distance to that corner with the axes divided by 95% of the image's sides; past `d = 1` the
weight is negative and pushes away from the color. Noise shifts `x` and `y` together by an integer
drawn uniformly in `±width/48`, per pixel. Without noise, ours is pixel-identical to the
reference at 97×61, 320×240 and 1920×1080; with it, the pixels differ (different random draws,
mean difference 1.3 per channel at 1920×1080) and the mean color agrees to 0.01.

We differ on purpose in four places. A remote image is fetched from Plex's hosts over TLS only, as
`/photo/:/transcode` does (`403` otherwise). A relative `url` answers only for an item's art,
`/library/metadata/{id}/art[/{stamp}]`, with the colors stored for the item, since this server
serves no artwork; any other relative address is `404`. A color the reference's `sscanf` cannot
read (`zzzzzz`, `#3f280a`, `3f28`) draws whatever was left on its stack; here it is `400`. Our
noise is drawn from a fixed seed, so an image is the same on every request.

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
the manifest once that exists, about 0.5 seconds later.

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

`subtitles=burn` with a selected subtitle forces a video transcode on the reference, and so here
(*Choosing a track, and burning subtitles in*).

### Choosing a track, and burning subtitles in

`PUT /library/parts/{partId}` is what Plex Web sends when its player's menu changes the audio
or the subtitles. Measured on the reference's test film:

- `audioStreamID` or `subtitleStreamID` names one of the part's tracks of that kind, and
  `subtitleStreamID=0` turns subtitles off. The answer is an empty `text/html` `200`, and the
  next lookup marks the track `selected`. A track of another kind or of no part of this one, or
  neither parameter, is a `400`. A `GET` on the same address changes nothing.
- **With no choice made**, the default audio track plays, and the forced subtitle in its
  language: French forced subtitles under French audio, unchosen.
- `allParts=1` is sent along; with one file per release it changes nothing measurable.

Ours match on each case. The choice is per viewer and per file, a visitor's automatic.

With a subtitle selected and `subtitles=burn`, which Plex Web in Chrome sends, the reference
re-encodes the video — 1920 wide, 20 Mbps and CRF 16 with no quality limit, the top of the
ladder — and draws the subtitle in. The decision marks it `decision="burn"`,
`location="segments-video"`, `burn="1"`. Its command, captured:

- `-map_inlineass 0:{subtitle}` before the video's filter, which gains a third stage:
  `[1]inlineass=font_scale={size}:font_path=…/NotoSans-Medium.otf:fonts_dir=…/Fonts:language=fr:
  overrides=…:outline=2.6:shadow=1.7:font_size=54[2]`, every value escaped twice;
- `font_scale` is `subtitleSize / 100`, six decimals: 0.5, 1.5, 2.0 for 50, 150, 200, the
  `font_size` staying 54;
- a second output, `-map 0:{subtitle} -f null -codec ass nullfile`, which decodes what the
  filter draws: without it the filter draws nothing.

Ours writes the same filter, character for character on the reference's own paths, from the
fonts of the Plex install (`Resources/Fonts` beside its binary, or
`OPENPLEX_TRANSCODER_FONTS`). Checked on a frame: the subtitle is drawn. Not handled, and
answering `501`: image subtitles (PGS, VobSub), which the process overlays by a command not
captured, and sidecar files.

**`burn` is not what Plex Web usually sends.** Its code asks for soft subtitles, which it draws
itself, whenever the server supports them — by version, which ours passes — and the user's
burn setting allows: `subtitles=sidecar` when it plays the file directly, `subtitles=auto` when
the server converts it, with `advancedSubtitles=text` under "only image formats". `burn` comes
only from "always", or when soft subtitles are impossible.

With `auto` over DASH, the reference copies the video and converts the subtitle to ASS beside
it, in the same process:

- the decision marks the track `codec`, `container` and `format` `ass`, `decision="transcode"`,
  `location="segments-subs"`; the manifest holds only the video and the audio;
- the command gains a second output: `-map 0:{subtitle} -metadata:s:0 language=fra -codec:0
  ass -strict_ts:0 0 -f segment -segment_format ass -segment_time 1 -segment_header_filename
  sub-header … -segment_list CB/manifest?stream=subtitles… -fflags +flush_packets sub-chunk-%05d`;
- Plex Web fetches `/video/:/transcode/universal/subtitles` with the stream's own parameters,
  which answers the ASS script whole — header, then each second's events — as the process
  writes it, on a response left open, declared `text/vtt`. Plex Web asks for it right after
  the decision, before it starts the stream: the reference answers at once and holds the
  body until the stream starts, and so do we, for up to 20 seconds. A start, there, needs the
  session to have asked for a decision first.

Ours does the same, the command identical to the captured one. Not handled yet: `sidecar`, for
a file played directly (`/subtitles/:/transcode/universal/start`), and soft subtitles over
HLS. A seek that restarts the process ends the subtitle response: the new process numbers its
pieces from 0 again.

**The process often sends a callback's head before its body, and waits for the answer.** With
two outputs it does so for most of its announcements, which the reference answers at once.
Answering only once the body has arrived, as we did, held both sides: the first announcement
passed and the rest, the DASH manifest included, never arrived. Callbacks are now answered on
their head and read after.

### Play queues

Measured on the reference with queues of its test libraries 27 (Soul) and 28 (Bull's two
episodes): creating, editing and reading back every operation below, then replaying the whole
sequence on both servers with the same trace.

**Creating.** `POST /playQueues` needs `type` and `uri` or `playlistID` (`400`). The URI is
`server://{machine}/com.plexapp.plugins.library{path}`. The path names a film or an episode (one
entry), a series (every episode), a season, a list of keys, or a library listing with its filters
and sort. `continuous=1` plays an episode on to the end of its series (the first episode gives
both, the last only itself) and changes nothing on a film. A URI that names nothing, another
server or an unknown item is still a `200`: an empty queue, without the selected attributes or
`playQueueTotalCount`. `key` selects the entry to play first, `shuffle=1` randomizes and plays the
selected entry first. `playQueueSourceURI` restates the source as `library://x/item/…` for one
item and `library://x/directory/…` otherwise. `onDeck`, `extrasPrefixCount` and the
`include…` flags change nothing on these libraries.

**One queue per player and type.** A new queue deletes the previous one the same
`X-Plex-Client-Identifier` made with the same `type`: `GET` on it answers `404` at once. Another
player's queue, or the same player's of another `type` (`audio`, `photo`), stays. This is the only
way the reference forgets a queue; the one it numbers 1 is still there. Without a client
identifier the reference refuses the request `400`; here such a queue is made and replaces
nothing. Queues are also deleted after 7 days, a figure of ours.

**The window**, measured on a queue of 22 centred on position 4: `window=N` (20 by default) shows
N entries on each side plus the centre. `includeBefore=0` or `includeAfter=0` drops that side
**and the centre**; both together are `400`. `window=0` shows nothing, and so does a `center` that
is not in this queue. Entries come in the form of a lookup, `playQueueItemID` first.
`repeat=1` on `GET`, measured on a queue of 5: a window clipped by one end takes what it lacks from
the other end, never showing an entry twice — `window=1` around the last entry shows the fourth,
the fifth and the first; `window=3` around the last shows 1, 2, 3, 4 and 0; a window that already
holds the whole queue is not reordered, and with either side refused nothing wraps.

**The "up next" area.** `playQueueLastAddedItemID`, after `playQueueID`, names the last entry
queued by hand. `PUT /playQueues/{id}?uri=…` inserts after it, or right after the entry playing
when there is none, and the last inserted becomes it. `next=1` inserts right after the entry
playing and leaves it alone, unless there was none. The area is positional only: an addition goes
after that entry even once it stands before the one playing. `playlistID` adds a playlist's items
the same way; a URI or playlist naming nothing adds nothing and leaves the version alone. Moving an
entry right after it makes the moved entry the area's end; moving or deleting the end itself makes
the entry before it the end. Clearing ends the area.

**Editing.** Every edit raises `playQueueVersion` by one, except a reset and an addition of
nothing. Unknown queues and entries answer `404` everywhere, entries of another queue included;
a non-numeric id or `after` is `400`.

| Operation | Rule measured |
| --- | --- |
| `PUT …/items/{id}/move` | `after` absent or `-1`: first. `after` an entry: right after it. `after` naming no entry (`0`, `999`): second, after the first other entry. `after` itself: `404`. The entry playing keeps playing. |
| `DELETE …/items/{id}` | The entry after the one playing plays when it goes. The answer is the container *as it was* — version, total and area's end before the deletion — around the entries left, centred on the entry that was playing: gone, the window is empty, `playQueueSelectedItemOffset` is 0 and `playQueueSelectedMetadataItemID` absent. |
| `DELETE …/items` | Every entry but the one playing goes; the answer is the queue after. |
| `PUT …/reset` | The first entry plays. The version stays, and the answer is the container as it was before the reset, with no entries. |
| `PUT …/shuffle` | Rebuilt from the source: the entry playing first with its id, every other entry new (new ids each time, removed entries back, entries queued by hand lost). `404` while the area has an end, or for an empty queue. |
| `PUT …/unshuffle` | Rebuilt the same way in natural order, the entry playing at its item's place; also on a queue that was never shuffled, which still renews its entries and raises the version. |

An entry playing that its source never held — queued by hand, everything else cleared — keeps its
id when the queue is rebuilt and plays the first item of the rebuilt order instead.

Here, a queue holds at most 500 entries, where the reference took all 9,667 films of a library:
the source is cut there, and an addition that would pass it is cut too. `GET … ?own=1` is
tolerated, and `onDeck=1` is not honored: the queue starts where `key` says.

### Playlists

Measured on the reference with playlists of its test libraries 27 (Soul) and 28 (Bull's two
episodes), every one deleted afterwards.

**Creating.** `POST /playlists` needs `type` and `title`, and `uri` or `playQueueID` (`400`
otherwise). The `uri` is a play queue's (`server://{machine}/com.plexapp.plugins.library{path}`):
a list of keys, a series or a season as its episodes, a library listing. Repeats are dropped,
within the request and against what the playlist holds: a video playlist holds an item once. A
URI of another server, or of an unknown item, makes an empty playlist with a `200`.
`playQueueID` copies a queue's items; a queue the caller does not have copies nothing, and so does
the reference's queue of a playlist, which holds the playlist rather than its items.
`smart=1` with a library listing saves the listing as the playlist's rule; its items are the
listing run anew on every read, in the listing's order, without `playlistItemID`. Here `smart=1`
with anything but a listing of this server's libraries is `400`, and a filter the listing cannot
honor is `501`, so that no rule is saved that lists less than it says.

**What a playlist shows.** `ratingKey`, `key` (`/playlists/{id}/items`), `guid`
(`com.plexapp.agents.none://{uuid}`), `type="playlist"`, `title`, `summary`, `smart`,
`playlistType`, `composite` (`/playlists/{id}/composite/{updatedAt}`, absent when the playlist is
empty), `icon="playlist://image.smart"` on a smart one, `duration` in milliseconds (absent when
empty), `leafCount`, `addedAt`, `updatedAt`, then `Field` for its locks. Three places differ:

| Where | Adds |
| --- | --- |
| `GET /playlists` | `viewCount` and `lastViewedAt`, after `icon` |
| `GET /playlists/{id}` | `content` first on a smart playlist: its listing, restated `library://x/directory/{path}` |
| `POST`, `PUT …/items` and every change | Neither; `PUT …/items` adds `leafCountAdded` and `leafCountRequested` to the container, on a playlist of chosen items only |

`viewCount` starts at 1 and `lastViewedAt` at the creation: the reference counts the creation as a
play, then each play queue made from the playlist (`POST /playQueues?playlistID=`, or a `uri` of
`/playlists/{id}/items`). That queue names `playQueuePlaylistID` and `playQueuePlaylistTitle`
after `playQueueID`, and has no `playQueueSourceURI`. XML names the element `<Playlist>` and, like
every item, writes it in the long form even with nothing inside.

The duration is whole seconds per item, summed: 6,031,568 ms and 2,467,160 ms make
`duration="8498000"`. Ours sums the items' running times (a version's, else the item's) and
drops the milliseconds of the total, so a playlist of n items can read up to n − 1 seconds more
than the reference's. A smart playlist has no duration here; the reference computes one.

**Its items.** `GET /playlists/{id}/items` lists the items in a listing's form with their library
(`librarySectionTitle`, `librarySectionID`, `librarySectionKey`), and `playlistItemID` just before
`duration`. The container carries `composite` (even empty), `duration` in **seconds**,
`leafCount`, `playlistType`, `ratingKey`, `smart` and `title`, and pages with the container
parameters like a listing. `type=4` keeps the episodes; values past the first serve a background
list the reference keeps and we do not.

**Order.** `/playlists` puts smart playlists first, then the rest by title — with
`sort=titleSort:desc` too, smart still first. Any other sort (`addedAt`, `lastViewedAt`,
`duration`…) orders by its value alone, ties by title. We serve the title, `addedAt`,
`updatedAt` and `lastViewedAt` orders and answer `501` for `duration` and the counts, which a
smart playlist knows only by listing. `playlistType`, `smart` and `type=15` filter;
`sectionID` keeps the playlists of the library's kind, a smart one only if it lists that library.

Entries move to fractional orders on the reference (1000, then 1500 between 1000 and 2000, 500
before the first). Ours are integers 1,024 apart that a move halves, and a playlist renumbers
when two neighbours touch. `PUT …/items/{id}/move` without `after` moves the entry first; an
`after` naming no entry of the playlist moves it first on the reference too, where we answer
`400`, and `after` naming the entry itself leaves it in place.

**Statuses.**

| Request | Reference | Here |
| --- | --- | --- |
| `GET`, `DELETE`, `PUT /playlists/{unknown}` and its `items`, `generators` | `404` | Same |
| `GET /playlists/{a film's id}` | The film, as `/library/metadata` shows it | `404`: playlists have ids of their own, so a film's id is no playlist's |
| `PUT /playlists/{a film's id}?title=` | `200`, and **the film is renamed** | `404` |
| `GET /playlists/{a film's id}/items` | `500` | `404` |
| `DELETE /playlists/{id}` | `204`, then `404` | Same |
| `PUT /playlists/{id}?title=&summary=` | `200`, empty `text/html` body; `.value` and `.locked` also work | Same; artwork fields answer `501` |
| `DELETE …/items`, `…/move`, `DELETE …/items/{id}`, `PUT …/items` with an item, on a smart playlist | `400` | Same |
| `DELETE …/items/{entry}` twice, or an unknown entry | `404` | Same |
| `GET …/items/{generator}`, `…/items/{generator}/items`, `PUT …/{metadataId}/reprocess` | `404`, on a chosen item's entry and on a smart playlist's rule alike | `404`: these are the optimizer's, and nothing here optimizes |
| `PUT …/items/{generator}?Item[title]=` | `400` | Same on the caller's playlist |
| `POST /playlists?type=audio` with a film | `400`, **and the playlist is created** | `400`, nothing created |
| `POST /playlists/upload` without `path` | `400` | Same; with a `path`, `403` |

**Generators.** `GET /playlists/{id}/generators` lists a `PlayQueueGenerator` per entry
(`id` = `playlistItemID`, `playlistID`, `metadataItemID`, `order`, `createdAt`, `updatedAt`,
`changedAt`, `type=-1`), or one carrying the `uri` for a smart playlist. Ours has no `changedAt`
(an internal counter), gives an entry's position as its `order`, its addition time as both dates,
and the playlist's id as a smart playlist's generator id.

**Ownership, which is ours.** A playlist is its owner's: one user's, an owner of the server's
included, keyed by its user id as watch state is. Another viewer's playlist answers `404`
everywhere; a request with no caller has none. An entry whose item lies in a library its owner may no longer read is hidden and
not counted, and comes back with the library; an item deleted from the library is hidden the same
way and returns if it is imported again under its id.

**Not served.** `POST /playlists/upload` imports `.m3u` files from a path the client names on the
server, and no endpoint here accepts a path from a client (`security.md`): it stays refused, by
design rather than for want of time. `composite` points at an image of the items' art, and
`/playlists/{id}/composite/{updatedAt}` is not served, since this server serves no artwork yet.
A playlist's ids are its own, from 1, where the reference draws them from its items' ids: a
client reading `/library/metadata/{ratingKey}` for a playlist finds the item of that id instead.

### Markers: intro, credits, bookmarks

What "Skip intro" and "Skip credits" read. Plex keeps two kinds of marker in two places, and
publishes them in one element:

- **the work's**, found by its analysis: one `taggings` row per marker, attached to the one tag
  of `tag_type` 12 (189893 here), the kind in `text` (`intro`, `credits`; `commercial` from the
  advertisement detection, none here), the span in `time_offset` and `end_time_offset`, and
  `extra_data` packing `pv:final=1` on the credits that run to the end and `pv:version` for the
  detection (`4` for credits, `5` for intros). 217,863 in the reference library: 80,228 intros and
  137,635 credits, 124,636 of them final, on 20,713 films and 197,150 episodes. `index` numbers
  an item's markers in the order they were found, which is `id` order on every row;
- **an account's**, `metadata_item_setting_markers` under the account's `metadata_item_settings`
  row: `marker_type` (3 for a bookmark), the span, `title`, and `extra_data` packing
  `at:color`. Empty in both databases; this is what `POST`/`PUT`/`DELETE …/marker` write.

**The reference publishes the work's markers only with a subscription.** Its account has none
(`myPlexSubscription="0"`), and no route shows them: not `/library/metadata/{id}?includeMarkers=1`,
`/children`, `/grandchildren`, `/library/sections/{id}/all` nor the `includeElements` forms its
binary carries, on an episode whose two markers its live database holds. An account's
bookmarks it publishes regardless. So the element's shape is measured on bookmarks, and on the
work's markers it is the spec's example plus what the analysis stores:

| | Bookmark (measured) | The work's (spec and storage) |
| --- | --- | --- |
| `id` | absent | the marker's |
| `type` | `bookmark` | `intro`, `credits`, `commercial` |
| `startTimeOffset`, `endTimeOffset` | ms; the end absent for a point | ms |
| `final` | absent | `1` / `true` on final credits only |
| `<Attributes>` | `color`, `id`, `title`, in that order, an absent one omitted | `id`, `version` |

`Attributes` is an object in JSON (`"Attributes":{"id":1}`), not an array, and `id` a number.
`version` is published as a number; the reference was not seen publishing it. Markers come in the
order they were added, after the tags and before `Field` (measured on a listing, after a
`Collection`, and on a lookup), and only with `includeMarkers=1`: `0` or nothing omits them. On
`/children`, `/grandchildren`, `/allLeaves`, `/library/sections/{id}/all` and play queues as on a
lookup — the reference's binary asks its own `/children` and `/grandchildren` for them, and
Plex Web sends `includeMarkers=1` on every lookup and play queue.

**Writing bookmarks**, measured on Soul (27) and Bull's first episode (28), every bookmark deleted
afterwards:

| Request | Reference | Here |
| --- | --- | --- |
| `POST …/marker?type=bookmark&startTimeOffset=10000&endTimeOffset=20000` | `200`, the container with `<Marker type="bookmark" …><Attributes /></Marker>`: no id yet | Same |
| `type=0`…`3`, `type=foo` | `400` (`Could not convert "type"`) | Same |
| `type=intro`, `credits`, `commercial` | Not measured: a marker made this way could not be removed, `DELETE` refusing what is not a bookmark | `400`: the work's markers are written on the official API |
| No `startTimeOffset`, `-5`, `x`; an end at or before the start | `400` | Same |
| An end left out | A point: no `endTimeOffset` | Same |
| `attributes[title]`, `attributes[color]`, `attributes[foo]` | Title and colour kept, the rest ignored | Same |
| The same span again | `200` with the bookmark already there (`<Attributes id="4" />`), its title unchanged; a start alone and the same start with an end are two | Same |
| Overlapping another bookmark | Allowed | Same |
| An item that does not exist | `404` | Same |
| A series | `200`, a bookmark on it | `400`: nothing plays a series |
| `PUT …/marker/{id}?startTimeOffset=` | `200`, the bookmark with its attributes; `type` ignored, the end and the title kept when not given | Same |
| `PUT`/`DELETE` a marker of another item | `400` | Same |
| `PUT`/`DELETE` an unknown marker | `404` | Same |
| `DELETE …/marker/{id}` | `200`, empty `text/html` | Same |
| `DELETE` of a marker that is not a bookmark | `400` (the spec) | Same; `PUT` likewise |

Bookmarks are the caller's: another user, an owner of the server included, does not see them,
and a `PUT` or
`DELETE` of someone else's answers `404`. `mediaTagVersion` is `0` here, as everywhere.

**The official API** holds both kinds: `Movie.markers` and `Episode.markers`, the work's for
whoever reads the item and the caller's own bookmarks; `addMarker`, `updateMarker`,
`deleteMarker`, for a library's admins and the server's owners on the work's markers, for its keeper on
a bookmark. A marker of the work edited by hand loses its `version`. Ours, not measured: the
credits alone are final, every marker but a bookmark has an end, and a title or colour is a
bookmark's.

**Imported, not detected.** `PUT /library/metadata/{ids}/intro` and `/credits`, which start the
detection on an item, and `DELETE /library/sections/{id}/intros` are `deferred` with the rest of
the analysis. A re-import replaces the work's markers, edits included, and leaves the viewers'
bookmarks: they are keyed by item without a foreign key, as playlist entries are. 51 credits of the
reference library end before they start — at 50 to 72% of it, all written within three hours of
one day by detection version 4; they are imported and published as stored.

Every populated column of a marker's `taggings` row, and where it goes:

| Column | Populated | Here |
| --- | --: | --- |
| `metadata_item_id` | 217,863 | `item_marker.item_id` |
| `tag_id` | 217,863 | the marker tag: what makes the row a marker |
| `text` | 217,863 | `kind` |
| `time_offset`, `end_time_offset` | 217,863 | `start_ms`, `end_ms` |
| `index` | 217,863 | the order of our ids, which `index` matches on every row |
| `extra_data` `pv:final`, `pv:version` | 124,636, 217,863 | `is_final`, `version` |
| `extra_data` `url` | 217,863 | not kept: the same pairs, URL-encoded |
| `created_at` | 217,863 | not kept: published nowhere |
| `id` | 217,863 | not kept: our ids are our own |
| `thumb_url` | 0 | — |

### Extras: trailers, deleted scenes, featurettes

**How the reference keeps them.** An extra is a metadata item of `metadata_type` 12 outside any
library (`library_section_id` null), tied to the item it comes with by a `metadata_relations`
row (`metadata_item_id` the item, `related_metadata_item_id` the extra), its kind in its packed
`extra_data` (`ex:extraType=1`) and repeated as the relation's `relation_type`; the two
disagree on 2 of 30,183 rows, and the item's is the one its lookup publishes. 30,260 clips in the
reference library, 30,253 of them linked, every linked one an Internet Video Archive stream its
metadata provider supplied (`guid` `iva://…`): by relation 24,659 trailers, 4,915 behind the
scenes, 423 scenes, 186 interviews, and 70 of kind `-1`, which no client names and we leave out. Each has one
version per bitrate (85,963 versions, 3 for most), one part per version without exception, its
address packed in the part's `extra_data` (`at:key=/services/iva/assets/200752/video.mp4?…`),
and a video and an audio track whose sizes are packed in the track's. An item has 50 extras at
most. A local extra is the same row with a file: a part with a path and a full analysis.

**The reference lists none of the provider's to an account without a subscription.** On 60
items of the snapshot carrying 1 to 50 extras, `/extras` answered `size="0"` every time, and a
lookup's `Extras` is empty; `/library/metadata/{extra}` answers each one, with
`premium="1"` on its versions. A series still names its first one as `primaryExtraKey` (59 of 59
series with a trailer); a film does not (0 of 180), and both carry `hasPremiumPrimaryExtra="1"`
instead, and `hasPremiumExtras="1"` on 15 of the 19 with another kind. There is no paywall here:
we list them, each entry as the reference looks it up, and name a film's first trailer as a
series'. `hasPremiumPrimaryExtra` and `hasPremiumExtras` are not published.

**The rules,** measured on the test libraries with local extras of every kind beside a film, a
series, a season and an episode (every one removed afterwards):

| Request | Reference | Here |
| --- | --- | --- |
| `GET /library/metadata/{id}/extras` | The item's own extras, by `index` then id (`order by related_items.index`, as its binary queries): a series lists its own, not its seasons', and a season or an episode its own | Same, an extra without a position first |
| `extraType=1`, `5`, `1,2`, `99`, `trailer`; `type=12`; `sort=title`; `includeExtraTypes` | Change nothing | Same |
| `X-Plex-Container-Start` and `-Size` together | A window, with `totalSize`, `offset` and the two paging headers; either alone ignored | Same |
| A list of ids, an unknown number, `abc` | `404` | Same |
| An extra's own extras | `400` | Same |
| `?includeExtras=1` on a lookup | `Extras size="n"` last, after `Location`, on every item of the lookup, `size="0"` (long XML form) for an item without | Same |
| `includeExtras=0`, `2`, `true`; on `/all`, `/children` | No `Extras` | Same |
| `primaryExtraKey` | The first trailer's key, after the last attribute, on films and series in lookups, listings and `/search`; absent with no trailer, whatever other extras there are | Same |
| `extrasPrefixCount=0…5` on `POST /playQueues` of a film with three trailers | Changes nothing | Same |

The entry is a `clip`: `ratingKey`, `key`, `guid`, `type`, `title`, `titleSort` (when it differs),
`summary` (always, empty without one), `index`, `year`, `primaryGuid` (the item's `guid`, on 246
of 280 provider extras looked up, every one of an item Plex's agent matched, and on none of the
local extras of an unmatched film), `subtype` (`trailer`, `deletedScene`, `interview`, `musicVideo`, `behindTheScenes`,
`sceneOrSample`, `liveMusicVideo`, `lyricMusicVideo`, `concert`, `featurette`, `short`, `other`),
`duration` (the first version's, as an item's: the reference publishes one on 63 of 264 trailers
whose own row has none), `originallyAvailableAt`, `addedAt` and `updatedAt` (when set),
`extraType` (1 to 12 in that order), then `Media` with its `Part` and `Stream` elements; the container carries `size` and the
library identifier only. In a lookup's `Extras` the same, without `art`. A provider's version
carries `optimizedForStreaming="1"` and `protocol` (its container) when its own flag is set,
`premium="1"`, a part keyed by the provider's address with no `file`, and the part's own
`optimizedForStreaming` when that is what the source set: 59,557 versions have the first, 26,406
parts the second. Local extras number their parts `/library/parts/{id}/0/file.{ext}` as here.
A track's language is named in the client's `X-Plex-Language`, and with none in itself
(`Français`, `English`): ours falls back to English, as on items.

**Compared field by field** (`/extras` of 150 imported items against the reference's lookup of
each extra, artwork aside, `X-Plex-Language: en` on both): 264 extras, 33 more no longer on the
reference; after the two rules above, the one difference left is `primaryGuid` on 34. What
decides it is in no column of the snapshot — not the clip's, its media's or its relation's: we
publish it on every extra of an item with a `plex://` guid.

Where we differ: artwork (`thumb`, `art`, `Image`) is left out, as on items. A provider's part
lists `key` before `duration`, where the reference writes it after `container`. A local extra's
tracks keep codec, language, channels, bitrate and size from the import, not the rest of their
analysis, which no extra of the reference library has. The provider's address is published as
imported and answers `404` here: this server streams no provider's trailer, and the extras of
the imported library are no more playable than its files. `/library/metadata/{extra}`, which the
reference answers, is a `404`: an extra is no item.

**Adding one.** `POST /library/metadata/{ids}/extras?url=&title=&extraType=`: the reference
answers `404` for an unknown item, then `400` without `url` or `title` (an empty title passes)
and for an extra, then `404` for every address measured, a remote `http://` file and a local
`file://` one, adding nothing. Here the same checks, then `403`: the `url` is a path or an address
to fetch, and no endpoint takes one from a client.

**The official API** has `Movie.extras`, `Show.extras`, `Season.extras`, `Episode.extras` (a
connection, by position then id), `Movie.primaryExtra` and `Show.primaryExtra`, and `Extra` as a
node with `kind`, its item and `versions`, which are `MediaVersion`s: their files and tracks
refetch like an item's, by whoever reads the item. `MediaFile.streamUrl` holds the provider's
address. Nothing writes an extra.

Every populated column of a linked extra's rows, and where it goes:

| Column | Populated | Here |
| --- | --: | --- |
| `metadata_relations.metadata_item_id` | 30,253 | `extra.item_id`; an item not imported leaves it out |
| `metadata_relations.relation_type` | 30,183 | the kind, where `extra_data` names none |
| `metadata_items.id`, `guid`, `title`, `index` | 30,253 | `id`, `guid`, `title`, `position` |
| `title_sort` | 30,253 | `title_sort`; differs from the title on 5,562 |
| `extra_data` `ex:extraType` | 30,253 | `kind`; `-1` left out |
| `duration`, `year`, `originally_available_at` | 21,451, 17,442, 17,442 | `duration_ms`, `year`, `released_on` |
| `added_at`, `updated_at` | 29,390, 862 | `added_at` (0 when absent), `updated_at` |
| `user_thumb_url`, `user_art_url` | 30,095, 5 | not kept: artwork is not served |
| `media_item_count`, `created_at`, `hash`, `changed_at`, `resources_changed_at` | 30,253 to 25,703 | not kept: counters and stamps published nowhere |
| `media_items` `width`, `height`, `display_aspect_ratio`, `duration`, `container`, codecs | 85,963 | `extra_media` |
| `media_items.bitrate` | 82,052 | `bitrate` |
| `media_items.optimized_for_streaming`, `extra_data` `ma:protocol` | 59,557 | `optimized_for_streaming`; the protocol is the container on every row |
| `media_items.display_offset` | 38,085 | not kept: published nowhere |
| `media_parts.id`, `duration` | 85,963 | `part_id`, the version's duration |
| `media_parts.extra_data` `at:key`, `ma:optimizedForStreaming` | 85,963, 26,406 | `stream_url`, `file_optimized_for_streaming` |
| `media_streams` `stream_type_id`, `index`, `codec`, `channels`, `bitrate`, `language` | 171,926 to 71,416 | `extra_track` |
| `media_streams.extra_data` `ma:width`, `ma:height` | 85,963 | `width`, `height` |

### Collections

Measured on the reference with collections of its test libraries 27 (Soul) and 28 (Bull, its
season 5 and two episodes), every one deleted afterwards, and on the imported library read only.

**How the reference keeps them.** A collection is an item of type 18 in its library, holding the
title, summary, settings (`pr:collectionMode`, `pr:collectionSort` in `extra_data`) and locks
(`user_fields`); its members carry a tag of type 2 whose id is the collection's `index`. One tag
serves every library with a collection of that name — 588 tags span several — so a membership is
a tagging of an item whose library holds a collection under that number. `rating_count` stores
the `type` the collection was created with, plus 100 for a smart one. We keep the collection and
its members as entries of it (`collection`, `collection_entry`), with the number as
`tagNumber`. The imported library gives 2,295 collections and 4,586 entries: 7 taggings name a
number no collection of the item's library carries and are left out, as are 3 repeated ones and
the 4 smart collections, which all list `episode.duplicate`, a filter the official API cannot
express. 115 collections keep a number whose tag is gone: that is what emptying through the tag
leaves on the reference (below).

Of the columns the reference fills on its 2,299 collections, the importer keeps `id`,
`library_section_id`, `guid`, `title`, `title_sort`, `summary`, `content_rating`, `index` (the
number), `rating_count` (the kind), `added_at`, `updated_at`, `pr:collectionMode` and
`pr:collectionSort`, the locks of `user_fields`, and the tag's `user_thumb_url` and
`user_art_url` when they are web addresses (544 and 506). Left out, each for a reason:
`at:childCount`, `at:minYear`, `at:maxYear`, which are derived from the entries (2 of 2,299
stale); the local poster (`user_thumb_url`, 788) and every blur hash and ultra-blur colour,
artwork not being served; `absolute_index` (always 1), `created_at` (always `added_at`), `hash`,
`changed_at` and `resources_changed_at`, which are a scanner's bookkeeping; the tag's `at:guid`
and `at:summary`, see below.

**What a collection shows.** `ratingKey`, `key` (`/library/collections/{id}/children`), `guid`
(`collection://{uuid}` when made here), `type="collection"`, `title`, `titleSort` when it differs,
the library trio where a lookup puts it, `contentRating`, `subtype` (`movie`, `show`, `season`,
`episode`), `summary` always, empty included, `index` (the number), `content` on a smart one (its
`uri`, as given), `ratingCount`, `thumb` (`/library/collections/{id}/composite/{updatedAt}?width=400&height=600`),
`addedAt`, `updatedAt`, `childCount`, `collectionMode` and `collectionSort` once set, `maxYear` and
`minYear` as **strings** (a smart collection has none), `smart="1"`, then `Field` for its locks in
a lookup. XML names it `<Directory>`, in the long form.

| Where | Shape |
| --- | --- |
| `GET /library/sections/{id}/collections`, `/all?type=18` | `/all`'s container; no library on the collection |
| `GET /library/metadata/{id}`, `/library/collections/{id}` | The lookup container; the library trio after `title`, and `Field` |
| `POST /library/collections` | `{size, Metadata}`; no library on the collection |
| `PUT …/items`, `DELETE …/items/{item}`, `PUT …/move` | `{size, Metadata}`; the library trio, no `Field` |
| `GET …/children`, `…/items` | `{size, Metadata}` of items in the listing form with their library; `totalSize` and `offset` with both container parameters |

`updatedAt` moves when an item joins or leaves, and only then: not on a repeat, an edit, a
setting or a move. A lookup of `{collection},{item}` answers both, in that order. Imported
collections read identically to the reference's, artwork and `mediaTagVersion` aside.

**Its items.** Release date by default, items without one first (`collectionSort` 0); title (1);
custom (2), the order `PUT …/items/{item}/move?after=` arranges, without `after` first. Measured
on two episodes, of which one has no date: 0 and 1 both listed the undated one first, so the
title order is not confirmed beyond what an item listing does. A smart collection lists its rule
anew, in the rule's order.

**Creating and changing.**

| Request | Reference | Here |
| --- | --- | --- |
| `POST` without `sectionId`, with an unknown one, without `title` | `400` | Same |
| `POST` without `type` | The library's top level | Same |
| `POST?type=99` | `200`, a collection of `subtype="unknown"`, `ratingCount=99` | `400`; nothing could join it |
| `POST` with items of another kind or library in `uri` | `200`; they are left out | Same |
| `POST?uri=garbage` | `200`, empty | Same |
| `POST?smart=1` without `uri` | `400` | Same; a listing of another library `400`, a filter we cannot honor `501` |
| `PUT …/items` with only items that cannot join, no `uri`, `uri=garbage` | `400` | Same |
| `PUT …/items` with a repeat, or one that can join among others | `200` | Same |
| `PUT …/items`, `…/move` on a smart collection | `400` | Same; `DELETE …/items/{item}` there changes nothing, as on the reference |
| `DELETE …/items/{item}` twice, or for an item not in it | `200` | Same |
| **`PUT …/items/{item}`**, the spec's removal | `404` | Same: `DELETE` removes |
| `…/move?after=` itself, an item not in it, a word | `400` | Same |
| `…/move?after=` an item not in it | `200`, moved first | Same |
| Any of these on an unknown collection | `404` | Same |
| `PUT /library/metadata/{id}/prefs?collectionSort=&collectionMode=` | `200`, empty; an unknown name `400` | Same; `collectionSort=7`, which the reference keeps, `400` |
| `GET /library/metadata/{id}/prefs` | `404` | Not served either |
| `PUT /library/sections/{id}/all?type=18&id=` | Every field an item takes, `year`, `studio`, `tagline` included | `title`, `titleSort`, `summary`, `contentRating` and their locks, `thumb` and `art` locks; the rest `501` |
| `DELETE /library/metadata/{id}` | `200`, then `404` | Same |
| **`DELETE /library/sections/{s}/collection/{n}`** | `n` is the **tag number**: every item of library `s` leaves the collection, which stays, `updatedAt` untouched; another library's number `200`, an unknown `404` | Same |
| `GET …/composite/{updatedAt}` | `404`, for every collection tried | Same |

A title edit derives the sort title, as an item's does, and a sort title given with it is kept
here (the reference derives over it unless it is locked).

**Rights, which are ours.** A collection is its library's: readable by whoever reads the library,
changed by its admins and the server's owners, as items are edited. What a caller may not change answers
as if it did not exist: `404` on the Plex surface, `COLLECTION_NOT_FOUND` on the official API.

**Where we differ.**

- **An item names its collections by their own title, guid and summary.** The reference names
  the tag they share across libraries: its name, whose case differs from the collection's title
  on 38 of 2,299 (`Men in Black - Saga` for `Men In Black - Saga`), and the tag's `at:guid` and
  `at:summary`, which 1,231 tags lack and 162 take from a sibling library's collection. On 100
  sampled films: 36 identical, 52 with a `guid` the reference leaves out, 6 with another `guid`,
  6 without the tag's `summary`, 6 names in another case; the order, the order the item joined
  them in, identical on all.
- **A listing names one collection, the first the item joined.** The reference reads the listing
  form from the stale `tags_collection` column — measured identical on 16,827 of 16,834 films —
  which holds one name, or none for 1,343 films in a collection.
- **`childCount` is always written.** The reference leaves it out of a collection that never held
  an item, and writes `0` once one has left.
- **`collectionMode="-1"` is never written**, the library's setting being what storage keeps when
  none is set; the reference writes it once a client set it.
- **Collations.** Collections list by sort title in byte order, as items do: `21 Jump Street`
  before `3 histoires de l'au-delà`, where the reference's numeric collation puts `3` first.
- **`includeCollections=1` mixes nothing into `/all`.** The reference does it in a library whose
  collection mode shows collections (`2`, its test libraries' setting), not in one that hides them
  (`0`, all of its real libraries'); collections are listed apart here, by `type=18` and
  `/collections`.
- **Numbers.** A collection made here takes the id past the highest item's or collection's, and
  the number past the highest collection's: a later import of a library holding an item of that
  id would clash. An import replaces every library's contents, collections made here included.
- No shelf of collections was seen on the reference's `/hubs/sections/{id}`, so none is served.

**Search and filter choices.** `/hubs/search` lists collections in its `collection` shelf as tags
(`type="tag"`, `tagType="2"`): key `/library/sections/{s}/all?collection={n}`, the library and
`reason`, the score of a tag, `id`, `filter`, `tag`, `thumb` and `art` (the provider's poster and
background, which the tag holds), `count` (its items), `guid` and `summary`; one entry per library,
ranked as people are. `GET /library/sections/{id}/collection` lists the collections holding an
item — 1,024 of a film library's 1,116 on the reference — as `fastKey`, `thumb`, `key` (the
number) and `title`, under `title2="By Collection"`. `collection=` filters a listing by number or
title, comma for OR, as a tag filter does.

### Playback sessions are stored

None of the reference's 71 tables holds a session in progress: only what outlives one, the play
queues and the viewing history. Ours are stored in `playback_session`, by the repo owner's
decision, and exposed by `playbackSessions` on the official API, which `/status/sessions` reads.

A session expires by itself, 300 seconds after its last sign of life, the same limit as the
transcoder's idle processes. Every sign of life counts for the session and its stream together:
a `ping`, a timeline report, `touchPlaybackSession`, `reportPlayback`, a playlist or a segment
fetched from the Plex routes, and a read of a stream's own address (`/streams/{id}/…`), which
pushes the session's expiry forward at most every 30 seconds. An expired session is no longer
read at once. At startup every row is deleted and the transcoder's directory emptied. A server
killed outright (not stopped with Ctrl-C) leaves its processes running until their source, which
they read back from the server, stops answering: measured here, 5 to 15 seconds. One still alive
at the next start holds its directory, so the sweep also removes any directory no session owns
once nothing has written to it for 60 seconds.

In `/status/sessions`, `Session.id` and `sessionKey` follow a capture of the reference (the
player's session identifier, and a number of the server's as a string); the other attributes are
what Plex Web reads. `Player.state` is what `/:/timeline` last reported. Direct Play through the
Plex routes creates no session: only the timeline could reveal one, and we do not open one from it.

### Ending a session

A session and its stream begin together and end together (`core::lifecycle`). Whatever ends one
stops the process, forgets the stream's addresses and their secret, lets its directory go once
the process has exited, and deletes the row, each change to a key made under that key's lock:

| What ends it | Path |
| --- | --- |
| The player stops | `/:/timeline` with `state=stopped`, or `reportPlayback(STOPPED)`: every session of the playback |
| A client stops the conversion | `/video/:/transcode/universal/stop`, `endPlaybackSession`, `stopStream` |
| A new start replaces it | a change of quality under the same playback (`startStream`) |
| An owner | `/status/sessions/terminate`, `terminatePlaybackSession` (below) |
| Nobody gives a sign of life | the sweep, every 30 seconds: a row past its expiry, or a stream whose process has gone idle for 300 seconds or has been stopped |
| A stream read whose session is gone | the stream is stopped there and then, and answers `404` |
| The server stops | Ctrl-C ends every session before the server stops listening, then waits 5 seconds at most for its connections |

An automatic end checks again under the lock that its reason still holds, so a player restarting
under the same key between the sweep's listing and its deletion keeps what it started. A file
played as it is (direct play through `startStream`) lives as long as its session, where it used
to stay reachable for six hours: a player keeps it with `reportPlayback` or by reading it.

Measured on the reference with a remux of its test film (library 27): `stop` answers `200` once
and `404` after, as does `ping`; the reference then keeps listing the session in
`/status/sessions`, its `TranscodeSession` still there, until the player's timeline reports
`stopped`. Ours ends the session with its conversion, by the repo owner's decision that a
session never outlives its stream.

### Terminating a session

`/status/sessions/terminate` is a subscription feature on the reference: without one it answers
`401` to any `sessionId`, known or not, and on every method, so only its refusals could be
measured. `400` with no `sessionId` at all is the reference's, measured; `403` for an empty one
and `404` for an unknown one are the spec's. `sessionId` is `Session.id` from `/status/sessions`,
which on the reference is the player's `X-Plex-Session-Identifier`, measured; it was the
conversion's key here before, and now follows the reference. Plex Web calls it with `GET`, so
both `GET` and `POST` are served.

What a terminated player sees comes from the reference's binary and Plex Web, not from a capture:
the binary logs `Denying access to transcode of key %s due to terminated session` and carries
`terminationCode`, `terminationText` and the text `Admin Terminated Playback.` in its decision
table; Plex Web stops playback with `terminationText` as the error when a timeline answer holds a
`terminationCode`. Ours stops the conversion at once and keeps the row, marked terminated and
listed nowhere, for one more lifetime: the player's next timeline answers
`terminationCode="2006"` and the owner's `reason` as `terminationText`, or
`Admin Terminated Playback.` without one. The code is the one Plex's published decision codes
give that text; Plex Web only checks that one is there. The player's `stopped` then removes the
row. Until then a start under the same key is refused, as the binary's log line says the
reference refuses it: `SESSION_TERMINATED` on `startStream`, `403` on `start.m3u8` and
`start.mpd`, a status of ours.

`/status/sessions/background` lists the optimizer's and downloads' conversions, none of which run
here: it is always the empty container the reference answers with none running, measured in both
formats.

### One server per user

Nothing here comes from the reference, whose sharing goes through plex.tv. A user token is a
Plex token: `X-Plex-Token`, in a header or the query string. The Plex surface then answers with
that user's `machineIdentifier` on `/`, `/identity` and `/media/providers`, lists only the
libraries it was given, and answers `404` for anything else. A library's `viewer` browses and
plays; its `admin` also gets past the check on `PUT /library/sections/{id}/prefs`. `/status/sessions`
and `/updater/*` stay with the server's owners, who keep the server's own identifier unless an
extension of theirs gives them one.

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

The reference announces **one** representation in DASH and one variant in HLS (five client
profiles, LAN and WAN), and its binary holds a mechanism to move it: `Segments have been delayed`,
`Client buffer has dropped; switching to measurement`, `Reducing bandwidth of %s by %d to %d`.
No test here fed it: four attempts left the bitrate alone, with one job and one `Bandwidth` entry
(downloads throttled to 2.5 and 3 Mbps for up to 215 seconds, over loopback and the machine's own
address, with a real playback clock and `state=buffering`, `location=lan` and `wan`). Plex Web's
timelines carry no bandwidth, so the reference can only be timing its own deliveries.

**Divergence: ours moves, by a rule of our own.** `autoAdjustQuality=1` is the official API's
`SERVER_ADAPTIVE` quality (*Adaptive streams, chosen by the server or by the player*): the same
decision and the same first encode, then lower bitrates at the same picture size as the link
requires, and back up as it allows.

A timeline for a running conversion also answers `Bandwidths` and `TranscodeSession` (`size` 2),
fixed quality and copies included. `Bandwidth.bandwidth` is the rounded-up sum of 2.1 times the
bitrate of each copied track and 1.05 times that of each encoded one, and `resolution` follows the
width of what is sent: for an adaptive stream, the rung of the latest video segment the player
received. Plex Web shows it as the current quality, once `TranscodeSession.key` ends
with the session it started. We send the attributes we can state and leave out `progress`, `speed`
and the codec fields. After `stopped` our session is closed, so neither is sent; the reference
still sends them until `/stop`.

### Adaptive streams, chosen by the server or by the player

`startStream` takes `quality`: `FIXED` (one rung, as above), `SERVER_ADAPTIVE` or
`CLIENT_ADAPTIVE`, over HLS and DASH alike; `maxVideoBitrate` caps every rung. The rungs come from
the measured ladder (`plan::ADAPTIVE_RUNG_LIMITS`, Plex Web's 8, 4, 2, 0.72 and 0.32 Mbps), and
there is still one process per stream: a segment of another rung, or one too far ahead, restarts
it there. What earlier runs wrote stays on disk and is served without a restart.

**Chosen by the player.** The manifest lists every rung: DASH as `Representation`s of one
`AdaptationSet`, HLS as `#EXT-X-STREAM-INF` variants with `#EXT-X-INDEPENDENT-SEGMENTS`. The top
one is the copy of the video whenever the file allows it (H.264, no subtitle to burn, within the
cap), then encodes below 75% of the file's bitrate: Soul (7.7 Mbps) gets the copy, 1280×720 at
3.4 Mbps, 720×404 at 1.3, 480×268 at 0.3 and 320×180 at 0.2. Every rung carries the same audio,
copied or converted by the same encoder, since a player switching video keeps its audio. Segments
are 5 seconds on every rung and both protocols. Addresses: `/streams/{id}/{rung}/index.m3u8` and
`…/{rung}/00012.ts`; `/streams/{id}/{rung}/header` and `…/{rung}/12.m4s` for DASH video, and
`/streams/{id}/audio/…` for the audio every rung shares.

Copied video can only be cut on the source's key frames, and Plex's muxers cut their segments on
a grid that starts where *their run* starts: measured on a test episode, a copy restarted at 30 s
began on the key frame at 27 and then numbered every chunk 3 seconds early. Two runs of the same
rung started at different points therefore disagree on where a segment begins, and a player
moving between them meets gaps. ffmpeg-plex puts the grid on the media's timeline instead
(`-absolute_segments` for DASH, `-segment_absolute` for HLS, asked of these streams only, which a
build without them refuses): segment N starts at the first key frame shown at or after N × 5 s and
carries the number N whatever the run's start, so every run writes the same segment N and any run
of the rung that wrote it answers for it, one file served as it is. Encodes put their key frames
where the source has them (`-force_key_frames source`, `scenecut=0:keyint=infinite`) and every
run starts on the key frame before its segment (`-noaccurate_seek`), so all rungs cut at the same
frames; the end of the segment before, which a run starts with, carries that segment's number
and is never served from it. When the source leaves a segment without a key frame, no file has
its number and the one it is shown in answers. Without a copy (a burned subtitle, a cap below the
file) the encodes are keyed on the 5-second grid instead.

The audio has to be cut where the video is. A muxer cutting at a key frame ends the audio it has
received so far, and the interleaving runs the audio behind the video by a distance that depends
on the encoder and the seek: 0.48 s on a copy of Soul, 0.15 s on an encode. Two runs then cut the
same audio at different places, and a segment from one followed by the next from another left
the audio between their cuts unheard (0.33 s and 0.21 s at two restarts, about 0.25 s in HLS), a
hole a browser buffers since it plays only where audio and video both are: the 0.66 s gap shaka
stepped over below. With the grid, ffmpeg-plex cuts the audio on its first packet shown at or
after the key frame: in DASH by keeping each cut until the audio reaches it, an audio segment
taking its video segment's number when closed; in HLS, whose muxer writes one file at a time, by
holding the key frame and what follows until the audio gets there. Its MPEG-TS muxer is also
handed `-avoid_negative_ts disabled`: left to its own, it shifted every timestamp of a run from
the start by the B-frames' delay, 83 ms on Soul, and of no other run.

Probed on Soul over twenty segments and five restarts, then after seeks to 3000 s and 1500 s:
every segment's video starts where the one before ended, whatever the run, and its audio within
one AAC frame of it (+11 ms to −32 ms in DASH, ±11 ms in HLS, the times a player shows, edit lists
applied): converted audio is framed from where each run's encoder starts, a residue a player
covers. Within a run the audio is continuous to the sample.

`-skip_to_segment 1` makes this build write an encode's timestamps from zero wherever the input
starts (from 2 on they stay the source's), so a run is still told its first segment; and
`-fps_mode cfr` after a seek fills the time before the first frame with copies of it, each a key
frame, so a run started further on encodes without it.

Played by shaka-player 4.7 and hls.js 1.5 in a browser, on Soul, forcing a rung every 9 seconds
in both directions between the copy and the encodes: the picture changed size each time
(1920×1080, 1280×720, 720×405, 480×270, 320×180), playback never stopped, no frame was dropped
(2,847 and 1,951). DASH buffered one gap of 0.66 s at 39.3 s, which shaka stepped over in 0.07 s:
the audio cut described above, since aligned; hls.js had none, filling such holes with silence,
and retried three segment requests that timed out while a restart waited. That play predates the
media's grid, when each rung was written in pieces of a group of pictures and assembled here.

**Chosen by the server.** One representation at one picture size, the top rung's, and lower
bitrates under it (Soul: 20, 7.4, 3.4, 1.3, 0.3 and 0.2 Mbps, all 1920×1080), in 3-second
segments. A player keeps the initialization segment it read first, which another picture size
would not decode, and x264 otherwise writes other headers for another bitrate: the level follows
the bitrate and the picture parameters the CRF. Every rung is encoded `stitchable=1` and at level
4.2, which makes their `avcC` byte for byte the same (checked at 9, 1.3 and 0.3 Mbps, and after a
seek). HLS needs no discontinuity, the picture never changing.

The server times every video segment it sends, from its first byte to its last, after the wait for
the process to write it. That alone is not enough: a connection's buffers take a whole segment at
once over a fast path (1.3 MB "sent" in 0.35 s to a reader at 1.5 Mbps over loopback), so the
listening socket's send buffer is held at 256 KiB, and the wait until the player asks for the next
segment counts too once it is longer than 1.2 segments, which an idle player never waits. From
those deliveries, the rules (`adapt::ServerAdaptation`, with their reasons beside them):

| Rule | Value |
| --- | --- |
| throughput | the lower of two moving averages, weights 0.5 and 0.15; a sample counted at no more than twice the top rung's needs, a send at no less than 20 ms |
| stay | while the rung needs at most 90% of it |
| move to | the best rung needing at most 75% of it, of the latest delivery when going down |
| down | after 2 slow deliveries in a row |
| up | one rung, after 30 s of room for it |
| after a move | 9 s without a decision |
| failed probe | down within 60 s of going up: the next wait to try that rung doubles, up to 240 s |
| restart | down at the segment asked for; up one segment later, the one asked for being written |

Measured on a test episode over loopback, with a player keeping 20 s of buffer on a real clock:
the link dropped from unlimited to 1.5 Mbps, the second slow segment (14 s later) moved the stream
from 9 Mbps to 0.3, and the first segment of the new rung arrived 0.9 s after it was asked for;
restored, it climbed one rung every 42 s back to the top. 0.2 s of stall in 360 s, and everything
received, decoded as one stream under the first initialization segment, gave 9,525 frames without
an error, DASH and HLS alike. On the Plex surface, the timeline's `Bandwidth` followed it: 9719,
584, 1631, 3859.

**Restarts are held back.** A request for another rung than the running one waits, and the process
is restarted only when allowed (`adapt::client_switch_wait`): restarts for a change of rung at
least 4 seconds apart, so that each run writes a segment or two before it can be replaced; a
better rung only once its request has been waiting 2 seconds, so that a player changing its mind
or abandoning the request has cost nothing; a worse one at once. Seeks within a rung are not held.
Measured: the first change of rung costs 0.6 to 0.7 s (the encode's start and first piece), a
change within 4 s of the last about 3.5 to 4 s.

Not handled: a rung change, like a seek, ends the response of soft subtitles, the new process
numbering its pieces from 0. And the server does not slow the process down: a low rung runs minutes
ahead of the player, which is why an upgrade does not wait for what it wrote.

### Refusing a chunk

A request for a DASH header, a DASH chunk or an HLS segment waits for the file, up to 20 seconds
while a process is running. Once the session's process has ended it is held for 5 seconds before
the `404`, and is served if the file appears meanwhile. A session that is unknown or stopped
answers `404` at once, a wait already under way included. Measured on the reference, an unknown
session and a dead process both answer `404` within 0.1 s; it cannot read its files here, so a
chunk that is merely late could not be timed. The hold is a deliberate choice against retry loops.
Playlists and manifests are not held.

### Falling back

The spec describes `POST /{transcodeType}/:/transcode/universal/fallback` as "a transcoder
fallback, ex: HEVC to h.264 or hw to sw", taking `transcodeSessionId`. No client in `files/`
calls it: Plex Web falls back by itself, restarting the playback without direct play or with
`directStream=0` (`Direct Play failed … Attempting to fallback`, `Video transcode failed …`).
The reference's binary registers `/universal/fallback` beside `ping` and `stop`, reads `session`,
logs `Got a transcode session fallback without a valid session ID.`, keeps `canFallback` on its
transcode sessions and logs `Transcoder: session %s indicated fallback to software decoding`; it
also formats `/video/:/transcode/universal/fallback?session=%s` itself, next to its recorder's
decisions. A fallback is therefore a restart of a hardware or HEVC conversion in software and
H.264, asked by whoever sees it fail.

Measured on the reference, on Soul (library 27), a copy over DASH and a re-encode over HLS:

| Request | Answer |
| --- | --- |
| no `session`, or only `transcodeSessionId` | `400` |
| `session=` or an unknown session | `404`, with the log line above |
| a running conversion | `412`, whose body is the `500` page (`Internal Server Error`, 109 bytes) |
| the same under `/music`, `/audio`, `/subtitles` | as under `/video`: the kind is not looked at |
| `/photo/:/…/fallback` | `404` |
| `GET`, `PUT`, `DELETE` | `404`, known session or not, without the log line: no such route |
| no token | `401`: not a callback of the transcoder's |

Both conversions had `transcodeHwRequested=false`: the reference transcodes in software, and
`412` is its answer for a conversion with nothing to fall back to. The session was left as it
was — same key, same `timeStamp`, still answering `ping`. What a successful fallback answers could
not be measured, since it needs the hardware transcoding the reference has off, and its
preferences are not ours to change.

Ours is the official API's `fallBackStream`, applied by the stream engine (`StreamEngine::
fall_back`) under the session's lock, and a sign of life for the session. Every conversion here
decodes in software and encodes H.264, so a running one answers `NO_FALLBACK`, bound to the `412`
above, and a session gone or sending its file as it is `SESSION_NOT_FOUND`, a `404`. Same
answers as the reference on every row of the table, checked on a conversion of Soul here.

### Streams at addresses of their own

**Divergence.** The reference answers `start.mpd` and `start.m3u8` with its manifest, whose
segments are relative URLs under `session/{id}/…` and carry no token. Ours answers `307` to the
stream the official API's `startStream` started: `/streams/{id}/manifest.mpd?token=…` or
`/streams/{id}/master.m3u8?token=…`, whose renditions are under `/streams/{id}/{rendition}/…`. The id is a fresh UUID, the token a 256-bit secret of that
stream alone, and every URL the manifest names (headers, chunks, the variant playlist, its
segments) is an absolute path carrying it, because a relative reference drops the query. A wrong
or missing secret answers `404`, as an unknown stream does. The manifest's body is otherwise the
reference's, unchanged. So the Plex binding no longer builds a manifest: there is one, and it is
the official API's. The cost: a client that does not follow redirects cannot start a stream, and
a stream's URLs are tied to its secret rather than to the session's name.
`/video/:/transcode/universal/subtitles` keeps its Plex route. The transcoder's callbacks have
left the Plex surface: the reference's process calls back under `/video/:/transcode/session/`,
ours under `/transcoder/{session}/{run}/`, served by the server over the transcoder and only to
loopback.

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

### Editing an item, and locking its fields

`PUT /library/sections/{id}/all?type=&id=` with `{field}.value` and `{field}.locked`, as the spec
says. Measured on the reference's two test libraries, every value and lock restored afterwards:

- **A value does not lock its field.** `summary.value=…` alone leaves `user_fields` empty. Plex
  Web locks a field client-side, adding `{field}.locked=1` once the user types into it
  (`onLockableFieldChange`). The official API does the same: `lock` and `unlock` are separate
  from the values.
- **Locks are stored as Plex numbers** in `metadata_items.user_fields`, `lockedFields=1|2|9`,
  each found by locking one name alone on a test film: `title` 1, `titleSort` 2,
  `originalTitle` 3, `studio` 4, `rating` 5, `tagline` 6, `summary` 7, `contentRating` 8,
  `thumb` 9, `art` 10, `banner` 11, `theme` 12, `originallyAvailableAt` 13, `year` 14, `genre`
  15, `collection` 16, `director` 17, `writer` 18, `actor` 19, `producer` 23, `country` 24,
  `index` 25, `absoluteIndex` 27, `label` 29, `audienceRating` 30, `guid` 32, `editionTitle`
  33, `clearLogo` 36, `contentRatingAge` 37, `squareArt` 39, `mood` 300, `style` 301,
  `similar` 305, `network` 319, `userRating` 1002, `parentIndex` 1054. The real library holds
  locks 1–4, 6, 7, 9, 10, 13–18 and 24 on its films, series, seasons and episodes, nothing else
  in `user_fields`; the importer keeps the 20 we model, in a table of their own.
- **A film's or a series' year follows its date.** `originallyAvailableAt.locked=1` stores
  13 and 14 and unlocking it removes both; `year.locked` is accepted and does nothing. On an
  episode the two lock apart.
- **A lookup publishes the locks** as `<Field locked="1" name="title"/>`, after every other
  child element, in number order; JSON `"Field":[{"locked":true,"name":"title"}]`. Nothing for
  an unlocked field, and nothing in a listing.
- **A new title derives the sort title**, unless the sort title is locked — and even when the
  same request gives one, which we do not reproduce: a sort title given with the title is kept.
  The rule, on twenty titles: spaces trimmed, one leading article from the hidden
  `ArticleStrings` preference (`the,das,der,a,an,el,la`) dropped when another word follows it
  after a space, then accents folded with case kept (`Les Misérables` → `Les Miserables`,
  `The The Band` → `The Band`, `the\tTab` unchanged). An emptied title keeps its sort title.
- **A date sets the year**, and clearing it clears the year, unless the same request sets the
  year. A locked year does not stop it.
- **Empty clears**, to `NULL` for a number; a title can be emptied. `updatedAt` does not
  change.
- **Out of scope is ignored with a `200`**: an id of another `type` or library, a non-numeric
  id. A missing `type` is a `400`, as are a field name it does not know and `.locked` other than
  `0`/`1`. Without `id` the reference edits **every** item of the library.

Where we answer differently: no `id` (or `filters`) is a `501` rather than an edit of the whole
library; tags (`genre[0].tag.tag`), artwork values, seasons, music and the fields we do not model
(`banner`, `index`, `network`…) are `501`; a value the field cannot hold — a year that is not a
number, a date that is not `YYYY-MM-DD`, a rating outside 0–10 — is a `400` where the reference
stores it (`rating=42`, `year=-5`) or fails with a `500` (`year=abc`). A library's viewer gets
`404`, as for its settings.

The reference keeps `lockedFields=` once an item has had a lock, where it held an empty
`user_fields` before; that and a few internal counters (`changed_at`, an `extra_data` key) are
what the test items kept from being measured.

`PUT /library/metadata/{ids}`, the spec's own edit, reads the same fields with the same rules
and answers otherwise, measured on the same items, every value and lock restored:

- **A field comes bare too**, `title=…`, the form the spec's example writes (`args`), as well as
  `title.value=…`; given both, `.value` wins, in either order. `title.locked` locks as above.
- **`type` is ignored**, even a wrong one, and so is the library: every id named is edited as
  what it is, a film and a series of two libraries in one request (`577986,577987`).
- **A lock other than `0` or `1` is ignored** with a `200`, where the section's route answers
  `400`. A name it does not know is a `400` bare or as `.value`; `includeExternalMedia` and the
  `X-Plex-*` parameters pass.
- **No parameter at all is a `400`**; `type` alone is an empty `200`.
- **An id that names nothing is a `404`**, a non-numeric one too, unless another id of the
  request is edited: `577986,999999999` is a `200`.

Ours differs as the section's route does: an item the caller is not a library admin of counts
as absent, seasons, tags, artwork and music are `501`, and a year that is not a number is a
`400` where the reference fails with a `500`.

### A library's settings

`GET /library/sections/{id}/prefs` lists 17 settings for a film library and 18 for a series, nearly
all about scanning and matching. Plex Web draws its dialog from whatever the list holds and looks
up only two by name, `hidden` and `includeAdultContent`, so those are what we publish, at their
defaults: identical to the reference in English, French and German. The labels come from its
catalogs. Unlike `/:/prefs`, every `default` and `value` here is a JSON string, a boolean included.
`PUT` answers `501`.

### Server settings

`GET /:/prefs` on the reference lists 162 settings: transcoder, DLNA, maintenance tasks,
remote access, most of them hidden. We answer `403`, `PUT` included, by the repo owner's
decision, and that is what hides Plex Web's server settings. Its menu (`settings/server/…`)
offers General, Remote Access, Agents, Languages, Optimized Versions, Live TV & DVR and
Troubleshooting only once that list has loaded, and the per-group screens (Library, Network,
Transcoder, DLNA, Scheduled Tasks, Extras) only for a group the list holds; refused, the whole
Settings section and those Manage entries are gone, with no error shown. Plex Web reads the
list nowhere else: `AcceptedEULA` sends it to the licence screen only when the list loaded and
says `false`. Editing is not affected: Plex Web offers it on the provider's `manage` feature
(`/media/providers`), not on settings.

What stays is gated on the server being the viewer's own (`owned` on plex.tv) and nothing the
server answers: Dashboard, Alerts, Conversions and Manage › Libraries. Plugins follows
`allowChannelAccess`, which is `0`: there is no plug-in framework here.

### Global search, `/search`, is cut per kind in title order

`GET /search?query=` applies the same match rule as the `title=` filter — a word prefix over
the three title fields, confirmed in both directions (`jouet` finds *Le Nouveau Jouet*, `hell`
finds *3 from Hell*) — and answers **at most 15 of each kind, in title order**: films, then
series, then episodes, each the first fifteen a title-ordered match begins with. It ranks
nothing; there is no score on its items. Measured on 53 terms (`captures/search/`): no kind
over 15, and for `hell` the fifteen films are exactly the first fifteen of our 36 matches by
sort title, `Hellboy` left out after `Hell on the Border`.

We answer the same, through the official API's `search` with `order: TITLE` and
`first: 15`, kind by kind. On 117 kinds over the 53 terms, the same first entry on 73 and the
same entries on 65; the rest
differ in collation (*Sort collation*) and in what the term matches: the reference wants the
words in order (`bad breaking` finds nothing there), and an episode is matched by its own
titles only, never through its series as the search shelves match it.

`/search` also carries a static `Provider` array of four entries, and its `size` counts the
items **plus** the providers: 37 items and 4 providers are reported as 41.

### Relevance: the reference's score

`/hubs/search` and `/library/search` rank what they match by one score, written with five
decimals, the same in both. Measured on 9,151 scored items over 53 terms (distinctive, common,
several words, people's names, prefixes; `captures/search/`):

- **The rung**, two tenths apiece, is how well the title answers the term — the table under
  *Search by shelves*. 0.93 a title or sort title equal to the term; 0.73 a title or original
  title whose first word is the term's; 0.53 one starting with the term's first word; 0.33
  any other match. An episode scores two hundredths less on each rung (0.91 … 0.31), and an
  episode found through its series' title is on the lowest.
- **The audience rating** breaks ties, one ten-thousandth per point: *Le Pire voisin au monde*,
  rated 9.7 by its audience, scores 0.33097; *Dune* (1984), 6.5, scores 0.93065; an item with
  no rating keeps the round value. 9,123 of the 9,151 tails are exactly the rating the
  reference publishes on the item; the 28 others are a hundred-thousandth off, all on ratings
  stored with a second decimal of 5.
- Within one score the reference's order is not stable from one request to the next (*Toy
  Story 5*, *Elvis* and *La Ligne verte*, all 0.33094, came back in two orders). Ours is the
  sort title, then the id.

The official API's `search` orders by that score by default (`order: RELEVANCE`), each edge
carrying `score` and `titleMatch`. Storage computes the rung in SQL from a table of each
item's folded whole titles (`item_title_key`, written by `write::insert_items` and
`write::update_item`), so a page is a `WHERE`/`ORDER BY`/`LIMIT` like any other.

**Paging a ranked search.** A keyset cursor carrying the score, in two exact parts — the
hundredths that rung and kind make, an integer, and the audience rating as stored — then the
sort title and the id. Integers and stored values compare exactly, so a page resumes on
equality where a float recomputed per row might not; the id makes the order total, so no item
is skipped or repeated even when hundreds share a score. A ranked search is not bounded and
offset-paged: it can be walked to its end forwards and backwards like any listing, and a
cursor from the title order is refused rather than misread.

**Where the reference ranks something else.** It does not rank every match: it draws a pool
first and ranks that, and for a common or a loosely matched term the pool leaves its best
titles out. `le` at `limit=10` on `/library/search` answers *Le Nouveau Jouet* (0.53) and the
nine films of *VF - Films* whose titles open with a digit (0.33) — the first ten of that
library in title order — where *Le Parrain* and *Le Robot sauvage* score 0.73; `dune` on
`/hubs/search` lists *Journal d'une ado hors norme* (0.33) on the series shelf before *Dune :
Prophecy* in its other libraries (0.73). We rank every match, which is what the owner asked
for. Hence, on the 53 terms, the same score for 4,657 of the 4,873 items both servers list on
`/hubs/search` at `limit=100`, and 96.5% of the pairs of differently scored items both list
in the same order; on the default shelves (`limit=3`), the same first score on 80 of 100 for
distinctive terms, names and several words, where the pools do not bite, against 51 of 88 for
common terms and prefixes. What remains is mostly the library having changed since our
snapshot: *Matrix* has an audience rating there and none here.

### What a listing says it can do

`includeMeta=1` on `/library/sections/{id}/all` adds a `Meta` block before the items, which Plex
Web builds its filter and sort menus from: one `Type` per item type the library lists, each with
its `Filter`s and `Sort`s, then `FieldType`. Without the parameter, or with `0`, it is absent. The
requested `type` is the active one, the first by default. The active `Sort` carries `active` and
`activeDirection`: `sort=addedAt:desc` marks it `desc`, `sort=addedAt` marks it `asc`.

We publish the filters and sorts a listing honors, not the reference's whole list: 18 of 27
filters and all 9 sorts for a film library, 11 of 14 and 7 of 8 for a series, 1 of a season's 2
sorts, 9 of 17 and 6 of 7 for an episode. Two tests hold every filter and every sort offered to
being one `listing::parse` accepts. What we do publish has the reference's attributes, wording
and relative order on every type compared. Left out: the `folder` type, which no route serves,
and the advanced filters' `Field` and `FieldType`.

A filter's `key` (`/library/sections/4/genre`) is the list of its values (*Filter choices*).
A series library qualifies it with the type (`genre?type=2`), except `country` and the four
track filters, whose lists span the whole library (`/library/sections/5/videoCodec` on the
episode type too).

`GET /library/sections/{id}/filters` and `/sorts` are one type's `Filter` or `Sort` list of
the same block, as `Directory` elements under a `secondary` container with no `title2` (the
first type, or the one `type` names; a season's filter list is empty). No sort is marked
active, `sort=` notwithstanding. They are whole unless both container parameters are sent,
and then windowed with `totalSize` and `offset`, as the reference does. Ours are built from
the same tables as `Meta`, so the three cannot disagree: every entry is the reference's,
attribute for attribute, in its relative order, on both types of library.

### The fields a set of items has in common

`GET /library/sections/{id}/common` is what Plex Web's multi-item editor loads: `type` (a
`400` without), `id` as a comma list, `includeExternalMedia=1`, and any listing filter, which
narrows the set as it narrows `/all` — one that we cannot honor answers `501` there too. The
answer is one element, `<Common>` in XML and `type="common"` under `Metadata` in JSON, even
for a set of none. Measured on films, series, seasons and episodes, singly, in pairs, in
fours and whole libraries:

- **`mixedFields`** names, in a fixed order per type, the fields on which two items differ;
  one holding a value and another none counts. Films and series: `title, ratingKey,
  titleSort, tagline, (parentIndex,) rating, summary, year, studio, originallyAvailableAt,
  originalTitle, contentRating, (isAdult, userRating)`; episodes: `title, parentTitle,
  grandparentTitle, ratingKey, parentRatingKey, grandparentRatingKey, titleSort, index,
  rating, summary, year, originallyAvailableAt, originalTitle, contentRating, …`.
- **A shared field is written with its value; a mixed one, or one no item holds, is absent**,
  but `guid` and `titleSort` are always written, empty when not shared, and so is an episode's
  `parentTitle` when not mixed — empty, since seasons are untitled. One item shares its
  `ratingKey` and a `key` of `/library/metadata/{id}/children`; ratings come to one decimal.
- **Tags**: those every item carries. For one item they are its own, in billing order with
  the character played; for several, by tag id, every `Role` with `role=""`. An episode's own
  credits only: a single episode carries no `Genre`, as on the reference.
- **`Field`**: the fields every item has locked, by the lookup's names: two films locking
  `thumb` share `<Field locked="1" name="thumb" />`; one locking `title` and the other
  `genre` share none.
- An unknown id, or an item of another type, makes an empty set: `mixedFields=""`, `guid=""`,
  `type`, `titleSort=""`.

Ours answer the same elements in the same attribute order, and differ where the reference's
data is newer than the snapshot. What we do not publish: `Guid`, `Similar` and
`CommonSenseMedia` on a single item, and the `Rating` the reference writes as empty
placeholders on several; `parentIndex`, `isAdult` and `userRating` in `mixedFields`, which we
do not hold; and a season's `originallyAvailableAt`, which the reference writes as
`1969-12-31` for a season with no date. The set is aggregated where it is stored, so a whole
library costs one pass: 2.6 s for 79,518 episodes, 0.9 s on the reference. An `id` list is
capped at 500, past which the request is a `400`.

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
or one counted back from today: `-1y`, `-6w`, `-30d`), `lastViewedAt>=` and `<=` (a time, or
one counted back from now), `viewCount>=1`, `viewOffset=0` and
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
  season orders of seasons and episodes when asked for by name (`show.titleSort,index…`), and a
  list of fields, which is how the reference states tiebreakers. The episodes' order is honored
  as their default, though (*A library's leaves*). Its `year`, `viewCount` or `title` work there and are
  refused here; an unknown field is a `500` there.
- **`random` keeps one order for ten minutes.** The reference reshuffles on every request, so a
  client paging a random list meets some items twice and never sees others. Ours is a shuffle
  seeded by the clock's ten-minute period, so the pages of one browse agree; the official API
  takes the seed as an argument.
- **`lastViewedAt` is the caller's own**, like all watch state; a visitor's list is in title order.

### A library's leaves, `/library/sections/{id}/allLeaves`

The spec names it "Set section leaves" and says nothing else. On the reference it is `/all` with
`type=` the library's leaves — `4` on a series library, `1` on a film library — measured
identical in items, order and `totalSize` on 92 requests over two film and two series libraries:
the default order, `sort=` by title, date added, release date, rating, last viewed and duration
(not `random`, which reshuffles), and the filters `year`, `decade`, `title`, `contentRating`, `resolution`, `unwatched`, `studio`, `genre`,
`audioCodec`, `show.title`. Captured on the test libraries 27 and 28 and on four others:

| Behavior | Detail |
| --- | --- |
| Container | `/all`'s without the library's identity or the view's title: `size`, `totalSize`, `offset` (when windowed), `allowSync="0"`, `art`, `identifier`, `mediaTagPrefix`, `mediaTagVersion`, `nocache="1"` on a series library only, `thumb`, `title1` the library, `viewGroup` the library's (`show` for episodes), `viewMode="65592"`. No `librarySection*` trio, no `title2`. |
| Items | Element for element `/all?type=` the leaves' type: the library is stated on neither the container nor the items, and a series' `studio` is not copied onto its episodes as `/library/metadata/{id}/allLeaves` does. XML names them after their kind (`<Video>`). |
| `type` | Narrows the leaves rather than replacing them: `type=2` or `3` on a series library, `type=4` on a film library, and any number naming no kind (`0`, `13`, `99`, `-1`) answer an empty container. `type=18` on a film library lists its collections (1,118); here it is empty. A `type` that is not a number is a `500` there and is ignored here, as on `/all`. |
| `includeMeta=1` | Adds nothing. |
| Unknown library | `404`, a number or not. |
| Paging | As `/all`: the two headers and `totalSize`/`offset` only when both container parameters are sent. We always window. |

**The episodes' default order** is the one the reference's `Meta` states for the episode type,
`show.titleSort,season.index:nullsLast,episode.index:nullsLast,episode.originallyAvailableAt:nullsLast,episode.titleSort,episode.id`,
and the same on `/all?type=4`. Measured over every episode of four libraries (138,948): series
are not kept together, so two series sharing a sort title (*Good Doctor* and *The Good Doctor*)
interleave episode by episode; ties on both numbers follow the release date (0 of 472 tied pairs
out of order, where the id, the series and the title put 191 to 256 out in one library). On the official
API this is `orderBy: SERIES`, and `depth: LEAVES` is the leaves; a film follows its title under
that order, so a film library's leaves are in title order.

Against ours, over the same four libraries: the order of each series' episodes is identical for
all 4,002 series, and 127,292 of 138,948 episodes stand at the same position; the rest is the
collation of series titles (*Sort collation*) — `#blackAF` and *2 Broke Girls* open the
reference's list, and *Avatar : Le Dernier Maitre de l'air* ties with its lower-case namesake
there. Fields differ as `/all`'s do on the same items: artwork, `Part.key`, the watch state, which
is not imported, and what changed on the live reference since the import.

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
attributes, with none of a listing's. Its items, as those of the `movie.similar` shelf, name
their library; a person's shelf's do not.

`GET /hubs/metadata/{id}/related` answers the same shelves in a container without
`mediaTagPrefix` and `mediaTagVersion`. `GET /hubs/metadata/{id}`, which the spec reserves for
music, answers an artist's four shelves for any item — `artist.mostplayedtracks`,
`artist.mostpopulartracks`, `artist.albums`, `music.videos.popular.new` — each empty, its key a
listing of the item's library with `artist.id={id}`; ours writes `viewCount%3E=10` where the
reference has a raw `>` (*Range filter operators*). An empty shelf keeps the long form in XML,
`<Hub …>` then `</Hub>`, on every endpoint.

### The end of a playback

`GET /hubs/metadata/{id}/postplay` is what a player shows once an item ends. Measured on the
reference over the owner's series and films, read only, and on its test series Bull, whose
watch state was moved through each case and put back. The container is bare — `size`,
`allowSync="0"`, `identifier` — and `count` (default 6) sizes every shelf; `count=0` lists each
shelf empty with `more="1"`. A series or a season has none: `size="0"`. An unknown item is a
`404`.

**After an episode**, one shelf at most:

- `tv.upnext` (`Up Next`): the series' entry in Continue Watching — not the episode after the
  one played. Every episode of South Park, S9E1 as S29E3, offers S24E3, the episode after the
  one watched last; with Bull's first episode in progress, its second offers the first. The
  shelf has `upNext="1"` and no `key`, one item, dated by its series as in Continue Watching.
- None when that entry is the episode itself: Bull's first episode in progress offers
  nothing, nor does it once both episodes are watched, nor anything of a series never played.
- `tv.ondeck` (`On Deck`, key `/library/sections/{s}/onDeck`) when the series has no entry —
  not begun, finished, or played longer ago than the window, as Game of Thrones: the library's
  Continue Watching, the episode played left out. The reference's series libraries never held
  more than one entry, so its size beyond one was not measured; ours is `count`.

**After a film**, in this order:

- `movie.upnext`, `Up Next in {collection}`, `upNext="1"`, no `key`: the film after this one
  by release date, watched or not, in the first of its collections, in the order it joined
  them, that both shows it and continues past it. *Shows it* is the shelf's: the reference
  looks for the film among each collection's first `count` films, and offers what follows it
  even past them — the sixth Jurassic Park offers the seventh; a James Bond past the sixth of
  a saga of twenty is offered the next of a smaller saga that lists it third, and with
  `count=20` the next of the large one; with `count=2` the fourth Scary Movie is offered
  nothing.
- `movie.collection.{n}` for each collection, `{title} Collection`: its films by release date,
  the list behind `/library/sections/{s}/all?collection={n}&sort=originallyAvailableAt`. A
  collection holding only the film is still listed.
- `movie.similar`, the shelf `/related` shows, whether or not `/related` shows it.

Items are a listing's, with their library on each; the similar films come in the form
`/related` gives them. Titles are the client's language: `Lecture suivante dans {1}`,
`{1} collection`, `À suivre`, `À l'affiche`.

Against the reference on 200 films of collections in a film library: the same shelves on 191,
the same next film on 111 of 112, and every difference traced to the snapshot — collections
made or emptied since, a film's tags gone, a collection title whose case differs (*Collections*).
On the official API: `Episode.upNext`, and `Movie.upNext(within:)`, which names the collection
too; `within` is the shelf's size.

### The home screen's shelves

`GET /hubs` answers eight shelves, in this order, each `promoted="1"`:

| `hubIdentifier` | `key` | Holds |
| --- | --- | --- |
| `home.continue` | `/hubs/home/continueWatching` | What was started, films and episodes; style `hero` |
| `home.ondeck` | `/hubs/home/onDeck` | Each series' episode, in progress or next |
| `home.movies.recent` | `/hubs/home/recentlyAdded?type=1` | Films by date added |
| `home.television.recent` | `/hubs/home/recentlyAdded?type=2` | Series entries, grouped |
| `home.music.recent` | `…?type=8` | Albums; empty here |
| `home.photos.recent` | `…?type=13` | Photos; empty here |
| `home.videos.recent` | `…?type=1&personal=1` | A personal library's videos (`clip`); empty here |
| `home.playlists` | `/playlists/all?type=15&sort=lastViewedAt:desc&playlistType=video,audio` | Playlists played last |

An episode in progress is on both of the first two: Bull's first episode, at 78 seconds, was
first on each. `count` (default 6) sizes each shelf, `identifier` keeps the shelves it lists
by `hubIdentifier`, comma-separated; `includeStations=1` changes nothing, and
`identifier=home.popular` an empty container. `/playlists/all` takes `playlistType` as a list.

`GET /hubs/promoted` keeps the first two, then each library's own recently added shelf —
`movie.recentlyadded.{s}` or `tv.recentlyadded.{s}`, as in `/hubs/sections/{s}` — then the
playlists. The libraries come by title, case aside (`open-plex tests` before `VF - Films`),
and the two hidden ones have none. The reference's personal-video library shows as
`video.recent.{s}`, a kind this server does not hold. Ours are identical in `hubKey` to the
reference on its eight film and series libraries.

Where ours differ:

- **`home.videos.recent` is always empty**: a library of personal videos is a film library
  with no agent on the reference, which this server does not tell apart.
- **`onlyTransient` is tolerated**: every shelf here is transient but the playlists.

`GET /hubs/popularInLibraries` is a `404` on the reference, with or without `count` and
`contentDirectoryID`, and `/hubs` leaves its `home.popular` out. Its binary builds the shelf from
the plays of the server's *other* users over `PopularInLibrariesWindow` weeks, a setting
`/:/prefs` does not list. Ours answers the same `404`, and has a reason the reference may not:
each user here has a server of its own, whose history no other user's plays reach, so there
is nothing to count.


### The search field calls `/library/search`

Plex Web's search field does not use `/hubs/search` or `/search`. It calls the
`universalsearch` feature from `/media/providers`, `GET /library/search?query=&limit=&searchTypes=
&includeMetadata=1&filterPeople=1`, which the spec does not declare. The answer is a list of
`SearchResult`, each a `score` (a JSON number) around one `Metadata`, or one `Directory` for a
person, a genre or a collection in one library.

Measured: `query` is required (`400`; empty is an empty container); `limit` defaults to 10, a
non-number is `400` and zero `500` (a `400` here); `sectionId` changes nothing. Items come in
the listing's form, naming `librarySectionTitle`; only a series adds `librarySectionID` and
`librarySectionKey`. A film with no synopsis carries `summary=""`, where a listing omits it.

**Order and cut.** One list, grouped by kind and cut to `limit` as a whole: films, series,
episodes, then actors, directors, genres and collections, each group best first — items by
the score of *Relevance*, tags by the order and scores of the `/hubs/search` shelves (0.92 the
name itself, 0.52 a name starting with the term, 0.32 the rest; then library title, the most
carried, the newest tag). `the` at `limit=100` is a hundred films; `luc besson` is one film,
one episode, then Luc Besson the actor and the director in each library crediting him. A
`Directory` is the shelf's without `reason` and `score`: `key`, `librarySectionID`, `Key`,
`Title`, `Type`, `type="tag"`, `id`, `filter`, `tag`, `tagType`, a person's `tagKey` and
`thumb`, a collection's `thumb`, `art`, `guid` and `summary`, and `count`.

**`searchTypes`**, measured value by value on four terms:

| Value | Brings |
| --- | --- |
| `movies` | Films, and the genres and collections of film libraries |
| `tv` | Series and episodes, and the genres and collections of series libraries |
| `people` | Actors and directors, of the libraries the other values name; of every library when it comes alone |
| absent or empty | `movies,tv,people` |
| anything else (`music`, `photos`, `livetv`, `bogus`) | Nothing, and it still narrows `people`: `music,people` and `people,bogus` find nobody |

`tv,people` finds no Luc Besson, credited in film libraries only; `comedie` with `movies`
brings the genre *Comédie* of the film libraries, with `people` alone no genre at all. An
episode is matched through its series' titles here, as on the shelves.

Differences that remain, each knowingly:

- **Matching.** The reference's is loose: `zzzqqq` matches *Dragon Ball Z*, `comedie` finds
  actors named *Comer* and *Cometti*, and `hanks` finds *Hank's Back*. Ours is the word
  prefix of every search here, every word of the term required, in any order.
- **The pool.** The reference ranks the first matches in title order, by library; we rank every
  match (*Relevance*). For a distinctive term the lists agree — `dune`, `luc besson` and `tom
  hanks` entry for entry at `limit=10` — and for `le` they share nothing.
- **`otherVideos`** brings the genres and collections of film libraries on the reference; it
  brings nothing here, as a kind this server holds none of.
- **No film is found through a person here**, as on the reference: `tom hanks` lists Tom Hanks
  and no film, whatever `filterPeople` says. That is the shelves' doing (`/hubs/search`).

Against the reference on 53 terms at `limit=100`, 2,862 of the reference's 3,748 results are
listed here too, 2,700 of them with the same score.

### Search by shelves, `/hubs/search`

The spec's only search operation, and what most clients other than Plex Web's search field call.
Measured on the reference with sixteen terms — distinctive (`dune`, `kaamelott`), common (`le`,
`tom`), accented (`amélie`/`amelie`), people (`tom hanks`, `luc besson`, `spielberg`), a genre
(`action`), single characters — and on every parameter below; then again on 53 terms
across `/hubs/search`, `/library/search` and `/search` at several limits for ranking and
for what a person's name finds.

**Shape.** Always seventeen `Hub`s, filled or empty: `movie`, `show`, `episode`, `artist`,
`album`, `track`, `photoalbum`, `autotag`, `photo`, `tag`, `actor`, `director`, `genre`,
`collection`, `playlist`, `shared`, `place`. Each carries `title` (from the reference's catalogs,
in the client's language: `Films`, `Séries`, `Réalisateurs`), `type` and `hubIdentifier` (the
same word), `context=""`, `size`, `more`, `style="shelf"`, and no `key`. `more` is false even
when `limit` cut the shelf short. An empty shelf has no item list at all. The container holds
`size=17` and nothing else.

**Items.** Films, series and episodes come as `Metadata` (`<Video>`/`<Directory>` in XML), each
exactly as `/library/search` serves it plus `librarySectionID` and `librarySectionKey` on every
kind, and a `score` written as a string of five decimals (`"0.93065"`). An item sharing its title
with another on the same shelf carries `reason="section"`, `reasonID` and `reasonTitle` naming its
library — the two copies of *Dune*, but not the sequel shown alone. The reference applies it a
little more widely (*Batman Begins* alone at `limit=10`), by a rule not found.

**People and genres.** Actors, directors and genres come as `Directory`, one per library whose
items carry them: `key` (`/library/sections/4/all?actor=469791`), the library's `librarySectionID`,
`Key`, `Title` and `Type` (`1` films, `2` series), `reason="section"` with the library again,
`score`, `type="tag"`, the tag's `id`, `filter`, `tag` (the name), `tagType` (`6` actor, `4`
director, `1` genre), a person's `tagKey` (provider key) and `thumb`, and `count`, the items
carrying it in that library. Writers, producers and countries are not searched: `luc besson`
finds the actor and the director, never the writer, and `france` no country. A library whose
episodes alone credit someone is left out — Tom Hanks the director of two episodes appears for
the two film libraries only — since its listing filtered on him would be empty; `count` still
counts episodes where a series carries the credit (17 in *VF - Series*).

**Ranking.** Scores sit on a ladder whose first decimal is what ranks, and the audience rating
breaks ties within a rung (*Relevance*): `0.93065` is *Dune* (1984), rated 6.5. People,
genres and collections have no rating, and keep the round value.

| | Films and series | Episodes | People and genres |
| --- | --: | --: | --: |
| Title (or sort title) equal to the term; name equal to it | 0.93 | 0.91 | 0.92 |
| Title whose first word is the term's | 0.73 | 0.71 | |
| Title starting with the term's first word | 0.53 | 0.51 | |
| Name starting with the term | | | 0.52 |
| Anything else matched | 0.33 | 0.31 | 0.32 |

Words are split on spaces and apostrophes for this — *Amelie's Miracle* is 0.73 for `amélie`,
*Dune: Part Two* 0.53 and *Dune : Deuxième partie* 0.73 for `dune` — and the original title
counts, which is how an episode ranks by its series (*Batman Beyond*'s episodes are 0.71 for
`batman`). Titles fold their accents; names do not: for `amelie`, Amelie Kiefer is 0.52 and Amélie
Glenn 0.32, and the reverse for `amélie`. Within a shelf, people and genres are ordered by rung,
then library title, then `count` descending, then tag id descending — which the reference
matches entry for entry on `tom hanks`, `luc besson`, `besson`, `spielberg`, `action` and `dune`
(up to `limit=12`); on `tom`, 10 of 12 actors, two swapped or replaced where the counts differ.
Items in a rung follow their audience rating, then their title.

Shelves are ordered by their best rung, ties falling back on the order listed when nothing
matches (`show`, `artist`, `album`, `movie`, `episode`, …). On the shelves both servers fill,
that gives the reference's order for `dune`, `hell`, `amélie`, `amelie`, `tom hanks`, `batman`,
`kaamelott` and `breaking`. It does not for `action`, where the reference puts the actors (0.52)
before the films (0.73), nor where the shelves differ in rung because the matches do (`le`,
`the`); the reference's
order between shelves of one rung also varies from one term to the next.

**Items found through a person.** A film or series shelf that no title fills lists the works
of the people the term names, as `tom hanks` lists Tom Hanks' films, each with
`reason="actor"` (or `"director"`), `reasonID` the tag and `reasonTitle` the name, scored as a
match anywhere: 0.33 and the audience rating, *Le Pire voisin au monde* first at 0.33097.
Measured on thirteen names and, for `tom hanks` and `hanks`, six limits from 3 to 100:

- *Who:* the first actor, and the first director, of the actor and director shelves as listed
  — in the order above and cut to `limit` — whose library holds that kind. `hanks` gives Tom
  Hanks' films in *VF - Films*, and Colin Hanks' series in *VF - Series*, where he is the most
  credited Hanks. At `limit=3` the three Tom Hanks listed are in film libraries, and the series
  shelf stays empty; from `limit=5`, *VF - Series* is listed and his two series come.
- *What:* the `limit` items that person's tag was put on last in that one library, of any kind,
  then those of the shelf's kind: the actor's, then the director's not already listed, up to
  `limit`. Plex credits a guest on the series as well as on the episode, so Colin Hanks'
  series come — but only from `limit=100`: his thirty newest credits are episodes. `nolan`
  with `limit=100` lists Nolan North's 16 films and Christopher Nolan's 11; with `limit=3`,
  the three Nolan North films added last, and no director's.
- A title found keeps the people off the shelf: `luc besson` finds the documentary *Luc Besson
  Juste une mise au point*, and nothing of his.

The official API's `searchCredits` serves it. Same set and same scores as the reference on the
film shelf of all eleven names at `limit=3`, and on the series shelf of seven of the ten with one at
`limit=100`; the others name a person the reference's own pool of people leaves out (*Nolan
Gould*, the most credited Nolan in *VF - Series*, is absent from its list for `nolan`).
`/library/search` never finds anything through a person.

**Parameters.** `query` is required: absent, empty or blank is `400`. A term of one character
finds only the titles equal to it and no tag — `a` gives the two episodes titled *A*, `x` the
films *X*, `t` nothing. `limit` caps each shelf (default 3); a non-number is `400`, and zero,
which the reference answers `500`, a `400` here. `sectionId` must name a library (`999` is `400`;
a non-number is ignored) and **narrows nothing**: the reference answers `sectionId=6` with films
from library 4 and series from library 5. `includeCollections`, `includeExternalMedia` and the
other flags clients send change nothing on the reference, and are ignored.

Differences that remain, each knowingly:

- **Matching** is the word-prefix rule of every search here, with every word of the term
  required. The reference matches loosely: `dune` finds *Journal d'une ado hors norme*
  (`d'une`) and `zzqxw` finds *C.W.* and *Zweig*. A distinctive term gives the same shelves;
  a loose match's 0.33s are absent. An episode is found through its series' titles, as on the
  reference: every episode of *Kaamelott* for `kaamelott`, on the lowest rung.
- **Counts are live.** The reference's `count` comes from its own tallies, a few above the
  items it holds: 36 Tom Hanks films in *VF - Films* against 33 carrying the tag, as the filter
  choices already show.
- **Articles.** A term that is an article ranks lower on the reference, by a rule not found:
  for `the`, *The Boys* is 0.53 where ours is 0.73, and for `le` the reference shows no
  *Le Bureau des légendes* at all. French sort titles keep their article, so the sort title is
  not what explains it.
- **Pools.** The reference ranks a pool it draws first (*Relevance*): `dune` lists *Journal
  d'une ado hors norme* on the series shelf before *Dune : Prophecy* in its other libraries.
  Ours ranks every item matched; people, genres and collections are ranked among the 500 most
  credited, and the reference cuts its own pool of people by a rule of its own: for `amelie`
  at `limit=6` it lists one Amelie at 0.52 where we list six.
- **Single characters** find the exact titles first, the score putting them ahead of every
  other match: `x` gives the two films and eight episodes titled *X*, entry for entry and
  score for score with the reference.

`GET /hubs/search/voice` is not derived from it: see the next section.

### Voice search, `/hubs/search/voice`

A different search altogether: every title is ranked by its edit distance to the whole term,
however far, so that what a voice assistant mishears still finds something (`dune` finds *Cube*
at 2, `tom hanks` *Four Hands* at 4, `kaamelott` *Charlotte* at 5). Served through
`closestTitles` on the official API (`core::closest`, `db::closest_store`).

**The distance**, worked out on a thousand distances the reference published and checked on
the 808 items both servers list in the comparisons below, which agree to the unit:

- The term against each item's **sort title**, falling back to the title: *La Brea* is 5 from
  `le bureau`, compared as `Brea`, where its title would give 3.
- **One leading article leaves both sides**, from the reference's default list (the server
  preference whose default is `the,das,der,a,an,el,la`, read from the binary): `the office` is
  0 from *The Office*, `das office` and `la office` too, `le office` 3. Only one goes (`the the
  office` is 4), it needs a word after it (`the` alone stays `the`), and leading spaces go with
  it (` the  office` is 0); a tab is not a space (`the<TAB>office` is 4) and a trailing space
  counts (`the office ` is 1). A sort title that kept its article loses it the same way: *La
  défonce des animaux* is 11 from `le seigneur des anneaux`, compared as `défonce…`, and *La
  Momie* 5 from `un homme`.
- **Case folds for ASCII only, and a fixed set of letters folds to its base**, measured one by
  one over U+00C0–U+024F and U+1E00–U+1EFF by replacing a letter of a title: every Latin-1
  letter with a base (`ÉTÉ` is 1 from *Ève*), `ß` as `s`, the lower-case macrons `ā ē ī ō ū`
  (not the capitals), the carons (`Č Ď Ě Ľ Ň Ř Š Ť Ž Ǎ Ǐ Ǒ Ǔ Ǧ Ǩ ǰ` and their small forms).
  Nothing else in Latin Extended-A, -B or Additional folds: `boze cialo` is 4 from *Boże Ciało*.
  Combining marks are dropped (`e` + U+0301 is `e`).
- **What is left counts in bytes**: `鉄男` is 5 from `아랑`, `wall·e` 2 from *Wall-E*, `İ` 2
  from `i`. The ten Latin-1 characters with no base — `Æ Ð Ø Þ æ ð ø þ × ÷` — count as one unit
  each, equal only to themselves (`duneæ` is 1 from *Dune*, and `docteuræ` 1 from *Docteur?*).

**Kinds and shelves.** By default films and series; `type=1`, `2`, `3`, `4` ranks films,
series, seasons or episodes alone, a music or collection type nothing (`size="0"`). `limit`
counts **across** the shelves, not per shelf as the spec says: `breaking bad` at the default 3
is two series and one film. Each kind is a `Hub` with `title` and `type` its English word
(`movie`, whatever the client's language), `hubIdentifier` `results.search.{type}`,
`context="hub.results.search"`, `size`, `more="0"`, `style="shelf"`, `distance` its closest,
and `hubKey` its items as one lookup. Shelves come closest first, films before series when
they tie (`tom hanks`: both at 4). Items are `/library/search`'s, with `distance` first and the
library attributes; a film without a summary carries `summary=""`. The container holds `size`,
the number of shelves, and nothing else.

**Parameters.** `query` absent or empty is `400`; a blank term is a term (` ` ranks the
one-letter titles). `limit` defaults to 3, `0` answers an empty container, a non-number `400`,
a negative number `500` on the reference and `400` here, and it is capped at 500. `type` that
is not one number (`abc`, `1,2`) is `400`. `sectionId` is ignored, even `999`.

**What differs, each knowingly:**

- **The reference can skip a closer title.** Its SQL is `ORDER BY LEVENSHTEIN('<term>',
  metadata_items.title) LIMIT n` (both strings are in the binary), a distance on the *raw*
  title, case kept, that it then republishes on the folded sort title. So its `n` are the
  closest by a measure other than the one it shows: for `the` it lists *Abe* at 2 and not *Ted*,
  also 2 (`Ted` is 3 with its capital), and for `dune` at the default 3 *Cube* (2) rather than
  *Dude* (1). An upper-case term picks other titles again (`LE MAL-AIMÉ`). Ours are the closest
  by the distance published. On 63 terms at the default limit, 175 of 189 positions hold the same
  distance and the other 14 a smaller one; at `limit=10`, 534 of 630 and 96; on episodes, 463 and
  167. None is farther, and every item both list has the same distance on both.
- **Ties.** Among titles at one distance the reference follows its scan; ours come in folded
  title order, then by id (for the two *Breaking Bad*, the reference lists library 5's first).
- **A term past 256 bytes is a `400`**, which the reference would rank: each title costs a
  pass over the term.

**Cost.** Ours reads the titles of about the term's length only, from an index of the folded
titles and their lengths (`m0032_folded_title`): a distance is at least the difference of the
lengths, so lengths are read in widening bands until the closest found are nearer than any band
left. That bound is strong when the closest are near: `dune` reads 1,608 of the 22,337 films and
series; it weakens as they get farther, since titles cluster around a dozen units: `breaking
bad` 7,928, `kaamelott` (third match at 5) 13,745, a term near nothing 19,891. What is read is
two columns of one covering index range per kind and side, measured by a bit-parallel distance;
items are loaded for the page alone. The reference reads every item. On a debug build, `dune`
answers in 77 ms where the reference takes 96, `kaamelott` in 121 against 81, `pilot` on
episodes in 78 against 331.

### Autocompletion, `/library/sections/{id}/autocomplete`

What a filter field offers as it is typed into. One `{field}.query` parameter names the field and
the text; `type` is required. Measured on films and series libraries of the reference:

- **The rule.** The text is one phrase beginning a word: `dun` completes *Crocodile Dundee*, `first
  sl` and `slam d` *The First Slam Dunk*, `dun ` nothing. A word begins after a space only — `aim`
  does not complete *Le mal-aimé*, `homme` not *L'homme qui aimait les soucoupes volantes* — and a
  space is matched as typed (` ` completes the titles holding two spaces, or starting with one).
  Case does not count, `À TOUS` and `à tous` alike complete *À tous les garçons*; accents do, `aime`
  completes *Mange, prie, aime* and not *C'est dur d'être aimé*. `%` and `_` are text. Only the
  title is read: `dunkirk` does not complete *Dunkerque*, whose original title it is.
- **Items.** `title` completes items, listed as `/all` lists them, in its order, with the other
  listing filters applied (`year=2021` keeps one *Dune*), and each carries `title.queryRange`,
  `first,last` in **bytes**, both included: *À tous les garçons que j'ai aimés* is `30,32` for
  `aim`. The range is the first occurrence of the text anywhere in the title, by bytes with ASCII
  case aside, not the word the match was made on: `d` completes *3 Billboards : Les Panneaux de la
  vengeance* by `de` and is ranged `10,10`, the `d` of `Billboards`; a match on a capital outside
  ASCII has no range (`à tous`). Unpaged on the reference (`d`: 2,723 films); windowed here, as
  `/all` is.
- **Tags.** `genre`, `country`, `actor`, `director`, `writer`, `producer` complete the tags the
  library's items of `type` carry, by the same rule on the name (`hanks` and `tom h` complete Tom
  Hanks), every credit counted, ordered by name, **100 at most** (`actor.query=a` answers
  `totalSize="100"`). Each is a `Directory` with `id`, `filter` (`actor=469791`), `tag`, `tagType`,
  and a person's `tagKey` and `thumb`.
- **The container** is a "secondary" view: `allowSync="0"`, the library's `art` and `thumb`,
  `content="secondary"`, `identifier`, `mediaTagPrefix`, `mediaTagVersion`, `title1` the library,
  `viewGroup="secondary"`, `viewMode="65592"`.
- **Refusals.** No `{field}.query`, an empty one, two of them, no `type` or one that is not a
  number, an unknown field (`foo`) are `400`; an unknown library `404`. `decade`, `resolution`,
  `audioLanguage` and `unwatched` answer `500` on the reference, a fault, and `400` here.

Compared on 37 cases over four libraries: the totals agree on 30, 3,083 of 3,538 entries are
listed on both sides, and of those 3,082 carry the same range. The rest is the live reference
having moved since the import — a film renamed (*Arthur 3: la guerre des deux mondes*), actors
added (Dan Jeannotte) — and two orders:

- **Episodes** follow `/all`'s order for `type=4`, by series, season and episode as on the
  reference since `/library/sections/{id}/allLeaves` (*A library's leaves*); they followed the
  episode's own title when this was measured (`épisode`: 74 of the first 500 in common).
- **Case outside ASCII** inside a phrase: storage lowers ASCII alone, and tries the text with
  the initials of its words in upper case, so `à tous` and `les évadés` complete as on the
  reference; a capital outside ASCII inside a word would not.

`titleSort`, `originalTitle`, `studio`, `contentRating`, `summary`, `tagline` and `year`, which
the reference completes as items (`year.query=19` by prefix), and `collection`, `label`, `mood`
and `style`, which it completes as tags, answer `501` here.

### Live TV and DVR, with none configured

This server has no grabber, no tuner and no guide provider, and answers all 52 Live TV, DVR,
guide and recording operations as the reference does without a DVR (`livetv.rs`). Captured on
the reference, which has no DVR and one dead tuner left over from an old setup:

| Request | Reference | Here |
| --- | --- | --- |
| `GET /livetv/dvrs`, `/livetv/sessions` | `200`, `<MediaContainer size="0">`, `{"MediaContainer":{"size":0}}`; with both container parameters, `totalSize="0" offset="0"` and the two paging headers | Byte-identical |
| Anything under `/livetv/dvrs/{id}`, `/livetv/sessions/{id}`, `/media/grabbers/devices/{id}`, `/media/grabbers/operations/{id}`, `/media/subscriptions/{id}` | `404` for an id it does not know, every method (`999`; GET and PUT on a subscription answer `401`, see below) | `404` |
| `/livetv/epg/countries/{country}/{epgId}/…` | `200` empty `Lineup` lists under its cloud provider, `404` under any other `epgId` | `404`: no provider here |
| `/livetv/epg/channelmap`, `channels`, `lineupchannels`, `lineup` | `400` missing `device`/`lineup`/`lineupGroup`; `404` for a device or a lineup of a provider it does not know | Same |
| `/tv.plex.providers.epg.{id}:{device}/…` | `404` | `404`: no route |
| `GET /media/subscriptions/template` | `400` without `guid` | `400` |

Where we answer otherwise, on purpose:

- **Empty where the reference lists what it has.** `/media/grabbers` (the reference ships four:
  HDHomeRun, Stream, Download, Remote), `/media/grabbers/devices` (its leftover tuner),
  `/livetv/epg/countries` (97, from plex.tv's guide) and `/livetv/epg/languages` (594). Not
  one of them can be used here, and a list is what a client offers. Their empty form is the
  one above; the reference's own empty devices list was not observable.
- **No subscription refusal.** The reference answers `401` to `GET /media/subscriptions`,
  `/scheduled`, `/template?guid=` and to reading or editing one subscription, with the owner's
  token: a refusal for want of a subscription, which this server does not make. The lists are
  empty, the template offers nothing, a subscription is a `404`.
- **Writes that cannot be honored, decided without a capture** — creating on the reference
  would change it: `POST /livetv/dvrs` is a `400` without `device` and a `404` with one, since
  no device exists; `POST /media/grabbers/devices` is the spec's `400` for a device that cannot
  be added; `POST /media/subscriptions` a `400`; discovery an empty device list; processing
  subscriptions an empty `200`, `text/html`, as the spec types it.
- **Bodies of two errors.** `/livetv/epg/lineup` with a device it does not know answers `404`
  with the body `Unknown media grabber`; a lineup of its own provider it cannot parse is a
  `400` carrying a `MediaContainer` (`message="Failed to parse data" status="-3"`). Ours are
  the usual HTML pages: clients branch on the status.

`GET /` keeps `livetv="7"` and `allowTuners`, as the reference does without a DVR; whether Live
TV shows in a client depends on `/media/providers`, which, as on the reference, carries no guide
provider.

### Music, held nowhere

This server imports no music: a music section is copied as an empty library (`type="artist"`),
and nothing of kind artist, album or track exists. The reference has no music library either,
so its empty answers were measured on its film and series libraries:

| Request | Reference | Here |
| --- | --- | --- |
| `/library/sections/{id}/all?type=8`, `9`, `10` on a film library | `200`, the library's container with `size="0"` | Same, through the official API: `ItemType` has `ARTIST`, `ALBUM` and `TRACK`, which keep nothing. Before, an unknown `type` placed no filter and listed every film |
| `firstCharacter`, `genre`, `year` with a music `type` | Empty containers | Same, by the same filter |
| `/library/sections/{id}/albums` | `size="0"`, `allowSync="0"`, `mixedParents="1"`, `title2="By Album"`, `viewGroup="album"`, `nocache` on a series library; `404` for an unknown library | Same, always windowed like `/all` |
| `/hubs/home/recentlyAdded?type=8` | Empty | Empty (already so) |
| `/search?query=dune&type=10` | `size="0"`, no `Provider` | Same. `type` is honored: `type=1` films only, `type=4` episodes only, an unknown number nothing, each without the `Provider` list a search of every kind carries |

A music library's `/hubs/sections/{id}` has no shelf here; Plex's would be a music library's
own, and there is nothing to fill them. Not measured: the reference has no such library. A
`type` the official API has no kind for, a photo (13) for one, still places no filter on `/all`,
where the reference answers empty.

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
  unknown session answers `404` with or without a token. Ours are opened by the stream's own
  secret instead: see "Streams at addresses of their own".
- A client with no token matches the server it reached with its plex.tv entry through
  `/identity`, so that answer must be the paired server's identifier.
- Without a token, `GET /` answers `401` on the reference, from loopback and from the LAN alike,
  where `/identity` answers `200`. Plex Web needs that `401`: a `200` tells it the server admits
  anonymous visitors, so it never offers a sign-in and then stops at "Not authorized".
- With a claimed account, `/` and `/media/providers` both carry, in this order, `myPlex`,
  `myPlexMappingState` (`mapped`), `myPlexSigninState` (`ok`), `myPlexSubscription` and
  `myPlexUsername`. Plex Web treats a server as claimed when `myPlexSigninState` is `ok`.
