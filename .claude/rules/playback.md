---
paths:
  - "crates/core/src/stream.rs"
  - "crates/core/src/playback.rs"
  - "crates/core/src/lifecycle.rs"
  - "crates/core/src/bitrate.rs"
  - "crates/api/src/stream.rs"
  - "crates/api/src/playback.rs"
  - "crates/server/src/streams.rs"
  - "crates/server/src/callbacks.rs"
  - "crates/db/src/playback_repository.rs"
  - "crates/api-plex/src/playback.rs"
  - "crates/api-plex/src/timeline.rs"
  - "crates/api-plex/src/sessions.rs"
  - "crates/api-jellyfin/src/playback.rs"
  - "crates/db/tests/playback.rs"
---

# Streams and playback sessions

- **Started on the official API.** `startStream` (direct play, direct stream or transcode,
  decided from the file and the player's limits; `quality`: `FIXED`, `SERVER_ADAPTIVE`,
  `CLIENT_ADAPTIVE`) reaches the media engine through the `core::stream::StreamEngine` port,
  implemented by `server::streams` over the transcoder. Plex's `start.mpd` / `start.m3u8` answer
  `307` to `/streams/{id}/manifest.mpd` or `master.m3u8`: the binding builds no manifest.
- **A stream is read at its own address**, `/streams/{id}/…?token=<stream secret>`, never the
  account's token: players hand those URLs to components that never see the account.
- **Transcoder callbacks** `/transcoder/{session}/{run}/…` (`server::callbacks`) carry no token
  and answer only on loopback, to the id of a running session.
- **One end path: `core::lifecycle`.** Whatever ends a session or its stream (player stop,
  `stopStream`, Plex `/stop` or `state=stopped`, replacing start, owner termination, expiry
  sweep, idle stream, server shutdown) stops the process, forgets addresses and secret, frees the
  directory and deletes the row, under the per-key `SessionLocks`. An automatic end re-checks
  its reason under the lock. Never end one half without the other.
- **Sessions are rows** (`playback_session`): every sign of life pushes `expires_at` 300 s, a
  `/streams` read counts at most every 30 s, a sweep every 30 s ends what expired and removes
  orphan transcode directories; startup and Ctrl-C clear everything. Client session ids are
  stored under a per-user name.
- **Termination** (`terminatePlaybackSession`, owners): the stream stops at once, the row stays
  so the next timeline carries `terminationCode` 2006 and the reason; a start under its key is
  refused meanwhile.
- A chunk whose process ended is refused after 5 s (not at once, so players don't spin); a
  running process keeps 20 s; an unknown or stopped session is `404` at once.
- Quality follows a fixed ladder of sizes and bitrates measured on the reference; adaptive rules
  live in `transcoder::adapt` (see `docs/plex/spec-gaps.md`). A fallback is always `NO_FALLBACK`.
- `transcoder.h264.preset` and `transcoder.h264.crf` are read at every start (`settings`), passed
  as `transcoder::plan::H264Tuning`, never as stored text. An empty CRF leaves each rung its
  measured factor; a chosen one applies to every rung, under its bitrate cap.
