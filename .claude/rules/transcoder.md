---
paths:
  - "crates/transcoder/**"
  - "crates/api-plex/src/photo.rs"
  - "crates/api-plex/src/ultrablur.rs"
  - "crates/api-plex/src/subtitles.rs"
  - "crates/core/src/backdrop.rs"
  - "files/Resources/Profiles/**"
  - "tools/capture_transcoder.py"
---

# The media engine (`crates/transcoder`)

To media what `api` is to data: speaks in images, sizes and formats, knows nothing of HTTP or
Plex. Parameter names, statuses and Plex quirks stay in `api-plex`; which URLs may be fetched
stays here. Built to run on its own one day.

- **Every media operation is a `klapz-ffmpeg` child** (`KLAPZ_TRANSCODER`). The engine never
  opens a media file to read it: no libav bindings, no container parsing, no decoding in
  process. It only hands the file to the child, or sends its bytes unchanged. `plan.rs` builds
  the command line, `session.rs` runs it (`session_status` reports the live conversion),
  `adapt.rs` holds both adaptive rules.
- **Argument injection** is the main risk: a client value never becomes an option; files reach
  ffmpeg as `file:…`. Values from settings arrive as engine types (`plan::H264Preset`), and a
  test keeps setting choices and engine names in step.
- Client-adaptive segments rely on ffmpeg-plex's `absolute_segments` (grid on the media's
  timeline, so every rung and restart agree). Picture subtitles as TTML need its `imsc_image`;
  burned subtitles use the reference's `inlineass` filter and `KLAPZ_TRANSCODER_FONTS`
  (without fonts a burn answers `501`).
- **Images are the one in-process exception**: the `image` crate, memory-safe decoders on a
  format allowlist. Fetching an image: TLS to `plex.tv` and its subdomains only, checked on every
  redirect hop; anything else `403`. A drawn gradient's area is capped at 3840×2160.
- **Backdrop colors** (`backdrop/`): the reference's algorithm read from its binary
  (median-cut palette per quarter, Okhsl scoring); exact on PNGs, JPEGs decode differently. Noise
  is seeded so an image never changes.
- **Subtitle sidecars**: resolved by `media::resolve_sidecar`, only within the library and in the
  media's directory or below, never the media itself; served UTF-8 `text/plain` with `nosniff`
  (a non-UTF-8 file is read as Windows-1252), never `text/html`.
- Plex's per-client transcode decisions are in `files/Resources/Profiles/` (XML).
