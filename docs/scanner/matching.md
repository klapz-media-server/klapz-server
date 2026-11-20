# Matching and metadata refresh on Plex Media Server

How the reference server (PMS 1.43.5.11029) matches a library item to an online record and
fetches its metadata, written for open-plex's scanner, which will do the same through the
official API. Researched on 2026-11-17 against the live reference at `127.0.0.1:32400`, its
database (opened read-only), its DEBUG logs, the strings of `Plex Media Server.exe` and
`Plex Media Scanner.exe`, and direct calls to `metadata.provider.plex.tv` with the owner's token.

Every statement is marked:

- **measured**: observed on the live server, its database, its logs or the provider;
- **binary**: read from the executables' strings, not exercised;
- **inferred**: a reading of measured facts that no single test settled.

Item types are Plex's numbers: 1 film, 2 series, 3 season, 4 episode, 12 extra (clip),
18 collection.

## 1. Who does what

| Step | Process | Evidence |
| --- | --- | --- |
| Walk the folders, parse names, create `metadata_items` with a title, year and `index` | PMS, in process ("Native Scanner") | **measured**: log stages `Local Metadata`, `Agent Match`, `Add to Database`, `Media Analysis` |
| Gather an item's local hints (its files, auxiliary images, sidecar subtitles) | `Plex Media Scanner.exe --match` child process | **measured**, §4 |
| Ask the provider for a match, fetch the record, write it, download images | PMS, in process | **measured** (logs and database); the provider HTTP calls themselves are **not logged** at DEBUG |
| Blur hashes and UltraBlur colours | PMS post-processors, after the save | **measured**, §9 |

The file scanner has no part in contacting `plex.tv`. The `--match` child is started with the
account token in `X_PLEX_TOKEN`, but it prints only local hints (**measured**). The binary
holds `https://metadata.provider.plex.tv` and a
`/library/metadata/%d/matches?agent=%s&language=%s&manual=%d` path, so it can call back to PMS
(**binary**). That was not exercised here.

## 2. Agents and providers

### Tables (**measured**)

`metadata_agent_providers` on the reference:

| id | identifier | title | uri | agent_type | metadata_types |
| --- | --- | --- | --- | --- | --- |
| 1 | `tv.plex.agents.movie` | Plex Movie | `provider://tv.plex.provider.metadata` | 0 | 1 |
| 2 | `tv.plex.agents.series` | Plex Series | same | 0 | 2,3,4 |
| 3 | `tv.plex.agents.music` | Plex Music | same | 0 | 8,9,10 |
| 4 | `org.musicbrainz.agents.music` | MusicBrainz | same | 2 | 8,9,10 |
| 5 | `tv.plex.agents.none` | Plex Personal Media | *(empty)* | 0 | 1,2,3,4,8,9,10,13,14,20,21,22 |
| 6 | `tv.plex.agents.localmedia` | Plex Local Media | *(empty)* | 1 | same as none, `online=0` |
| 7–9 | `tv.plex.agents.nfo.{movie,series,music}` | Plex NFO … | `provider://tv.plex.provider.metadata` | 0 | per kind |

- `metadata_agent_provider_groups`: one built-in group per primary agent (`at:builtIn=1`).
- `metadata_agent_provider_group_items`: maps each group to its provider, `order` 1000.0.
- `metadata_items.metadata_agent_provider_group_id` is NULL on every item inspected. The
  section's agent decides.

`library_sections.agent` is the agent identifier, `scanner` is the scanner name (`Plex Movie`,
`Plex TV Series`), and `language` is a BCP-47 tag. Matching settings sit in `user_fields`
as `pr:*` pairs: `pr:country`, `pr:includeAdultContent`, `pr:originalTitles`,
`pr:respectTags`, `pr:useLocalAssets`, `pr:showOrdering` (`aired`…), `pr:ratingsSource`
(`thetvdb`), `pr:autoCollectionThreshold`, `pr:collectionMode`, `pr:skipNonTrailerExtras`,
`pr:includeExtrasWithLocalizedSubtitles`, `pr:enableBIFGeneration` (**measured**, sections
4–20). The test sections 27 and 28 use `tv.plex.agents.none` with only `pr:respectTags=1`.

### Published lists (**measured**)

- `GET /media/providers/metadata` lists `MetadataAgentProvider` elements: `id`,
  `identifier`, `title`, `uri`, `agentType` (`primary`), `online`, and `MetadataType[{type}]`.
