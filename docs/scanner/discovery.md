# How Plex discovers files and groups them into items

Research for the klapz scanner: how Plex Media Server 1.43.5 walks a library location, decides
what changed, classifies files, and groups them into movies, shows, seasons and episodes before
any metadata agent runs. Each rule carries its source:

- **[M]** measured on the reference server (`127.0.0.1:32400`, test sections 27 *Plex Movie* and 28
  *Plex TV Series*, both on the agent `tv.plex.agents.none`, both rooted at the same test folder,
  written `<root>` below), with log excerpts or row dumps;
- **[B]** read from `files/Plex Media Server.exe` (strings and the regexes compiled into it; the
  native scanner lives there, and the same tables are in `Plex Media Scanner.exe`);
- **[L]** read from the legacy Python scanners shipped in
  `files/Resources/Plug-ins-6bf8af3f0/Scanners.bundle` (`VideoFiles.py`, `Filter.py`, `Stack.py`,
  `Plex Movie Scanner.py`, `Plex Series Scanner.py`) and `LocalMedia.bundle`. The native scanner is a
  C++ port of these: most regexes reappear verbatim in the binary, some changed. **Where [L] and [M]
  disagree, [M] wins**, and the disagreement is noted.

About 230 probe files were created (3-second 320×240 H.264 MKVs of 88 KB, each with a distinct tag so
that no two share content), scanned, and removed. The section on what is left behind is at the end.

> **Reading the measurements.** Both test sections use the *Personal Media* agent
> (`tv.plex.agents.none`). Right after the scanner inserts a movie, that agent rewrites its `title`
> to the file name without extension and clears `year`. So the scanner's own output was captured
> two ways: the log line `Added new metadata item (<title>)`, and a read-only poller that reads
> `metadata_items` every 5 ms during the scan and keeps the first version of each row. Episode and
> show rows are not retitled by that agent.

---

## 1. Triggers

### 1.1 `GET /library/sections/{id}/refresh`

| Parameter | Effect |
| --- | --- |
| *(none)* | Full scan of every `section_locations` row of the section. **[M]** |
| `path=<absolute dir>` | Partial scan: only that directory and its subtree (`* Scanning <path>`, `Processing directory <path> (parent: yes)`). The path must lie inside a location, or the scan logs `ERROR - '<path>' was not inside any known section location, skipping.` and does nothing. **[M]** A path that no longer exists *is* scanned: the walk fails (`Error scanning directory, we'll skip and continue: boost::filesystem::last_write_time: …`) and everything the database had under that path is removed (the partial scan's "map" holds only the items under the path, and unvisited items are deleted, see §2.4). **[M]** |
| `force=1` | **Not** a rescan of unchanged directories: the unchanged-directory skip still applies. Afterwards, `Refreshing section N of type: T` / `Refreshing K IDs.`: a metadata refresh of **every item of the section**, ignoring `path`, which includes re-*matching* items whose guid is still `local://` (`Matching 'X' (ID n) with agent …`). **[M]** (A `force=1&path=…` on section 28 refreshed all 15 shows of the section, not just the one under `path`.) |

Answers `200` with an empty body at once; the scan runs on the library-updater queue (`Library
Updater: Requested that section 27 be updated, force=0, subdir=<path> iTunes=0` → `Queueing section
27`). A second request while the section is scanning is coalesced (`Section %d is already being
updated.` [B]). **[M]**

### 1.2 `DELETE /library/sections/{id}/refresh` (cancel)

Measured on a 40-episode folder, sent 2 s after starting: the handler **blocks until the scanner
stops** (answered after 1,098 ms) and logs `Scanner [Plex TV Series]: Cancelling scan.`, then
`Notification: Library scan interrupted - The scan of the "…" section was cancelled`. The directories
already walked are still stamped (`Updating directory …`), and the items already added stay. **[M]**
Strings `Scanner: scanning in %s was cancelled`, `Library Updater: Canceling library update.` [B].

### 1.3 Automatic triggers (server preferences, not measured)

| Preference | Default | Meaning |
| --- | --- | --- |
| `FSEventLibraryUpdatesEnabled` | `false` | "Scan my library automatically": filesystem watching (`LibraryUpdateManager will start watching %s` / `path watching is disabled` [B]). |
| `FSEventLibraryPartialScanEnabled` | `false` | "Run a partial scan when changes are detected": a watched change scans only the changed directory, the same as `refresh?path=`. |
| `ScheduledLibraryUpdatesEnabled` / `ScheduledLibraryUpdateInterval` | `false` / `3600` | Periodic full scans. |
| `ButlerTaskRefreshLibraries` | `false` | "Update all libraries during maintenance" (butler window `ButlerStartHour`–`ButlerEndHour`, 2–5). |
| `autoEmptyTrash` | `true` | "Empty trash automatically after every scan" (§2.5). |

A watcher event inside an extras folder is dropped: `Library Updater: skipping path '%s' as it
looks like an extras subfolder` [B].

### 1.4 Activity and notifications [M]

```
Activity: registered new activity <uuid> - "Scanning"
Notification: Scanning the "<section title>" section -
Scanning section 27
Activity: updated activity <uuid> - completed 0.0% - Scanning <section title>
… (per directory) …
Activity: updated activity <uuid> - completed 100.0% - Scanning <section title>
Activity: Ended activity <uuid>.
Notification: Library scan complete - Extra information may still be downloading from the Internet
```

Each added item also gets an `"Updating Metadata"` activity (`Analyzing Media` 25 %, `Updating
Metadata` 50 %), from the agent refresh and media analysis that follow insertion.
`[LibraryTimeline] Scanner activity on section 27: N added, M deleted` counts the changes.

---

## 2. Walking and change detection

### 2.1 Pipeline of one scan [M]

```
Scanning <section> using <lang>(<scanner>) with <N> current media items and <L> section locations in the database.
Performing a scan with 'Plex Movie' (language: en-US virtual: 0).
  * Scanning <location or path>
Scanner: Processing directory <dir> (parent: no|yes)      -- one per directory, depth first
Directory had 3 files, database had 1 files, can't skip.  -- or "Skipping over directory …"
  stages per batch of grouped items, in a pipeline:
    Native Scanner: Executed Local Metadata stage
    Native Scanner: Executed Agent Match stage          ("unable to find agent match" with agent none)
    Native Scanner: Executed Add to Database stage      (insert or reuse, §2.3)
    Native Scanner: Executed Media Analysis stage       (spawns "Plex Media Scanner.exe" per item)
Scanner [Plex Movie]: Idle and left with <n> media items.
Removing <n> media items that were left.
Removing <d> directories that were left.
Updating directory '<relative path>' (ID n) to time <mtime>.   -- for every directory walked
Collections: …  (collection tag sync)
Refreshing section 27 of type: 1   -- only with force=1
```

`parent: no` marks a location's root, `parent: yes` everything below it (and the start of a
partial scan).

