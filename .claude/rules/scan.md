---
paths:
  - "crates/scanner/**"
  - "crates/core/src/scan.rs"
  - "crates/core/src/scan/**"
  - "crates/core/src/files.rs"
  - "crates/core/src/library.rs"
  - "crates/core/src/teletext.rs"
  - "crates/core/src/chapter.rs"
  - "crates/db/src/scan_repository.rs"
  - "crates/db/src/scan_repository/**"
  - "crates/db/tests/scan*.rs"
  - "crates/server/src/analyzer.rs"
  - "crates/server/src/extracts.rs"
  - "crates/server/src/files.rs"
  - "crates/server/src/commands.rs"
  - "crates/api/src/scan.rs"
  - "crates/api/src/library_management.rs"
  - "crates/api/src/server_directories.rs"
  - "crates/api-plex/src/scan.rs"
  - "docs/plex/scanner/**"
  - "tools/capture_scan_samples.py"
---

# Libraries, scanning and file analysis

How Plex does each part: `docs/plex/scanner/discovery.md`, `matching.md`, `analysis.md`.

## Media files are touched only by `klapz-ffmpeg` — no exception

The server never opens a media file to look inside it. Every action on one (probing, hashing,
measuring, decoding, extracting a track, subtitle, attachment, artwork or frame) goes through
`klapz-ffmpeg` or its ffprobe (`klapz-ffmpeg -probe`), run as children by
`klapz-scanner` (`engine/`: `probe`, `identify`, `loudness`, `fingerprint`, `extract_subtitle`,
`preview_index`, `chapter_thumbs`, `artwork`) and read into `klapz_scanner::report`. No
container/codec crate (`mp4`, `matroska`, `symphonia`, libav…), no hand-written byte parser. A
need ffmpeg-plex does not meet is a patch to ffmpeg-plex. The scanner creates no directory,
overwrites nothing, reaches no network, holds no state; every child dies with its future.
The walk itself reads names, sizes and times only, and opens no media file; the one file it
reads is `.plexignore` (`MAX_IGNORE_FILE`).

## Files and roots

- Library files only through `core::files::LibraryFiles` (`server::files::LocalFiles` today). A
  root is a backend string, below it a `RelativePath` that cannot climb out; the scan never
  builds a filesystem path.
- Roots are one of the two places a path is accepted (owners only, `createLibrary` /
  `updateLibrary`; `library::checked_roots`: 1–32, resolved by `resolve_root`, repeats kept
  once, none inside another). The other is `serverDirectories` (`list_directories`: names and
  paths only, hidden ones left out, `MAX_LISTED_DIRECTORIES`). Neither is a precedent.
- No roots change and no deletion while `SCANNING`. Deletion ends playbacks through
  `core::lifecycle` first, then removes everything of the library in one transaction; watch
  state and ratings (keyed by work) stay; files on disk are never touched.

## The scan (`core::scan`)

- One per library, in the background (`BackgroundScans`), lists before it reads: a library is
  browsable from names, sizes and times alone. Skip rule: a directory whose media kept names,
  sizes and mtimes is not looked at again.
- Grouping by the measured naming rules in `scan::layout` (CleanName, stacking, versions, 18
  episode patterns, dates, sidecars, `.plexignore`). Writes through `ScanRepository`, items via
  `db::write`.
- Removal is refused when a whole root is unreachable or the parent directory unreadable; an
  unreadable directory is skipped; a failed listing stops the scan into `Library.scanFailure`.
  A walk step over 30 s is a dead share.
- New/changed/forced files queue in storage, one row per task left (`media_task`,
  `scan::MediaTask`). `BackgroundAnalysis` does one task per file at a time, each kind in its own
  pool sized by `analysis.concurrency.{headers,deep,subtitles,preview_thumbnails}` (2/1/1/1,
  1–16), read before every round. Headers first (`MediaAnalyzer`, ≤ 1 min, always), which queue
  what the file calls for: packets (deep, ≤ 10 min, headers standing if it cannot finish),
  embedded subtitles, seeking thumbnails (BIF).
- Subtitles and thumbnails go through `MediaExtractor` and live on disk by content hash, not in
  storage (`server::extracts`, `KLAPZ_EXTRACTS_DIR`), built in a working directory and moved in
  whole; found there, not made again for any copy; no content hash, no extraction.
- `analysis.deep`, `analysis.subtitles`, `analysis.preview_thumbnails` switch tasks on and off,
  server-wide and per library: a task switched off stays queued; a change wakes the queue
  (`ScanControl::settings_changed`).
- A result from a file changed since is dropped; scan and analysis write in turn; a library
  being walked is not read. `startStream` / `readMediaFile` read headers at once without
  waiting.
- Failures are classified: the media's (recorded on the file), the tools' (queue held), the
  file's absence (library waits for next scan). Cancel drops scan and matching at once.
- `force` analyzes every file again; Plex's `refresh?path=` is `403`.

## Deliberate rules on stored tracks

- Rates and durations stored are the deep analysis' measured ones, not the declared ones.
- One untagged audio track + a whole word of the filename: `VFQ` → `fr-CA`; `VFF`, `VFI`,
  `FRENCH`, `TRUEFRENCH` → `fr-FR`; `VFQ` wins; a tag or a second audio track disables it
  (`scan_repository::analysis::dub_language`).
- Teletext: one subtitle row per page ffprobe-plex names (declared, then undeclared), sharing the
  stream's `index` (`track_rows`).
- What ffprobe states as printed is kept beside Plex's reading (`stated`), for Jellyfin.

Fixtures are real ffprobe output (`crates/scanner/tests/fixtures`, `crates/db/tests/scan_samples.rs`),
captured by `tools/capture_scan_samples.py`.
