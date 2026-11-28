# Live endpoint probe

Which `GET` operations of `docs/plex/openapi.json` the reference server actually answers. **Generated** — regenerate with `PLEX_TOKEN=... python tools/probe_endpoints.py`.

The spec is not perfectly faithful, so this informs what is worth implementing. Write operations are not probed, and deferred scanner/analysis paths are skipped.

**Read `absent` carefully.** It means *not served for the identifiers this probe could synthesize*, which is not the same as *the route does not exist*. Path parameters like `collectionId`, `playlistId`, `partId`, `streamId`, `mediaId`, `updatedAt` and `changestamp` are filled with `1`, so a 404 on those is usually a wrong id rather than a missing route. `/:/websocket/notifications` also 404s because it expects a WebSocket upgrade, not a plain GET. Treat `absent` as a shortlist to verify by hand, and confirm each one the way `/library/sections/{sectionId}/firstCharacters` was confirmed — by finding the form the server does answer.

| Verdict | Meaning |
| --- | --- |
| `exists` | 2xx — route present and answered |
| `absent` | 404 — declared in the spec, not served |
| `needs-params` | 4xx other than 404 — route exists but our probe was incomplete |
| `error` | 5xx or no response |

| Verdict | Count |
| --- | --: |
| `exists` | 61 |
| `absent` | 32 |
| `needs-params` | 27 |
| `error` | 2 |
| **Total** | **122** |

## Results

