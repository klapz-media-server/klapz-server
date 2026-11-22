# Jellyfin endpoints

Tracker for the 364 operations of the reference Jellyfin's OpenAPI document (`docs/jellyfin/openapi.json`, 12.4.0), served below `/jellyfin`.

**Generated** — `python tools/gen_jellyfin_endpoints.py`. A status is derived from the routes `crates/api-jellyfin` declares: `served` has a handler, `todo` answers the surface's `404`. Notes say what a served operation leaves out; the measured differences are in `docs/jellyfin/spec-gaps.md`.

## Progress

| Status | Operations | Share |
| --- | --: | --: |
| `served` | 85 | 23.4 % |
| `todo` | 279 | 76.6 % |

## By domain

| Domain | Operations | `served` |
| --- | --: | --: |
| Artist | 3 | 0 |
| Audio | 6 | 0 |
| Authentication | 13 | 2 |
| Backup | 4 | 0 |
| Branding | 3 | 3 |
| Channel | 5 | 0 |
| Collection | 3 | 3 |
| Device | 5 | 0 |
| DisplayPreference | 2 | 2 |
| Environment | 5 | 0 |
| Filter | 2 | 2 |
| Genre | 2 | 2 |
| Image | 37 | 5 |
| InstantMix | 7 | 0 |
| ItemLookup | 11 | 0 |
| ItemUpdate | 3 | 0 |
| Library | 34 | 13 |
| LibraryStructure | 8 | 0 |
| LiveTv | 38 | 0 |
| Localization | 4 | 0 |
| Lyric | 6 | 0 |
| MediaInfo | 5 | 5 |
| MediaSegment | 1 | 1 |
| Movie | 1 | 0 |
| MusicGenre | 1 | 0 |
| Person | 2 | 2 |
| Playlist | 11 | 11 |
| Plugin | 17 | 0 |
| RemoteImage | 3 | 0 |
| ScheduledTask | 5 | 0 |
| Search | 1 | 1 |
| Session | 18 | 7 |
| Show | 4 | 4 |
| Startup | 7 | 0 |
| Studio | 2 | 2 |
| Subtitle | 10 | 0 |
| Suggestion | 1 | 0 |
| SyncPlay | 22 | 1 |
| System | 19 | 5 |
| Trailer | 1 | 0 |
| TrickPlay | 2 | 0 |
| User | 10 | 3 |
| UserData | 8 | 4 |
| UserView | 2 | 1 |
| Video | 8 | 4 |
| Year | 2 | 2 |

## Artist

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Artists` | `todo` |  |
| GET | `/Artists/AlbumArtists` | `todo` |  |
| GET | `/Artists/{name}` | `todo` |  |

## Audio

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Audio/{itemId}/stream` | `todo` |  |
| HEAD | `/Audio/{itemId}/stream` | `todo` |  |
| GET | `/Audio/{itemId}/stream.{container}` | `todo` |  |
| HEAD | `/Audio/{itemId}/stream.{container}` | `todo` |  |
| GET | `/Audio/{itemId}/universal` | `todo` |  |
| HEAD | `/Audio/{itemId}/universal` | `todo` |  |

## Authentication

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Auth/Keys` | `todo` |  |
| POST | `/Auth/Keys` | `todo` |  |
| DELETE | `/Auth/Keys/{key}` | `todo` |  |
| GET | `/Auth/PasswordResetProviders` | `todo` |  |
| GET | `/Auth/Providers` | `todo` |  |
| POST | `/QuickConnect/Authorize` | `todo` |  |
| GET | `/QuickConnect/Connect` | `todo` |  |
| GET | `/QuickConnect/Enabled` | `served` | false: Quick Connect is not built. |
| POST | `/QuickConnect/Initiate` | `todo` |  |
| POST | `/Users/AuthenticateByName` | `served` |  |
| POST | `/Users/AuthenticateWithQuickConnect` | `todo` |  |
| POST | `/Users/ForgotPassword` | `todo` |  |
| POST | `/Users/ForgotPassword/Pin` | `todo` |  |

## Backup

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Backup` | `todo` |  |
| POST | `/Backup/Create` | `todo` |  |
| GET | `/Backup/Manifest` | `todo` |  |
| POST | `/Backup/Restore` | `todo` |  |

