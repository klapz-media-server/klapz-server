# Jellyfin: measured behaviors and deliberate divergences

The Jellyfin surface answers as the reference Jellyfin (12.4.0) does, measured on a test instance
holding the same 35 files as an open-plex library (`Z:\samples\Movies` and `Z:\samples\TVShows`).
Captures come from `tools/capture_jellyfin.py`, which also writes the scrubbed fixtures of
`crates/api-jellyfin/tests/fixtures`; `tools/compare_jellyfin.py` sets our answers against the
reference's, capture by capture and track by track. The operations served are tracked in
`ENDPOINTS.md`.

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

## Users and devices

- **A Jellyfin extension, per user.** An owner switches it on (`connectExtension(JELLYFIN)`, or
  the switch at `/admin`). Jellyfin's clients sign in with the user's own password
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
  scan or the import gave), no favorites (the list is empty; adding one is refused `501` rather
  than accepted and lost), no external links, no trailers, theme media or intros.
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
- **The series library's grouping comes from the scan.** On the reference's test files, the scan
  takes the folders below the root (`Multi`, `VF`, `VO`) for series names, where Jellyfin finds the
  series one level further down. A library rooted at each of those folders reads as Jellyfin's.

## Tracks

The labels and values below were read off the reference's 35 files and are reproduced
(`track.rs`):

- `DisplayTitle`: a video's size class, codec and range (`480p H264 SDR`); an audio's language,
  profile or codec, layout and default flag (`English - OPUS - Stereo - Default`); a subtitle's
  language, hearing-impaired, default and forced flags and codec. A track named in its file keeps
  its name first, followed by the attributes its name does not already hold (`Stereo - French -
  AAC - Default`; `aac 5.1 - Default`). Size classes are the reference's: the smallest of 144p,
  240p, 360p, 384p, 404p, 480p, 540p, 576p, 720p, 1080p, 1440p, 4K, 8K that holds both sides,
  `i` for interlaced (`384x284` is `360p`, `704x396` is `404p`); a coded height of 1088 is 1080p.
- Codecs as ffprobe names them, with DTS as `dts`, PGS as `PGSSUB`, DVB as `DVBSUB` and
  `DVBTXT`; AC-3 and E-AC-3 labelled `Dolby Digital` and `Dolby Digital+`; profiles as ffprobe
  writes them (`LC`, `HE-AAC`, `DTS-HD MA`, `Main 10`), `LC` left out of labels.
- Languages as ISO 639-2 terminological codes (`fra`, `deu`), English names in labels, an unknown
  code capitalized (`Qaa`).
- Text subtitles and PGS images are extractable (`SupportsExternalStream`); DVB images are not.
- **Not reproduced**: `TimeBase`, `NalLengthSize`, `CodecTag`, the Dolby Vision flags beyond the
  profile, and the reference's frame rates to five decimals (ours keep the three the scan
  stores). `RefFrames` and bitrates are the scan's measurements, which differ from the
  reference's (`RefFrames` 4 where it says 1; a bitrate a few hundred bits apart).
  `AspectRatio` is the picture's own, where the reference reads the container's display ratio
  (`4:3` for an anamorphic `720x576`, `1.25:1` here).

## Playing

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

## Measured with Jellyfin Web

Jellyfin Web 12.4, served by open-plex at `/jellyfin/web` from the reference's install
(`OPENPLEX_JELLYFIN_WEB_DIR`), finds the server, signs in, shows both libraries and their latest
items, opens a film with its tracks labelled, and plays `doodlebug` directly (720×480, H.264 and
Opus in Matroska) and *Jurassic World: Fallen Kingdom* through an HLS conversion (its DTS tracks
converted), reporting progress back. The other clients (Swiftfin, Findroid, Infuse, the Android
and TV apps) were not tried.