| Verdict | Status | Path | Domain |
| --- | --: | --- | --- |
| `absent` | 404 | `/:/websocket/notifications` | Events |
| `absent` | 404 | `/downloadQueue/{queueId}/item/{itemId}/decision` | Download Queue |
| `absent` | 404 | `/downloadQueue/{queueId}/item/{itemId}/media` | Download Queue |
| `absent` | 404 | `/downloadQueue/{queueId}/items/{itemId}` | Download Queue |
| `absent` | 404 | `/hubs/items` | Hubs |
| `absent` | 404 | `/hubs/popularInLibraries` | Hubs |
| `absent` | 404 | `/library/collections/{collectionId}/composite/{updatedAt}` | Content |
| `absent` | 404 | `/library/collections/{collectionId}/items` | Content |
| `absent` | 404 | `/library/media/{mediaId}/chapterImages/{chapter}` | Library |
| `absent` | 404 | `/library/metadata/augmentations/{augmentationId}` | Library |
| `absent` | 404 | `/library/metadata/{ids}/users/top` | Library |
| `absent` | 404 | `/library/metadata/{ids}/{element}/{timestamp}` | Library |
| `absent` | 404 | `/library/parts/{partId}/{changestamp}/{filename}` | Library |
| `absent` | 404 | `/library/sections/{sectionId}/composite/{updatedAt}` | Library |
| `absent` | 404 | `/library/sections/{sectionId}/firstCharacters` | Library |
| `absent` | 404 | `/library/streams/{streamId}.{ext}` | Library |
| `absent` | 404 | `/library/streams/{streamId}/levels` | Library |
| `absent` | 404 | `/library/streams/{streamId}/loudness` | Library |
| `absent` | 404 | `/livetv/dvrs/{dvrId}` | DVRs |
| `absent` | 404 | `/livetv/epg/countries/{country}/{epgId}/lineups` | EPG |
| `absent` | 404 | `/livetv/epg/countries/{country}/{epgId}/regions` | EPG |
| `absent` | 404 | `/livetv/epg/countries/{country}/{epgId}/regions/{region}/lineups` | EPG |
| `absent` | 404 | `/livetv/sessions/{sessionId}` | Live TV |
| `absent` | 404 | `/livetv/sessions/{sessionId}/{consumerId}/index.m3u8` | Live TV |
| `absent` | 404 | `/livetv/sessions/{sessionId}/{consumerId}/{segmentId}` | Live TV |
| `absent` | 404 | `/media/grabbers/devices/{deviceId}/thumb/{version}` | Devices |
| `absent` | 404 | `/playlists/{playlistId}/items/{generatorId}` | Library Playlists |
| `absent` | 404 | `/playlists/{playlistId}/items/{generatorId}/items` | Library Playlists |
| `absent` | 404 | `/tv.plex.providers.epg.{identifier}:{deviceId}/grid` | EPG |
| `absent` | 404 | `/tv.plex.providers.epg.{identifier}:{deviceId}/lineups/dvr/channels` | EPG |
| `absent` | 404 | `/tv.plex.providers.epg.{identifier}:{deviceId}/watchnow` | EPG |
| `absent` | 404 | `/tv.plex.providers.epg.{identifier}:{deviceId}/watchnow/all` | EPG |
| `error` | — | `/library/all` | Library |
| `error` | 500 | `/media/grabbers/devices/{deviceId}/channels` | Devices |
| `exists` | 200 | `/` | General |
| `exists` | 200 | `/:/eventsource/notifications` | Events |
| `exists` | 200 | `/:/prefs` | Preferences |
| `exists` | 200 | `/activities` | Activities |
| `exists` | 200 | `/butler` | Butler |
| `exists` | 200 | `/downloadQueue/{queueId}` | Download Queue |
| `exists` | 200 | `/downloadQueue/{queueId}/items` | Download Queue |
| `exists` | 200 | `/hubs` | Hubs |
| `exists` | 200 | `/hubs/continueWatching` | Hubs |
| `exists` | 200 | `/hubs/metadata/{metadataId}` | Hubs |
| `exists` | 200 | `/hubs/metadata/{metadataId}/postplay` | Hubs |
| `exists` | 200 | `/hubs/metadata/{metadataId}/related` | Hubs |
| `exists` | 200 | `/hubs/promoted` | Hubs |
| `exists` | 200 | `/hubs/sections/{sectionId}` | Hubs |
| `exists` | 200 | `/hubs/sections/{sectionId}/manage` | Hubs |
| `exists` | 200 | `/identity` | General |
| `exists` | 200 | `/library/metadata/{ids}` | Content |
| `exists` | 200 | `/library/metadata/{ids}/allLeaves` | Library |
| `exists` | 200 | `/library/metadata/{ids}/extras` | Library |
| `exists` | 200 | `/library/metadata/{ids}/related` | Library |
| `exists` | 200 | `/library/metadata/{ids}/similar` | Library |
| `exists` | 200 | `/library/metadata/{ids}/tree` | Library |
| `exists` | 200 | `/library/people/{personId}` | Library |
| `exists` | 200 | `/library/people/{personId}/media` | Library |
| `exists` | 200 | `/library/randomArtwork` | Library |
| `exists` | 200 | `/library/sections/all` | Library |
| `exists` | 200 | `/library/sections/{sectionId}` | Library |
| `exists` | 200 | `/library/sections/{sectionId}/albums` | Content |
| `exists` | 200 | `/library/sections/{sectionId}/all` | Content |
| `exists` | 200 | `/library/sections/{sectionId}/allLeaves` | Content |
| `exists` | 200 | `/library/sections/{sectionId}/arts` | Content |
| `exists` | 200 | `/library/sections/{sectionId}/audioCodec` | Library |
| `exists` | 200 | `/library/sections/{sectionId}/audioLayout` | Library |
| `exists` | 200 | `/library/sections/{sectionId}/categories` | Content |
| `exists` | 200 | `/library/sections/{sectionId}/cluster` | Content |
| `exists` | 200 | `/library/sections/{sectionId}/collections` | Library |
| `exists` | 200 | `/library/sections/{sectionId}/filters` | Library |
| `exists` | 200 | `/library/sections/{sectionId}/location` | Content |
| `exists` | 200 | `/library/sections/{sectionId}/moment` | Content |
| `exists` | 200 | `/library/sections/{sectionId}/prefs` | Library |
| `exists` | 200 | `/library/sections/{sectionId}/sorts` | Library |
| `exists` | 200 | `/library/sections/{sectionId}/subtitleCodec` | Library |
| `exists` | 200 | `/library/sections/{sectionId}/videoCodec` | Library |
| `exists` | 200 | `/livetv/dvrs` | DVRs |
| `exists` | 200 | `/livetv/epg/countries` | EPG |
| `exists` | 200 | `/livetv/epg/languages` | EPG |
| `exists` | 200 | `/livetv/sessions` | Live TV |
| `exists` | 200 | `/media/grabbers` | Devices |
| `exists` | 200 | `/media/grabbers/devices` | Devices |
| `exists` | 200 | `/media/grabbers/devices/{deviceId}` | Devices |
| `exists` | 200 | `/media/providers` | Provider |
| `exists` | 200 | `/playQueues/{playQueueId}` | Play Queue |
| `exists` | 200 | `/playlists` | Playlist |
| `exists` | 200 | `/playlists/{playlistId}` | Playlist |
| `exists` | 200 | `/playlists/{playlistId}/generators` | Library Playlists |
| `exists` | 200 | `/playlists/{playlistId}/items` | Playlist |
| `exists` | 200 | `/status/sessions` | Status |
| `exists` | 200 | `/status/sessions/background` | Status |
| `exists` | 200 | `/status/sessions/history/all` | Status |
| `exists` | 200 | `/status/sessions/history/{historyId}` | Status |
| `exists` | 200 | `/updater/status` | Updater |
| `needs-params` | 400 | `/:/prefs/get` | Preferences |
| `needs-params` | 400 | `/hubs/search` | Search |
| `needs-params` | 400 | `/hubs/search/voice` | Search |
| `needs-params` | 400 | `/library/metadata/{ids}/file` | Library |
| `needs-params` | 403 | `/library/metadata/{ids}/nearest` | Library |
| `needs-params` | 400 | `/library/metadata/{ids}/subtitles` | Library |
| `needs-params` | 400 | `/library/sections/prefs` | Library |
| `needs-params` | 400 | `/library/sections/{sectionId}/autocomplete` | Library |
| `needs-params` | 400 | `/library/sections/{sectionId}/common` | Library |
| `needs-params` | 400 | `/library/sections/{sectionId}/computePath` | Content |
| `needs-params` | 400 | `/library/sections/{sectionId}/nearest` | Content |
| `needs-params` | 400 | `/library/tags` | Library |
| `needs-params` | 400 | `/livetv/epg/channelmap` | EPG |
| `needs-params` | 400 | `/livetv/epg/channels` | EPG |
| `needs-params` | 400 | `/livetv/epg/lineup` | EPG |
| `needs-params` | 400 | `/livetv/epg/lineupchannels` | EPG |
| `needs-params` | 401 | `/media/subscriptions` | Subscriptions |
| `needs-params` | 401 | `/media/subscriptions/scheduled` | Subscriptions |
| `needs-params` | 400 | `/media/subscriptions/template` | Subscriptions |
| `needs-params` | 401 | `/media/subscriptions/{subscriptionId}` | Subscriptions |
| `needs-params` | 400 | `/photo/:/transcode` | Transcoder |
| `needs-params` | 400 | `/security/resources` | General |
| `needs-params` | 400 | `/services/ultrablur/colors` | UltraBlur |
| `needs-params` | 400 | `/services/ultrablur/image` | UltraBlur |
| `needs-params` | 400 | `/{transcodeType}/:/transcode/universal/decision` | Transcoder |
| `needs-params` | 400 | `/{transcodeType}/:/transcode/universal/start.*` | Transcoder |
| `needs-params` | 400 | `/{transcodeType}/:/transcode/universal/subtitles` | Transcoder |