## Branding

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Branding/Configuration` | `served` |  |
| GET | `/Branding/Css` | `served` |  |
| GET | `/Branding/Css.css` | `served` |  |

## Channel

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Channels` | `todo` |  |
| GET | `/Channels/Features` | `todo` |  |
| GET | `/Channels/Items/Latest` | `todo` |  |
| GET | `/Channels/{channelId}/Features` | `todo` |  |
| GET | `/Channels/{channelId}/Items` | `todo` |  |

## Collection

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/Collections` | `served` | In its first item's library; no item, ParentId or IsLocked answers 501. |
| POST | `/Collections/{collectionId}/Items` | `served` |  |
| DELETE | `/Collections/{collectionId}/Items` | `served` |  |

## Device

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Devices` | `todo` |  |
| DELETE | `/Devices` | `todo` |  |
| GET | `/Devices/Info` | `todo` |  |
| GET | `/Devices/Options` | `todo` |  |
| POST | `/Devices/Options` | `todo` |  |

## DisplayPreference

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/DisplayPreferences/{displayPreferencesId}` | `served` | The reference's defaults. |
| POST | `/DisplayPreferences/{displayPreferencesId}` | `served` | Accepted, not kept. |

## Environment

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Environment/DefaultDirectoryBrowser` | `todo` |  |
| GET | `/Environment/DirectoryContents` | `todo` |  |
| GET | `/Environment/Drives` | `todo` |  |
| GET | `/Environment/ParentPath` | `todo` |  |
| POST | `/Environment/ValidatePath` | `todo` |  |

## Filter

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Items/Filters` | `served` | Below a library or across the caller's; no tags (keywords are a provider's). |
| GET | `/Items/Filters2` | `served` | Languages as tracks state them; Live TV flags answer 501. |

## Genre

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Genres` | `served` | searchTerm completes a word of the name; name ranges answer 501. |
| GET | `/Genres/{genreName}` | `served` |  |

## Image

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Artists/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| HEAD | `/Artists/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| GET | `/Branding/Splashscreen` | `todo` |  |
| POST | `/Branding/Splashscreen` | `todo` |  |
| DELETE | `/Branding/Splashscreen` | `todo` |  |
| GET | `/Genres/{name}/Images/{imageType}` | `todo` |  |
| HEAD | `/Genres/{name}/Images/{imageType}` | `todo` |  |
| GET | `/Genres/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| HEAD | `/Genres/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| GET | `/Items/{itemId}/Images` | `served` | Empty: no artwork. |
| GET | `/Items/{itemId}/Images/{imageType}` | `served` | The image of `Item.images` an item shows (`Primary` an episode's still else the poster, `Backdrop`, `Logo`, `Banner`, `Thumb`), scaled into `fillWidth`/`fillHeight`, `maxWidth`/`maxHeight` or `width`/`height`; without a token only with the `tag` a lookup published for that item. |
| POST | `/Items/{itemId}/Images/{imageType}` | `todo` |  |
| DELETE | `/Items/{itemId}/Images/{imageType}` | `todo` |  |
| HEAD | `/Items/{itemId}/Images/{imageType}` | `served` |  |
| GET | `/Items/{itemId}/Images/{imageType}/{imageIndex}` | `served` | As without an index; one image of each kind, so any index past `0` is a `404`. |
| POST | `/Items/{itemId}/Images/{imageType}/{imageIndex}` | `todo` |  |
| DELETE | `/Items/{itemId}/Images/{imageType}/{imageIndex}` | `todo` |  |
| HEAD | `/Items/{itemId}/Images/{imageType}/{imageIndex}` | `served` |  |
| POST | `/Items/{itemId}/Images/{imageType}/{imageIndex}/Index` | `todo` |  |
| GET | `/Items/{itemId}/Images/{imageType}/{imageIndex}/{tag}/{format}/{maxWidth}/{maxHeight}/{percentPlayed}/{unplayedCount}` | `todo` |  |
| HEAD | `/Items/{itemId}/Images/{imageType}/{imageIndex}/{tag}/{format}/{maxWidth}/{maxHeight}/{percentPlayed}/{unplayedCount}` | `todo` |  |
| GET | `/MusicGenres/{name}/Images/{imageType}` | `todo` |  |
| HEAD | `/MusicGenres/{name}/Images/{imageType}` | `todo` |  |
| GET | `/MusicGenres/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| HEAD | `/MusicGenres/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| GET | `/Persons/{name}/Images/{imageType}` | `todo` |  |
| HEAD | `/Persons/{name}/Images/{imageType}` | `todo` |  |
| GET | `/Persons/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| HEAD | `/Persons/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| GET | `/Studios/{name}/Images/{imageType}` | `todo` |  |
| HEAD | `/Studios/{name}/Images/{imageType}` | `todo` |  |
| GET | `/Studios/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| HEAD | `/Studios/{name}/Images/{imageType}/{imageIndex}` | `todo` |  |
| GET | `/UserImage` | `todo` |  |
| POST | `/UserImage` | `todo` |  |
| DELETE | `/UserImage` | `todo` |  |
| HEAD | `/UserImage` | `todo` |  |

