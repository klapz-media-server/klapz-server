Captured on the reference (1.43.5) on the test film of library 27, with sidecars put beside
it for the capture and removed afterwards. The user's directory in paths is scrubbed to
`C:\Media`; nothing else is changed. No `X-Plex-Language`: languages are named in themselves.

- `lookup_streams.json`: the subtitle `Stream` elements of `GET /library/metadata/577986`
  with two sidecars (`.de.srt`, `.it.ass`), as JSON.
- `offset_streams.json`: two of them after `PUT /library/streams/12262676?offset=300` and
  `PUT /library/streams/12265847?offset=-1200`, each captured right after its `PUT`.
- `search_none.json`: `GET /library/metadata/577986/subtitles?language=zu`, where the
  provider found nothing, as JSON.