- `GET /system/agents` lists the legacy plug-in agents (`com.plexapp.agents.imdb` "Plex Movie
  (Legacy)", `thetvdb`, `themoviedb`, `lastfm`, `none`, secondary ones such as `fanarttv`,
  `localmedia`, `htbackdrops`, `movieposterdb`, `plexthememusic`, `lyricfind`) followed by
  the seven `tv.plex.agents.*` with negative or positive `id`.
- `GET /system/agents/tv.plex.agents.movie/config/1` answers an empty `com.plexapp.system`
  container.

### What the section agent changes (**measured**)

The section's agent alone decides how an item is matched. `PUT /match` with
`agent=tv.plex.agents.movie` on an item of a `tv.plex.agents.none` section sets the guid,
then re-matches straight away with the section's agent and restores
`tv.plex.agents.none://{id}` (§6).

### Guids seen in the database (**measured**, libraries 4–20)

| Prefix | Meaning | Counts |
| --- | --- | --- |
| `plex://{movie,show,season,episode,collection}/{24 hex}` | matched by a Plex agent | 18,305 films, 3,991 series, 9,656 seasons, 132,773 episodes |
| `local://{metadata_items.id}` | no match, or a child absent from the provider | 5,912 episodes, 64 seasons, 8 films |
| `tv.plex.agents.none://{id}` | Personal Media agent | test sections and a few real items |
| `com.plexapp.agents.{imdb,themoviedb,thetvdb}://…?lang=xx` | legacy agents, never re-matched | about 300 |
| `collection://…` | collections made on the server | 1,471 |

`metadata_items.hash` is `sha1(guid)` in hex: true for 18,333 of 18,355 films and
139,003 of 139,102 episodes. It is recomputed whenever PMS sets a guid. On test item 577986 it
went from `sha1("local://577986")` to `sha1("tv.plex.agents.none://577986")` at the first
re-match (**measured**). The hash names the image bundle (§8), so two libraries holding the
same work share one bundle.

## 3. The provider: `https://metadata.provider.plex.tv`

### Authentication and headers (**measured**)

- Requests carry the account's plex.tv token as `X-Plex-Token`; every call here sent it as a
  header. Calling without a token was not tried (see §11).
- `Accept: application/json` gives JSON.
- `X-Plex-Language: fr-FR` localizes titles, summaries, taglines, genres and role labels
  ("Directeur", "Scénariste"), the content rating (`fr/12`) and the chosen poster. A
  `language` field in a match body has no visible effect, and a query parameter
  `X-Plex-Language=fr-FR` also worked.

### Root: `GET /` (**measured**)

```json
{"MediaProvider":{"identifier":"tv.plex.provider.metadata","title":"Metadata","version":"6.1.0",
 "protocols":["stream"],"Feature":[
  {"type":"metadata","key":"/library/metadata"},
  {"type":"actions","Action":[{"id":"rate","key":"/actions/rate"},
                              {"id":"scrobble","key":"/actions/scrobble","reverseKey":"/actions/unscrobble"}]},
  {"type":"collection","key":"/library/collections"},
  {"type":"location","key":"/location"},
  {"type":"markers","key":"/markers"},
  {"type":"imagetranscoder","key":"https://images.plex.tv/photo","public":true},
  {"type":"match","key":"/library/metadata/matches"}]}}
```

### Match: `GET` or `POST /library/metadata/matches` (**measured**)

Both methods take the same hints, either as query parameters (`GET`) or as a JSON body
(`POST`, `Content-Type: application/json`). The response is a `MediaContainer` (`identifier`
`tv.plex.provider.metadata`, `librarySectionID` `"home"`) whose `Metadata` entries carry
the full record (§3, *Record*).