## InstantMix

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Albums/{itemId}/InstantMix` | `todo` |  |
| GET | `/Artists/{itemId}/InstantMix` | `todo` |  |
| GET | `/Items/{itemId}/InstantMix` | `todo` |  |
| GET | `/MusicGenres/InstantMix` | `todo` |  |
| GET | `/MusicGenres/{name}/InstantMix` | `todo` |  |
| GET | `/Playlists/{itemId}/InstantMix` | `todo` |  |
| GET | `/Songs/{itemId}/InstantMix` | `todo` |  |

## ItemLookup

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/Items/RemoteSearch/Apply/{itemId}` | `todo` |  |
| POST | `/Items/RemoteSearch/Book` | `todo` |  |
| POST | `/Items/RemoteSearch/BoxSet` | `todo` |  |
| POST | `/Items/RemoteSearch/Movie` | `todo` |  |
| POST | `/Items/RemoteSearch/MusicAlbum` | `todo` |  |
| POST | `/Items/RemoteSearch/MusicArtist` | `todo` |  |
| POST | `/Items/RemoteSearch/MusicVideo` | `todo` |  |
| POST | `/Items/RemoteSearch/Person` | `todo` |  |
| POST | `/Items/RemoteSearch/Series` | `todo` |  |
| POST | `/Items/RemoteSearch/Trailer` | `todo` |  |
| GET | `/Items/{itemId}/ExternalIdInfos` | `todo` |  |