### 2.2 The `directories` table [M]

One row per directory that holds, or held, scanned media: `library_section_id`,
`parent_directory_id` (a tree under the location's root, whose row has `path = ''`), `path`
**relative to the location root with `/` separators** (`zzprobe_series/Multi Probe`), `created_at`,
`updated_at` = the directory's **mtime** at the end of its walk (`Updating directory 'flat' (ID
11295) to time 2026-11-17 10:55:26.`), `deleted_at`. `media_parts.directory_id` points at the
directory holding the file (NULL for extras).

A directory row is also created for an intermediate directory with no media of its own, and was
**left behind** when a partial scan removed its subtree (`zzprobe_series/Multi Probe` and
`zzprobe_series/Hinted Probe {tvdb-12345}` survived a partial scan of the deleted `zzprobe_series`;
the next full scan removed them: `Removing 2 directories that were left.`).

### 2.3 The skip rule [M][B]

For each directory, the scanner lists its files after filtering (§3.1), then reads the parts the
database holds for that directory:

```sql
select distinct media_parts.file, media_parts.size, media_parts.updated_at, media_parts.deleted_at
from media_parts join media_items on media_items.id = media_parts.media_item_id
where media_parts.directory_id …
```

and refuses to skip, in this order of checks (all five messages are in the binary; the first four
were measured):

| Condition | Log |
| --- | --- |
| File count differs | `Directory had %zu files, database had %zu files, can't skip.` |
| A database file is gone from disk | `File '%s' didn't exist, can't skip.` |
| mtime differs from `media_parts.updated_at` | `File '%s' changed write time, can't skip.` |
| Size differs from `media_parts.size` | `File '%s' changed size, can't skip.` |
| Part has `deleted_at` | `File '%s' was marked as deleted, can't skip.` |

Otherwise `Skipping over directory '<rel>', as nothing has changed; removing <n> media items from
map.`: the directory's items are taken out of the "map" of items to delete and are not
regrouped. Consequences, all measured:

- **The directory's own mtime plays no part.** Removing a subfolder changes its parent's mtime, yet
  the parent was skipped; only its files count.
- Subdirectories are always descended into, skipped parent or not.
- A directory with no files and no database files is skipped (`had 0, had 0`), even on the first
  scan of a new tree.
- The "count" is of files the scanner keeps: `Directory had 1 files` for a folder holding one film
  and 14 extras, images and sidecars.
- A skipped directory is not regrouped, so local extras, assets and subtitles found at insertion
  are **not re-evaluated** until one of its media files changes (the agent refresh does that, §6).

### 2.4 Add, reuse, move, delete [M]

Within a non-skipped directory, every grouped item goes through *Add to Database*:

1. **Path match.** `Scanner [Plex Movie]: found existing media id 5567869` → `Path matched, we're
   reusing media item 5567869`. The media item and its metadata item are kept. If the file's mtime or
   size changed: `Turbo analysis on modified item …` and `Updating part with ID=…`, with
   `media_parts.size`, `updated_at` and `hash` rewritten (`hash` changed after appending 8 bytes, not
   after `touch`). The item is not regrouped, and the agent is not rerun.
2. **Otherwise, a size-then-hash lookup** among the section's parts that went missing: `Skipping
   hash check, no size match for 88701 bytes.`, or `Checking by hash to see if we can find a match with
   <hash> (display offset: 0, not part -1)` → `We found a hash match for [new] which was [old].` →
   `INFO - Part rename detected [old] was renamed [new]`. Then:
   - the **"hint name"** of the new grouping is compared with the old media item's:
     `Hint name didn't match ('KB2' != 'Kill Bill Vol 2') for DB media item 5567873`. On a mismatch
     a new metadata item is made (`Added new metadata item (KB2)`), per-user settings move to it
     (`Migrating metadata settings from tv.plex.agents.none://578722 -> local://578889`), and the old
     item is destroyed. A rename that keeps the hint name (`Amelie (2001) [1080p].mkv` →
     `Amelie (2001) [720p].mkv`, both parsed `Amelie`) kept its item;
   - in both cases the new part and media item received **new ids**: the hash match carries the
     identity of the work, not the rows.
3. Otherwise a new item (§5).

At the end, every item still in the map (never visited) is removed: `Deleting media item
5567875.` → `Was connected to metadata item 578726, count is now 0.` → `Destroying metadata item
578726 (Episode 7)` → `Destroying metadata item 578725 (Season 4)` (a season or show losing its last
child goes with it) → `Updating deletion state for metadata item 578724, is has a dead item count of
0.` Then `Removing N directories that were left.` / `Deleting directory [rel] (id)`.

**`media_parts.hash`** is 40 hex characters, content-dependent (it changed after 8 bytes were
appended), path-independent (unchanged across a rename) and mtime-independent (unchanged by
`touch`). It is **not** the SHA-1 of the whole file (checked on an 88 KB file), nor SHA-1/MD5 of the
first or last 4–64 KiB, with or without the size. It also names the part's bundle under
`Media/localhost/<h[0]>/<h[1:]>.bundle` [M]. The repo owner gives its definition, which the scan
stores (`docs/scanner/analysis.md`): the SHA-1 of the size in decimal and the hex SHA-1s of the
first and last 64 KiB; it fits every property above, and is not yet compared with this library.
`open_subtitle_hash` (16 hex) is the OpenSubtitles hash.

### 2.5 Deletion, trash and availability