| Hint | Effect, measured |
| --- | --- |
| `type` | **Required, numeric** (1, 2, 3, 4). `"movie"` finds nothing. |
| `title`, `year` | Title search. The year is a strong filter: `Heat`/1995 → *Heat*, `Heat`/1996 → *Midnight Heat* (1996), `Soul`/2021 → *The Soul* (2021). No ±1 tolerance when another title has the exact year. `Matrix` with no year → *The Matrix* (1999). `Le Fabuleux Destin`/2001 and `Amelie`/2001 → *Amélie*. |
| `guid` | An external id wins over everything else: `imdb://tt2948372`, `tmdb://782`, `tvdb://131117` each resolve directly, even with a wrong title. A `Guid:[{id}]` array is ignored. |
| `filename` | A base name, or a path with forward slashes, parsed by the provider: `Soul.2020.MULTi.TRUEFRENCH.1080p.WEB.H264-EXTREME.mkv` → *Soul* 2020; `Gattaca (1997) {tmdb-782}.mkv` and `Wrongname (1997) {tmdb-782}.mkv` → *Gattaca*; `{imdb-tt0078748}` likewise; `/mnt/Films/Gattaca (1997)/movie.mkv` → *Gattaca*, the folder name is read. **A Windows path with backslashes answers 500.** `filename` **overrides** `title`/`year` when both are sent (`title=Gattaca, filename=x.mkv` → a film titled *X - 1*). |
| `grandparentTitle` or `grandparentGuid`, `parentIndex`, `index` (type 4) | Episode by number: `Bull`, 5, 1 → *My Corona*, `plex://episode/5f9c3927f07efd002effd920`. Without one of `filename`, `plexHash`, grandparent guid or title the answer is `400 "Missing required param filename, plexHash, grandparent guid or title!"`. `grandparentTitle` + `originallyAvailableAt` (by air date) found nothing. A `filename` alone (`Bull.2016.S05E01…mkv`) finds the episode. |
| `parentTitle` or `parentGuid`, `index` (type 3) | Season: → `plex://season/5f9c3927f07efd002effd918`. |
| `manual=1` | Up to 20 candidates per page (`totalSize` 70 for *Soul*/2020), ranked by the provider. Without it the answer is **0 or 1 result: the provider decides the automatic match**. |
| `includeAdultContent=1` | Accepted; no difference seen on a non-adult title. |

There is **no score in the provider's answer** and none in PMS's `/matches` answer either. The
automatic match threshold lives on plex.tv: open-plex cannot reproduce it, only call it.
Nonsense (`qwzxv`) or an unparseable release name (`random.release.name.mkv`) gives
`totalSize: 0`.

Trimmed automatic answer (**measured**, `POST {"type":1,"title":"Soul","year":2020}`):

```json
{"MediaContainer":{"librarySectionID":"home","librarySectionTitle":"Metadata","offset":0,
 "totalSize":1,"identifier":"tv.plex.provider.metadata","size":1,"Metadata":[{
  "guid":"plex://movie/5d77702d594b2b001e746263","key":"/library/metadata/5d77702d594b2b001e746263",
  "ratingKey":"5d77702d594b2b001e746263","type":"movie","title":"Soul","year":2020,
  "originallyAvailableAt":"2020-12-24","studio":"Pixar","contentRating":"PG","duration":6060000,
  "rating":9.5,"ratingImage":"rottentomatoes://image.rating.ripe","audienceRating":8.8,
  "audienceRatingImage":"rottentomatoes://image.rating.upright","slug":"soul-2020",
  "thumb":"https://metadata-static.plex.tv/d/gracenote/d22c….jpg","art":"https://metadata-static.plex.tv/5/gracenote/5364….jpg",
  "Image":[…],"Genre":[…],"Guid":[…],"Rating":[…],"Role":[…],"Director":[…],"Writer":[…],
  "Producer":[…],"Country":[…],"Studio":[…],"Similar":[…],"CommonSenseMedia":[…],"AwardNominee":[…],"Summary":[…]}]}}
```

### Record: `GET /library/metadata/{ratingKey}` (**measured**)

`ratingKey` is the 24-hex part of the guid. The record has the match's fields plus
`Review` when `includeReviews=1` is sent (`includeExtras`, `includeChildren`,
`asyncAugment` changed nothing visible). `/reviews` is a 404. Other top-level fields:
`tagline`, `summary`, `banner`, `theme` (series: `https://tvthemes.plexapp.com/{tvdb}.mp3`),
`budget`, `revenue`, `imdbRatingCount`, `publicPagesURL`, `addedAt`, `userState`,
`originalThumb`, `playableKey`, `availabilityId`, `streamingMediaId` (the last four on a title
with free streaming), `subtype` (`short`…), `childCount`, `leafCount`,
`lastEpisodeOriginallyAvailableAt`, `lastSeasonOriginallyAvailableAt`.

Sub-elements, first entry shown:

```json
"Guid":[{"id":"imdb://tt2948372"},{"id":"tmdb://508442"},{"id":"tvdb://131117"}],
"Rating":[{"image":"imdb://image.rating","type":"audience","value":8}, …],
"Role":[{"id":"5d77682485719b001f3a04d4","order":1,"slug":"jamie-foxx","tag":"Jamie Foxx",
         "thumb":"https://metadata-static.plex.tv/2/people/2b8b….jpg","role":"Joe Gardner (voice)",
         "key":"/library/people/5d77682485719b001f3a04d4","type":"person"}],
"Director":[{"id":"5d776827151a60001f24aaf0","tag":"Pete Docter","role":"Director", …}],
"Genre":[{"id":"5d9c046c08fddd001f28ab9e","tag":"Family","slug":"family","filter":"genre=…","context":"tag.genre", …}],
"Similar":[{"guid":"plex://movie/5d776836151a60001f24de1f","tag":"WALL·E"}],
"Studio":[{"tag":"Pixar"}], "Country":[{"tag":"United States of America"}],
"Review":[{"tag":"Kambole Campbell","text":"…","image":"rottentomatoes://image.review.rotten","link":"https://…","source":"Hyperallergic"}],
"CommonSenseMedia":[{"oneLiner":"…","AgeRating":[{"age":8,"rating":4,"type":"official"}],"key":"…/commonsensemedia"}],
"AwardNominee":[{"award":"Oscar","category":"…","event":"Academy Awards","isWinner":true,"year":2021,"personIds":[…]}],
"Summary":[{"size":100,"tag":"A band teacher discovers…","type":"default"}],
"Image":[{"type":"background","url":"…"},{"type":"backgroundSquare",…},{"type":"banner",…},
         {"type":"clearLogo",…},{"type":"clearLogoWide",…},{"type":"coverArt",…},
         {"type":"coverPoster",…},{"type":"coverSquare",…},{"type":"snapshot",…}]
```

A series also has `Network:[{tag:"CBS"}]` and `SeasonType`, the episode orderings on offer:

```json
"SeasonType":[{"id":"tmdbAiring","source":"tmdb","tag":"Aired","title":"The Movie Database (Aired)"},
              {"id":"tvdbAiring",…},{"id":"tvdbDvd",…,"tag":"DVD"},{"id":"tvdbAbsolute",…,"tag":"Absolute"}]
```

### Children (**measured**)

- `GET /library/metadata/{show}/children` returns the seasons. Each has `index`,
  `parentGuid`, `parentTitle`, `hasGenericTitle`, `leafCount`, `thumb`, `art`, `Guid`,
  `Rating`, `Role`…
- `GET /library/metadata/{season}/children` returns the episodes, with `index`,
  `parentIndex`, `grandparentGuid`, `grandparentTitle`, `originallyAvailableAt`,
  `duration`, `thumb`, `Image[9]`, `Role`, `Director`, `Writer`, `Producer`, `Guid`,
  `Rating`.
- `GET /library/metadata/{show}/grandchildren` returns every episode (125 for *Bull*) and
  pages with `X-Plex-Container-Start`/`-Size`.

### Images: `GET /library/metadata/{ratingKey}/images` (**measured**)

```json
{"MediaContainer":{"totalSize":154,"size":154,
 "Thumb":[{"key":"https://image.tmdb.org/t/p/original/hm58Jw4Lw8OIeECIq5qyPYhAeRJ.jpg","source":"tmdb",
           "width":1000,"height":1500,"previewKey":"…"}, …83],
 "Art":[{"key":"https://image.tmdb.org/t/p/original/rQaHA74pevnGsxcKGaoZVGWe9TC.jpg","source":"tmdb",…}, …71]}}
```

The sources are tmdb, fanarttv, tvdb, gracenote and imdb. The order depends on the language
(88 posters for fr-FR).

### Extras: `GET /library/metadata/{ratingKey}/extras` (**measured**)

The answer is a list of `type: clip` entries with `subtype` (`trailer`…), `extraType` (1 =
trailer), `primaryGuid`, `guid` `plex://clip/…`, `thumb`, `duration`, and
`Media[{url, bitrate, width, height, videoCodec, Part[{key, Stream[…]}]}]`. The URL is a
third-party CDN (`video.internetvideoarchive.net`, signed). *Bull* has none.

### Other hosts (**binary**)

- `https://mp.plex.tv/providers/universal/library/matches`, with the strings `plexHash`,
  `openSubtitlesHash`, `fileSize`, `filename`, `includeFullMetadata`,
  `includeAncestorMetadata` and the setting `UniveralMetadataProviderUrl`. Its use was not
  observed. It sits next to strings about downloads (`SyncMP…`).
- `customAgentProviderUrl`, and "Custom metadata provider identifiers must be prefixed with
  'tv.plex.agents.custom.'": custom providers are supported.

## 4. The `--match` job (local hints)

**Measured**: PMS starts it for every refresh, of a matched item too, as
`MetadataAgentPostProcessor`:

```
set "X_PLEX_TOKEN=…" & "Plex Media Scanner.exe" --match --type 1 --log-file-suffix " Matcher"
    --item 207679 --content-type=application/json [--match-tag-mode=all]
```

Its standard output, run by hand on the test items, is only:

```json
{"MediaContainer":{"size":1,"Metadata":[{"type":"movie","Media":[{"Part":[
  {"file":"C:\\Users\\…\\Stacked Film (2008)\\Stacked Film (2008) cd1.mkv"},
  {"file":"C:\\Users\\…\\Stacked Film (2008)\\Stacked Film (2008) cd2.mkv"}]}]}]}}
```

For a series it prints only `{"type":"show"}`. Its log says
`Matcher: found N auxiliary files for {title}` and
`Matcher: no sidecar subtitle files found for {path}`. `{tmdb-…}`/`{imdb-…}` in a name are
**not** parsed out here: they reach the provider inside the file name. Usage text
(**binary**): `--match-tag-mode=X  Tag mode (normal, all, exclusive)`,
`--match-artist`/`--match-album` for music.

## 5. Automatic matching rule

- **Measured**: PMS sends hints, the provider answers 0 or 1 candidate, and that candidate is
  the match. With no title given, `GET /library/metadata/577986/matches?agent=tv.plex.agents.movie`
  found *Soul* 2020 for an item whose stored title is the raw stem
  `Soul.2020.MULTi.TRUEFRENCH…`. The provider finds nothing for that stem as a `title`, so
  PMS must forward the file name (**inferred**).
- **Inferred**: so the hints are the item's `title`/`year` (the scanner's parse) and the
  part's file name, as a base name or `/`-separated, since a backslash path is a 500.
  Episodes use the series' guid with `parentIndex`/`index`. The exact request PMS sends is
  not logged and was not captured.
- **Measured**: no match leaves `guid = local://{id}`. The title is the scanner's parse,
  title-cased word by word ("Parents D Eleves", 2020). The poster is the video frame PMS
  extracted, `media://{h0}/{h1…}.bundle/Contents/Thumbnails/thumb1.jpg` (bundle under
  `Media/localhost/`). An episode the provider lacks gets `local://{id}` under a `plex://`
  season.
- **Measured, PMS log for the Personal Media agent**:
  `Matching '{title}' (ID n) with agent tv.plex.agents.none` →
  `MetadataAgent: Found a search result '{title}'` → ` * Score: 100 ({title})` →
  `Setting GUID for metadata item n (title: …) to tv.plex.agents.none://n (thumb: ) (instance rating key: )`.
  The score scale and log format exist; what a Plex agent logs there was not observed.

## 6. Manual matching

| Route | Measured behaviour |
| --- | --- |
| `GET /library/metadata/{id}/matches?manual=1&title=&year=&agent=&language=` | `SearchResult` entries: `guid`, `name`, `year`, `summary`, `type`, `thumb` (an `images.plex.tv/photo?height=336&width=225&minSize=1&upscale=1&url=…` address), `lifespanEnded`. No `score`. Calls the provider directly, with no `--match` job. Without `manual` it answers the single automatic candidate. Without `title` it still finds the work (file name forwarded). **`PUT` answers 404**, although the spec declares `PUT`. |
| `PUT /library/metadata/{id}/match?guid=&name=&year=` | Logs `Setting GUID … to {guid} (thumb: {thumb}) (instance rating key: )`, then **migrates `metadata_item_settings` from the old guid to the new one, merging rows of the same account** ("Merging over old setting into new one with metadata setting ID …"), then saves, re-matches with the section's agent and refreshes. In a `none` section it ends up back on `tv.plex.agents.none://{id}`. 131 films carry an `https://images.plex.tv/photo?…` poster, which reads as a fix-match whose `thumb` was the search result's (**inferred**). |
| `PUT /library/metadata/{id}/unmatch` | Sets `local://{id}`, migrates settings to it, then **re-matches immediately with the section's agent** and refreshes. Seen only in a `none` section. |
| `PUT /library/metadata/{id}/refresh?force=1` | Queues the item: media part analysis → `--match` job → `Loaded metadata for … in N ms` → save → BlurHash → UltraBlur. |
| `GET /library/matches?type=1&title=&year=` | Searches the **local** library, not the provider: exact title (case-insensitive) and exact year, `score: 100` on each hit (`Sol`/2020 → a local *SOL*). |

The guid migration is the dangerous part. Watch state (`metadata_item_settings`) is keyed
by guid, so matching an item onto a guid other copies already hold moves and merges that
state, and every user's rows follow the item when it moves away again. **Measured**: it
happened here (see the closing notes).

## 7. What a refresh writes

Measured on *The Escort* (id 207679, section 4, fr-FR), refreshed by the Butler at 03:58, and
compared with the provider's record fetched with `X-Plex-Language: fr-FR`:

| Column | Source |
| --- | --- |
| `guid` | `plex://movie/{ratingKey}` |
| `hash` | `sha1(guid)` |
| `title`, `tagline`, `summary` | localized fields |
| `title_sort` | **not from the provider**: diacritics folded and an English leading article dropped (`The Transporter` → `Transporter`; `Le Seigneur…` keeps `Le`, `Batman : Le Défi` → `Batman : Le Defi`) |
| `original_title` | `originalTitle`, empty when absent |
| `studio` | first `Studio` |
| `content_rating` | `contentRating` as given (`fr/12`, `PG`) |
| `rating` | `rating` (critic); NULL when absent |
| `audience_rating` | `audienceRating` |
| `duration` | provider's `duration` (5,252,117 equal), not the file's |
| `originally_available_at` | `originallyAvailableAt` at midnight UTC |
| `year`, `slug` | as given |
| `refreshed_at`, `updated_at` | time of the refresh |
| `changed_at`, `resources_changed_at` | change stamps (a counter, not a time) |
| `tags_genre`, `tags_director`, `tags_writer`, `tags_star`, `tags_country`, `tags_collection` | denormalized `|` lists capped at 2, 3, 2, 3, 2, 2 entries; `tags_star` is the first roles by `index` (1,992 of 2,000 films) |
| `user_thumb_url`, `user_art_url`, `user_square_art_url`, `user_clear_logo_url` | §8; `banner` is not stored on films (`user_banner_url` empty) |
| `extra_data` | `at:ratingImage`, `at:audienceRatingImage` (`rottentomatoes://image.rating.spilled`…), `ex:primaryExtraKey` (`/library/metadata/{local extra id}`), `ex:hasPremiumPrimaryExtra`, `ex:hasPremiumExtras`, plus the post-processing keys of §9. Rare keys: `pv:rematchCount`, `pv:addedAtRestored` (series), `pv:titleSortSaved`, `pr:languageOverride`/`pr:showOrdering` (series' own settings). |

Taggings, `tags.tag_type` (**measured**):

| tag_type | Holds | Per tagging |
| --- | --- | --- |
| 1 genre, 8 country | localized names | `index` |
| 4 director, 5 writer, 7 producer | person, `tags.key` = provider person id | `text` = localized role label ("Directeur") |
| 6 role | person, `tags.key` = id, `tags.user_thumb_url` = photo | `text` = character |
| 10 review | critic name | `text` = quote; `extra_data` `at:image`, `at:link`, `at:source` |
| 305 | Similar (titles) | `index` 1–10 |
| 312 Thumb, 313 Art, 323 ClearLogo, 325 SquareArt | image URLs | `text` = URL; 323 `extra_data` `at:source` |
| 314 | external guid (`imdb://tt…`, `tmdb://…`, `tvdb://…`) | `index` |
| 315 | provider collection (`plex://collection/…`, `extra_data` `at:summary`) | |
| 316 | rating source image (`imdb://image.rating`) | `text` = value, `tags.extra_data` `at:type` (`audience`/`critic`) |
| 317 Theme | theme music URL | `text` |
| 318 | studio | `index` |
| 319 | network (series) | |
| 322 | series ordering offered (`The Movie Database (Aired)`) | |
| 324 | Common Sense Media `oneLiner` | `pv:ageRatingAge`, `pv:ageRatingRating`, `pv:ageRatingType` |
| 12 | markers (`credits`…), from analysis, not refresh | |

The 312/313/323/325 taggings hold the chosen URL after a refresh. **Measured**:
`GET /library/metadata/{id}/posters` on the server (`/arts` presumably too, not tried) fetches the provider's whole
`/images` list and **writes it** as taggings, 81 posters and 53 backgrounds plus 323/325 for
*Soul*. The answer marks each `key`, `ratingKey` (the URL), `thumb` (images.plex.tv),
`selected` and `provider`. Type 9, ten rows with an empty tag on *Soul*, was not identified.

**Children (measured)**: seasons and episodes are matched under the series, not on their
own, and get `plex://season/…` / `plex://episode/…`. Their own `extra_data` holds the
post-processing keys and `at:audienceRatingImage` (episodes). An episode the provider lacks
stays `local://`.

**Extras (measured)**: `metadata_relations(metadata_item_id, related_metadata_item_id,
relation_type=1)` links a film to type-12 items with no section. Each extra item stores
`extra_data` `ex:extraType`, its `user_thumb_url` is the provider's https URL, and it has three
`media_items`/`media_parts` (5000, 2500 and 750 kb/s). A part has an empty `file` and
`extra_data` `at:key=/services/iva/assets/{id}/video.mp4?fmt=4&bitrate=…`. Refresh logs
`Extras: N existing extras.`, `Extras: Checked extra '…' and found matched: N Media matched: N`,
`Added new metadata item … `, `Extras: Deleted N remaining extras.`

**Collections (measured)**: after a scan or refresh, `Collections: Found N items in provider
collection '…', promoted candidates`, governed by `pr:autoCollectionThreshold` (2).

**Locks (measured)**: `user_fields` `lockedFields=a|b|…`, numbered as in
`crates/db/src/import/metadata.rs` (`plex_field`). 120 of the 126 items with an `upload://`
poster have field 9 (poster) locked; the 50 other poster-locked films and series hold a
`metadata://` poster. That a refresh skips a locked field is **inferred** from this, not
observed field by field.

**Scheduled refresh (measured, Butler task `RefreshPeriodicMetadata`, 03:58)**:

```
Butler: refreshing metadata for 0 seasons because they contain episodes released in the past week
Butler: refreshing metadata for 22 movies because they're recently released and have no been refreshed in the past week
Butler: refreshing metadata for 100 movies because they haven't been refreshed in the past month
Butler: refreshing metadata for 100 shows because they haven't been refreshed in the past month
Butler: Considering 0 artists for periodic refresh.
```

The 100 looks like a per-run cap. Each item logs `Loaded metadata for '…' (ID n) in N ms`,
`Doing expensive tags write … because something changed`,
`Local media already exists (metadata://posters/…)`, `There was a change for metadata item …,
saving.` and `Done with metadata update for n`.

## 8. Images: choice, naming, storage

- **Choice (measured)**: the poster is the record's `thumb`, the background its `art`, the
  square art the `Image` of type `backgroundSquare`, and the clear logo the `Image` of type
  `clearLogo`. A record with only `clearLogoWide` leaves `user_clear_logo_url` empty. They
  are localized by `X-Plex-Language`.
- **Name (measured)**: `{kind}/{agent}_{sha1(source URL)}`.
  `sha1("https://assets.fanart.tv/fanart/the-escort-55ba8e07bdbe4.jpg")` =
  `f13197593cd05989e7419c9b9ab4486690f19c70` and *The Escort* has
  `user_thumb_url = metadata://posters/tv.plex.agents.movie_f13197593cd05989e7419c9b9ab4486690f19c70`.
  The same holds for its art, its square art and *Soul*'s poster and art. Kinds are
  `posters`, `art`, `clearLogos`, `squareArt` and `themes`.
- **File (measured)**: `Metadata/{Movies|TV Shows|Collections}/{hash[0]}/{hash[1:]}.bundle/Contents/_combined/{kind}/{agent}_{sha1}`.
  The bytes are the downloaded JPEG/PNG, without an extension. The download is a plain `GET` of
  the source URL (`image.tmdb.org`, `assets.fanart.tv`, `metadata-static.plex.tv`,
  `artworks.thetvdb.com`), logged `HTTP requesting GET …`. An existing file is not
  fetched again ("Local media already exists").
- **Children (measured)**: a season's or episode's `metadata://posters/…` resolves into the
  **series' bundle**. 157 of 158 season images logged by UltraBlur did, and 613 season and 6,560
  episode posters were found as files there. The legacy agents wrote
  `metadata://seasons/{n}/posters/…` and `metadata://seasons/{n}/episodes/{m}/thumbs/…`.
- **Uploads (measured)**: `upload://posters/{agent}_{sha1}`, stored under
  `{bundle}/Uploads/posters/…`, with `Uploads/posters/seasons/{n}/episodes/{m}/…` for
  children. `_stored/` directories also exist, empty here.
- **Local, unmatched (measured)**: `media://{h0}/{h1…}.bundle/Contents/{Thumbnails/thumb1.jpg|Art/art1.jpg}`
  under `Media/localhost/`, made from the video.
- **Missing file**: a 404, and nothing fetches it again (known before this study). Only 1,713
  bundles exist on the reference for some 180,000 items.

## 9. Post-processing (measured)

After the save, in the same activity:

1. `[BlurHashProcessor]`: `Created thumbnail of size 67x100` (poster) or `100x56` (art), a
   downscale to 100 px on the long side, then `Generated for item n thumb|art in N ms
   (success: 1)`. The results go to `pv:thumbBlurHash` and `pv:artBlurHash`, with
   `pv:blurHashesChangedAt` set to the item's change stamp. A portrait poster's hash starts
   with `T`, 3×4 components; a landscape image's with `L`, 4×3 (orientation decides).
2. `[UltraBlurProcessor] Calculated media file path for path [metadata://art/…]` →
   `Generated for item n in N ms`, written to `pv:ultraBlurTopLeft|TopRight|BottomLeft|BottomRight`
   (hex RGB) and `pv:ultraBlurChangestamp`. It reads the **art**; episodes, which have no art,
   still get colours, so the poster is the fallback (**inferred**: 128,773 episodes coloured,
   2 with an art hash). `Failed to get color palette: Empty pixels when quantize` on blank
   images.
3. Section-level `pv:lastBlurHashChangedAt`, `pv:lastUltraBlurChangedAt` and `pv:lastAddedAt`
   in `library_sections.user_fields`.

Both run on every refresh, the `none` agent's included.

## 10. What depends on plex.tv

Without a plex.tv account none of this works:

- the provider's match, record, children, images and extras (`X-Plex-Token` of a plex.tv
  account on every call measured);