## ItemUpdate

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/Items/{itemId}` | `todo` |  |
| POST | `/Items/{itemId}/ContentType` | `todo` |  |
| GET | `/Items/{itemId}/MetadataEditor` | `todo` |  |

## Library

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Albums/{itemId}/Similar` | `todo` |  |
| GET | `/Artists/{itemId}/Similar` | `todo` |  |
| GET | `/Items` | `served` | Filters the official API cannot express answer 501; one sort key. |
| DELETE | `/Items` | `todo` |  |
| GET | `/Items/Counts` | `todo` |  |
| GET | `/Items/Latest` | `served` |  |
| GET | `/Items/Root` | `todo` |  |
| GET | `/Items/{itemId}` | `served` |  |
| DELETE | `/Items/{itemId}` | `served` | Playlists and collections; media files answer 501. |
| GET | `/Items/{itemId}/Ancestors` | `served` |  |
| GET | `/Items/{itemId}/Collections` | `served` |  |
| GET | `/Items/{itemId}/Download` | `todo` |  |
| GET | `/Items/{itemId}/File` | `todo` |  |
| GET | `/Items/{itemId}/Intros` | `served` |  |
| GET | `/Items/{itemId}/LocalTrailers` | `served` |  |
| POST | `/Items/{itemId}/Refresh` | `todo` |  |
| GET | `/Items/{itemId}/Similar` | `served` | Films and series, by the official API's measure. |
| GET | `/Items/{itemId}/SpecialFeatures` | `served` |  |
| GET | `/Items/{itemId}/ThemeMedia` | `served` | Empty: no theme media is held. |
| GET | `/Items/{itemId}/ThemeSongs` | `todo` |  |
| GET | `/Items/{itemId}/ThemeVideos` | `todo` |  |
| GET | `/Libraries/AvailableOptions` | `todo` |  |
| POST | `/Library/Media/Updated` | `todo` |  |
| GET | `/Library/MediaFolders` | `served` |  |
| POST | `/Library/Movies/Added` | `todo` |  |
| POST | `/Library/Movies/Updated` | `todo` |  |
| GET | `/Library/PhysicalPaths` | `todo` |  |
| POST | `/Library/Refresh` | `todo` |  |
| POST | `/Library/Series/Added` | `todo` |  |
| POST | `/Library/Series/Updated` | `todo` |  |
| GET | `/Movies/{itemId}/Similar` | `todo` |  |
| GET | `/Shows/{itemId}/Similar` | `todo` |  |
| GET | `/Trailers/{itemId}/Similar` | `todo` |  |
| GET | `/UserItems/Resume` | `served` |  |

## LibraryStructure

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Library/VirtualFolders` | `todo` |  |
| POST | `/Library/VirtualFolders` | `todo` |  |
| DELETE | `/Library/VirtualFolders` | `todo` |  |
| POST | `/Library/VirtualFolders/LibraryOptions` | `todo` |  |
| POST | `/Library/VirtualFolders/Name` | `todo` |  |
| POST | `/Library/VirtualFolders/Paths` | `todo` |  |
| DELETE | `/Library/VirtualFolders/Paths` | `todo` |  |
| POST | `/Library/VirtualFolders/Paths/Update` | `todo` |  |

## LiveTv

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/LiveTv/ChannelMappingOptions` | `todo` |  |
| POST | `/LiveTv/ChannelMappings` | `todo` |  |
| GET | `/LiveTv/Channels` | `todo` |  |
| GET | `/LiveTv/Channels/{channelId}` | `todo` |  |
| GET | `/LiveTv/GuideInfo` | `todo` |  |
| GET | `/LiveTv/Info` | `todo` |  |
| POST | `/LiveTv/ListingProviders` | `todo` |  |
| DELETE | `/LiveTv/ListingProviders` | `todo` |  |
| GET | `/LiveTv/ListingProviders/Default` | `todo` |  |
| GET | `/LiveTv/ListingProviders/Lineups` | `todo` |  |
| GET | `/LiveTv/ListingProviders/SchedulesDirect/Countries` | `todo` |  |
| GET | `/LiveTv/LiveRecordings/{recordingId}/stream` | `todo` |  |
| GET | `/LiveTv/LiveStreamFiles/{streamId}/stream.{container}` | `todo` |  |
| GET | `/LiveTv/Programs` | `todo` |  |
| POST | `/LiveTv/Programs` | `todo` |  |
| GET | `/LiveTv/Programs/Recommended` | `todo` |  |
| GET | `/LiveTv/Programs/{programId}` | `todo` |  |
| GET | `/LiveTv/Recordings` | `todo` |  |
| GET | `/LiveTv/Recordings/Folders` | `todo` |  |
| GET | `/LiveTv/Recordings/{recordingId}` | `todo` |  |
| DELETE | `/LiveTv/Recordings/{recordingId}` | `todo` |  |
| GET | `/LiveTv/SeriesTimers` | `todo` |  |
| POST | `/LiveTv/SeriesTimers` | `todo` |  |
| GET | `/LiveTv/SeriesTimers/{timerId}` | `todo` |  |
| POST | `/LiveTv/SeriesTimers/{timerId}` | `todo` |  |
| DELETE | `/LiveTv/SeriesTimers/{timerId}` | `todo` |  |
| GET | `/LiveTv/Timers` | `todo` |  |
| POST | `/LiveTv/Timers` | `todo` |  |
| GET | `/LiveTv/Timers/Defaults` | `todo` |  |
| GET | `/LiveTv/Timers/{timerId}` | `todo` |  |
| POST | `/LiveTv/Timers/{timerId}` | `todo` |  |
| DELETE | `/LiveTv/Timers/{timerId}` | `todo` |  |
| POST | `/LiveTv/TunerHosts` | `todo` |  |
| DELETE | `/LiveTv/TunerHosts` | `todo` |  |
| GET | `/LiveTv/TunerHosts/Types` | `todo` |  |
| GET | `/LiveTv/Tuners/Discover` | `todo` |  |
| GET | `/LiveTv/Tuners/Discvover` | `todo` |  |
| POST | `/LiveTv/Tuners/{tunerId}/Reset` | `todo` |  |

