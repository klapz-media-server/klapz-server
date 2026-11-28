# Jellyfin: measured behaviors and deliberate divergences

The Jellyfin surface answers as the reference Jellyfin (12.4.0) does, measured on a test instance
holding the same 35 files as an klapz library (`Z:\samples\Movies` and `Z:\samples\TVShows`).
Captures come from `tools/capture_jellyfin.py`, which also writes the scrubbed fixtures of
`crates/api-jellyfin/tests/fixtures`; `tools/compare_jellyfin.py` sets our answers against the
reference's file by file and field by field (below, *Measured on the 35 files*). The operations
served are tracked in `ENDPOINTS.md`.

## Where the surface lives, and who reaches it

- **Below `/jellyfin`.** Jellyfin routes without regard to case: `/users/authenticatebyname`
  reaches `/Users/AuthenticateByName`, `/library/…` would reach Jellyfin's `/Library/…` and
  Plex's `/library/…` alike. A base path, which every Jellyfin client accepts as part of a server's
  address (Jellyfin's own "base URL" setting), keeps the two surfaces apart. Clients are given
  `http://host:port/jellyfin`. Paths below it are lowered before routing, measured on the
  reference: `/SYSTEM/INFO/PUBLIC` answers.
- **Jellyfin tokens only.** Below `/jellyfin` the server's authentication resolves a Jellyfin
  token and nothing else: not a Plex token, not a session cookie or bearer. A browser signed in
  at `/admin` carries its cookie to any page of the same site, and the Jellyfin surface does not
  honor it. A user reaches the surface through an enabled Jellyfin extension, or not at all.
- **Where the token travels.** The reference (12.4) reads it from
  `Authorization: MediaBrowser … Token="…"` and from an `ApiKey` query parameter, and refuses
  `X-Emby-Token`, `X-MediaBrowser-Token`, `X-Emby-Authorization`, the `Emby` scheme and
  `api_key` (`401`): its legacy authorization is off by default since 10.11. We accept them, as
  10.10 and earlier did, because older clients send nothing else.
- **Refusals.** A request without a credential, or with one that opens nothing, is a bare `401`
  (no body, no content type), as on the reference. A sign-in without the `Authorization`
  header naming the client is a `400`, as on the reference. A wrong password, an account waiting
  for an owner, a user with no Jellyfin extension or with it switched off: all a bare `401`, so
  that a stranger learns nothing of the account (the official API tells them apart, after the
  right password only).
- **Public paths**: `/System/Info/Public`, `/System/Ping`, `/Users/AuthenticateByName`,
  `/Users/Public`, `/Branding/*`, `/QuickConnect/Enabled`, and Jellyfin Web under `/web`. The
  reference also answers `/Users/Public` without a token; it lists no user (they are hidden by
  default), nor do we.
- **The base redirects to Jellyfin Web.** `/jellyfin` and `/jellyfin/` answer `302` to
  `/jellyfin/web/` without a token, as a reference with a base URL does
  (`BaseUrlRedirectionMiddleware`; the measured reference, without one, answers `302` to `web/`
  at `/`). Some clients open the address they were given first and give up on a `401`. Paths
  outside the base are not redirected, as the reference would: they are the Plex surface's.

## Users and devices

- **A Jellyfin extension, per user.** The user switches it on and off itself, or an owner does
  (`connectExtension(JELLYFIN)`, `disableExtension`). A user without a password, as an owner
  creates one, is given one first (`setPassword`, by an owner or by the user itself). The devices signed in are listed on
  `UserExtension.devices` and forgotten one by one with `forgetExtensionDevice`. Jellyfin's clients sign in with the user's own password
  (`signInThroughExtension`), and each device keeps a token of its own (`extension_device`):
  signing in again from a device replaces its token; signing out forgets it; switching the
  extension off or deleting the user forgets every device. The connection's own token, which
  `connectExtension` returns, works as a Jellyfin API key.
- **One server for every user.** Jellyfin clients read `/System/Info/Public` before anyone signs
  in, and no directory tells them apart as plex.tv does for Plex: every Jellyfin user sees the
  same server id, derived from the machine identifier (`ServerInfo::of`), 32 hexadecimal digits.
- **No administrator.** Every user's `Policy.IsAdministrator` is `false`, the owners' too: the
  server is managed at `/admin`, and Jellyfin Web shows an administrator dashboards over machinery
  this server does not have (as Plex's server settings are refused).
- **Not kept**: last sign-in and activity dates, display preferences (the reference's defaults
  are answered, a change is accepted and forgotten), session capabilities.

## Identifiers

Jellyfin ids are GUIDs, 32 hexadecimal digits, read with or without dashes and in either case.
Ours carry the official API's global id: a type number, four zero bytes, the key on 64 bits
(`id.rs`). A GUID that is none of ours is a `404`, as an unknown id is on the reference; what is
no GUID is a `400`. An item's first media source carries the item's own id, as on the reference:
Jellyfin Web looks the item up again by the id of the source it plays. Other versions go by their
first file's id.

## Items

- **What the server does not hold is not invented.** No artwork (empty `ImageTags`, image routes
  `404`), no metadata from online providers (no overview, rating, premiere date… beyond what the
  scan or the import gave), no external links, no trailers, theme media or intros.
- **Favorites are the official API's** (`setFavorite`, `Item.isFavorite`,
  `ItemFilterInput.favorite`), kept per user and per work as a rating is: every copy of a film is
  one at once. `POST` and `DELETE /UserFavoriteItems/{id}` (and `/Users/{userId}/FavoriteItems/{id}`)
  answer the item's `UserItemDataDto`; `Filters=IsFavorite` or `IsFavorite=true` narrows a listing.
  Only a library's items are made favorites: a person, a genre, a collection or a playlist is
  refused `501`, and their favorites list nothing. A favorites filter over items asked by id,
  below an item or found by a search, none of which the official API filters, answers `501`.
- **Fields.** The reference writes a base set on a listed item and the rest when `Fields` names
  them; a lookup has them all. Same here, field for field on the fixtures but for the above.
- **Listings.** `/Items` under a library, a series or a season, by `Ids`, by `SearchTerm`, or
  across the caller's libraries. The filters the official API expresses are honored (types,
  recursion, played, resumable, years as a range, genres, a title word); the others the reference
  honors (`PersonIds`, `NameStartsWith`, `Is4K`, ratings…) answer `501`, because answering
  without them would hand the client the whole library as the result. One sort key: the first,
  ties broken by title. A page holds at most 500 items. Music, photos and box sets are held
  nowhere: asked for alone, they list nothing.