- the automatic-match decision and its threshold, which happen on plex.tv;
- the `plex://` guids themselves, the identity watch state and collections key on;
- `images.plex.tv/photo` thumbnails in manual results.

Without an account these keep working: name parsing, `local://` items, video-frame posters,
`media://` bundles, BlurHash and UltraBlur, NFO and local assets (`tv.plex.agents.nfo.*`,
`tv.plex.agents.localmedia`; their protocol was not studied), and the downloads from the
images' sources, which need no token.

## 11. Open questions

1. **The exact request PMS sends.** GET or POST, which hints (both `title` and `filename`,
   though `filename` overrides `title`?), headers (`X-Plex-Language` from the section,
   `X-Plex-Country` from `pr:country`?) and paging. Nothing is logged. A capturing proxy or
   PMS's verbose logging would settle it, but the second means changing a preference.
2. **Does the provider need the token at all?** Not tried, so as not to spend the account
   against rate limits.
3. **A real match and refresh end to end** in a Plex-agent section. Both test sections use
   the `none` agent, and changing their agent was out of bounds. A third test section with
   `tv.plex.agents.movie` would allow watching image selection, lock handling, extras and
   series children as they are written.
4. **Unmatch in a Plex-agent section**: does it re-match immediately as in a `none` section,
   making "unmatched" unreachable? The binary has a field label `Unmatched`.