## Localization

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Localization/Countries` | `todo` |  |
| GET | `/Localization/Cultures` | `todo` |  |
| GET | `/Localization/Options` | `todo` |  |
| GET | `/Localization/ParentalRatings` | `todo` |  |

## Lyric

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Audio/{itemId}/Lyrics` | `todo` |  |
| POST | `/Audio/{itemId}/Lyrics` | `todo` |  |
| DELETE | `/Audio/{itemId}/Lyrics` | `todo` |  |
| GET | `/Audio/{itemId}/RemoteSearch/Lyrics` | `todo` |  |
| POST | `/Audio/{itemId}/RemoteSearch/Lyrics/{lyricId}` | `todo` |  |
| GET | `/Providers/Lyrics/{lyricId}` | `todo` |  |

## MediaInfo

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Items/{itemId}/PlaybackInfo` | `served` | Direct play decided by MediaFile.readableAsIs; HLS otherwise. |
| POST | `/Items/{itemId}/PlaybackInfo` | `served` | Direct play decided by MediaFile.readableAsIs; HLS otherwise. |
| POST | `/LiveStreams/Close` | `served` | Nothing is open: 204. |
| POST | `/LiveStreams/Open` | `served` | No source requires opening: 400 without a token, 500 with one. |
| GET | `/Playback/BitrateTest` | `served` |  |

## MediaSegment

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/MediaSegments/{itemId}` | `served` | Intro, Outro and Commercial from the work's markers. |

## Movie

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Movies/Recommendations` | `todo` |  |

## MusicGenre

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/MusicGenres/{genreName}` | `todo` |  |

## Person

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Persons` | `served` | With searchTerm, or one role at a time; every role unsearched answers 501. |
| GET | `/Persons/{name}` | `served` |  |

## Playlist

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/Playlists` | `served` | The caller's own: sharing (Users, IsPublic) answers 501. |
| GET | `/Playlists/{playlistId}` | `served` | OpenAccess false, no shares: a playlist is its owner's. |
| POST | `/Playlists/{playlistId}` | `served` | Name and items; sharing answers 501. |
| GET | `/Playlists/{playlistId}/Items` | `served` |  |
| POST | `/Playlists/{playlistId}/Items` | `served` | Appended; a position before the end answers 501. |
| DELETE | `/Playlists/{playlistId}/Items` | `served` |  |
| POST | `/Playlists/{playlistId}/Items/{itemId}/Move/{newIndex}` | `served` |  |
| GET | `/Playlists/{playlistId}/Users` | `served` | Empty: shared with nobody. |
| GET | `/Playlists/{playlistId}/Users/{userId}` | `served` |  |
| POST | `/Playlists/{playlistId}/Users/{userId}` | `served` | 501: playlists are not shared. |
| DELETE | `/Playlists/{playlistId}/Users/{userId}` | `served` | 501: playlists are not shared. |