- With `autoEmptyTrash = true` (the reference's setting), missing items are destroyed in the same
  scan. **[M]**
- With it off, rows are marked instead of destroyed: `deleted_at` on `media_parts`, `media_items`,
  `metadata_items` and `directories` (each indexed, `index_*_on_deleted_at`), parents recomputed with
  `select min(count(*), 1) from media_items where … deleted_at is not null` and
  `update metadata_items set deleted_at=? where id=?`, and restored with
  `update metadata_items set deleted_at=null` if the file comes back. `PUT
  /library/sections/{id}/emptyTrash` then destroys them. **[B]** (not measured: the preference was
  not to be changed)
- **Unavailable locations** [B]: when a location root cannot be reached, its items and directories
  are taken out of the map rather than deleted (`* Taking %zu items out of the map (which has %zu
  items) for being unavailable.`, `* Taking %zu directories out of the map … for being unavailable.`,
  `* Marking media not available in %s`), and `section_locations.available` drops to 0. This
  protects a library on an unmounted disk from being emptied. A *subdirectory* that disappears is
  not "unavailable": its contents are deleted (measured, §1.1).

---

## 3. Classification

### 3.1 Files kept for grouping

**Video extensions.** Kept [M]: `mkv`, `mp4`, `avi`, `ts`, `m2ts`, `webm`, `wmv`. Not kept [M]:
`iso`, `rar`, `IFO`, `VOB` (a `VIDEO_TS` holding `VIDEO_TS.IFO` + `VTS_01_1.VOB` gave `Processing
directory …\VIDEO_TS` with no "Directory had" line and no item). The legacy list [L], minus
`vob`/`bup`, which the native scanner evidently dropped:

```
3g2 3gp asf asx avc avi avs bivx bup divx dv dvr-ms evo fli flv m2t m2ts m2v m4v mkv mov mp4 mpeg
mpg mts nsv nuv ogm ogv tp pva qt rm rmvb sdp svq3 strm ts ty vdr viv vob vp3 wmv wtv xsp xvid webm
```

(The 4-letter entries `divx bivx m2ts mpeg rmvb svq3 strm xvid webm` are visible as strings in the
binary next to the ignore tables [B]; shorter ones are inlined in code.)

**Files dropped** (Filter.py [L], confirmed where marked):

- extension not in the list;
- zero-byte files [M] (`Zero Film (2000).mkv`, 0 bytes, absent);
- unreadable files and broken links [L];
- hidden files, a basename starting with `.` [M];
- `.plexignore` matches [M] (§3.3);
- samples: `[-._]sample` or `sample[-._]` anywhere in the path, case-insensitive, when the file is
  under 300 MB [L]; measured on `Inception.2010.sample.mkv` and `Sample Film (2000)-sample.mkv`
  (absent) [M];
- trailers and extras by name (§3.4): `-trailer.`, `^trailer.?$`, `-deleted.`,
  `-behindthescenes.`, `-interview.`, `-scene.`, `-featurette.`, `-short.`, `-other.`, `^movie-trailer.*`
  [L]; native: `-(?<suffix>[a-z]+)[-_]?(?<index>[0-9]*)$` on the basename without extension [B], so
  `foo-trailer2.mkv` is a trailer too [M].

**Directories never entered** (case-insensitive full match on the directory name [L][B]):

```
everywhere:   @eaDir(:?\s\(copy [0-9]+\))?   .*_UNPACK_.*   .*_FAILED_.*   \..*   lost\+found
              .AppleDouble   .*\.itlp$   @Recycle   .*\.photoslibrary   #recycle   @Recently-Snapshot
              #snapshot[s]?                                    (#snapshot is native only)
at a location root only:
              \$Recycle.Bin   System Volume Information   Temporary Items   Network Trash Folder
below the root (VideoFiles.ignore_dirs):
              \bextras?\b   !?samples?   bonus   .*bonus disc.*   bdmv   video_ts   ^interview.?$
              ^scene.?$   ^trailer.?$   ^deleted.?(scene.?)?$   ^behind.?the.?scenes$   ^featurette.?$
              ^short.?$   ^other.?$
```

Measured: `@eaDir/`, `Extras/`, `.hiddendir/`, `Samples/`, `lost+found/` (never processed); `BDMV/`
(not entered, so a Blu-ray folder gave no item); extras folders such as `Trailers/` are walked by
the local-media step instead (§3.4). `VIDEO_TS` **was** entered on the reference despite the legacy
list, but holds no kept file.

### 3.2 Sidecar subtitles [M]

Found when the item is created or refreshed, for each part, among the files of the part's
directory. Stored as `media_streams` rows (`stream_type_id = 3`, `url = file:///…` percent-encoded,
`index` NULL, attached to the media item and part). Measured on a film with these sidecars:

| File (basename = the part's) | `codec` | `language` | `forced` | `extra_data` |
| --- | --- | --- | --- | --- |
| `X.en.srt` / `X.eng.srt` | srt | `en` | 0 | `ma:format=srt` |
| `X.pt-BR.srt` | srt | `pt-BR` | 0 | |
| `X.French.srt` | srt | *(empty)* | 0 | full language names are not recognised |
| `X.srt` | srt | *(empty)* | 0 | |
| `X.en.cc.srt`, `X.en.hi.srt`, `X.de.sdh.srt` | srt | `en`/`de` | 0 | `ma:hearingImpaired=1` |
| `X.fr.forced.srt` | srt | `fr` | **1** | |
| `X.pl.sdh.forced.srt` | srt | `pl` | 1 | `ma:hearingImpaired=1` |
| `X.es.default.srt` | srt | `es` | 0 | `default` stays 0 |
| `X.ja.vtt` | vtt | `ja` | 0 | `ma:format=vtt` |
| `X.ko.smi` | smi | `ko` | 0 | |
| `Other Name.de.srt` (basename differs) | — | — | — | not attached |
| `X.it.forced.ass`, `X.nl.ssa`, `X.sv.txt` holding SRT text | — | — | — | not attached (content validated?) |

The same rules held for an episode (`Show - S01E01 - Pilot.en.srt`, `.fr.forced.srt`). Legacy rule
[L], the shape to reproduce: subtitle extensions `utf utf8 utf-8 srt smi rt ssa aqt jss ass idx sub txt
psb vtt`; strip a trailing `.forced` / `.normal` / `.default`; language from
`.+\.([^-.]+)(?:-[A-Za-z]{2,4})?$`; a sidecar matches a part when its basename, minus up to two dotted
suffixes, equals the part's basename, and when the directory holds a single video file any sidecar
matches (the native scanner did **not** attach `Other Name.de.srt` beside a single film [M]); `.idx`
needs a sibling `.sub` (`Scanner: found idx file with no matching sub file (%s) - skipping` [B]).

### 3.3 `.plexignore` [M][B][L]

Read from the directory being scanned and from the location root. One pattern per line; blank lines
and `#` comments skipped; `\#` escapes a literal `#`. A pattern **without `/`** is a glob on file
names (fnmatch, case-insensitive): `Ignored*.mkv` → `PlexIgnore: Ignoring file '<path>' because of
rule 'Ignored*.mkv'`. A pattern **with `/`** is a glob relative to the `.plexignore`'s directory and
excludes directories: `IgnSub/*` → `PlexIgnore: Ignoring directory '/zzprobe_movies/Ign
Parent/IgnSub' because of rule 'IgnSub/*'`. Invalid rule: `PlexIgnore: Invalid rule '%s' ~ %s` [B].

### 3.4 Local extras [M]

Found by the agent-refresh step after an item is inserted (`Extras: 0 existing extras.` → `Extras:
Checked extra '<title>' and found matched: 0 Media matched: 0` → `Added new metadata item (<title>)`),
not by the walk. Each extra is its **own `metadata_items` row**, `metadata_type = 12`,
`library_section_id` NULL, `guid` = the file's `file:///` URL, `index` = its rank among the item's
extras, `extra_data` `ex:extraType=<n>`, with a `media_items`/`media_parts` pair whose
`library_section_id`, `section_location_id` and `directory_id` are NULL. It is tied to its owner by
`metadata_relations(metadata_item_id = owner, related_metadata_item_id = extra, relation_type = <same
n>)`, and the owner gets `ex:primaryExtraKey=/library/metadata/<first trailer>` in `extra_data`.

| Form | Type (`relation_type` = `ex:extraType`) | Title |
| --- | --- | --- |
| `Trailers/<f>`, `<x>-trailer[N]`, `trailer.*`, `movie-trailer.*` | 1 trailer | file stem; `<x>` for `<x>-trailer`; `trailer`, `movie` for the last two |
| `Deleted Scenes/`, `<x>-deleted` | 2 deleted scene | |
| `Interviews/`, `<x>-interview` | 3 interview | |
| `Behind The Scenes/`, `<x>-behindthescenes` | 5 behind the scenes | |
| `Scenes/`, `<x>-scene` | 6 scene | |
| `Featurettes/`, `<x>-featurette` | 10 featurette | |
| `Shorts/`, `<x>-short` | 11 short | |
| `Other/`, `<x>-other` | 12 other | |
| anime opening/ending (below) | 13 | `NCOP1` |

`Extras/` is not an extras folder for movies (it is an ignored directory). `Specials/` inside a
film's folder became **a separate movie**, titled `Specials`. Extras are ranked by type code, then
title (`movie` before `t` among trailers). **Folder names are matched loosely** [L]: the name with
non-word characters removed, lower-cased, must *start with* the key (`behindthescenes`,
`deleted`, `featurette`, `interview`, `scene`, `short`, `trailer`, `other`), which is why plurals
work.

**When extras are looked for at all** [M]: only when the film's directory yields **exactly one**
media file. Measured by bisection, a single `baz-sample.mkv` beside the film suppressed every
extra, folders included, though the sample itself is never an item; `trailer.mkv`,
`movie-trailer.mkv`, `foo-trailer2.mkv` and the `-scene/-short/-other/-behindthescenes` suffixes did
not. A `Specials/` subfolder that grouped into another movie also suppressed them. In a folder of
several films (`Multi Flat/`, `flat2/`), suffixed extras were neither items nor extras: they were
dropped. So a film keeps local extras only in a folder of its own. The legacy rule is the same with
a different count (it ignored samples) [L].

Series [M]: an `-trailer` file in `Season 01/` became an extra (type 1) of the **season**; an anime
`… - NCOP1.mkv` at the show's root became an extra of the **show** (type 13), matched by [B]:

```
(?:\b|_|\.)(?<title>(?:NC|(?:Creditless|Clean)[ ._])?(?<type>OP|ED|Opening|Ending|OPED)(?:[ ._]?(?<index>\d{1,2})[A-Za-z]?)?(?:[ ._][A-Za-z])?)(?:v\d)?(?:\b|_|\.)
```

Extras folders under a show (`Extras/` was ignored [M]; `Featurettes/`, `Trailers/`… at show or
season level were not tested).

### 3.5 Local artwork [M][B]

Picked up at the same refresh step (`Local media already exists (metadata://posters/<40 hex>)`) into
`metadata_items.user_thumb_url` / `user_art_url` as `metadata://posters/<id>` / `metadata://art/<id>`
(the id is not the file's SHA-1; it is derived some other way, see §9). Measured: `poster.jpg` and
`fanart.jpg` beside a film → poster and art; `poster.jpg` at a show root → show poster;
`season01-poster.jpg` in `Season 01/` → season poster; `<film>.jpg` + `<film>-fanart.jpg` → poster and
art. Patterns from the binary [B], matched against the file's stem, case-insensitive, extensions
`jpg jpeg png tbn` (+ `webp`?):

```
movie poster  (?<poster>^poster)|(?<default>^default)|(?<cover>^cover)|(?<movie>^movie)|(?<folder>^folder)|<stem>   then -?[0-9]{0,2}$
              (the stem alternative allows (\s?-\s?(cd|disc|disk|dvd|part|pt)\d{1,3})? for stacked films)
movie art     (?<fanart>^fanart)|(?<art>^art)|(?<background>^background)|(?<backdrop>^backdrop)|<stem>-fanart
movie logo    (?<clearlogo>^clearlogo)|(?<logo>^logo)|<stem>-clearlogo
square art    (?<squareart>^squareart)|(?<square>^square)|(?<backgroundsquare>^backgroundsquare)|<stem>-squareart
show poster   ((?<poster>^poster)|(?<default>^default)|(?<cover>^cover)|(?<show>^show)|(?<folder>^folder))-?[0-9]{0,2}$
show art      ((?<fanart>^fanart)|(?<art>^art)|(?<background>^background)|(?<backdrop>^backdrop))-?[0-9]{0,2}$
season poster (?<season>^season)(-|\s)?[0]?%s[a-z]?(-poster)?$          %s = season number; "specials" for 0
season art    (?<season>^season)(-|\s)?[0]?%s-(fanart|art|background|backdrop)-?[a-z]?$
episode thumb <episode stem>(-thumb)?-?[0-9]{0,2}$
```

`banner`, `theme.mp3` (show theme) and `season-specials-poster` come from LocalMedia [L]; the
binary has `banner` and `specials` strings near these tables. Not measured: `useLocalAssets=true`
is the section default.

---

## 4. Grouping: the movie scanner (`Plex Movie`)

### 4.1 Per directory

For each directory with kept files [M]:

1. **TV-looking files are skipped.** A file matching an episode pattern (§5.2) is not a movie:
   `Show.Name.S01E01.mkv` gave nothing in the movie section [M] (legacy: `episode_regexps[0:-3]`, i.e.
   `SxxEyy`, `Sxx-yy`, `NxNN` [L]).
2. **One movie per remaining file**, titled by `CleanName(stem)` (§4.3).
3. **Stacking** joins parts (§4.4).
4. **Folder name rule:** when the directory yields **exactly one** movie, the movie takes the
   directory's cleaned name and year instead of the file's [M]:

   | Layout | Title, year written by the scanner |
   | --- | --- |
   | `Folder Name (2012)/random.release.name.mkv` | `Folder Name`, 2012 |
   | `Nested (1990)/Sub/deeper.file.mkv` | `Sub`, — |
   | `Single In Folder/Totally Different (1999).mkv` | `Single in Folder`, **no year** |
   | `Ign Parent/Kept (2000).mkv` | `Ign Parent` |
   | `KB2/Kill Bill Vol 2 (2004).mkv` | `KB2` |
   | `Two Films/Film A (2001).mkv` + `Film B (2002).mkv` | `Film A` 2001, `Film B` 2002 |

   The folder wins even when it has no year and the file has one (the legacy code did the same:
   `len(cleanName) > 1 or year`) [L]. A folder of one film and its extras counts as one.
5. **Versions are not merged by the scanner.** `Brazil (1985) - 1080p.mkv` + `Brazil (1985) - 720p.mkv`
   (flat folder) and `Versions Film (2011)/… - 1080p.mkv` + `… - 4K.mkv` each gave **two** movies,
   titled `Brazil`/`Brazil` and `Versions Film`/`Versions Film` [M]. On the reference with real agents
   the two would match the same guid and merge into one item with two `media_items` (the Agent Match
   stage); with agent `none` they cannot. So a new klapz scanner must decide versions itself: by
   identical (title, year, edition) in one directory, presumably.
6. **CD subfolders** [M]: `Disc Film (2003)/CD1/a.mkv` + `CD2/b.mkv` → **one** movie `Disc Film`
   2003 with two parts (legacy: more than one subfolder matching `cd[ ]*([0-9]+)`; binary has
   `cd[\s]*([0-9]+)` [B]).
7. **Disc rips are not imported** [M]: `VIDEO_TS/` (IFO + VOB) and `BDMV/STREAM/00001.m2ts` gave
   nothing. The legacy DVD/Blu-ray branch does not survive in the native scanner, or its files
   are filtered out first.
8. **Files at a location root** are flat files, each its own movie [M] (`zzRoot Movie Like (2001).mkv`
   → `ZzRoot Movie Like`).

### 4.2 Hints in names and `.plexmatch`

- **`{edition-…}`** [B]: `[\{\[]edition-(.+?)[\}\]]` extracts the edition; braces are then removed
  with brackets (`\[[^\]]+\]|\{[^}]+\}`). Measured: the title is clean (`Amelie`, `Edition Folder`,
  `Film`) and the two Amelie files stayed **separate** items; `edition_title` was **empty** in the
  scanner's own write and after it (agent `none`). Where the edition is stored before a real agent
  runs is open (§9).
- **`{imdb-tt…}`, `{tmdb-…}`, `{tvdb-…}`** [B]: `^(?<imdb>(?:imdb)?-?(tt[0-9]+))|(?<tmdb>tmdb-?([0-9]+))|(?<tvdb>tvdb-?([0-9]+))$`
  applied to a brace group; they become agent hints (`imdb://tt…`, `tmdb://…`, `tvdb://…`, `Malformed
  GUID`). They are removed from the title [M] (`Alien (1979) {imdb-tt0078748}` → `Alien` 1979;
  `Hinted Probe {tvdb-12345}` → show `ZzHinted Probe`) and wrote nothing to the row with agent
  `none`. **Caution:** with a real agent these hints make the item adopt a real work's guid, and
  per-user state follows guids server-wide (`metadata_item_settings`).
- **`.plexmatch`** [B][M] (the file `.plexmatch` in an item's folder; `PlexMatch` errors: `Missing
  directive separator`, `Unrecognized directive`, `Invalid hint '%s' ~ %s`, `Failed to parse
  episode directive`). Directives `key: value`: `title`, `year`, `guid`, `imdbid`, `tmdbid`,
  `tvdbid`, `show`, `date`, `path`, and episode lines matched by
  `(?<fullEp>(s(?<season>[0-9]+)e|e|sp)?(?<ep>[0-9]+)(-(s(?<secondSeason>[0-9]+)e|e)?((?<secondEp>[0-9]+)))?):\s*(?<path>.+)`
  (e.g. `ep: S02E05: weird name.mkv`), plus a pattern syntax with match tokens (`Invalid match token
  name`, `episode`, `special`, `Pattern did not contain a valid episode match element`). Measured:
  - series: `title: Overridden Show` / `year: 1999` / `ep: S02E05: weird name.mkv` → the scanner
    wrote show `Overridden Show`, year 1999, and `weird name.mkv` as S02E05 (`Added new metadata item
    (Overridden Show)`);
  - movie: `title: Matched Title` / `year: 1977` in a film's folder did **not** change the scanner's
    title (`Plexmatch Folder`): for movies, `.plexmatch` feeds only the agent match.

### 4.3 Title and year: `CleanName` [L], with native differences [M]

The native port keeps the legacy algorithm (`*yearBreak*`, the garbage lists and the year regex are
in the binary [B]). Steps:

1. If more than two tokens of the name are *reversed* garbage tokens (length > 3), reverse the
   whole string.
2. NFKC-normalise, lower-case.
3. **Year**: first match of `([\(\[\.\-])([1-2][0-9]{3})([\.\-\)\]_,+])` with
   `1900 < year < current year + 1`; replace `<before><year><after>` with ` *yearBreak* `. The year
   needs a delimiter **on both sides** among `( [ . -` before and `. - ) ] _ , +` after.
4. Remove bracketed groups: native `\[[^\]]+\]|\{[^}]+\}` (legacy only `[...]`).
5. Remove the suffix `.dvdmedia`; remove audio specs `([^0-9])5\.1[ ]*ch(.)`, `([^0-9])5\.1([^0-9]?)`,
   `([^0-9])7\.1[ ]*ch(.)`, `([^0-9])7\.1([^0-9])`; native also `\b[hx]\.26[45]\b`.
6. Tokenise on `[^ \-_\.\(\)+]+` (separators are space, `-`, `_`, `.`, `(`, `)`, `+`).
7. Mark tokens as garbage, scanning **from the end**, a garbage word counting only the first time
   it is seen from the end. Garbage = `multi multisubs` ∪ misc ∪ format ∪ `se` ∪ sources ∪ video
   extensions:
   ```
   misc    cd1 cd2 1cd 2cd custom internal repack read.nfo readnfo nfofix proper rerip dubbed subbed
           extended unrated xxx nfo dvxa web
   format  ac3 divx fragment limited ogg ogm ntsc pal ps3avchd r1 r3 r5 720i 720p 1080i 1080p remux x264
           xvid vorbis aac dts fs ws 1920x1080 1280x720 h264 h 264 prores uhd 2160p truehd atmos hevc
           (+ h265 in the native list [B])
   sources bdrc bdrip bluray bd brrip hdrip hddvd hddvdrip | cam | ddc dvdrip dvd r1 r3 r5 | retail |
           dsr dsrip hdtv pdtv ppv | stv tvrip | bdscr dvdscr dvdscreener scr screener | svcd | vcd |
           tc telecine | ts telesync | webrip web-dl | wp workprint
   ```
8. Keep tokens left to right while fewer than 2 garbage tokens have been seen; stop at
   `*yearBreak*` (skip it if it is the first token); with ≤ 2 tokens keep all; if nothing is kept, keep
   the first token.
9. Join with spaces and **title-case** (the Python `titlecase` port: small words such as `a`, `in`,
   `the`, `of` lower-cased except first; an all-capitals title left alone).

Measured scanner titles, file name → (title, year):

| File | Title | Year |
| --- | --- | --- |
| `The.Matrix.1999.1080p.BluRay.x264-GRP` | The Matrix | 1999 |
| `Le.Fabuleux.Destin.2001.FRENCH.720p.WEB.H264-XYZ` | Le Fabuleux Destin | 2001 |
| `Star.Wars.Episode.IV.1977.REMASTERED.2160p.UHD.BluRay.x265.HDR` | Star Wars Episode IV | 1977 |
| `Ghost.Movie.2012.DVDRip.XviD-GRP` (`.avi`) | Ghost Movie | 2012 |
| `Movie.2008.720p.WEB-DL.DD5.1.H264` | Movie | 2008 |
| `Title.With.Dots.and.Year.In.Middle.1984.Remake.2003` | Title With Dots and Year in Middle | 1984 |
| `Movie.Title.(2010)` | Movie Title | 2010 |
| `Bracket Movie [2015]` | Bracket Movie | 2015 |
| `Tenet (2020) (2160p)` | Tenet | 2020 |
| `Some Movie (2015) - Extended Edition` | Some Movie | 2015 |
| `[Group] Paprika (2006) [BD 1080p]` | Paprika | 2006 |
| `The Lord of the Rings - The Fellowship of the Ring (2001)` | The Lord of the Rings the Fellowship of the Ring | 2001 |
| `Dash-Title - Part Two (2005)` | Dash Title Part Two | 2005 |
| `Les Misérables (2012)` | Les Misérables | 2012 |
| `Blade Runner 2049 (2017)` | Blade Runner 2049 | 2017 |
| `1917 (2019)` | 1917 | 2019 |
| `the godfather (1972)` | The Godfather | 1972 |
| `UPPER CASE TITLE (1999)` | UPPER CASE TITLE | 1999 |
| `Kill Bill Vol 1 (2003)` | Kill Bill Vol 1 | 2003 |
| `Multi One (2001)` | **One** (`multi` is garbage) | 2001 |
| `Internal.Affairs.1990-INTERNAL` | **Affairs** (legacy intends `Internal Affairs`) | 1990 |
| `Movie Title 2010 1080p` | Movie Title 2010 (a space is no year delimiter) | — |
| `2001.A.Space.Odyssey.1968` | 2001 a Space Odyssey 1968 (no delimiter before `2001` or after `1968`) | — |
| `Future.Film.2030` | Future Film 2030 (year in the future) | — |
| `Old Film (1850)` | Old Film 1850 (≤ 1900) | — |
| `no year at all` | No Year at All | — |
| `Ext Mp4 (2001)` | Ext (`mp4` is garbage) | 2001 |

`title_sort` equals `title` at insertion; `original_title`, `edition_title`, `originally_available_at`
empty.

### 4.4 Stacking [L][M]

Legacy rule (Stack.py): sort by file name; for each adjacent pair whose names differ by **one
replaced character** among `123456789abcdefghijklmn`, take the prefix before it, drop a trailing
`0`, refuse if it ends with `vol`/`volume`; stack when the prefix ends with one of `cd dvd part pt
disk disc scene` (then stripped) or the differing character is followed by `of` (`1 of 2`). The
native scanner has `(\s?-\s?(cd|disc|disk|dvd|part|pt)\d{1,3})?` [B]. Measured:

| Files | Result |
| --- | --- |
| `Heat (1995) - pt1` / `- pt2` | one movie `Heat`, two parts |
| `Stacked Film (2008) cd1` / `cd2` | one movie, two parts |
| `Twin (2001) - 1 of 2` / `- 2 of 2` | one movie, two parts |
| `Movie.Name.2014.Part.1` / `.Part.2` | **two** movies `Movie Name` |
| `Kill Bill Vol 1 (2003)` / `Vol 2 (2004)` | two movies |
| `Letter A (1999)` / `Letter B (1999)` | two movies |
| `CD1/a.mkv` / `CD2/b.mkv` | one movie, two parts (§4.1) |

Parts of a stack share one `media_items` row; `media_parts.index` stayed NULL [M].

---

## 5. Grouping: the series scanner (`Plex TV Series`)

### 5.1 Show, season, episode from the path [M]

Paths are taken **relative to the location root**. Measured rules:

1. **The show is the first path component** under the root, cleaned (`CleanName`, so a trailing
   `(year)` becomes the show's year; `{tvdb-…}` removed). A test tree placed under one extra
   folder (`zzprobe_series/<show>/Season 1/…`) made **every** file an episode of one show named
   `Zzprobe Series`. Title-casing applies: `zzShow Probe (2019)` → `ZzShow Probe`, 2019.
2. **Shows group by cleaned (title, year)**, not by folder: `zzDup Show/` and `zzDup.Show/` became
   one show `ZzDup Show` with episodes 1 and 2; `zzYear Show (2010)/` and `zzYear Show (2015)/`
   stayed two shows.
3. **A file directly at the location root** takes its show from the file name: the part before the
   episode pattern (`zzRoot.Episode.S07E03.mkv` → show `ZzRoot Episode`, S07E03).
4. **Season** = the season in the file name if the pattern has one (it wins over the folder:
   `Season 2/Other Name S05E09.mkv` → S05E09; `Season 3/Weak Probe 1x05.mkv` → S01E05); else
   the **last number in the innermost folder**, by `.*(?:^|[^0-9])(?<season>[0-9]+)` [B]:
   `Season 01` → 1, `Saison 4` → 4, `S04` → 4, `Series 3` → 3, `Season 5 - Name` → 5,
   `Season 1/Disc 2/02.mkv` → **2** (the disc folder wins); else **1** (`Season Two/… - 03` → S01E03; a
   file at the show's root without a season → season 1).
5. **`Specials/`** gave season 0 for `S00E01`; a file without a number in `Specials/` (`Behind
   it.mkv`) produced nothing.
6. Episodes are keyed by (show, season, episode): **two files with the same key are two
   `media_items` of one episode** (versions): `S01E02.mkv` + `S01E02 - 720p.mkv`; and, with a show
   whose files sat in three folders, three versions of S01E01.
7. **Stacking** applies to episodes: `S01E01 cd1` + `cd2` and `s02e02 - part1` + `- part2` → one
   media item with two parts.
8. **Unmatched files are dropped** silently (no row): `Episode 6.mkv` (one digit), `Behind it.mkv`.

### 5.2 File-name patterns [B], in the binary's order

The native list (case-insensitive; the stem is matched after the cleaning of §5.3):

```
episode (SxxEyy family)
 1 (?<show>.*?)s(?<season>[0-9]+)[._ ]*ep?(?<ep>[0-9]+)([- ._]?s(?<secondSeason>[0-9]+))?([- ]?e+p?(?<secondEp>[0-9]+)[- ._])?
 2 (?<show>.*?)s(?<season>[0-9]{2})[-._ ]+(?<ep>[0-9]+)                                    S03-03
 3 (?<show>.*?)([^0-9]|^)(?<season>(19[3-9][0-9]|20[0-5][0-9]|[0-9]{1,2}))x(?<ep>[0-9]+)((-[0-9]+)?x(?<secondEp>[0-9]+))?   3x03, 3x03-3x04, 3x03x04
 4 (.*?)(^|[-._ ])+(?<season>sp)(?<ep>[0-9]{2,3})([-._ ]|v[0-9]+|$)+                        SP01 = S00E01
standalone (folder named like an episode, single file)
 5 (?<show>.*?)(?: \((?<year>[0-9]+)\))?s(?<season>[0-9]+)+ep?(?<ep>[0-9]+)(?:-[0-9]+x(?<secondEp>[0-9]+))?(?: - (?<epTitle>.*))?
 6 (?<show>.*?)(?: \((?<year>[0-9]+)\))? - (?<season>[0-9]+)+x(?<ep>[0-9]+)(?:-[0-9]+x(?<secondEp>[0-9]+))?(?: - (?<epTitle>.*))?
date
 7 (?<year>[0-9]{4})[^0-9a-z()[\]]+(?<month>[0-9]{2})[^0-9a-z()[\]]+(?<day>[0-9]{2})([^0-9]|$)     2009-02-10
 8 (?<month>[0-9]{2})[^0-9a-z()[\]]+(?<day>[0-9]{2})[^0-9a-z()[\]]+(?<year>[0-9]{4})([^0-9a-z]|$) 02-10-2009 (month first)
 9 (?:[.\s-](?<year>[12][0-9]{3})[.\s-](?<month>0?[1-9]|1[012])[.\s-](?<day>[12][0-9]|3[01]|0?[1-9])(?:[.\s-]|$))
episode number only (absolute / no season)
10 (?<ep>[0-9]{1,4})(?:-(?<secondEp>[0-9]{1,4}))?[-._ ]*of[-._ ]*[0-9]{1,4}              15 of 26 (prevents stacking)
11 ^(?<ep>[0-9]{2,4})(?:-(?<secondEp>[0-9]{2,4}))?[^0-9a-f]                              05 - Title
12 ^(?<ep>[0-9])(?:-(?<secondEp>[0-9]{1,2}))?[^0-9a-f]
13 (\b|_)e[a-z]*[-._ ]*(?<ep>[0-9]{2,4})(?:-(?<secondEp>[0-9]{2,4}))?([^0-9c-uw-z%]|$)   ep 14, Episode 14
14 .*?[-._ ](?<ep>[0-9]{2,4})(?:-(?<secondEp>[0-9]{2,4}))?(?:v[0-9]+)?$                   Show - 13v2
15 .*?[-._ ](?<ep>[0-9]{2,4})(?:-(?<secondEp>[0-9]{2,4}))?[^0-9c-uw-z%]+               Show - 12 [1080p]
16 .*[-._ ](?<![0-9]{2}-)(?<ep>[0-9]{2,4})(?:-(?<secondEp>[0-9]{2,4}))?[^0-9c-uw-z%]+
17 .*?[^0-9x](?<ep>[0-9]{2,4})(?:-(?<secondEp>[0-9]{2,4}))?$                              Show707
18 ^(?<ep>[0-9]{1,4})(?:-(?<secondEp>[0-9]{1,4}))?$                                       01
show-name clean-up
   [ ]*[0-9]{1,2}x[0-9]{1,3}$   [ ]*S[0-9]+E[0-9]+$     (an episode stuck to the show name)
```

The legacy fifth episode pattern (`.602.` → S06E02) is **gone**: numbers are never split into
season and episode [M]. Measured results:

| File (folder) | Season | Episode(s) |
| --- | --- | --- |
| `zzShow Probe - S01E01 - Pilot` (`Season 01`) | 1 | 1 |
| `zzShow.Probe.1x04.720p.HDTV.x264` | 1 | 4 |
| `05 - Fifth` (`Season 01`) | 1 | 5 |
| `zzShow Probe 107` (`Season 01`) | 1 | **107** (legacy would give 7) |
| `s02e01` (`Season 2`) | 2 | 1 |
| `zzShow Probe - S03E01` (show root) | 3 | 1 |
| `04x02` (`Saison 4`) | 4 | 2 |
| `Weak.Probe.302` (show root) | 1 | 302 |
| `Weak.Probe.304` (`Season 3`) | 3 | 304 |
| `zzAnime Probe - 105` / `- 12 [1080p]` / `- 13v2` / `ep 14` / `15 of 26` | 1 | 105 / 12 / 13 / 14 / 15 |
| `zzFlat Show 102` (show root) | 1 | 102 |
| `Daily Probe S2021E10` | 2021 | 10 |
| `Daily.Probe.2021-03-04` | **−1** | **−1**, `originally_available_at` = 2021-03-04 00:00 UTC |
| `Daily Probe 05.03.2021` | −1 | −1, 2021-05-03 (month first) |

### 5.3 Before the patterns [B][L]

Legacy order, kept by the native scanner (`SERIES `, `[-._ ()+]+` and the resolution regexes are
next to the episode patterns in the binary [B]):

1. MP4 metadata (legacy: `tvshow`, `tvseason`, `tracknum`/`tvepisodenum`, `title` atoms) when present.
   Not measured; both test sections have `respectTags=1`.
2. Date patterns first, unless a season/episode pattern also matches.
3. Clean the stem: the year (§4.3) replaced by `XXXX`; resolutions removed:
   `(?<=[^[0-9])(480|720|1080|2160)[pPiI]`, `(?<=[^[0-9])([0-9]{3,4}[xX](480|720|1080|2160)[pPiI]?)`,
   codecs `([hHx]\.?26[4,5])(?=[^0-9])`; the show's name, with `[-._ ()+]+` between its words,
   replaced by `SERIES `.
4. Episode patterns 1–4; then the number-only patterns 10–18, whose season is the folder's (§5.1).
5. Nothing matched: the file is dropped.

### 5.4 Multi-episode files [M]

A range creates **one episode row per number**, each with its **own `media_items` row pointing at
the same file**, `display_offset` = `(ep − first) × 100 / count` (0 and 50 for two, 0…98 for 87):

| File | Episodes |
| --- | --- |
| `S01E02-E03` | 2, 3 |
| `S01E04-E05-E06` | 4, 5 (the last `-E06` is lost) |
| `1x07x08` | 7, 8 |
| `S01E11 S01E12` | 11, 12 |
| `S01E09-10` | **9 only** (the second number needs an `e`) |
| `S01E01E02E03` | **1 only** (the second number needs a separator after it) |
| `S01E13-E99` | 13…99: **87 episodes** |
| `S01E20-E15` | 20 only: `WARN - Scanner: episode range for '…' is inverted (epStart: 20, epEnd: 15), assuming this is a single episode` |

A "too large" guard also exists (`… is too large (epStart: %d, epEnd: %d), assuming this is a single
episode` [B]); 87 did not trip it.

---

## 6. What a new item writes, before any agent [M]

From the poller's first version of each row (agent `none`; a real agent then rewrites title, guid,
summary and so on).

**`metadata_items`**

| Column | Movie | Show | Season | Episode | Extra |
| --- | --- | --- | --- | --- | --- |
| `library_section_id` | section | section | section | section | NULL |
| `parent_id` | NULL | NULL | show | season | NULL |
| `metadata_type` | 1 | 2 | 3 | 4 | 12 |
| `guid` | `local://<id>` | `local://<id>` | `local://<id>` | `local://<id>` | `file:///<url-encoded path>` |
| `title`, `title_sort` | `CleanName` | cleaned folder | `''` | `''` (logged `Episode N`) | stem without the suffix |
| `index` | 1 | 1 | season number (0, −1 for dates) | episode number (−1 for dates) | rank among the owner's extras |
| `year` | parsed year | folder year | NULL | the **show's** year | NULL |
| `originally_available_at` | NULL | NULL | NULL | the date for date-based episodes | NULL |
| `added_at`, `created_at` | scan time | scan time | scan time | scan time (date-based: `created_at` = the date) | scan time |
| `updated_at`, `refreshed_at` | NULL until the agent | | | | |
| `hash` | 40 hex, per item | | | | |

The `local://<id>` guid is swapped for `<agent>://<id>` by agent `none` within the second
(`Migrating metadata settings from local://578769 -> tv.plex.agents.none://578769`). Agent `none`
also overwrote movie titles with the file stem, cleared `year`, and set episode
`originally_available_at` and `created_at` from the file time with a time-zone bug
(`at:createdAtTZOffset=3600` in `extra_data`; one hour added per part). None of that is scanner
behaviour.

**`media_items`**: `library_section_id`, `section_location_id`, `metadata_item_id`, `size` (sum of
parts), `display_offset` (multi-episode share), `created_at` = `updated_at` = the file's mtime; the
analysis fields (`duration`, `container`, `media_analysis_version` = 8, `extra_data` with
`ma:videoProfile`…) are filled by the Media Analysis stage moments later.

**`media_parts`**: `media_item_id`, `directory_id`, `file` (absolute, native separators), `size`,
`hash`, `open_subtitle_hash`, `created_at` = `updated_at` = mtime, `index` NULL (stacked parts
too), `duration` and `extra_data` from analysis.

**`directories`**: one per walked directory with media (§2.2).

Order inside a batch [M]: show → season → episode rows, then media item and part, then
`MetadataAgentPostProcessor: adding id … to refresh queue`. Local extras, artwork and sidecar
subtitles are added by that refresh (`Extras:`, `Local media already exists`, `Committing media stream
changes for part …`), as are BIF and chapter-thumbnail jobs and BlurHash/UltraBlur colours.

---

## 7. Notes for the klapz scanner

- Keep the skip rule exactly (count, existence, mtime, size, deleted flag per directory), and keep
  "everything not visited is removed" scoped to the scanned subtree. A partial scan of a vanished
  directory must delete its subtree, and a vanished **location root** must not (§2.5).
- Identify a moved file by size, then content hash, then accept the move only if the regrouped
  "hint name" matches. Otherwise make a new item and carry the per-user state, which in klapz
  is keyed by `guid` (watch state, ratings), so a move must not lose it.
- The folder-name rule for single-film folders and the "first path component is the show" rule are
  the two decisions that most change grouping; both are measured.
- Versions of a film are merged by the reference's agent match, not by its scanner. klapz has
  no agent, so it must merge them itself: same cleaned (title, year, edition) in one directory is the
  conservative choice.
- `force=1` on the reference is a section-wide metadata refresh. Reproducing it as "rescan
  everything" is a choice, not a copy.

## 8. What the experiments left on the reference

Restored: all probe files removed and both sections rescanned. Sections 27 and 28 hold exactly
their original rows again (`metadata_items` 577986–578173, `media_items` 5566046/5566048/5566505,
`media_parts` 5571261/5571263/5571720: same content as before the final scans; 0 probe parts, 0
probe streams, 0 relations, 0 directory rows other than the two roots). No `metadata_item_settings`
or `metadata_item_views` row references a probe item, and no probe row ever held a guid other than
`local://`, `tv.plex.agents.none://` or `file://`.

Not restored:

1. **14 orphaned extras** (`metadata_type = 12`, `library_section_id` NULL, ids 578706, 578768,
   578782, 578972, 578975, 578985, 578988, 578991, 578995, 578998, 579001, 579004, 579063, 579064),
   each with a media item and part pointing at a deleted probe file. When their owner was destroyed
   the reference removed the relations and the non-primary extras but **kept each owner's first
   extra**. 8 such orphans already existed in the database before these experiments, so the
   reference leaves them behind routinely.
2. **Episode 578173 and the `Bull` rows of section 28.** A `force=1&path=…` on section 28 refreshed
   the whole section with agent `none`: 578173's `title` went from `Episode 1` to `''` and its
   `originally_available_at` (1619787526) to NULL; `refreshed_at` of 577987, 577988, 577990 and 578173
   is now 1794910455.
3. `directories.updated_at` of both roots (11279, 11280) and `section_locations.scanned_at`, and
   `library_sections.scanned_at`: new times (the test folder's mtime changed).
4. Autoincrement counters advanced (metadata ids up to 579067); `[LibraryTimeline]` entries;
   bundles under `Media/localhost/` (index files, thumbnails) for the probe parts, normally cleared by
   the butler's *clean old bundles* task; ten-odd MB of log.

Item 577986 was changed during this session by another agent's `/unmatch` (log `PUT
/library/metadata/577986/unmatch` at 10:59:58), not by these experiments.

## 9. Open questions

1. **`media_parts.hash`**: content- and size-dependent, path- and mtime-independent, not the whole
   file's SHA-1 nor SHA-1/MD5 of its first/last 4–64 KiB. Needed only to reproduce move detection
   byte for byte; any strong content hash serves the same purpose.
2. The ids in `metadata://posters/<40 hex>` are neither the image's SHA-1 nor its path's.
3. Where the edition (`{edition-…}`) is stored at insertion: `edition_title` stayed empty with agent
   `none`, though the scanner parsed it out of the title and kept the two Amelie files apart.
4. Version merging under a real agent (two files, same guid → one item, two media items) was not
   measured, and could not be without matching test files to real works.
5. Why `.ass`, `.ssa` and `.txt` sidecars were not attached: a properly formed `.ass` file was not
   re-tested after the folder that held it lost its extras check.
6. `.plexmatch` pattern syntax (match tokens, `special`, validity ranges) is known only from its error
   messages.
7. Behaviour with `autoEmptyTrash=false`, unavailable locations, filesystem watching and partial
   scans on change: from the binary only; the preferences were not to be changed.
8. The "too large" episode-range threshold (87 was accepted).
9. Local extras under shows and seasons (`Featurettes/`, `Trailers/` at those levels), and local
   artwork with `useLocalAssets=false`.
10. Whether MP4 metadata atoms (`respectTags`) feed the native series scanner as they did the legacy
    one.