5. **Series ordering** (`pr:showOrdering`, `SeasonType`): which parameter selects the order
   in `/children`, and how `absolute_index` is filled.
6. **Locks field by field**: refresh skipping a locked field was not observed directly.
7. **Tag type 9** (ten empty-tag rows on *Soul*), and `pv:rematchCount` /
   `pv:addedAtRestored`.
8. **`mp.plex.tv/providers/universal/library/matches`** (`plexHash`, `openSubtitlesHash`):
   matching by file hash, used for downloads? Not observed.
9. **Age of what the database shows**: the provider's current `thumb` for *Soul* (gracenote)
   is not the poster stored at its last refresh (tmdb). Whether PMS keeps a previously chosen
   image across refreshes or the provider changed its choice since is unknown.

---

### Side effects of this research on the reference (2026-11-17)

- **Watch state of the real *Soul* moved onto a test item.** Two `PUT /match` calls on test
  item 577986 (section 27) to `plex://movie/5d77702d594b2b001e746263` made PMS migrate
  `metadata_item_settings` both ways. All 46 rows of that guid, 45 accounts plus the owner's
  (id 490528, which absorbed the test item's own row 504090), now carry
  `guid = 'tv.plex.agents.none://577986'`, and the real *Soul* items (379494, 379501, 379507,
  379513) have none. History (`metadata_item_views`) did not move. No API call can undo it,
  because any match in a `none` section round-trips. Restoring means, with PMS stopped:
  `UPDATE metadata_item_settings SET guid='plex://movie/5d77702d594b2b001e746263' WHERE
  guid='tv.plex.agents.none://577986'`. The test item's own row (account 1, unwatched) is gone
  and need not be recreated. **Repaired on 2026-11-17** this way (the database backed up
  beside it as `…library.db.before-soul-repair-20261117`): checked on 2026-11-20, the 46 rows
  carry the real guid again, none the test item's, and the owner's *Soul* reads `viewCount` 1
  through the API.
- **Test item 577986**: `user_thumb_url` went from `x` (left by an earlier experiment) to its
  video frame, `user_art_url` from `art3.jpg` to `art1.jpg`, `hash` from
  `sha1(local://577986)` to `sha1(tv.plex.agents.none://577986)`, and blur hashes, colours and
  stamps were recomputed. Guid, title and taggings are as before.
- **Real item 379507 (*Soul*, section 6)**: `GET /library/metadata/379507/posters` cached the
  provider's image list as 81 + 53 + 15 + 2 taggings (types 312, 313, 323, 325). Its chosen
  images are unchanged.