## Plugin

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Packages` | `todo` |  |
| POST | `/Packages/Installed/{name}` | `todo` |  |
| DELETE | `/Packages/Installing/{packageId}` | `todo` |  |
| GET | `/Packages/{name}` | `todo` |  |
| GET | `/Plugins` | `todo` |  |
| DELETE | `/Plugins/{pluginId}` | `todo` |  |
| GET | `/Plugins/{pluginId}/Configuration` | `todo` |  |
| POST | `/Plugins/{pluginId}/Configuration` | `todo` |  |
| POST | `/Plugins/{pluginId}/Manifest` | `todo` |  |
| DELETE | `/Plugins/{pluginId}/{version}` | `todo` |  |
| POST | `/Plugins/{pluginId}/{version}/Disable` | `todo` |  |
| POST | `/Plugins/{pluginId}/{version}/Enable` | `todo` |  |
| GET | `/Plugins/{pluginId}/{version}/Image` | `todo` |  |
| GET | `/Repositories` | `todo` |  |
| POST | `/Repositories` | `todo` |  |
| GET | `/web/ConfigurationPage` | `todo` |  |
| GET | `/web/ConfigurationPages` | `todo` |  |

## RemoteImage

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Items/{itemId}/RemoteImages` | `todo` |  |
| POST | `/Items/{itemId}/RemoteImages/Download` | `todo` |  |
| GET | `/Items/{itemId}/RemoteImages/Providers` | `todo` |  |

## ScheduledTask

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/ScheduledTasks` | `todo` |  |
| POST | `/ScheduledTasks/Running/{taskId}` | `todo` |  |
| DELETE | `/ScheduledTasks/Running/{taskId}` | `todo` |  |
| GET | `/ScheduledTasks/{taskId}` | `todo` |  |
| POST | `/ScheduledTasks/{taskId}/Triggers` | `todo` |  |

## Search

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Search/Hints` | `served` | Items then people, first page only; Live TV and media filters answer 501. |

## Session

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Sessions` | `todo` |  |
| POST | `/Sessions/Capabilities` | `served` |  |
| POST | `/Sessions/Capabilities/Full` | `served` |  |
| POST | `/Sessions/Logout` | `served` |  |
| POST | `/Sessions/Playing` | `served` |  |
| POST | `/Sessions/Playing/Ping` | `served` |  |
| POST | `/Sessions/Playing/Progress` | `served` |  |
| POST | `/Sessions/Playing/Stopped` | `served` |  |
| POST | `/Sessions/Viewing` | `todo` |  |
| POST | `/Sessions/{sessionId}/Command` | `todo` |  |
| POST | `/Sessions/{sessionId}/Command/{command}` | `todo` |  |
| POST | `/Sessions/{sessionId}/Message` | `todo` |  |
| POST | `/Sessions/{sessionId}/Playing` | `todo` |  |
| POST | `/Sessions/{sessionId}/Playing/{command}` | `todo` |  |
| POST | `/Sessions/{sessionId}/System/{command}` | `todo` |  |
| POST | `/Sessions/{sessionId}/User/{userId}` | `todo` |  |
| DELETE | `/Sessions/{sessionId}/User/{userId}` | `todo` |  |
| POST | `/Sessions/{sessionId}/Viewing` | `todo` |  |

## Show

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Shows/NextUp` | `served` |  |
| GET | `/Shows/Upcoming` | `served` | Below a library or across them; below an item answers 501. |
| GET | `/Shows/{seriesId}/Episodes` | `served` |  |
| GET | `/Shows/{seriesId}/Seasons` | `served` |  |