- **Dates** are written as .NET writes them (`2018-06-06T00:00:00.0000000Z`). A release date is its
  day's midnight UTC; the reference stores the local midnight of its own time zone, so its dates
  read a day early in UTC (`2018-06-05T22:00:00Z` for a server in Paris).
- **Grouping comes from the scan, which follows Plex's measured naming rules.** The reference's
  series library has three roots, `TVShows\Multi`, `TVShows\VF` and `TVShows\VO`
  (`/Library/VirtualFolders`), and its film library one, `Movies`; ours are made alike
  (`library add --kind show --language en-US --title Séries …\Multi …\VF …\VO`; the
  reference's series library names no metadata language, which its server reads as English).
  What still differs is the rules', and is not bent to Jellyfin's:
  - a folder with no media in it is a series on the reference (ten of them here), and nothing
    in our scan;
  - a series found under two roots (`Daria`, `Steven Universe`, `The Simpsons`) is one series
    here, two there;
  - a film alone in its folder is named after the folder (`doodlebug` is `VO`, `Le mal aimé`
    `VF`), and a date in a name is read as the year (`Peur primale … (Arte - 13-09-2021)` is
    2021), as Plex names them; the reference names by the file;
  - the reference keeps the folders as items: a film's `ParentId` is its folder, and a season with
    a folder of its own (`Saison 01`) is a `FileSystem` item, `Virtual` here.
- **Names.** An episode or a film with no title is named after its file, as the reference names
  what no provider matched; a season without one `Season n`, season 0 `Specials`. `SortName` is
  the reference's: an episode's `001 - 0021 - name`, a season's number on four digits, anything
  else its name in lower case without articles or `,&-{}'`, `.+%` as spaces, numbers on ten
  digits.

## Tracks

A track is published from what ffprobe printed of it, which the scan keeps beside the values it
stores the reference Plex's way, and the official API publishes as `MediaStream.stated`,
`VideoAnalysis.averageFrameRate` and its neighbours, `MediaFile.stated` and
`MediaFile.sideStreams` (migration `m0040`). An imported file states none of it, and is shaped
from Plex's analysis as before. The rules are the reference's own (`ProbeResultNormalizer`,
`MediaStream`, `MediaStreamSelector`), each checked on the 35 files (`track.rs`):

- **Language**: as the file states it, in its terminological code (`fre`, `fr-FR` are `fra`), an
  unknown one as written (`qaa`, `fra,fra`), none when the file states none. A Matroska track
  declaring `und` has none, as ffmpeg-plex reports it since v8.1.3-21 (the reference's wrote
  `eng`, which Plex's analysis still reads), a track without a `Language` element being English
  by the format's default.
- **Title**: the track's name, else for sound and subtitles an MP4's `name`, else its handler's
  unless it is the muxer's (`SoundHandler`); `cc` is no title.
- **Codec, profile, tag, time base, framing**: ffprobe's names, picture subtitles as `PGSSUB`,
  `DVDSUB`, `DVBSUB`, `DVBTXT`, PCM by its sample format with no profile; `CodecTag` unless
  ffprobe's is zeros; `TimeBase`; H.264's `IsAVC` and `NalLengthSize`, as a string.
- **Rates**: `BitRate` is `MediaStream.estimatedBitrate`, the reference's rule moved to
  `core::bitrate`: the header's rate, else the muxer's tags (`BPS`, `BPS-eng`, bytes over
  duration; not a subtitle's), else a rate typical of an audio codec and its channels, and a
  file's one video what the container's declared rate leaves. Frame rates are 32-bit floats of
  ffprobe's ratios (`23.976025`), the average left out when ffprobe has none; `Level` is `-99`
  where ffprobe knows none.
- **Picture**: `AspectRatio` is the stated display ratio snapped to the reference's named ones
  (`16:9`, `4:3`, `1.85:1`, `2.40:1`…), else as stated; `IsAnamorphic` from the stated ratios,
  1% off square, never for a cropped picture; the pixel format as ffprobe names it; a picture
  subtitle's canvas as `Width` and `Height`; Dolby Vision's record in full, and the reference's
  `VideoRangeType` and `VideoDoViTitle` (an out-of-spec record is `DOVIInvalid`).
- **Labels**: `DisplayTitle` as the reference composes it per kind, `Undefined` for a subtitle of
  no language, `LocalizedLanguage` .NET's English name (`Undetermined` for `und`).
- **Selection**: for a user who set no preference, which every user is here, subtitles the
  default mode would play carry the reference's `Score`, and a media source names the default
  audio and subtitle by the reference's rule (`DefaultAudioStreamIndex`,
  `DefaultSubtitleStreamIndex`).
- **What else a file carries**: covers are `EmbeddedImage` streams and, with Matroska's attached
  files, `MediaAttachments`; data tracks are `Data` streams.

## Media sources and items

- `Container` on an item is the reference's normalization of ffprobe's format names
  (`mov,mp4,m4a,3gp,3g2,mj2`, `mkv,webm` when every stream may be WebM's, `ts`); a media source
  takes the one its file's extension names, else the first. A file played as it is goes by the
  name the device profile lists: a Matroska file of WebM's codecs is `webm` to a browser, and
  read as it is, which `MediaFile.readableAsIs` now admits (`core::stream::container_names`).
- `RunTimeTicks` and the source's `Bitrate` are what the container declares, read as .NET reads
  them (`178.301` s is 1783009999 ticks); `Timestamp` is a transport stream's (`Valid` for
  Blu-ray's 192-byte packets).
- `Chapters`: the file's names as written, a name that reads as a time or none numbered in the
  library's language (`Chapitre 3` in the French film library, `Chapter 3` otherwise), starts to
  the millisecond. The scan keeps both these and Plex's reading (six significant digits, numbered
  titles cleared), the first as `Chapter.stated`.
- `IsHD` for a picture 720 lines tall at least; `DisplayPreferencesId` the reference's per type
  (the MD5 of its own type's name).

## Upcoming episodes

`/Shows/Upcoming` lists the episodes released from the start of yesterday (UTC) on, the earliest
first and then by title, as the reference's rule reads; the reference's own library has none to
measure against (`{"Items":[],"TotalRecordCount":0,"StartIndex":0}`, which we answer alike). It
looks across the caller's series libraries, or below the library `ParentId` names, through
`Library.items` with `releasedFrom`; below a series or a season it answers `501`. Its other
parameters are ignored there too. `TotalRecordCount` counts the episodes returned, not those
there are, as the reference counts it. `MinPremiereDate` on `/Items` is that same lower bound,
RFC 3339, without an offset (read as UTC) or a day alone.

## Filters

`/Items/Filters` and `/Items/Filters2` list what a library's items hold, or every library of the
caller's: genres (with their ids), release years, content ratings, and the audio and subtitle
languages tracks state, named as the reference names them (`French (fre)`, `qaa`), each language
once as written (`fre` and `fra` apart, through `TrackField.STATED_AUDIO_LANGUAGE` on the official API, beside `Library.trackLanguages`, where they are one `fr`).
Tags are keywords a provider gives: none. A kind of item below the library's own (`IncludeItemTypes=Episode` in a series library)
narrows the values to those items, through the official API's `filter`; below a library, several
such kinds at once, or the Live TV flags of `Filters2`, answer `501`; music, photos and box sets,
held nowhere as items, list nothing.

## Playlists

The caller's own playlists, over the official API's (`createPlaylist`, `addToPlaylist`,
`movePlaylistEntry`, `removeFromPlaylist`, `clearPlaylist`, `updatePlaylist`,
`deletePlaylist`), as measured on a playlist made on the reference for the capture
(`tools/capture_jellyfin.py --writes`):

- a playlist is a `Playlist` folder item, listed by `/Items?IncludeItemTypes=Playlist` and in a
  `Playlists` view (`CollectionType` `playlists`) the reference adds among the libraries, by name,
  once the user has one; opened by `/Playlists/{id}/Items` or `/Items?ParentId=`, counting what is
  left to watch;
- each entry carries a `PlaylistItemId` of its own, which `Move/{index}` and `entryIds` name;
  the reference's is the item's id, which cannot tell two entries of one item apart;
- a playlist is its owner's alone, as on the official API: `OpenAccess` is `false`, `Shares`
  empty, and sharing (`Users`, `IsPublic`, `/Users/{userId}`) answers `501`; another user's is
  not found;
- items are appended: a `position` before the end answers `501`. `DELETE /Items/{id}` deletes a
  playlist, and never a media file (`501`).

## Collections

Box sets are the official API's collections (`createCollection`, `addToCollection`,
`removeFromCollection`, `deleteCollection`, `Item.collections`), as measured on a collection
made on the reference for the capture. A collection here belongs to a library: one made from a
Jellyfin client goes to the library of its first item, and holds that item's kind; made with no
item, in a folder of its own (`ParentId`) or locked (`IsLocked`), it answers `501`. An item of
another library or kind is refused `400`. Collections are listed by type
(`IncludeItemTypes=BoxSet`, across the caller's libraries or one), in a `Collections` view
(`CollectionType` `boxsets`) among the libraries once there is one, by `/Items/{id}/Collections`,
and opened as a parent; changed by a library's administrators and the owners, as on the
official API. Deleting one keeps its items.

## Genres, people, studios, years and search

Pages of what items share, from the official API (`Library.tags`, `searchTags`, `tags`,
`Library.fieldValues`, `Library.years`, `search`), shaped as the reference's genres, people,
studios and years; their artwork is not held.

- `/Genres` lists a library's genres, or those of every library of the caller's holding the
  types asked, by name, counted per kind with `Fields=ItemCounts`; `searchTerm` completes a word
  of the name, as the official API completes one. `/Genres/{name}` finds one by name.
- `/Persons` lists the people a term finds, or the people of one role (`personTypes`) by name;
  every role at once with no term answers `501`, more people than a page of credits lists.
  Favorite people (`Filters=IsFavorite`, as Jellyfin Web's favorites page asks) list nothing:
  only items are made favorites.
  `/Items?PersonIds=` lists one person's films and series through `Person.items`, a series for
  a credit on its episodes, with the request's other filters; several people at once, or another
  order than by name, answer `501`.
- A studio and a year are no entity of the official API: their ids carry the value, a year its
  number and a studio a hash of its name, found again among the studios listed. `StudioIds`
  on `/Items` answers `501`.
- `/Items?SearchTerm=` takes several types at once, as Jellyfin Web's search page asks for
  every one in a single request: the items the official API's search ranks, then the collections
  (`collections(matching:)`) and the caller's playlists (`playlists(matching:)`) whose title the
  term finds, by the same word-prefix rule. Below a parent, several types at once answer `501`.
- `/Search/Hints` hints the items the official API's search ranks, then the people a term finds
  (`includePeople`); a page past the first, and the Live TV and media filters, answer `501`.

## Segments

`/MediaSegments/{itemId}` lists the work's markers of a film or an episode (`Movie.markers`,
`Episode.markers`) as Jellyfin's segment types: an intro is an `Intro`, credits an `Outro`, a
commercial a `Commercial`, each with the marker's id; a bookmark, a viewer's own place, is none,
nor are the credits the reference library keeps ending before they start.
`includeSegmentTypes` narrows them. The reference detects none on its test files (no plugin
installed), and answers the empty list, as this server does for a file no marker was imported
for: nothing here detects them yet.

## Measured on the 35 files

`tools/compare_jellyfin.py` compares, for every file, the lookup (`/Items/{id}` and
`/Users/{user}/Items/{id}`), `PlaybackInfo` with a browser's device profile and the file's entry in
its series' episodes, then every series and season both servers hold, field by field, ids
rewritten into what they designate. On the last run: **26,379 fields identical, none unexplained**,
and 15,490 differing for the reasons below, each named in the tool:

- **metadata**: what an online provider gave the reference (overviews, people, genres, ratings,
  dates, provider ids, a matched title and its sort name, a series' running time); nothing here
  consults one.
- **artwork**: images, their tags and blur hashes; none is held.
- **opaque**, **dates**, **rights**: entity tags and keys, when an item was added, whether the
  user may delete it.
- **roots**: a mapped drive is stored as the share it maps (`\\nas\samples\…` for
  `Z:\samples\…`): library roots are canonical (`library add`).
- **grouping**, **naming**: the scan's rules, above.
- **ref-frames**: the reference's prober states one reference frame for every video, its codec
  context's default; ours is H.264's sequence header (2 to 16 here), none for other codecs.
- **conversion**: a conversion's address is the binding's own, with its own parameters.
- **delivery**: subtitles and attached files are not delivered to clients (`DeliveryMethod`,
  `DeliveryUrl`): no address is published that would not answer.
- **decoded**: what the reference's prober reads by decoding up to the first video frame
  (jellyfin-ffmpeg's `-only_first_vframe`, which ffmpeg-plex lacks): the rates of AAC in a
  transport stream (`Peur primale`, `S01E41`) and the video's that follows from them, the canvas
  of two picture subtitles ffprobe-plex states none for, PCM's 16 bits, and HDR10+ metadata in
  the first frame of `Star Trek: Discovery` (`DOVIWithHDR10Plus` there, `DOVIWithHDR10` here).

`crates/server/tests/jellyfin_samples.rs` replays the same on every run of the tests, without a
server: each sample keeps ffmpeg-plex's ffprobe output and the reference's answer
(`tools/capture_jellyfin_samples.py`), and the analysis read from it goes through the scan's storage, the
official API and the binding to the reference's media source and file-derived fields, these
reasons excepted.

## Playing

- **A file a scan found is read first.** A scan lists a file before reading it
  (`docs/plex/scanner/analysis.md`, §1); `PlaybackInfo` has the files it may answer with read through
  `readMediaFile` before deciding, and a stream's start does the same through `startStream`. A
  file that cannot be read yet is offered no direct play: `MediaFile.readableAsIs` is false for
  it. Not measured on the reference, which probes a file within its scan.
- **The decision is the official API's.** `PlaybackInfo` reads the device profile's video formats
  and asks `MediaFile.readableAsIs` whether the player reads the file as it is: one format holds
  its container, its video codec and the codec of the audio it would play, and the version weighs
  no more than `MaxStreamingBitrate`. The rule is in `core::stream::reads_as_is`.
- **Read as it is**: `SupportsDirectPlay` and `SupportsDirectStream`, the second being how a client
  that cannot open the file's path — any browser — reads it: `/Videos/{id}/stream?static=true`,
  which starts a direct-play stream through `startStream` and answers `307` to the stream's own
  address. A remux into the container the URL names (`stream.mp4` without `static`) is not built:
  `501`.
- **Converted**: a `TranscodingUrl` of the binding's own, `/videos/{id}/master.m3u8?…`, relative
  to the base as the reference writes it, carrying the play session, the audio index, the
  bitrate and the token. Reached, it starts an HLS conversion through `startStream` (keyed by the
  play session) and answers `307` to the stream's master playlist, as Plex's `start.m3u8` does.
  The reference's conversion parameters (codecs, segment container) are its own; ours are the
  engine's.
- **Numbers as strings.** Jellyfin Web sends `AudioStreamIndex` as `"1"`; ASP.NET binds it as a
  number, and so do we.
- **Reports**: `/Sessions/Playing`, `/Progress` and `/Stopped` go to `reportViewing` (measured
  against the item's own runtime, since Jellyfin clients send none) and `reportPlayback`; `/Ping`
  to `touchPlaybackSession`. A position past a minute is kept; crossing 90% counts a viewing.
- **Live streams are never opened.** A media source says `RequiresOpening: false`, so no client
  holds an `OpenToken`. `/LiveStreams/Open` answers as the reference does for a request no
  provider of its takes: `400` without a token (query or body), `500` with one, both its
  middleware's `Error processing request.` as `text/plain`. `/LiveStreams/Close` answers `204`
  for any id, as the reference does for one it does not know, and `400` without one.

## Measured with Jellyfin Web

Jellyfin Web 12.4, served by klapz at `/jellyfin/web` from the reference's install
(`KLAPZ_JELLYFIN_WEB_DIR`), finds the server, signs in, shows both libraries and their latest
items, opens a film with its tracks labelled, and plays `doodlebug` directly (720×480, H.264 and
Opus in Matroska) and *Jurassic World: Fallen Kingdom* through an HLS conversion (its DTS tracks
converted), reporting progress back. The other clients (Swiftfin, Findroid, Infuse, the Android
and TV apps) were not tried.
