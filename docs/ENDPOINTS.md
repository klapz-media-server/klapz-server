# Endpoints

Implementation tracker for the 260 operations in `docs/openapi.json` (Plex Media Server 1.2.3).

**Generated** — regenerate with `python tools/gen_endpoints.py` after a spec update. `partial` and `done` are judgements, carried over from the previous file along with every note; `todo`, `stub` and `deferred` are derived from the spec on each run, so they cannot drift out of date.

## Status vocabulary

| Status | Meaning |
| --- | --- |
| `todo` | Not started |
| `stub` | Route declared, answers `501 Not Implemented` |
| `partial` | Works on the common path, gaps listed in Notes |
| `done` | Conformant and covered by tests |
| `deferred` | Out of scope for now: scanning, matching and media analysis are a separate binary on Plex |

## Progress

| Status | Operations | Share |
| --- | --: | --: |
| `todo` | 3 | 1.2 % |
| `stub` | 56 | 21.5 % |
| `partial` | 36 | 13.8 % |
| `done` | 134 | 51.5 % |
| `deferred` | 31 | 11.9 % |
| **Total** | **260** | |

## By domain

| Domain | Operations | `done` | `partial` | `stub` | `todo` | `deferred` |
| --- | --: | --: | --: | --: | --: | --: |
| [Library](#library) | 85 | 28 | 18 | 19 | 2 | 18 |
| [Hubs](#hubs) | 15 | 5 | 3 | 7 | 0 | 0 |
| [Content](#content) | 13 | 5 | 3 | 5 | 0 | 0 |
| [Devices](#devices) | 13 | 13 | 0 | 0 | 0 | 0 |
| [EPG](#epg) | 13 | 13 | 0 | 0 | 0 | 0 |
| [Library Playlists](#library-playlists) | 13 | 12 | 1 | 0 | 0 | 0 |
| [DVRs](#dvrs) | 12 | 12 | 0 | 0 | 0 | 0 |
| [Metadata Agents](#metadata-agents) | 12 | 0 | 0 | 0 | 0 | 12 |
| [Subscriptions](#subscriptions) | 10 | 10 | 0 | 0 | 0 | 0 |
| [Download Queue](#download-queue) | 9 | 0 | 0 | 9 | 0 | 0 |
| [Play Queue](#play-queue) | 9 | 8 | 1 | 0 | 0 | 0 |
| [Status](#status) | 6 | 4 | 2 | 0 | 0 | 0 |
| [Butler](#butler) | 5 | 0 | 0 | 5 | 0 | 0 |
| [Transcoder](#transcoder) | 5 | 1 | 4 | 0 | 0 | 0 |
| [General](#general) | 4 | 2 | 0 | 2 | 0 | 0 |
| [Live TV](#live-tv) | 4 | 4 | 0 | 0 | 0 | 0 |
| [Provider](#provider) | 4 | 1 | 0 | 1 | 1 | 1 |
| [Library Collections](#library-collections) | 3 | 3 | 0 | 0 | 0 | 0 |
| [Log](#log) | 3 | 0 | 0 | 3 | 0 | 0 |
| [Playlist](#playlist) | 3 | 2 | 1 | 0 | 0 | 0 |
| [Preferences](#preferences) | 3 | 2 | 0 | 1 | 0 | 0 |
| [Timeline](#timeline) | 3 | 2 | 1 | 0 | 0 | 0 |
| [Updater](#updater) | 3 | 2 | 0 | 1 | 0 | 0 |
| [Activities](#activities) | 2 | 1 | 0 | 1 | 0 | 0 |
| [Events](#events) | 2 | 0 | 0 | 2 | 0 | 0 |
| [Search](#search) | 2 | 1 | 1 | 0 | 0 | 0 |
| [UltraBlur](#ultrablur) | 2 | 1 | 1 | 0 | 0 | 0 |
| [Collections](#collections) | 1 | 1 | 0 | 0 | 0 | 0 |
| [Rate](#rate) | 1 | 1 | 0 | 0 | 0 | 0 |

## Outside the spec

Served, but absent from `docs/openapi.json` and therefore from the counts above.

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/video/:/transcode/universal/ping` | user | Keep a transcode session alive | `200` for a session it knows, `404` otherwise, as the reference. No test yet. |
| `partial` | `GET` | `/video/:/transcode/universal/stop` | user | Stop a transcode session | Stops the process and closes the playback session; `404` for one it does not know, as the reference. No test yet. |
| `done` | `GET` | `/library/collections/{collectionId}` | user | A collection | As `/library/metadata/{collectionId}` shows it: in the lookup form, with its library and its locks. See spec-gaps.md. |
| `done` | `GET` | `/library/collections/{collectionId}/children` | user | Items in a collection | The same answer as `/items`, which the spec declares: what Plex Web calls. See spec-gaps.md. |
| `done` | `GET` | `/playlists/all` | user | The caller's playlists | `/playlists` under the key a library's shelf of playlists links to. |
| `done` | `GET` | `/hubs/home/recentlyAdded` | user | What a library, or every library of a kind, added lately | `type=1` films by date added, ties by arrival; `type=2` a series library's grouped entries, at most 50; anything else empty. The spec names it only in examples. See spec-gaps.md. |
| `done` | `GET` | `/hubs/continueWatching/items` | user | Every item of the home Continue Watching shelf | Films in progress and next episodes, newest first, with `includedAt`. See spec-gaps.md. |
| `done` | `GET` | `/hubs/home/continueWatching` | user | What was started | Films and episodes in progress, without the next episodes. |
| `done` | `GET` | `/hubs/home/onDeck` | user | The next episodes | Each series' episode, in progress or the one after the last watched; no films. See spec-gaps.md. |
| `done` | `GET` | `/hubs/sections/{sectionId}/continueWatching/items` | user | Every item of a library's Continue Watching shelf | The library's films in progress or its series' next episodes. |
| `done` | `GET` | `/:/progress` | user | Remember where a viewing stopped | Keeps a position past a minute with no check against the end, as the reference does; an unknown item is acknowledged. Not in the spec. |
| `partial` | `GET` | `/library/sections/{sectionId}/{filter}` | user | The choices of one filter | Genres, countries, directors, writers, starring actors, producers, years, decades, content ratings, studios and resolutions, encoded as the reference encodes each. No `None` rating. Windowed at 500. See `docs/spec-gaps.md`. |
| `done` | `GET` | `/library/metadata/{id}/userState` | user | An item's state for the watchlist button | Not a route of the reference, which answers `404`: the shape of plex.tv's Discover provider, for Plex Web. Always unwatched: watch state is not recorded yet. |
| `partial` | `GET` | `/library/search` | user | Search field (universal search) | Films, series and episodes ranked by the official API's `search` score, then actors, directors, genres and collections, each group best first, cut to `limit`; `searchTypes` honored as measured (`people` from the libraries the other values name). Gaps: the reference's fuzzy matching, and the title-ordered pool it ranks, which we do not cut. See `docs/spec-gaps.md`. |
| `done` | `GET` | `/library/sections` | user | Library sections | Byte-identical to the reference. The spec declares only its `/all` alias. |
| `partial` | `GET` | `/library/metadata/{id}/children` | user | Seasons of a series, episodes of a season | Zero differing fields against the reference on seven parents across two libraries; `400` on a film or an episode, as the reference. A series' first page leads with the `All episodes` entry, counted in `size` but not in `totalSize`, as the reference does. |
| `partial` | `GET` | `/library/metadata/{id}/grandchildren` | user | Every episode of a series, season by season | Same field fidelity as `/children`. Gaps: those of `/library/metadata/{ids}`. |
| `partial` | `GET` | `/search` | user | Global search across libraries | Matches exactly as the reference (word prefix over title, sort title, original title) and answers as it does: the first 15 films, series and episodes in title order, kind by kind, unranked. `type=` narrows to one kind without the `Provider` list, as the reference; a music or unknown type finds nothing. The spec declares only `/hubs/search`. |

## Library

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/library/all` | user | Get all items in library |  |
| `deferred` | `DELETE` | `/library/caches` | admin | Delete library caches | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/library/clean/bundles` | admin | Clean bundles | scanner/analysis, separate binary |
| `stub` | `POST` | `/library/file` | user | Ingest a transient item |  |
| `deferred` | `GET` | `/library/matches` | user | Get library matches | scanner/analysis, separate binary |
| `stub` | `GET` | `/library/media/{mediaId}/chapterImages/{chapter}` | user | Get a chapter image |  |
| `stub` | `GET` | `/library/metadata/augmentations/{augmentationId}` | user | Get augmentation status |  |
| `partial` | `DELETE` | `/library/metadata/{ids}` | admin | Delete a metadata item | A collection, as Plex Web deletes one (`deleteCollection`); an item stays `501`, deleting it being the scanner's business. |
| `partial` | `PUT` | `/library/metadata/{ids}` | admin | Edit a metadata item | Films, series and episodes, any library, each through `updateMovie`/`updateShow`/`updateEpisode`: the fields of `PUT /library/sections/{sectionId}/all`, as `{field}.value` or bare as the spec writes them, and `{field}.locked`; `type` ignored. Measured on the reference: `.value` wins over the bare form, a lock other than `0`/`1` is ignored, no parameter is `400`, no item named `404`. Library admins only; seasons, tags, artwork and music `501`. See spec-gaps.md. |
| `deferred` | `PUT` | `/library/metadata/{ids}/addetect` | admin | Ad-detect an item | scanner/analysis, separate binary |
| `partial` | `GET` | `/library/metadata/{ids}/allLeaves` | user | Get the leaves of an item | A series' episodes with its studio on each, what `/children`'s `All episodes` entry opens; a season, a film or an episode is an empty container naming it, as on the reference (seven seasons of six series measured). Containers identical to the reference on five nodes; the items' gaps are those of `/grandchildren` (artwork, slug, `audienceRatingImage`, `chapterSource`). One id, not a list. |
| `deferred` | `PUT` | `/library/metadata/{ids}/analyze` | admin | Analyze an item | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/library/metadata/{ids}/analyzeLoudness` | admin | Detect loudness of audio streams | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/library/metadata/{ids}/chapterThumbs` | admin | Generate thumbs of chapters for an item | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/library/metadata/{ids}/credits` | admin | Credit detect a metadata item | scanner/analysis, separate binary |
| `done` | `GET` | `/library/metadata/{ids}/extras` | user | Get an item's extras | The item's own extras, by position then id, for a film, a series, a season or an episode, through `Movie.extras` and its kin; `extraType` and `sort` change nothing, both container parameters window it; a list or an unknown id `404`, an extra's own `400`. Local extras as captured on the test libraries; the provider's trailers, which the reference lists only to a subscriber, as it looks each up. Artwork left out, as on items. See spec-gaps.md. |
| `done` | `POST` | `/library/metadata/{ids}/extras` | user | Add to an item's extras | Refused `403` after the reference's own checks (`404` for an unknown item, `400` without `url` or `title` and for an extra): its `url` is a path or an address to fetch, and no endpoint takes one. The reference answers `404` for every address measured. See spec-gaps.md. |
| `stub` | `GET` | `/library/metadata/{ids}/file` | user | Get a file from a metadata or media bundle |  |
| `stub` | `PUT` | `/library/metadata/{ids}/index` | admin | Start BIF generation of an item |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/intro` | admin | Intro detect an item | scanner/analysis, separate binary |
| `partial` | `POST` | `/library/metadata/{ids}/marker` | user | Create a marker | The caller's bookmark, through `addMarker`, with the rules measured on the reference's test libraries: `type=bookmark` by name, a required `startTimeOffset`, an optional end after it, `attributes[title]` and `[color]` kept; an identical span answers the bookmark already there. Gaps: any other `type` is a `400` (the work's intro and credits are written on the official API by the library's editors; the reference was not measured there), and a series or a season is a `400` where the reference keeps a bookmark. See spec-gaps.md. |
| `done` | `DELETE` | `/library/metadata/{ids}/marker/{marker}` | user | Delete a marker | The caller's bookmark, through `deleteMarker`: an empty `200`, `400` for another item's marker or one that is not a bookmark, `404` for one that does not exist, as measured. |
| `done` | `PUT` | `/library/metadata/{ids}/marker/{marker}` | user | Edit a marker | The caller's bookmark, through `updateMarker`: `type` ignored, the end and the title kept when not given, `400` for another item's marker or a span ending before it starts, `404` for one that does not exist, as measured. |
| `done` | `PUT` | `/library/metadata/{ids}/match` | admin | Match a metadata item | `matchItem`, in the background: `guid` names this server's record as `{agent}://{id}` (`tmdb://603`), as `/matches` lists it; without it the best result is taken. Another provider's guid is a `400`. |
| `done` | `PUT` | `/library/metadata/{ids}/matches` | admin | Get metadata matches for an item | `metadataMatches`: `SearchResult` elements with `guid` `{agent}://{id}`, `name`, `year`, `score`, `type`; `title` and `year` replace the item's own. |
| `stub` | `DELETE` | `/library/metadata/{ids}/media/{mediaItem}` | admin | Delete a media item |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/merge` | admin | Merge a metadata item | scanner/analysis, separate binary |
| `done` | `GET` | `/library/metadata/{ids}/nearest` | user | Get nearest tracks to metadata item | Nothing is a track here: `403` once a number names something, `404` when none does, `400` for an unreadable `limit`, `maxDistance` or exclusion, as measured on the reference's film and series libraries (`docs/spec-gaps.md`). |
| `partial` | `PUT` | `/library/metadata/{ids}/prefs` | admin | Set metadata preferences | A collection's `collectionMode` (`-1`–`2`) and `collectionSort` (`0`–`2`), any other name or value `400`; an item's preferences `501`. |
| `done` | `PUT` | `/library/metadata/{ids}/refresh` | admin | Refresh a metadata item | `refreshItemMetadata`, in the background: the record the item shows fetched again, metadata and images. |
| `partial` | `GET` | `/library/metadata/{ids}/related` | user | Get related items | A film: similar films (approximate ranking), its director and two first-billed actors, the last two matching the reference on 59 of 60 films. A series: similar series only. Not served: collection and `external.*` shelves, a series' network and cast shelves. |
| `partial` | `GET` | `/library/metadata/{ids}/similar` | user | Get similar items | Same container as the reference; the ranking is an approximation. |
| `deferred` | `PUT` | `/library/metadata/{ids}/split` | admin | Split a metadata item | scanner/analysis, separate binary |
| `partial` | `GET` | `/library/metadata/{ids}/subtitles` | admin | Add subtitles | The search of online subtitles. No provider here: the empty container the reference answers when OpenSubtitles finds nothing, after its checks — `language` required, two letters and an optional region (`400`; the reference fails a three-letter code `500`), one film or episode (`404`), `mediaItemID` one of its versions (`404`). Library admins and owners. The `PUT` that downloads a result has no route. See spec-gaps.md. |
| `partial` | `GET` | `/library/metadata/{ids}/tree` | user | Get metadata items as a tree | Films, series, seasons, episodes and collections: byte for byte on the test libraries, 196 of 225 real items identical, the rest snapshot drift. Times unshifted where the reference subtracts its UTC offset. Not served: a trailer's tree (`404`, no trailer is looked up by number) and the keys an OpenSubtitles download adds to its track (`docs/spec-gaps.md`). |
| `done` | `PUT` | `/library/metadata/{ids}/unmatch` | admin | Unmatch a metadata item | `unmatchItem`: links, identifiers and images forgotten, `local://{id}` back, texts kept. |
| `done` | `GET` | `/library/metadata/{ids}/users/top` | admin | Get metadata top users | The reference answers `404` on every video item; ours answers the spec's shape for the caller's own viewings of the work as `accountID` 1 (`viewers` on the official API). A deliberate divergence. See spec-gaps.md. |
| `deferred` | `PUT` | `/library/metadata/{ids}/voiceActivity` | admin | Detect voice activity | scanner/analysis, separate binary |
| `stub` | `DELETE` | `/library/metadata/{ids}/{element}` | user | Delete an item's artwork, theme, etc |  |
| `stub` | `POST` | `/library/metadata/{ids}/{element}` | user | Set an item's artwork, theme, etc |  |
| `stub` | `PUT` | `/library/metadata/{ids}/{element}` | user | Set an item's artwork, theme, etc |  |
| `done` | `GET` | `/library/metadata/{ids}/{element}/{timestamp}` | user | Get an item's artwork, theme, etc | `Item.images`: `thumb` (an episode's still, else the poster), `art`, `clearLogo`, `banner`, `poster`, read from the artwork store; any timestamp serves the current image. Also a source of `/photo/:/transcode`. |
| `deferred` | `PUT` | `/library/optimize` | admin | Optimize the Database | scanner/analysis, separate binary |
| `done` | `PUT` | `/library/parts/{partId}` | user | Set stream selection | The caller's audio or subtitle per file, `subtitleStreamID=0` for none; `400` for a track not of the part or kind. Burned in on `subtitles=burn`. See spec-gaps.md. |
| `deferred` | `GET` | `/library/parts/{partId}/indexes/{index}` | user | Get BIF index for a part | scanner/analysis, separate binary |
| `deferred` | `GET` | `/library/parts/{partId}/indexes/{index}/{offset}` | user | Get an image from part BIF | scanner/analysis, separate binary |
| `partial` | `GET` | `/library/parts/{partId}/{changestamp}/{filename}` | user | Get a media part | Direct Play: the file, by part id, with `Range`. The change stamp and the name are ignored. |
| `done` | `GET` | `/library/people/{personId}` | user | Get person details | `personId` is a tag id, any kind: one `Directory` with `tagType`. The spec's `tagKey` form is a `404`, as on the reference. See spec-gaps.md. |
| `partial` | `GET` | `/library/people/{personId}/media` | user | Get media for a person | What carries the tag, episodes rolled up to their series, hidden libraries left out, by library title then films by title and series by id; listing filters honored, `sort` and `type` ignored. Capped at 500 where the reference lists everything, and titles in byte order rather than Unicode collation. See spec-gaps.md. |
| `stub` | `GET` | `/library/randomArtwork` | user | Get random artwork |  |
| `done` | `GET` | `/library/sections/all` | user | Get library sections (main Media Provider Only) | Alias of `/library/sections` (not in spec). Byte-identical to reference apart from `contentChangedAt`, whose value differs only because the imported snapshot predates the live server |
| `todo` | `POST` | `/library/sections/all` | admin | Add a library section |  |
| `done` | `DELETE` | `/library/sections/all/refresh` | admin | Stop refresh | `cancelLibraryScan` on every library the caller administers. |
| `stub` | `GET` | `/library/sections/prefs` | admin | Get section prefs |  |
| `done` | `POST` | `/library/sections/refresh` | admin | Refresh all sections | `scanLibrary` on every library the caller administers, music and photos skipped. |
| `stub` | `DELETE` | `/library/sections/{sectionId}` | admin | Delete a library section |  |
| `stub` | `GET` | `/library/sections/{sectionId}` | user | Get a library section by id |  |
| `stub` | `PUT` | `/library/sections/{sectionId}` | admin | Edit a library section |  |
| `partial` | `PUT` | `/library/sections/{sectionId}/all` | user | Set the fields of the filtered items | Films, series and episodes by `id`: `{field}.value` for titles, summary, tagline, studio, content rating, year, date and ratings, `{field}.locked` for the 20 fields we model, through `updateMovie`/`updateShow`/`updateEpisode`; library admins only. No `id`, tags, artwork, seasons and music answer `501`. Locks published as `Field` on lookups. See spec-gaps.md. |
| `deferred` | `PUT` | `/library/sections/{sectionId}/analyze` | admin | Analyze a section | scanner/analysis, separate binary |
| `done` | `GET` | `/library/sections/{sectionId}/audioCodec` | user | Get audio codecs in a section | The distinct values every track of every version of the library holds, in byte order, key percent-encoded once and no `fastKey`, as measured; a series library lists its episodes' whatever `type` says. Identical to the reference on ten libraries. Each value filters `/all` (`audioCodec=`, a comma `OR`, `!=` for none, repeated `AND`): 780 of 782 totals identical over every value of four libraries, the two others 3 episodes' sidecar subtitles the snapshot predates. Through `Library.trackValues`. Always windowed, as every choice list here. See spec-gaps.md. |
| `done` | `GET` | `/library/sections/{sectionId}/audioLayout` | user | Get audio layouts in a section | The distinct values every track of every version of the library holds, in byte order, key percent-encoded once and no `fastKey`, as measured; a series library lists its episodes' whatever `type` says. Identical to the reference on ten libraries. Each value filters `/all` (`audioLayout=`, a comma `OR`, `!=` for none, repeated `AND`): 780 of 782 totals identical over every value of four libraries, the two others 3 episodes' sidecar subtitles the snapshot predates. Through `Library.trackValues`. Always windowed, as every choice list here. See spec-gaps.md. |
| `partial` | `GET` | `/library/sections/{sectionId}/autocomplete` | user | Get autocompletions for search | `title` and the six tag fields, as measured: one phrase beginning a word (after a space only), case aside and accents minded, `title.queryRange` in bytes; tags capped at 100. `titleSort`, `originalTitle`, `studio`, `contentRating`, `summary`, `tagline`, `year`, `collection`, `label`, `mood`, `style` answer `501`. Official API: `ItemFilterInput.titleCompletion`, `Library.tags(completing:)`. |
| `done` | `DELETE` | `/library/sections/{sectionId}/collection/{collectionId}` | admin | Delete a collection | As measured, `collectionId` is the collection's tag number (`index`), and the collection is emptied, not deleted: every item of that library leaves it. Another library's number answers `200` and changes nothing, one no library carries `404`. Through `clearCollection`. See spec-gaps.md. |
| `done` | `GET` | `/library/sections/{sectionId}/collections` | user | Get collections in a section | In `/all`'s container, as `/all?type=18` answers too; by sort title (byte order, the reference's is numeric), `addedAt` or `updatedAt`, other sorts `501`. Always windowed, 500 at most without container parameters. Imported collections identical to the reference field for field, less artwork. See spec-gaps.md. |
| `partial` | `GET` | `/library/sections/{sectionId}/common` | user | Get common fields for items | One `common` element for the items `type` (required, `400` without), `id` (a comma list, 500 at most) and any listing filter keep: `mixedFields`, the shared fields, the tags and collections every item carries and the fields every item locked, through `Library.common`. Identical to the reference on films, series and episodes but for data newer than the snapshot. Not published: `Guid`, `Rating` (empty placeholders there), `Similar`, `CommonSenseMedia`, and `parentIndex`, `isAdult` and `userRating` in `mixedFields`; a season's undated `originallyAvailableAt` (`1969-12-31` there). See spec-gaps.md. |
| `stub` | `GET` | `/library/sections/{sectionId}/composite/{updatedAt}` | admin | Get a section composite image |  |
| `stub` | `PUT` | `/library/sections/{sectionId}/emptyTrash` | admin | Empty section trash |  |
| `done` | `GET` | `/library/sections/{sectionId}/filters` | admin | Get section filters | One type's `Filter` list from the `Meta` block, as `Directory` elements (`type`, the first by default): what a listing honors, the reference's entries in its order — 18 of 27 on a film library, 11 of 14, 0 of 0 and 9 of 17 on a series' three types. Windowed only on both container parameters, as on the reference. |
| `partial` | `GET` | `/library/sections/{sectionId}/firstCharacters` | user | Get list of first characters | Bucket sets and counts identical to the reference on four libraries. Folded first character, `#` for everything unlettered, percent-encoded keys. Singular form, absent from the spec — see spec-gaps.md. |
| `deferred` | `DELETE` | `/library/sections/{sectionId}/indexes` | admin | Delete section indexes | scanner/analysis, separate binary |
| `deferred` | `DELETE` | `/library/sections/{sectionId}/intros` | admin | Delete section intro markers | scanner/analysis, separate binary |
| `partial` | `GET` | `/library/sections/{sectionId}/prefs` | user | Get section prefs | The two settings Plex Web looks up by name, at their defaults; identical to the reference on those. The other 15 or 16 are about scanning and matching. |
| `todo` | `PUT` | `/library/sections/{sectionId}/prefs` | admin | Set section prefs |  |
| `done` | `DELETE` | `/library/sections/{sectionId}/refresh` | admin | Cancel section refresh | `cancelLibraryScan`: the running scan stops after its current file; `200` whether one was running or not. |
| `done` | `POST` | `/library/sections/{sectionId}/refresh` | admin | Refresh section | `scanLibrary`: answers `200` at once and scans in the background (the reference's `GET` too), a running scan coalesces the request, `force=1` changes nothing (a metadata refresh there, without an agent here), `path=` is `403`: no endpoint takes a path. |
| `done` | `GET` | `/library/sections/{sectionId}/sorts` | user | Get a section sorts | One type's `Sort` list from the `Meta` block, unmarked: 9 of 9 on a film library, 7 of 8, 1 of 2 and 6 of 7 on a series' types, the reference's entries attribute for attribute. |
| `done` | `GET` | `/library/sections/{sectionId}/subtitleCodec` | user | Get subtitle codecs in a section | The distinct values every track of every version of the library holds, in byte order, key percent-encoded once and no `fastKey`, as measured; a series library lists its episodes' whatever `type` says. Identical to the reference on ten libraries. Each value filters `/all` (`subtitleCodec=`, a comma `OR`, `!=` for none, repeated `AND`): 780 of 782 totals identical over every value of four libraries, the two others 3 episodes' sidecar subtitles the snapshot predates. Through `Library.trackValues`. Always windowed, as every choice list here. See spec-gaps.md. |
| `done` | `GET` | `/library/sections/{sectionId}/videoCodec` | user | Get video codecs in a section | The distinct values every track of every version of the library holds, in byte order, key percent-encoded once and no `fastKey`, as measured; a series library lists its episodes' whatever `type` says. Identical to the reference on ten libraries. Each value filters `/all` (`videoCodec=`, a comma `OR`, `!=` for none, repeated `AND`): 780 of 782 totals identical over every value of four libraries, the two others 3 episodes' sidecar subtitles the snapshot predates. Through `Library.trackValues`. Always windowed, as every choice list here. See spec-gaps.md. |
| `done` | `DELETE` | `/library/streams/{streamId}.{ext}` | user | Delete a stream | A sidecar: its file from disk, where a sidecar of its media may be, and its track (`deleteSidecar`). Measured: an extension or an id that is no number `400`, an unknown id `404`, a track inside the media `403`. Library admins and owners (`403` otherwise). See spec-gaps.md. |
| `partial` | `GET` | `/library/streams/{streamId}.{ext}` | user | Get a stream | A sidecar's file (`MediaStream.sidecar.text`), with no extension or its format's in any case, one `Range` honored; a track inside the media `501`, another extension `404`. Served as UTF-8 always, where the reference sends the bytes as written unless `encoding` is given, and typed `text/plain` rather than `text/html`. Downloaded subtitles (`blob://`) were not imported: `404`. See spec-gaps.md. |
| `done` | `PUT` | `/library/streams/{streamId}.{ext}` | user | Set a stream offset | The caller's offset on a subtitle, in milliseconds (`setSubtitleOffset`), published as `offset` and removed by `0`. Measured: no `offset` changes nothing, an extension, an unknown id or a value that is no 32-bit integer `400`; an audio or video track `200`, publishing nothing. See spec-gaps.md. |
| `stub` | `GET` | `/library/streams/{streamId}/levels` | user | Get loudness about a stream in json |  |
| `stub` | `GET` | `/library/streams/{streamId}/loudness` | user | Get loudness about a stream |  |
| `partial` | `GET` | `/library/tags` | user | Get all library tags of a type | Genres, countries and the four credits in id order, as measured, over `tags(kind:)`; tags nothing readable carries left out, 500 a page. Collections (`type=2`) `501`; the reference's internal types empty. See spec-gaps.md. |

## Hubs

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/hubs` | user | Get global hubs | The reference's eight shelves in its order: started, on deck, recent films and series, empty music, photos and personal videos, the caller's playlists played last. `count` and `identifier` honored; `onlyTransient` and `includeStations` tolerated. See spec-gaps.md. |
| `done` | `GET` | `/hubs/continueWatching` | user | Get the continue watching hub | Films in progress and, for each series, the episode in progress or the one after the last watched, newest first, within 16 weeks, every copy of a work listed; `count` honored. Next-episode rules identical to the reference on 14 measured cases. See spec-gaps.md. |
| `stub` | `GET` | `/hubs/items` | user | Get a hub's items |  |
| `done` | `GET` | `/hubs/metadata/{metadataId}` | user | Get hubs for a section by metadata item | An artist's four shelves for any item, keyed by its library, empty, as the reference answers; `>` encoded in a key. `404` for an unknown item. |
| `done` | `GET` | `/hubs/metadata/{metadataId}/postplay` | user | Get postplay hubs | After an episode its series' Continue Watching entry (`tv.upnext`), else the library's (`tv.ondeck`); after a film the next of its first collection showing it, a shelf per collection, similar films. Same shelves and next film as the reference on 199 of 200 films, the rest snapshot drift. See spec-gaps.md. |
| `partial` | `GET` | `/hubs/metadata/{metadataId}/related` | user | Get related hubs | `/library/metadata/{id}/related`'s shelves, in a container without the media tag attributes, as on the reference. |
| `done` | `GET` | `/hubs/popularInLibraries` | user | Get the popular in your libraries hub | `404`, as the reference answers: the shelf counts other users' plays, and no other user's reach a user's own server. See spec-gaps.md. |
| `done` | `GET` | `/hubs/promoted` | user | Get the hubs which are promoted | Started, on deck, every shown library's recently added shelf by title, hidden ones left out, the playlists. Library shelves identical to the reference on eight libraries. |
| `partial` | `GET` | `/hubs/sections/{sectionId}` | user | Get section hubs | Continue watching (empty: nothing can be in progress), recently added — a film library's films, a series library's new episodes grouped into their season or series (same entries in the same order as the reference on four series libraries) — and recent playlists (empty) on `includeLibraryPlaylists=1`; titles in the client's `X-Plex-Language`, from the reference's 32 catalogs. Not served: shelves needing watch state or release-date and rating sorts. |
| `stub` | `DELETE` | `/hubs/sections/{sectionId}/manage` | admin | Reset hubs to defaults |  |
| `stub` | `GET` | `/hubs/sections/{sectionId}/manage` | admin | Get hubs |  |
| `stub` | `POST` | `/hubs/sections/{sectionId}/manage` | admin | Create a custom hub |  |
| `stub` | `PUT` | `/hubs/sections/{sectionId}/manage/move` | admin | Move Hub |  |
| `stub` | `DELETE` | `/hubs/sections/{sectionId}/manage/{identifier}` | admin | Delete a custom hub |  |
| `stub` | `PUT` | `/hubs/sections/{sectionId}/manage/{identifier}` | admin | Change hub visibility |  |

## Content

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/library/collections/{collectionId}/composite/{updatedAt}` | user | Get a collection's image | Always `404`: no image is drawn, artwork not being served, and the reference answered `404` for every composite asked of it (test libraries and an imported collection). The address is published as the reference publishes it. |
| `done` | `GET` | `/library/collections/{collectionId}/items` | user | Get items in a collection | The items in a listing form naming their library, as `/children` answers; in the collection's order (release date, title or custom) or its rule's. Paged by both container parameters. Item gaps as on `/all`. See spec-gaps.md. |
| `partial` | `GET` | `/library/metadata/{ids}` | user | Get a metadata item | Items, media and files, served through the GraphQL schema. Zero field differences against the reference on shared items. Seasons are named at read time (`Season N`, `Specials`) and carry their series' guid, studio and year; episodes carry their season's and series' guids and titles. Genres, countries and credits with ids, filters, provider keys and portraits; `Guid` and `Rating`. 355 of 420 sampled items identical: 44 live metadata newer than the snapshot, 21 a credit's portrait (one per person here, one per credit on the reference). `Extras` on `includeExtras=1`, `primaryExtraKey` on films and series. Gaps: artwork, view state, `Part.key`, audio/videoProfile (and the `audioCodec` composition that needs them), slug, chapterSource, `hasPremiumPrimaryExtra`/`hasPremiumExtras`, `Stream.language` as a name rather than a tag. Sort collation differs — see spec-gaps.md. Carries `Stream`, which a listing omits, with the audio track that plays marked `selected`; no subtitle is, as no choice is recorded. |
| `done` | `GET` | `/library/sections/{sectionId}/albums` | user | Set section albums | Empty: no music is held. The official API is asked for albums and has none; container captured on the reference's film and series libraries. Always windowed, like `/all`. |
| `partial` | `GET` | `/library/sections/{sectionId}/all` | user | Get items in the section | Items, media and files through the GraphQL schema; zero field differences against the reference on shared items. `includeMeta=1` publishes the filters and sorts it honors. Filters: `title` (word prefix), `year`, `decade`, `contentRating`, `studio`, `resolution` — exact totals on 11 of 12 probes; `genre`, `country`, `director`, `writer`, `actor`, `producer` by id or name, comma for OR, repeat for AND — exact totals on 16 probes. `collection` by tag number or title, as `genre`. Any other advertised filter (`label`, …) answers 501 rather than being dropped. `type=18` lists the library's collections. Tags in the listing form: first 2 or 3 names per kind, derived from the tags rather than from the reference's stale cache. Gaps: artwork, view state, audio/videoProfile. Sort collation and range operators — see spec-gaps.md. |
| `done` | `GET` | `/library/sections/{sectionId}/allLeaves` | user | Set section leaves | A library's films or episodes: `/all?type=` the leaves' type, filters and sorts included, over `Library.items(filter: {depth: LEAVES})`, by series, season and episode by default (`orderBy: SERIES`). Identical to the reference in items, order and totals on 92 requests over four libraries, and in every series' episode order on four whole libraries (4,002 series); positions differ only by series-title collation. Container as captured: no `librarySection*`, no `title2`, `allowSync=0`, no `Meta`. A `type` naming no leaf kind is empty, as there (`type=18` lists collections on the reference). |
| `stub` | `GET` | `/library/sections/{sectionId}/arts` | user | Set section artwork |  |
| `done` | `GET` | `/library/sections/{sectionId}/categories` | user | Set section categories | The library's genres by name (`Library.tags(kind: GENRE)`), each a `Directory` opening `/all?genre={id}`, under a `Meta` type naming the view; whole unless both container parameters are sent, every other parameter ignored, as on the reference. `blendColor` by the reference's rule — a fixed color for 14 genre names, the others a 7-color cycle by tag id — identical on 335 of 335 tiles of ten libraries. `thumb` is the reference's fallback for a genre without background art, since no artwork is kept here. See spec-gaps.md. |
| `stub` | `GET` | `/library/sections/{sectionId}/cluster` | user | Set section clusters |  |
| `stub` | `GET` | `/library/sections/{sectionId}/computePath` | user | Similar tracks to transition from one to another |  |
| `stub` | `GET` | `/library/sections/{sectionId}/location` | user | Get all folder locations |  |
| `stub` | `GET` | `/library/sections/{sectionId}/moment` | user | Set section moments |  |
| `done` | `GET` | `/library/sections/{sectionId}/nearest` | user | The nearest audio tracks | No library holds an analysed track: `403` once `type` and `values` are readable, `400` otherwise, `404` for an unknown library, as measured; a music library could not be (`docs/spec-gaps.md`). |

## Devices

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/media/grabbers` | user | Get available grabbers | Empty container: this server has nothing of the kind. The reference lists the four grabbers it ships; see spec-gaps.md. |
| `done` | `GET` | `/media/grabbers/devices` | admin | Get all devices | Empty container: this server has nothing of the kind. The reference lists its tuners; see spec-gaps.md. |
| `done` | `POST` | `/media/grabbers/devices` | admin | Add a device | `400`, the spec's answer for a device that cannot be added: no grabber is there to recognize it. Not captured. |
| `done` | `POST` | `/media/grabbers/devices/discover` | admin | Tell grabbers to discover devices | Empty device list: there is no grabber to discover with. Not captured. |
| `done` | `DELETE` | `/media/grabbers/devices/{deviceId}` | admin | Remove a device | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `GET` | `/media/grabbers/devices/{deviceId}` | admin | Get device details | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `PUT` | `/media/grabbers/devices/{deviceId}` | admin | Enable or disable a device | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `PUT` | `/media/grabbers/devices/{deviceId}/channelmap` | user | Set a device's channel mapping | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `GET` | `/media/grabbers/devices/{deviceId}/channels` | admin | Get a device's channels | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `PUT` | `/media/grabbers/devices/{deviceId}/prefs` | admin | Set device preferences | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `DELETE` | `/media/grabbers/devices/{deviceId}/scan` | user | Tell a device to stop scanning for channels | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `POST` | `/media/grabbers/devices/{deviceId}/scan` | user | Tell a device to scan for channels | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `GET` | `/media/grabbers/devices/{deviceId}/thumb/{version}` | admin | Get device thumb | `404`: none exists here. Captured for an unknown id on the reference. |

## EPG

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/livetv/epg/channelmap` | user | Compute the best channel map | `400` without `device` and `lineup`, `404` with them, as captured. |
| `done` | `GET` | `/livetv/epg/channels` | user | Get channels for a lineup | `400` without `lineup`, `404` with one, as captured for a provider it does not know. |
| `done` | `GET` | `/livetv/epg/countries` | user | Get all countries | Empty container: this server has nothing of the kind. The reference lists plex.tv's 97 guide countries; see spec-gaps.md. |
| `done` | `GET` | `/livetv/epg/countries/{country}/{epgId}/lineups` | user | Get lineups for a country via postal code | `404`: there is no guide provider, and the reference 404s any provider it does not have. |
| `done` | `GET` | `/livetv/epg/countries/{country}/{epgId}/regions` | user | Get regions for a country | `404`: there is no guide provider, and the reference 404s any provider it does not have. |
| `done` | `GET` | `/livetv/epg/countries/{country}/{epgId}/regions/{region}/lineups` | user | Get lineups for a region | `404`: there is no guide provider, and the reference 404s any provider it does not have. |
| `done` | `GET` | `/livetv/epg/languages` | user | Get all languages | Empty container: this server has nothing of the kind. The reference lists 594 guide languages; see spec-gaps.md. |
| `done` | `GET` | `/livetv/epg/lineup` | user | Compute the best lineup | `400` without `device` and `lineupGroup`, `404` with them, as captured (the reference's body reads `Unknown media grabber`). |
| `done` | `GET` | `/livetv/epg/lineupchannels` | user | Get the channels for mulitple lineups | `400` without `lineup`, `404` with one, as captured for a provider it does not know. |
| `done` | `GET` | `/tv.plex.providers.epg.{identifier}:{deviceId}/grid` | user | Get airing information | `404`, as on the reference without a DVR: no route, since there is no provider to serve one. |
| `done` | `GET` | `/tv.plex.providers.epg.{identifier}:{deviceId}/lineups/dvr/channels` | user | Get channels for a lineup | `404`, as on the reference without a DVR: no route, since there is no provider to serve one. |
| `done` | `GET` | `/tv.plex.providers.epg.{identifier}:{deviceId}/watchnow` | user | Get hubs | `404`, as on the reference without a DVR: no route, since there is no provider to serve one. |
| `done` | `GET` | `/tv.plex.providers.epg.{identifier}:{deviceId}/watchnow/all` | user | Get currently airing | `404`, as on the reference without a DVR: no route, since there is no provider to serve one. |

## Library Playlists

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `POST` | `/playlists` | user | Create a Playlist | From `uri` (items, a series or season as its episodes, a library listing) or `playQueueID`; `smart=1` saves the listing as a rule, `501` for a filter it cannot honor. Repeats dropped. `400` without `type`, `title` or content, and for a video item in an audio playlist (the reference answers `400` and creates it anyway). Another server's URI makes an empty playlist, as captured. |
| `done` | `POST` | `/playlists/upload` | admin | Upload | Refused by design: it imports `.m3u` files from a path the client names, and no endpoint accepts a path from a client. `403` with a `path`, `400` without, as the reference without one. See spec-gaps.md. |
| `done` | `DELETE` | `/playlists/{playlistId}` | user | Delete a Playlist | `204`, then `404`, as captured. |
| `done` | `PUT` | `/playlists/{playlistId}` | user | Editing a Playlist | `title` and `summary`, bare or `.value`, and `.locked`; `200` with an empty HTML body, as captured. Artwork fields answer `501`. Edits playlists only, where the reference edits any item by this route. |
| `partial` | `GET` | `/playlists/{playlistId}/generators` | user | Get a playlist's generators | One `PlayQueueGenerator` per entry, or a smart playlist's rule with its `uri`. Our `order` is the entry's position; no `changedAt`, and `updatedAt` is when the entry was added. |
| `done` | `DELETE` | `/playlists/{playlistId}/items` | user | Clearing a playlist | Empties a playlist of chosen items and answers it; `400` on a smart one, as captured. |
| `done` | `PUT` | `/playlists/{playlistId}/items` | user | Adding to a Playlist | Appends, dropping what it holds, with `leafCountAdded`/`leafCountRequested`; on a smart playlist a library listing replaces the rule and anything else is `400`, as captured. |
| `done` | `DELETE` | `/playlists/{playlistId}/items/{generatorId}` | user | Delete a Generator | Removes an entry by `playlistItemID`; `404` for one not in this playlist, `400` on a smart playlist, as captured. |
| `done` | `GET` | `/playlists/{playlistId}/items/{generatorId}` | user | Get a playlist generator | `404`: generators by id are the optimizer's, and nothing here optimizes. The reference answers `404` for a chosen item's and a smart playlist's too. |
| `done` | `PUT` | `/playlists/{playlistId}/items/{generatorId}` | user | Modify a Generator | `400` on the caller's playlist, `404` otherwise, as the reference answers on a playlist of chosen items: the optimizer's alone. |
| `done` | `GET` | `/playlists/{playlistId}/items/{generatorId}/items` | user | Get a playlist generator's items | `404`, as captured: the optimizer's alone. |
| `done` | `PUT` | `/playlists/{playlistId}/items/{generatorId}/{metadataId}/{action}` | user | Reprocess a generator | `404`, as captured: the optimizer's alone. |
| `done` | `PUT` | `/playlists/{playlistId}/items/{playlistItemId}/move` | user | Moving items in a playlist | After `after`, or first without it; `404` for an entry not in the playlist, `400` on a smart one, as captured. An `after` naming no entry is `400`, where the reference moves the entry first. See spec-gaps.md. |

## DVRs

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/livetv/dvrs` | user | Get DVRs | Empty container, as the reference answers without a DVR: captured, byte-identical in both formats, window included. |
| `done` | `POST` | `/livetv/dvrs` | admin | Create a DVR | `400` without `device`, `404` with one: no device exists. Not captured, since it would create a DVR. |
| `done` | `DELETE` | `/livetv/dvrs/{dvrId}` | admin | Delete a single DVR | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `GET` | `/livetv/dvrs/{dvrId}` | user | Get a single DVR | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `POST` | `/livetv/dvrs/{dvrId}/channels/{channel}/tune` | user | Tune a channel on a DVR | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `DELETE` | `/livetv/dvrs/{dvrId}/devices/{deviceId}` | admin | Remove a device from an existing DVR | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `PUT` | `/livetv/dvrs/{dvrId}/devices/{deviceId}` | admin | Add a device to an existing DVR | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `DELETE` | `/livetv/dvrs/{dvrId}/lineups` | admin | Delete a DVR Lineup | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `PUT` | `/livetv/dvrs/{dvrId}/lineups` | admin | Add a DVR Lineup | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `PUT` | `/livetv/dvrs/{dvrId}/prefs` | admin | Set DVR preferences | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `DELETE` | `/livetv/dvrs/{dvrId}/reloadGuide` | admin | Tell a DVR to stop reloading program guide | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `POST` | `/livetv/dvrs/{dvrId}/reloadGuide` | admin | Tell a DVR to reload program guide | `404`: none exists here. Captured for an unknown id on the reference. |

## Metadata Agents

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `deferred` | `GET` | `/media/providers/metadata` | admin | Get the list of available metadata agent providers | scanner/analysis, separate binary |
| `deferred` | `POST` | `/media/providers/metadata` | admin | Add a metadata agent provider | scanner/analysis, separate binary |
| `deferred` | `GET` | `/media/providers/metadata/group` | admin | Get the list of available metadata agent provider groups | scanner/analysis, separate binary |
| `deferred` | `POST` | `/media/providers/metadata/group` | admin | Add a metadata agent provider group | scanner/analysis, separate binary |
| `deferred` | `DELETE` | `/media/providers/metadata/group/{groupId}` | admin | Delete a metadata agent provider group | scanner/analysis, separate binary |
| `deferred` | `GET` | `/media/providers/metadata/group/{groupId}` | admin | Get a metadata agent provider group | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/media/providers/metadata/group/{groupId}` | admin | Modify a metadata agent provider group | scanner/analysis, separate binary |
| `deferred` | `DELETE` | `/media/providers/metadata/group/{groupId}/items/{providerId}` | admin | Delete a metadata agent provider group item | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/media/providers/metadata/group/{groupId}/items/{providerId}` | admin | Modify a metadata agent provider group's items | scanner/analysis, separate binary |
| `deferred` | `DELETE` | `/media/providers/metadata/{providerId}` | admin | Delete a metadata agent provider | scanner/analysis, separate binary |
| `deferred` | `GET` | `/media/providers/metadata/{providerId}` | admin | Get a metadata agent provider | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/media/providers/metadata/{providerId}` | admin | Modify a metadata agent provider | scanner/analysis, separate binary |

## Subscriptions

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `DELETE` | `/media/grabbers/operations/{operationId}` | user | Cancel an existing grab | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `GET` | `/media/subscriptions` | user | Get all subscriptions | Empty container. The reference refuses it `401` without a subscription; see spec-gaps.md. |
| `done` | `POST` | `/media/subscriptions` | user | Create a subscription | `400`: there is no DVR to record with. Not captured. |
| `done` | `POST` | `/media/subscriptions/process` | user | Process all subscriptions | Empty `200`, `text/html`: nothing to process. Not captured. |
| `done` | `GET` | `/media/subscriptions/scheduled` | user | Get all scheduled recordings | Empty container. The reference refuses it `401` without a subscription. |
| `done` | `GET` | `/media/subscriptions/template` | user | Get the subscription template | `400` without `guid`, as captured; otherwise empty: nothing can be recorded. The reference refuses it `401`. |
| `done` | `DELETE` | `/media/subscriptions/{subscriptionId}` | user | Delete a subscription | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `GET` | `/media/subscriptions/{subscriptionId}` | user | Get a single subscription | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `PUT` | `/media/subscriptions/{subscriptionId}` | user | Edit a subscription | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `PUT` | `/media/subscriptions/{subscriptionId}/move` | admin | Re-order a subscription | `404`: none exists here. Captured for an unknown id on the reference. |

## Download Queue

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `POST` | `/downloadQueue` | user | Get or create a download queue |  |
| `stub` | `GET` | `/downloadQueue/{queueId}` | user | Get a download queue |  |
| `stub` | `POST` | `/downloadQueue/{queueId}/add` | user | Add to download queue | Spec marks `X-Plex-Client-Identifier` required; the reference does not enforce it — see spec-gaps.md. |
| `stub` | `GET` | `/downloadQueue/{queueId}/item/{itemId}/decision` | user | Grab download queue item decision |  |
| `stub` | `GET` | `/downloadQueue/{queueId}/item/{itemId}/media` | user | Grab download queue media |  |
| `stub` | `GET` | `/downloadQueue/{queueId}/items` | user | Get download queue items |  |
| `stub` | `DELETE` | `/downloadQueue/{queueId}/items/{itemId}` | user | Delete download queue items |  |
| `stub` | `GET` | `/downloadQueue/{queueId}/items/{itemId}` | user | Get download queue items |  |
| `stub` | `POST` | `/downloadQueue/{queueId}/items/{itemId}/restart` | user | Restart processing of items from the decision |  |

## Play Queue

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `POST` | `/playQueues` | user | Create a play queue | From an item, a list of items, a series, a season, a library listing or a playlist, with `key`, `shuffle`, `continuous` (an episode plays on to the end of its series) and `repeat`. Replaces the player's previous queue of the same `type`. Same container and items as the reference. `onDeck` is not honored (the queue starts where `key` says); a queue holds at most 500 entries. |
| `done` | `GET` | `/playQueues/{playQueueId}` | user | Retrieve a play queue | `window`, `center`, `includeBefore`, `includeAfter` and `repeat`, with the reference's rules; `own` is tolerated. |
| `done` | `PUT` | `/playQueues/{playQueueId}` | user | Add a generator or playlist to a play queue | `uri` or `playlistID`, after the last entry queued by hand or, with `next=1`, right after the one playing; `playQueueLastAddedItemID` as measured. What names nothing adds nothing, with a `200`. |
| `done` | `DELETE` | `/playQueues/{playQueueId}/items` | user | Clear a play queue | Keeps the entry playing, as the reference does. |
| `done` | `DELETE` | `/playQueues/{playQueueId}/items/{playQueueItemId}` | user | Delete an item from a play queue | Answers with the container as it was before, as the reference does; the next entry plays when the one playing goes. |
| `done` | `PUT` | `/playQueues/{playQueueId}/items/{playQueueItemId}/move` | user | Move an item in a play queue | `after` absent or `-1` is first; an `after` naming no entry puts it second, as measured. Moving onto itself is `404`. |
| `done` | `PUT` | `/playQueues/{playQueueId}/reset` | user | Reset a play queue | Selects the first entry without raising the version, and answers with the container as it was, with no entries, as the reference does. |
| `done` | `PUT` | `/playQueues/{playQueueId}/shuffle` | user | Shuffle a play queue | Rebuilt from its source, the entry playing first with its id, the others new. `404` while an entry queued by hand remains, or for an empty queue. |
| `done` | `PUT` | `/playQueues/{playQueueId}/unshuffle` | user | Unshuffle a play queue | Rebuilt from its source in natural order, the entry playing where its item falls. Refused as shuffling is. |

## Status

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/status/sessions` | admin | List Sessions | Direct Stream sessions, from the official API, terminated ones left out. `Session.id` is the player's `X-Plex-Session-Identifier` and `sessionKey` the server's number, as captured on the reference. The state comes from `/:/timeline`, Direct Play is not tracked yet, and the other attributes follow what Plex Web reads. |
| `done` | `GET` | `/status/sessions/background` | admin | Get background tasks | Always empty: no optimizer or download conversions run here. The reference's answer when none run, byte for byte. |
| `done` | `GET` | `/status/sessions/history/all` | user | List Playback History | The caller's own viewings, through `viewingHistory`; filters, sorts and refusals as measured. Always windowed; no `deviceID`, no artwork. See spec-gaps. |
| `done` | `DELETE` | `/status/sessions/history/{historyId}` | admin | Delete Single History Item | Through `deleteHistoryEntry`; the caller's own entry, watch state untouched. |
| `done` | `GET` | `/status/sessions/history/{historyId}` | admin | Get Single History Item | The caller's own entry, without the item's keys, as measured. |
| `partial` | `POST` | `/status/sessions/terminate` | admin | Terminate a session | Also `GET`, what Plex Web sends. `terminatePlaybackSession` on the official API for every session under that `Session.id`: the conversion stops at once and the next `/:/timeline` carries `terminationCode` and `terminationText`. `400` without `sessionId`, `403` empty, `404` unknown. The reference refuses it `401` without a subscription, so the success path is unmeasured. See spec-gaps.md. |

## Butler

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `DELETE` | `/butler` | admin | Stop all Butler tasks |  |
| `stub` | `GET` | `/butler` | admin | Get all Butler tasks |  |
| `stub` | `POST` | `/butler` | admin | Start all Butler tasks |  |
| `stub` | `DELETE` | `/butler/{task}` | admin | Stop a single Butler task |  |
| `stub` | `POST` | `/butler/{task}` | admin | Start a single Butler task |  |

## Transcoder

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/photo/:/transcode` | user | Transcode an image | Images hosted on `plex.tv` (account avatars): same size, encoding and headers as the reference. This server's own images too — an item's at the `thumb`/`art` address it publishes, a stored file at its `/artwork/…` address — scaled once per size, each side snapped up to a fixed ladder, and kept on disk in the format they are stored in. Image effects and other local paths answer `501`; any other host `403`. |
| `partial` | `GET` | `/{transcodeType}/:/transcode/universal/decision` | user | Make a decision on media playback | Video only. Direct Play, Direct Stream and H.264 re-encoded under `maxVideoBitrate`, over HLS or DASH, with the decisions the reference gives; `501` for video of another codec and for `protocol=http`. |
| `done` | `POST` | `/{transcodeType}/:/transcode/universal/fallback` | user | Manually trigger a transcoder fallback | The official API's `fallBackStream`. Reads `session`, not the spec's `transcodeSessionId`, for any of the four kinds: none `400`, an unknown one `404`, a running conversion `412` under the reference's `500` page, since every conversion here decodes in software and encodes H.264, as the reference answers its own; other methods `404`. See spec-gaps.md. |
| `partial` | `GET` | `/{transcodeType}/:/transcode/universal/start.*` | user | Start A Transcoding Session | Video, Direct Stream, over HLS or DASH, started through the official API's `startStream` and answered with a `307` to the stream's own address (`/streams/{id}/manifest.mpd` or `master.m3u8`, with its secret), whose segments carry the secret too. HLS is the reference's fake VOD playlist, predicted from the duration; DASH its static manifest. A far seek restarts the process at the target segment (`-ss`, `-segment_start_number`, as the reference logs it), and a new session under the same `X-Plex-Session-Identifier` replaces the last. The reference's command lines; video copied, or re-encoded to H.264 under `maxVideoBitrate` along the reference's measured ladder; audio copied or converted to AAC. `autoAdjustQuality=1` is the official API's server-adaptive quality: one representation, whose rung the server lowers and raises by timing its segment deliveries. Same `501`s as the decision. |
| `partial` | `GET` | `/{transcodeType}/:/transcode/universal/subtitles` | user | Transcode subtitles | Video, DASH, a text subtitle: the stream's ASS script, extracted whole by a process of its own and sent once it ends. The reference serves it without `X-Plex-Client-Identifier` despite the spec, but refuses a request with no resolvable client profile (`X-Plex-Platform`) — see spec-gaps.md. |

## General

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/` | user | Get PMS info | Identical to the reference but for deliberate differences and environment: `myPlexSubscription=1` whatever the account; `myPlex` and the attributes around it describe the caller's own server, paired or not; `ownerFeatures` lists the 209 features of a Plex Pass server, names and GUIDs. Needs a token, as on the reference: its `401` is what sends Plex Web to sign in. |
| `done` | `GET` | `/identity` | token | Get PMS identity | XML+JSON byte-identical to reference; public despite spec security |
| `stub` | `GET` | `/security/resources` | user | Get Source Connection Information |  |
| `stub` | `POST` | `/security/token` | user | Get Transient Tokens |  |

## Live TV

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/livetv/sessions` | user | Get all sessions | Empty container, as the reference answers without a DVR: captured, byte-identical in both formats, window included. |
| `done` | `GET` | `/livetv/sessions/{sessionId}` | user | Get a single session | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `GET` | `/livetv/sessions/{sessionId}/{consumerId}/index.m3u8` | user | Get a session playlist index | `404`: none exists here. Captured for an unknown id on the reference. |
| `done` | `GET` | `/livetv/sessions/{sessionId}/{consumerId}/{segmentId}` | user | Get a single session segment | `404`: none exists here. Captured for an unknown id on the reference. |

## Provider

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/media/providers` | user | Get the list of available media providers | The 17 features and the per-library content directories (with pivots) identical to the reference; the capability attributes are those of `GET /`, so they differ where that one does on purpose. Plex Web reads it first, to build its sidebar. |
| `todo` | `POST` | `/media/providers` | user | Add a media provider |  |
| `deferred` | `POST` | `/media/providers/refresh` | user | Refresh media providers | scanner/analysis, separate binary |
| `stub` | `DELETE` | `/media/providers/{provider}` | user | Delete a media provider |  |

## Library Collections

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `PUT` | `/library/collections/{collectionId}/items` | admin | Add items to a collection | `uri` of items or of a listing; those of the collection's kind and library join, a repeat is skipped, none that can join is `400`, a smart collection `400`, an unknown one `404`. Answers the collection with its library. Through `addToCollection`. |
| `done` | `PUT` | `/library/collections/{collectionId}/items/{itemId}` | admin | Delete an item from a collection | `404`, as on the reference: it removes with `DELETE` on the same path, which Plex Web sends and which is served (`removeFromCollection`); an item not in it changes nothing. See spec-gaps.md. |
| `done` | `PUT` | `/library/collections/{collectionId}/items/{itemId}/move` | admin | Reorder an item in the collection | In the custom order, which `collectionSort=2` lists. After itself, an item not in it or a non-numeric `after` `400`; an `after` not in it moves first; a smart collection `400`. Through `moveCollectionItem`. |

## Log

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `POST` | `/log` | admin | Logging a multi-line message to the Plex Media Server log |  |
| `stub` | `PUT` | `/log` | admin | Logging a single-line message to the Plex Media Server log |  |
| `stub` | `POST` | `/log/networked` | admin | Enabling Papertrail |  |

## Playlist

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/playlists` | user | Retrieve Playlists | The caller's own playlists, smart ones first then by title, as captured; `playlistType`, `smart`, `sectionID` (a library's kind, its own smart ones) and `type=15`; `sort` by title, `addedAt`, `updatedAt`, `lastViewedAt`, `501` for `duration` and the counts. Windowed; `viewCount`/`lastViewedAt` only here, as captured. Gap: a smart playlist's `duration`. Also served at `/playlists/all`. See spec-gaps.md. |
| `done` | `GET` | `/playlists/{playlistId}` | user | Retrieve Playlist | The playlist in the captured order, a smart one's `content` first; `404` for an unknown id, a non-number, or another viewer's. A film's id is not a playlist here, where the reference shows the film. `composite` points at an image not served yet. |
| `done` | `GET` | `/playlists/{playlistId}/items` | user | Retrieve Playlist Contents | Listing form with the item's library and `playlistItemID` before `duration`, as captured; container `duration` in seconds and `composite` even empty. A smart playlist lists its rule anew, without entries. Items of libraries the caller may not read are hidden. `type` keeps one kind. |

## Preferences

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/:/prefs` | user | Get all preferences | `403`: Plex Web then offers none of the server settings screens, which drive machinery this server does not have (`docs/spec-gaps.md`). |
| `done` | `PUT` | `/:/prefs` | user | Set preferences | `403`, as `GET`. |
| `stub` | `GET` | `/:/prefs/get` | user | Get a preferences |  |

## Timeline

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `PUT` | `/:/scrobble` | user | Mark an item as played | Also `GET`, as the reference. A film or an episode once more, counted on its season and series; a season or a series by each of its episodes. Refused as the reference refuses: `400` without `identifier` or `key`, `404` for a foreign provider, a `uri` or an unknown item. See spec-gaps.md. |
| `partial` | `POST` | `/:/timeline` | user | Report media timeline | Also `GET`, what Plex Web sends, and `PUT`. Updates the state and position of a Direct Stream session; on `stopped` it ends the playback's sessions and their conversions through `reportPlayback`, as the reference does; after a termination it answers `terminationCode`/`terminationText`. `TranscodeSession` carries the reference's full field set, from the live conversion (`progress`, `speed`, `maxOffsetAvailable`…). Records the watch state of Direct Play and Direct Stream alike, and answers as the reference: `ignore`, `progress` or `complete`, with `skipCount`, `viewCount` and `viewOffset`. Identical to the reference over an 18-step sequence. See spec-gaps.md. |
| `done` | `PUT` | `/:/unscrobble` | user | Mark an item as unplayed | Also `GET`. An episode alone, a season with its episodes, a series with everything below it; a series keeps its count when a season is forgotten. |

## Updater

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `PUT` | `/updater/apply` | admin | Applying updates |  |
| `done` | `PUT` | `/updater/check` | admin | Checking for updates | Acknowledged with an empty `200` typed `text/html`, as the reference does; there is no channel to look in. |
| `done` | `GET` | `/updater/status` | admin | Querying status of updates | An install that cannot update itself: `canInstall` false, no release. klapz is not updated through plex.tv. |

## Activities

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/activities` | user | Get all activities | Always the empty container, as the reference sends it when idle: this server runs no scan, refresh or analysis. Identical in both formats. |
| `stub` | `DELETE` | `/activities/{activityId}` | admin | Cancel a running activity |  |

## Events

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/:/eventsource/notifications` | user | Connect to Eventsource |  |
| `stub` | `GET` | `/:/websocket/notifications` | user | Connect to WebSocket |  |

## Search

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/hubs/search` | user | Search Hub | The reference's seventeen shelves; films, series and episodes ranked by the official API's `search` score (rung and audience rating, the reference's own values), actors, directors and genres through `searchTags`, collections through `collections(matching:)`, each cut to `limit` (3 by default). A film or series shelf no title fills lists the works of the actor and director the term names (`searchCredits`), `reason="actor"`. Word-prefix matching rather than the reference's fuzzy one. `sectionId` must name a library and narrows nothing, as on the reference. See `docs/spec-gaps.md`. |
| `done` | `GET` | `/hubs/search/voice` | user | Voice Search Hub | Titles ranked by edit distance to the term, sort titles and term read alike (an article dropped, the reference's letters folded, bytes counted), `limit` across shelves, one shelf per kind; official API `closestTitles`. The reference pre-selects on its raw titles, case kept, and can skip a closer title: ours never ranks farther (`docs/spec-gaps.md`). |

## UltraBlur

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/services/ultrablur/colors` | user | Get UltraBlur Colors | The reference's algorithm, from its binary: identical on 269 of 269 PNGs and on every JPEG once decoded as libjpeg-turbo does; our JPEG decoder rounds differently, 34 of 150 photos identical (390 of 604 corners). Remote images from Plex's hosts over TLS only; an item's `/art` answers its stored colors; any other local address `404`. See spec-gaps.md. |
| `done` | `GET` | `/services/ultrablur/image` | user | Get UltraBlur Image | Pixel-identical to the reference without noise at 97×61, 320×240 and 1920×1080; the noise is ours, seeded. Area capped at 3840×2160, `url` honored as `/colors` reads it. Hex colors that do not parse are `400`. See spec-gaps.md. |

## Collections

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `POST` | `/library/collections` | user | Create a collection | By `uri` (items, or a listing for `smart=1`), `type`, `title`, `sectionId`, as measured: items of another kind or library left out, no `type` the library's own, an unknown `type` `400` where the reference makes an `unknown` one. A smart rule is the official API's listing input; a filter it cannot honor `501`. Library admins and the server's owners. Through `createCollection`. |

## Rate

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `PUT` | `/:/rate` | user | Rate an item | Also `GET`, `POST` and `DELETE`, as the reference. The caller's rating of the work, through `rateItem`: any kind, every copy at once, no parent adding up its children; `rating=-1` clears, `ratedAt` dates the change and an older one is ignored. `400` for a missing or malformed parameter or a future `ratedAt`, then `404` for a foreign provider or an unknown item. A rating off the scale (above 10, a negative other than `-1`) is a `400` where the reference keeps it. Published as `userRating` (one decimal) and `lastRatedAt`; `userRating` filters and sorts `/all`. See spec-gaps.md. |