## Startup

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/Startup/Complete` | `todo` |  |
| GET | `/Startup/Configuration` | `todo` |  |
| POST | `/Startup/Configuration` | `todo` |  |
| GET | `/Startup/FirstUser` | `todo` |  |
| POST | `/Startup/RemoteAccess` | `todo` |  |
| GET | `/Startup/User` | `todo` |  |
| POST | `/Startup/User` | `todo` |  |

## Studio

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Studios` | `served` | A studio's id is a hash of its name; StudioIds on /Items answers 501. |
| GET | `/Studios/{name}` | `served` |  |

## Subtitle

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/FallbackFont/Fonts` | `todo` |  |
| GET | `/FallbackFont/Fonts/{name}` | `todo` |  |
| GET | `/Items/{itemId}/RemoteSearch/Subtitles/{language}` | `todo` |  |
| POST | `/Items/{itemId}/RemoteSearch/Subtitles/{subtitleId}` | `todo` |  |
| GET | `/Providers/Subtitles/Subtitles/{subtitleId}` | `todo` |  |
| POST | `/Videos/{itemId}/Subtitles` | `todo` |  |
| DELETE | `/Videos/{itemId}/Subtitles/{index}` | `todo` |  |
| GET | `/Videos/{itemId}/{mediaSourceId}/Subtitles/{index}/subtitles.m3u8` | `todo` |  |
| GET | `/Videos/{routeItemId}/{routeMediaSourceId}/Subtitles/{routeIndex}/Stream.{routeFormat}` | `todo` |  |
| GET | `/Videos/{routeItemId}/{routeMediaSourceId}/Subtitles/{routeIndex}/{routeStartPositionTicks}/Stream.{routeFormat}` | `todo` |  |

## Suggestion

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Items/Suggestions` | `todo` |  |

## SyncPlay

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/SyncPlay/Buffering` | `todo` |  |
| POST | `/SyncPlay/Join` | `todo` |  |
| POST | `/SyncPlay/Leave` | `todo` |  |
| GET | `/SyncPlay/List` | `served` | Empty: no group is held. |
| POST | `/SyncPlay/MovePlaylistItem` | `todo` |  |
| POST | `/SyncPlay/New` | `todo` |  |
| POST | `/SyncPlay/NextItem` | `todo` |  |
| POST | `/SyncPlay/Pause` | `todo` |  |
| POST | `/SyncPlay/Ping` | `todo` |  |
| POST | `/SyncPlay/PreviousItem` | `todo` |  |
| POST | `/SyncPlay/Queue` | `todo` |  |
| POST | `/SyncPlay/Ready` | `todo` |  |
| POST | `/SyncPlay/RemoveFromPlaylist` | `todo` |  |
| POST | `/SyncPlay/Seek` | `todo` |  |
| POST | `/SyncPlay/SetIgnoreWait` | `todo` |  |
| POST | `/SyncPlay/SetNewQueue` | `todo` |  |
| POST | `/SyncPlay/SetPlaylistItem` | `todo` |  |
| POST | `/SyncPlay/SetRepeatMode` | `todo` |  |
| POST | `/SyncPlay/SetShuffleMode` | `todo` |  |
| POST | `/SyncPlay/Stop` | `todo` |  |
| POST | `/SyncPlay/Unpause` | `todo` |  |
| GET | `/SyncPlay/{id}` | `todo` |  |

## System

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/ClientLog/Document` | `todo` |  |
| GET | `/GetUtcTime` | `todo` |  |
| GET | `/System/ActivityLog/Entries` | `todo` |  |
| GET | `/System/Configuration` | `todo` |  |
| POST | `/System/Configuration` | `todo` |  |
| POST | `/System/Configuration/Branding` | `todo` |  |
| GET | `/System/Configuration/MetadataOptions/Default` | `todo` |  |
| GET | `/System/Configuration/{key}` | `todo` |  |
| POST | `/System/Configuration/{key}` | `todo` |  |
| GET | `/System/Endpoint` | `served` |  |
| GET | `/System/Info` | `served` | The reference's own paths are left empty. |
| GET | `/System/Info/Public` | `served` |  |
| GET | `/System/Info/Storage` | `todo` |  |
| GET | `/System/Logs` | `todo` |  |
| GET | `/System/Logs/Log` | `todo` |  |
| GET | `/System/Ping` | `served` |  |
| POST | `/System/Ping` | `served` |  |
| POST | `/System/Restart` | `todo` |  |
| POST | `/System/Shutdown` | `todo` |  |

