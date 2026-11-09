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
| `todo` | 15 | 5.8 % |
| `stub` | 185 | 71.2 % |
| `partial` | 16 | 6.2 % |
| `done` | 7 | 2.7 % |
| `deferred` | 37 | 14.2 % |
| **Total** | **260** | |

## By domain

| Domain | Operations | `done` | `partial` | `stub` | `todo` | `deferred` |
| --- | --: | --: | --: | --: | --: | --: |
| [Library](#library) | 85 | 1 | 5 | 47 | 8 | 24 |
| [Hubs](#hubs) | 15 | 0 | 1 | 14 | 0 | 0 |
| [Content](#content) | 13 | 0 | 2 | 11 | 0 | 0 |
| [Devices](#devices) | 13 | 0 | 0 | 13 | 0 | 0 |
| [EPG](#epg) | 13 | 0 | 0 | 9 | 4 | 0 |
| [Library Playlists](#library-playlists) | 13 | 0 | 0 | 13 | 0 | 0 |
| [DVRs](#dvrs) | 12 | 0 | 0 | 12 | 0 | 0 |
| [Metadata Agents](#metadata-agents) | 12 | 0 | 0 | 0 | 0 | 12 |
| [Subscriptions](#subscriptions) | 10 | 0 | 0 | 10 | 0 | 0 |
| [Download Queue](#download-queue) | 9 | 0 | 0 | 9 | 0 | 0 |
| [Play Queue](#play-queue) | 9 | 0 | 2 | 6 | 1 | 0 |
| [Status](#status) | 6 | 0 | 1 | 5 | 0 | 0 |
| [Butler](#butler) | 5 | 0 | 0 | 5 | 0 | 0 |
| [Transcoder](#transcoder) | 5 | 0 | 3 | 2 | 0 | 0 |
| [General](#general) | 4 | 2 | 0 | 2 | 0 | 0 |
| [Live TV](#live-tv) | 4 | 0 | 0 | 4 | 0 | 0 |
| [Provider](#provider) | 4 | 1 | 0 | 1 | 1 | 1 |
| [Library Collections](#library-collections) | 3 | 0 | 0 | 3 | 0 | 0 |
| [Log](#log) | 3 | 0 | 0 | 3 | 0 | 0 |
| [Playlist](#playlist) | 3 | 0 | 0 | 3 | 0 | 0 |
| [Preferences](#preferences) | 3 | 0 | 1 | 1 | 1 | 0 |
| [Timeline](#timeline) | 3 | 0 | 1 | 2 | 0 | 0 |
| [Updater](#updater) | 3 | 2 | 0 | 1 | 0 | 0 |
| [Activities](#activities) | 2 | 1 | 0 | 1 | 0 | 0 |
| [Events](#events) | 2 | 0 | 0 | 2 | 0 | 0 |
| [Search](#search) | 2 | 0 | 0 | 2 | 0 | 0 |
| [UltraBlur](#ultrablur) | 2 | 0 | 0 | 2 | 0 | 0 |
| [Collections](#collections) | 1 | 0 | 0 | 1 | 0 | 0 |
| [Rate](#rate) | 1 | 0 | 0 | 1 | 0 | 0 |

## Outside the spec

Served, but absent from `docs/openapi.json` and therefore from the counts above.

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/library/search` | user | Search field (universal search) | Films, series and episodes with their listing fields, in three score tiers. Not served: the people and collections the reference mixes in, and its fuzzy matching. See `docs/spec-gaps.md`. |
| `done` | `GET` | `/library/sections` | user | Library sections | Byte-identical to the reference. The spec declares only its `/all` alias. |
| `partial` | `GET` | `/library/metadata/{id}/children` | user | Seasons of a series, episodes of a season | Zero differing fields against the reference on seven parents across two libraries; `400` on a film or an episode, as the reference. Gaps: the leading `All episodes` `Directory` is omitted while `/allLeaves` answers 501, so `size` is one short. |
| `partial` | `GET` | `/library/metadata/{id}/grandchildren` | user | Every episode of a series, season by season | Same field fidelity as `/children`. Gaps: those of `/library/metadata/{ids}`. |
| `partial` | `GET` | `/search` | user | Global search across libraries | Matches exactly as the reference (word prefix over title, sort title, original title) but returns matches in title order, unranked and uncut. The spec declares only `/hubs/search`. |

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
| `todo` | `DELETE` | `/library/metadata/{ids}` | admin | Delete a metadata item |  |
| `todo` | `PUT` | `/library/metadata/{ids}` | admin | Edit a metadata item |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/addetect` | admin | Ad-detect an item | scanner/analysis, separate binary |
| `stub` | `GET` | `/library/metadata/{ids}/allLeaves` | user | Get the leaves of an item |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/analyze` | admin | Analyze an item | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/library/metadata/{ids}/analyzeLoudness` | admin | Detect loudness of audio streams | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/library/metadata/{ids}/chapterThumbs` | admin | Generate thumbs of chapters for an item | scanner/analysis, separate binary |
| `stub` | `PUT` | `/library/metadata/{ids}/credits` | admin | Credit detect a metadata item |  |
| `stub` | `GET` | `/library/metadata/{ids}/extras` | user | Get an item's extras |  |
| `stub` | `POST` | `/library/metadata/{ids}/extras` | user | Add to an item's extras |  |
| `stub` | `GET` | `/library/metadata/{ids}/file` | user | Get a file from a metadata or media bundle |  |
| `stub` | `PUT` | `/library/metadata/{ids}/index` | admin | Start BIF generation of an item |  |
| `stub` | `PUT` | `/library/metadata/{ids}/intro` | admin | Intro detect an item |  |
| `stub` | `POST` | `/library/metadata/{ids}/marker` | user | Create a marker |  |
| `stub` | `DELETE` | `/library/metadata/{ids}/marker/{marker}` | user | Delete a marker |  |
| `stub` | `PUT` | `/library/metadata/{ids}/marker/{marker}` | user | Edit a marker |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/match` | admin | Match a metadata item | scanner/analysis, separate binary |
| `deferred` | `PUT` | `/library/metadata/{ids}/matches` | admin | Get metadata matches for an item | scanner/analysis, separate binary |
| `stub` | `DELETE` | `/library/metadata/{ids}/media/{mediaItem}` | admin | Delete a media item |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/merge` | admin | Merge a metadata item | scanner/analysis, separate binary |
| `stub` | `GET` | `/library/metadata/{ids}/nearest` | user | Get nearest tracks to metadata item |  |
| `stub` | `PUT` | `/library/metadata/{ids}/prefs` | admin | Set metadata preferences |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/refresh` | admin | Refresh a metadata item | scanner/analysis, separate binary |
| `partial` | `GET` | `/library/metadata/{ids}/related` | user | Get related items | A film: similar films (approximate ranking), its director and two first-billed actors, the last two matching the reference on 59 of 60 films. A series: similar series only. Not served: collection and `external.*` shelves, a series' network and cast shelves. |
| `partial` | `GET` | `/library/metadata/{ids}/similar` | user | Get similar items | Same container as the reference; the ranking is an approximation. |
| `deferred` | `PUT` | `/library/metadata/{ids}/split` | admin | Split a metadata item | scanner/analysis, separate binary |
| `stub` | `GET` | `/library/metadata/{ids}/subtitles` | admin | Add subtitles |  |
| `stub` | `GET` | `/library/metadata/{ids}/tree` | user | Get metadata items as a tree |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/unmatch` | admin | Unmatch a metadata item | scanner/analysis, separate binary |
| `stub` | `GET` | `/library/metadata/{ids}/users/top` | admin | Get metadata top users |  |
| `deferred` | `PUT` | `/library/metadata/{ids}/voiceActivity` | admin | Detect voice activity | scanner/analysis, separate binary |
| `stub` | `DELETE` | `/library/metadata/{ids}/{element}` | user | Delete an item's artwork, theme, etc |  |
| `stub` | `POST` | `/library/metadata/{ids}/{element}` | user | Set an item's artwork, theme, etc |  |
| `stub` | `PUT` | `/library/metadata/{ids}/{element}` | user | Set an item's artwork, theme, etc |  |
| `stub` | `GET` | `/library/metadata/{ids}/{element}/{timestamp}` | user | Get an item's artwork, theme, etc |  |
| `deferred` | `PUT` | `/library/optimize` | admin | Optimize the Database | scanner/analysis, separate binary |
| `stub` | `PUT` | `/library/parts/{partId}` | user | Set stream selection |  |
| `deferred` | `GET` | `/library/parts/{partId}/indexes/{index}` | user | Get BIF index for a part | scanner/analysis, separate binary |
| `deferred` | `GET` | `/library/parts/{partId}/indexes/{index}/{offset}` | user | Get an image from part BIF | scanner/analysis, separate binary |
| `partial` | `GET` | `/library/parts/{partId}/{changestamp}/{filename}` | user | Get a media part | Direct Play: the file, by part id, with `Range`. The change stamp and the name are ignored. |
| `stub` | `GET` | `/library/people/{personId}` | user | Get person details |  |
| `stub` | `GET` | `/library/people/{personId}/media` | user | Get media for a person |  |
| `stub` | `GET` | `/library/randomArtwork` | user | Get random artwork |  |
| `done` | `GET` | `/library/sections/all` | user | Get library sections (main Media Provider Only) | Alias of `/library/sections` (not in spec). Byte-identical to reference apart from `contentChangedAt`, whose value differs only because the imported snapshot predates the live server |
| `todo` | `POST` | `/library/sections/all` | admin | Add a library section |  |
| `deferred` | `DELETE` | `/library/sections/all/refresh` | admin | Stop refresh | scanner/analysis, separate binary |
| `stub` | `GET` | `/library/sections/prefs` | admin | Get section prefs |  |
| `deferred` | `POST` | `/library/sections/refresh` | admin | Refresh all sections | scanner/analysis, separate binary |
| `stub` | `DELETE` | `/library/sections/{sectionId}` | admin | Delete a library section |  |
| `stub` | `GET` | `/library/sections/{sectionId}` | user | Get a library section by id |  |
| `stub` | `PUT` | `/library/sections/{sectionId}` | admin | Edit a library section |  |
| `todo` | `PUT` | `/library/sections/{sectionId}/all` | user | Set the fields of the filtered items |  |
| `deferred` | `PUT` | `/library/sections/{sectionId}/analyze` | admin | Analyze a section | scanner/analysis, separate binary |
| `stub` | `GET` | `/library/sections/{sectionId}/audioCodec` | user | Get audio codecs in a section |  |
| `stub` | `GET` | `/library/sections/{sectionId}/audioLayout` | user | Get audio layouts in a section |  |
| `stub` | `GET` | `/library/sections/{sectionId}/autocomplete` | user | Get autocompletions for search |  |
| `stub` | `DELETE` | `/library/sections/{sectionId}/collection/{collectionId}` | admin | Delete a collection |  |
| `stub` | `GET` | `/library/sections/{sectionId}/collections` | user | Get collections in a section |  |
| `stub` | `GET` | `/library/sections/{sectionId}/common` | user | Get common fields for items |  |
| `stub` | `GET` | `/library/sections/{sectionId}/composite/{updatedAt}` | admin | Get a section composite image |  |
| `stub` | `PUT` | `/library/sections/{sectionId}/emptyTrash` | admin | Empty section trash |  |
| `stub` | `GET` | `/library/sections/{sectionId}/filters` | admin | Get section filters |  |
| `partial` | `GET` | `/library/sections/{sectionId}/firstCharacters` | user | Get list of first characters | Bucket sets and counts identical to the reference on four libraries. Folded first character, `#` for everything unlettered, percent-encoded keys. Singular form, absent from the spec — see spec-gaps.md. |
| `deferred` | `DELETE` | `/library/sections/{sectionId}/indexes` | admin | Delete section indexes | scanner/analysis, separate binary |
| `deferred` | `DELETE` | `/library/sections/{sectionId}/intros` | admin | Delete section intro markers | scanner/analysis, separate binary |
| `partial` | `GET` | `/library/sections/{sectionId}/prefs` | user | Get section prefs | The two settings Plex Web looks up by name, at their defaults; identical to the reference on those. The other 15 or 16 are about scanning and matching. |
| `todo` | `PUT` | `/library/sections/{sectionId}/prefs` | admin | Set section prefs |  |
| `deferred` | `DELETE` | `/library/sections/{sectionId}/refresh` | admin | Cancel section refresh | scanner/analysis, separate binary |
| `deferred` | `POST` | `/library/sections/{sectionId}/refresh` | admin | Refresh section | scanner/analysis, separate binary |
| `stub` | `GET` | `/library/sections/{sectionId}/sorts` | user | Get a section sorts |  |
| `stub` | `GET` | `/library/sections/{sectionId}/subtitleCodec` | user | Get subtitle codecs in a section |  |
| `stub` | `GET` | `/library/sections/{sectionId}/videoCodec` | user | Get video codecs in a section |  |
| `todo` | `DELETE` | `/library/streams/{streamId}.{ext}` | user | Delete a stream | needs a hand-written handler: two parameters in segment `{streamId}.{ext}` |
| `todo` | `GET` | `/library/streams/{streamId}.{ext}` | user | Get a stream | needs a hand-written handler: two parameters in segment `{streamId}.{ext}` |
| `todo` | `PUT` | `/library/streams/{streamId}.{ext}` | user | Set a stream offset | needs a hand-written handler: two parameters in segment `{streamId}.{ext}` |
| `stub` | `GET` | `/library/streams/{streamId}/levels` | user | Get loudness about a stream in json |  |
| `stub` | `GET` | `/library/streams/{streamId}/loudness` | user | Get loudness about a stream |  |
| `stub` | `GET` | `/library/tags` | user | Get all library tags of a type |  |

## Hubs

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/hubs` | user | Get global hubs |  |
| `stub` | `GET` | `/hubs/continueWatching` | user | Get the continue watching hub |  |
| `stub` | `GET` | `/hubs/items` | user | Get a hub's items |  |
| `stub` | `GET` | `/hubs/metadata/{metadataId}` | user | Get hubs for a section by metadata item |  |
| `stub` | `GET` | `/hubs/metadata/{metadataId}/postplay` | user | Get postplay hubs |  |
| `stub` | `GET` | `/hubs/metadata/{metadataId}/related` | user | Get related hubs |  |
| `stub` | `GET` | `/hubs/popularInLibraries` | user | Get the popular in your libraries hub |  |
| `stub` | `GET` | `/hubs/promoted` | user | Get the hubs which are promoted |  |
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
| `stub` | `GET` | `/library/collections/{collectionId}/composite/{updatedAt}` | user | Get a collection's image |  |
| `stub` | `GET` | `/library/collections/{collectionId}/items` | user | Get items in a collection |  |
| `partial` | `GET` | `/library/metadata/{ids}` | user | Get a metadata item | Items, media and files, served through the GraphQL schema. Zero field differences against the reference on shared items. Seasons are named at read time (`Season N`, `Specials`) and carry their series' guid, studio and year; episodes carry their season's and series' guids and titles. Genres, countries and credits with ids, filters, provider keys and portraits; `Guid` and `Rating`. 355 of 420 sampled items identical: 44 live metadata newer than the snapshot, 21 a credit's portrait (one per person here, one per credit on the reference). Gaps: artwork, view state, `Part.key`, audio/videoProfile (and the `audioCodec` composition that needs them), slug, chapterSource, extras, `Stream.language` as a name rather than a tag. Sort collation differs — see spec-gaps.md. Carries `Stream`, which a listing omits. |
| `stub` | `GET` | `/library/sections/{sectionId}/albums` | user | Set section albums |  |
| `partial` | `GET` | `/library/sections/{sectionId}/all` | user | Get items in the section | Items, media and files through the GraphQL schema; zero field differences against the reference on shared items. `includeMeta=1` publishes the filters and sorts it honors. Filters: `title` (word prefix), `year`, `decade`, `contentRating`, `studio`, `resolution` — exact totals on 11 of 12 probes; `genre`, `country`, `director`, `writer`, `actor`, `producer` by id or name, comma for OR, repeat for AND — exact totals on 16 probes. Any other advertised filter (`collection`, `label`, …) answers 501 rather than being dropped. Tags in the listing form: first 2 or 3 names per kind, derived from the tags rather than from the reference's stale cache. Gaps: artwork, view state, audio/videoProfile. Sort collation and range operators — see spec-gaps.md. |
| `stub` | `GET` | `/library/sections/{sectionId}/allLeaves` | user | Set section leaves |  |
| `stub` | `GET` | `/library/sections/{sectionId}/arts` | user | Set section artwork |  |
| `stub` | `GET` | `/library/sections/{sectionId}/categories` | user | Set section categories |  |
| `stub` | `GET` | `/library/sections/{sectionId}/cluster` | user | Set section clusters |  |
| `stub` | `GET` | `/library/sections/{sectionId}/computePath` | user | Similar tracks to transition from one to another |  |
| `stub` | `GET` | `/library/sections/{sectionId}/location` | user | Get all folder locations |  |
| `stub` | `GET` | `/library/sections/{sectionId}/moment` | user | Set section moments |  |
| `stub` | `GET` | `/library/sections/{sectionId}/nearest` | user | The nearest audio tracks |  |

## Devices

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/media/grabbers` | user | Get available grabbers |  |
| `stub` | `GET` | `/media/grabbers/devices` | admin | Get all devices |  |
| `stub` | `POST` | `/media/grabbers/devices` | admin | Add a device |  |
| `stub` | `POST` | `/media/grabbers/devices/discover` | admin | Tell grabbers to discover devices |  |
| `stub` | `DELETE` | `/media/grabbers/devices/{deviceId}` | admin | Remove a device |  |
| `stub` | `GET` | `/media/grabbers/devices/{deviceId}` | admin | Get device details |  |
| `stub` | `PUT` | `/media/grabbers/devices/{deviceId}` | admin | Enable or disable a device |  |
| `stub` | `PUT` | `/media/grabbers/devices/{deviceId}/channelmap` | user | Set a device's channel mapping |  |
| `stub` | `GET` | `/media/grabbers/devices/{deviceId}/channels` | admin | Get a device's channels |  |
| `stub` | `PUT` | `/media/grabbers/devices/{deviceId}/prefs` | admin | Set device preferences |  |
| `stub` | `DELETE` | `/media/grabbers/devices/{deviceId}/scan` | user | Tell a device to stop scanning for channels |  |
| `stub` | `POST` | `/media/grabbers/devices/{deviceId}/scan` | user | Tell a device to scan for channels |  |
| `stub` | `GET` | `/media/grabbers/devices/{deviceId}/thumb/{version}` | admin | Get device thumb |  |

## EPG

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/livetv/epg/channelmap` | user | Compute the best channel map |  |
| `stub` | `GET` | `/livetv/epg/channels` | user | Get channels for a lineup |  |
| `stub` | `GET` | `/livetv/epg/countries` | user | Get all countries |  |
| `stub` | `GET` | `/livetv/epg/countries/{country}/{epgId}/lineups` | user | Get lineups for a country via postal code |  |
| `stub` | `GET` | `/livetv/epg/countries/{country}/{epgId}/regions` | user | Get regions for a country |  |
| `stub` | `GET` | `/livetv/epg/countries/{country}/{epgId}/regions/{region}/lineups` | user | Get lineups for a region |  |
| `stub` | `GET` | `/livetv/epg/languages` | user | Get all languages |  |
| `stub` | `GET` | `/livetv/epg/lineup` | user | Compute the best lineup |  |
| `stub` | `GET` | `/livetv/epg/lineupchannels` | user | Get the channels for mulitple lineups |  |
| `todo` | `GET` | `/tv.plex.providers.epg.{identifier}:{deviceId}/grid` | user | Get airing information | needs a hand-written handler: two parameters in segment `tv.plex.providers.epg.{identifier}:{deviceId}` |
| `todo` | `GET` | `/tv.plex.providers.epg.{identifier}:{deviceId}/lineups/dvr/channels` | user | Get channels for a lineup | needs a hand-written handler: two parameters in segment `tv.plex.providers.epg.{identifier}:{deviceId}` |
| `todo` | `GET` | `/tv.plex.providers.epg.{identifier}:{deviceId}/watchnow` | user | Get hubs | needs a hand-written handler: two parameters in segment `tv.plex.providers.epg.{identifier}:{deviceId}` |
| `todo` | `GET` | `/tv.plex.providers.epg.{identifier}:{deviceId}/watchnow/all` | user | Get currently airing | needs a hand-written handler: two parameters in segment `tv.plex.providers.epg.{identifier}:{deviceId}` |

## Library Playlists

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `POST` | `/playlists` | user | Create a Playlist |  |
| `stub` | `POST` | `/playlists/upload` | admin | Upload |  |
| `stub` | `DELETE` | `/playlists/{playlistId}` | user | Delete a Playlist |  |
| `stub` | `PUT` | `/playlists/{playlistId}` | user | Editing a Playlist |  |
| `stub` | `GET` | `/playlists/{playlistId}/generators` | user | Get a playlist's generators |  |
| `stub` | `DELETE` | `/playlists/{playlistId}/items` | user | Clearing a playlist |  |
| `stub` | `PUT` | `/playlists/{playlistId}/items` | user | Adding to a Playlist |  |
| `stub` | `DELETE` | `/playlists/{playlistId}/items/{generatorId}` | user | Delete a Generator |  |
| `stub` | `GET` | `/playlists/{playlistId}/items/{generatorId}` | user | Get a playlist generator |  |
| `stub` | `PUT` | `/playlists/{playlistId}/items/{generatorId}` | user | Modify a Generator |  |
| `stub` | `GET` | `/playlists/{playlistId}/items/{generatorId}/items` | user | Get a playlist generator's items |  |
| `stub` | `PUT` | `/playlists/{playlistId}/items/{generatorId}/{metadataId}/{action}` | user | Reprocess a generator |  |
| `stub` | `PUT` | `/playlists/{playlistId}/items/{playlistItemId}/move` | user | Moving items in a playlist |  |

## DVRs

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/livetv/dvrs` | user | Get DVRs |  |
| `stub` | `POST` | `/livetv/dvrs` | admin | Create a DVR |  |
| `stub` | `DELETE` | `/livetv/dvrs/{dvrId}` | admin | Delete a single DVR |  |
| `stub` | `GET` | `/livetv/dvrs/{dvrId}` | user | Get a single DVR |  |
| `stub` | `POST` | `/livetv/dvrs/{dvrId}/channels/{channel}/tune` | user | Tune a channel on a DVR |  |
| `stub` | `DELETE` | `/livetv/dvrs/{dvrId}/devices/{deviceId}` | admin | Remove a device from an existing DVR |  |
| `stub` | `PUT` | `/livetv/dvrs/{dvrId}/devices/{deviceId}` | admin | Add a device to an existing DVR |  |
| `stub` | `DELETE` | `/livetv/dvrs/{dvrId}/lineups` | admin | Delete a DVR Lineup |  |
| `stub` | `PUT` | `/livetv/dvrs/{dvrId}/lineups` | admin | Add a DVR Lineup |  |
| `stub` | `PUT` | `/livetv/dvrs/{dvrId}/prefs` | admin | Set DVR preferences |  |
| `stub` | `DELETE` | `/livetv/dvrs/{dvrId}/reloadGuide` | admin | Tell a DVR to stop reloading program guide |  |
| `stub` | `POST` | `/livetv/dvrs/{dvrId}/reloadGuide` | admin | Tell a DVR to reload program guide |  |

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
| `stub` | `DELETE` | `/media/grabbers/operations/{operationId}` | user | Cancel an existing grab |  |
| `stub` | `GET` | `/media/subscriptions` | user | Get all subscriptions |  |
| `stub` | `POST` | `/media/subscriptions` | user | Create a subscription |  |
| `stub` | `POST` | `/media/subscriptions/process` | user | Process all subscriptions |  |
| `stub` | `GET` | `/media/subscriptions/scheduled` | user | Get all scheduled recordings |  |
| `stub` | `GET` | `/media/subscriptions/template` | user | Get the subscription template |  |
| `stub` | `DELETE` | `/media/subscriptions/{subscriptionId}` | user | Delete a subscription |  |
| `stub` | `GET` | `/media/subscriptions/{subscriptionId}` | user | Get a single subscription |  |
| `stub` | `PUT` | `/media/subscriptions/{subscriptionId}` | user | Edit a subscription |  |
| `stub` | `PUT` | `/media/subscriptions/{subscriptionId}/move` | admin | Re-order a subscription |  |

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
| `partial` | `POST` | `/playQueues` | user | Create a play queue | From an item, a list of items, a series, a season or a library listing, with `key` and `shuffle`. Same container and items as the reference. A queue holds at most 500 entries. |
| `partial` | `GET` | `/playQueues/{playQueueId}` | user | Retrieve a play queue | `window`, `center`, `includeBefore` and `includeAfter`, with the reference's rules. |
| `todo` | `PUT` | `/playQueues/{playQueueId}` | user | Add a generator or playlist to a play queue |  |
| `stub` | `DELETE` | `/playQueues/{playQueueId}/items` | user | Clear a play queue |  |
| `stub` | `DELETE` | `/playQueues/{playQueueId}/items/{playQueueItemId}` | user | Delete an item from a play queue |  |
| `stub` | `PUT` | `/playQueues/{playQueueId}/items/{playQueueItemId}/move` | user | Move an item in a play queue |  |
| `stub` | `PUT` | `/playQueues/{playQueueId}/reset` | user | Reset a play queue |  |
| `stub` | `PUT` | `/playQueues/{playQueueId}/shuffle` | user | Shuffle a play queue |  |
| `stub` | `PUT` | `/playQueues/{playQueueId}/unshuffle` | user | Unshuffle a play queue |  |

## Status

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/status/sessions` | admin | List Sessions | Direct Stream sessions, from the official API. The state comes from `/:/timeline`, Direct Play is not tracked yet, and the attributes follow what Plex Web reads, with no capture to compare against. |
| `stub` | `GET` | `/status/sessions/background` | admin | Get background tasks |  |
| `stub` | `GET` | `/status/sessions/history/all` | user | List Playback History |  |
| `stub` | `DELETE` | `/status/sessions/history/{historyId}` | admin | Delete Single History Item |  |
| `stub` | `GET` | `/status/sessions/history/{historyId}` | admin | Get Single History Item |  |
| `stub` | `POST` | `/status/sessions/terminate` | admin | Terminate a session |  |

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
| `partial` | `GET` | `/photo/:/transcode` | user | Transcode an image | Images hosted on `plex.tv` (account avatars): same size, encoding and headers as the reference. Library artwork and image effects answer `501`; any other host `403`. |
| `partial` | `GET` | `/{transcodeType}/:/transcode/universal/decision` | user | Make a decision on media playback | Video only. Direct Play, Direct Stream and H.264 re-encoded under `maxVideoBitrate`, over HLS or DASH, with the decisions the reference gives; `501` for video of another codec and for `protocol=http`. |
| `stub` | `POST` | `/{transcodeType}/:/transcode/universal/fallback` | user | Manually trigger a transcoder fallback |  |
| `partial` | `GET` | `/{transcodeType}/:/transcode/universal/start.*` | user | Start A Transcoding Session | Video, Direct Stream, over HLS (growing playlist) or DASH (the reference's static manifest). The reference's command lines; video copied, or re-encoded to H.264 under `maxVideoBitrate` along the reference's measured ladder; audio copied or converted to AAC. Same `501`s as the decision. |
| `stub` | `GET` | `/{transcodeType}/:/transcode/universal/subtitles` | user | Transcode subtitles | The reference serves it without `X-Plex-Client-Identifier` despite the spec, but refuses a request with no resolvable client profile (`X-Plex-Platform`) — see spec-gaps.md. |

## General

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `done` | `GET` | `/` | user | Get PMS info | 32 of 43 attributes identical to the reference; the rest differ on purpose (`myPlex=0`, `myPlexSubscription=1`, `ownerFeatures` as names not account GUIDs) or by environment. Public despite spec security |
| `done` | `GET` | `/identity` | token | Get PMS identity | XML+JSON byte-identical to reference; public despite spec security |
| `stub` | `GET` | `/security/resources` | user | Get Source Connection Information |  |
| `stub` | `POST` | `/security/token` | user | Get Transient Tokens |  |

## Live TV

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/livetv/sessions` | user | Get all sessions |  |
| `stub` | `GET` | `/livetv/sessions/{sessionId}` | user | Get a single session |  |
| `stub` | `GET` | `/livetv/sessions/{sessionId}/{consumerId}/index.m3u8` | user | Get a session playlist index |  |
| `stub` | `GET` | `/livetv/sessions/{sessionId}/{consumerId}/{segmentId}` | user | Get a single session segment |  |

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
| `stub` | `PUT` | `/library/collections/{collectionId}/items` | admin | Add items to a collection |  |
| `stub` | `PUT` | `/library/collections/{collectionId}/items/{itemId}` | admin | Delete an item from a collection |  |
| `stub` | `PUT` | `/library/collections/{collectionId}/items/{itemId}/move` | admin | Reorder an item in the collection |  |

## Log

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `POST` | `/log` | admin | Logging a multi-line message to the Plex Media Server log |  |
| `stub` | `PUT` | `/log` | admin | Logging a single-line message to the Plex Media Server log |  |
| `stub` | `POST` | `/log/networked` | admin | Enabling Papertrail |  |

## Playlist

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/playlists` | user | Retrieve Playlists |  |
| `stub` | `GET` | `/playlists/{playlistId}` | user | Retrieve Playlist |  |
| `stub` | `GET` | `/playlists/{playlistId}/items` | user | Retrieve Playlist Contents |  |

## Preferences

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `partial` | `GET` | `/:/prefs` | user | Get all preferences | The 10 settings Plex Web reads, of the reference's 162, with its definitions and order; `AcceptedEULA` true, `FriendlyName` and `allowMediaDeletion` from the official API. The other 152 name machinery this server does not have. |
| `todo` | `PUT` | `/:/prefs` | user | Set preferences | Declared beside `GET`, answers 501. |
| `stub` | `GET` | `/:/prefs/get` | user | Get a preferences |  |

## Timeline

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `PUT` | `/:/scrobble` | user | Mark an item as played |  |
| `partial` | `POST` | `/:/timeline` | user | Report media timeline | Also `GET`, what Plex Web sends, and `PUT`. Updates the state and position of a Direct Stream session and closes it on `stopped`. Always answers `playbackState="ignore"`: no watch state is stored yet, and Direct Play has no session to update. See spec-gaps.md. |
| `stub` | `PUT` | `/:/unscrobble` | user | Mark an item as unplayed |  |

## Updater

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `PUT` | `/updater/apply` | admin | Applying updates |  |
| `done` | `PUT` | `/updater/check` | admin | Checking for updates | Acknowledged with an empty `200` typed `text/html`, as the reference does; there is no channel to look in. |
| `done` | `GET` | `/updater/status` | admin | Querying status of updates | An install that cannot update itself: `canInstall` false, no release. open-plex is not updated through plex.tv. |

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
| `stub` | `GET` | `/hubs/search` | user | Search Hub |  |
| `stub` | `GET` | `/hubs/search/voice` | user | Voice Search Hub |  |

## UltraBlur

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `GET` | `/services/ultrablur/colors` | user | Get UltraBlur Colors |  |
| `stub` | `GET` | `/services/ultrablur/image` | user | Get UltraBlur Image |  |

## Collections

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `POST` | `/library/collections` | user | Create a collection |  |

## Rate

| Status | Method | Path | Scope | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `stub` | `PUT` | `/:/rate` | user | Rate an item |  |