## Trailer

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Trailers` | `todo` |  |

## TrickPlay

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Videos/{itemId}/Trickplay/{width}/tiles.m3u8` | `todo` |  |
| GET | `/Videos/{itemId}/Trickplay/{width}/{index}.jpg` | `todo` |  |

## User

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Users` | `todo` |  |
| POST | `/Users` | `todo` |  |
| POST | `/Users/Configuration` | `todo` |  |
| GET | `/Users/Me` | `served` |  |
| POST | `/Users/New` | `todo` |  |
| POST | `/Users/Password` | `todo` |  |
| GET | `/Users/Public` | `served` | Empty, as the reference's hidden users. |
| GET | `/Users/{userId}` | `served` |  |
| DELETE | `/Users/{userId}` | `todo` |  |
| POST | `/Users/{userId}/Policy` | `todo` |  |

## UserData

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/UserFavoriteItems/{itemId}` | `served` | Refused 501: favorites are not kept. |
| DELETE | `/UserFavoriteItems/{itemId}` | `served` | Refused 501: favorites are not kept. |
| POST | `/UserItems/{itemId}/Rating` | `todo` |  |
| DELETE | `/UserItems/{itemId}/Rating` | `todo` |  |
| GET | `/UserItems/{itemId}/UserData` | `todo` |  |
| POST | `/UserItems/{itemId}/UserData` | `todo` |  |
| POST | `/UserPlayedItems/{itemId}` | `served` |  |
| DELETE | `/UserPlayedItems/{itemId}` | `served` |  |

## UserView

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/UserViews` | `served` |  |
| GET | `/UserViews/GroupingOptions` | `todo` |  |

## Video

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| POST | `/Videos/MergeVersions` | `todo` |  |
| GET | `/Videos/{itemId}/AdditionalParts` | `todo` |  |
| DELETE | `/Videos/{itemId}/AlternateSources` | `todo` |  |
| GET | `/Videos/{itemId}/stream` | `served` | static=true only: a remux to another container answers 501. |
| HEAD | `/Videos/{itemId}/stream` | `served` | static=true only. |
| GET | `/Videos/{itemId}/stream.{container}` | `served` | static=true only. |
| HEAD | `/Videos/{itemId}/stream.{container}` | `served` | static=true only. |
| GET | `/Videos/{videoId}/{mediaSourceId}/Attachments/{index}` | `todo` |  |

## Year

| Method | Path | Status | Notes |
| --- | --- | --- | --- |
| GET | `/Years` | `served` |  |
| GET | `/Years/{year}` | `served` |  |

## Served outside the document

Routes the reference answers that its document leaves out, which older clients call:

| Path | As |
| --- | --- |
| `/Users/{userId}/Items`, `/Users/{userId}/Items/{itemId}` | `/Items`, `/Items/{itemId}` |
| `/Users/{userId}/Items/Latest`, `/Users/{userId}/Items/Resume` | `/Items/Latest`, `/UserItems/Resume` |
| `/Users/{userId}/Views` | `/UserViews` |
| `/Users/{userId}/PlayedItems/{itemId}` | `/UserPlayedItems/{itemId}` |
| `/Users/{userId}/Items/{itemId}/Intros`, `/Users/{userId}/Items/{itemId}/SpecialFeatures`, `/Users/{userId}/Items/{itemId}/LocalTrailers` | the same, without the user |
| `/Videos/{itemId}/master.m3u8` | the address `PlaybackInfo` gives a conversion |
