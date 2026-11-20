---
name: reference-servers
description: How to measure behavior on a reference Plex Media Server or Jellyfin without damaging it, and how to compare klapz against it. Load before any capture, probe, experiment or comparison against a reference server, before writing a capture or compare script, and whenever a question about "what Plex does" could be answered by measuring instead of guessing.
---

# Working against a reference server

A reference server is the arbiter of every behavioral question (CLAUDE.md, "Reference server").
It is usually also someone's real server, with real accounts and real watch state. Measuring is
cheap; repairing is not.

## Where things are

**Plex** (`PLEX_URL`, default `http://127.0.0.1:32400`):

| What | Windows | Linux | macOS |
| --- | --- | --- | --- |
| Data | `%LOCALAPPDATA%\Plex Media Server\` | `/var/lib/plexmediaserver/Library/Application Support/Plex Media Server/` | `~/Library/Application Support/Plex Media Server/` |
| Token | registry `HKCU\Software\Plex, Inc.\Plex Media Server`, `PlexOnlineToken` | `Preferences.xml`, `PlexOnlineToken` | `Preferences.xml`, `PlexOnlineToken` |

Below the data folder: `Logs/` (`Plex Media Server.log`, and one log per scanner stage),
`Plug-in Support/Databases/com.plexapp.plugins.library.db` (the live database) and
`Metadata/…/<sha1(guid)>.bundle/` (artwork). Open the live database read-only (`?mode=ro`). A
snapshot copied earlier drifts from the live server: rule out an item edited since before
suspecting a rule.

**Jellyfin** (`http://localhost:8096`, swagger at `/api-docs/swagger/index.html`): credentials
from `JELLYFIN_USER` / `JELLYFIN_PASSWORD`, never from a file. Without a media-segment plugin,
`/MediaSegments` is empty for every file.

## Test libraries

Create libraries of your own for anything that writes: a film library and a series library with
the agent set to **none** (personal media), over a folder of a few files. Small and without
artwork, they are also where a fixture can be byte-identical to the reference, JSON and XML.

- Put **copies** of the files there (truncated ones are fine), never hard links: Plex's scanner
  dropped an episode that was a hard link.
- With the agent `none`, matching, intro and credits detection answer `403`/`404`; studying them
  needs a library with a real agent, which writes metadata.
- Whole-library comparisons belong on a copy of klapz's own database, not on the reference.

## Rules for every request

- **Token in the `X-Plex-Token` header only**: never in a URL, a file, a printed line or an
  exception message. Read it inside the script; catch exceptions and print the status alone.
- **Check every id's type before a write.** Plex shares one id space across kinds:
  `PUT /playlists/{id}?title=x` with a film's id renames the *film*.
- **Record the ids you create, and delete only those.** A cleanup by filter ("every history entry
  of this library") deletes other accounts' entries, which cannot be recreated.
- **Never write watch state, progress or ratings through a real account on a real item.**
  Ratings may be synced to plex.tv.
- **Never change the server's preferences, its library agents or its plex.tv registration.**
- Restore whatever a test changed: watch state, track choices, any setting changed for it.

## Calls that are forbidden outright

| Call | Why |
| --- | --- |
| `PUT /library/metadata/{id}/match`, `/unmatch`, fix-match, `/refresh` on an item that could share a guid with a real one | They move `metadata_item_settings` — every account's watch state — between guids **server-wide**. In a `none` library the move round-trips, so no API undoes it: matching a test copy of a film to its real guid moved every account's state for that film onto the test item. |
| `/library/sections/{id}/refresh?force=1` outside the test libraries | `path=` is ignored and the whole section's metadata is refreshed. Expect side effects even on test libraries: an episode lost its title and air date. |

Some *reads* write too: `GET /library/metadata/{id}/posters` caches the provider's image list as
taggings on the item. Prefer the test libraries even for reads that look free.

Record anything left changed in the "side effects" section of the `docs/scanner/*.md` page of
the work. Repairing the database itself is SQL with Plex **stopped**: back the file up beside
the original, write with the `Plex SQLite` binary shipped with the server, `PRAGMA quick_check`,
restart, verify through the API.

## Where answers come from when the API is silent

1. **The server's log**, at debug level. The exact transcoder command line of a session is logged
   even when the job dies at once; so are the reasons for its `400`s and its plex.tv calls.
2. **The binary.** It keeps SQL, log formats and URL templates as plain strings: extract printable
   runs, and search for the whole call, not fragments, before calling something unknown.
3. **Plex Web**, the bundle the server ships: which settings and attributes a client actually
   reads, and the rules it applies.
4. **The spec**, last: it is wrong often enough (`docs/spec-gaps.md`).

Without a Plex Pass on the reference account, the success path of gated features (markers,
session termination, the provider's extras) cannot be measured; its shape then comes from the
binary, Plex Web or the spec. Say so where it is used.

## Capturing tricks

- **A conversion is started with `decision` first, then `start`**, with Plex Web's full parameter
  set (`addDebugOverlay`, `autoAdjustQuality` included): `start` alone answers `400`. Then read
  the ffmpeg arguments in the log rather than infer them from another mode.
- **A shelf drawn at random is cached a while**: add a dummy parameter (`?nonce=N`) per request
  to get independent draws.
- On Windows, `curl … | python` reads stdin as cp1252: mojibake there is the pipe's, not the
  server's. Check the raw bytes first.

## Measuring a rule

- **Simulate it over whole libraries, then sweep its constant.** The recently-added batching was
  established by reproducing 200 of 200 entries, and by showing any window from 1156 to 1215
  minutes gives the same result — which is what makes "20 hours" a measurement, not a guess.
- **Replay a trace.** For a stateful surface (play queues), record a sequence of mutations on the
  reference, replay it on ours, diff ids, versions and order.
- **Cover every kind in the sample.** An attribute order derived from listings was wrong for
  episodes, which listings never contain. Use `/children` and `/grandchildren` too.
- **Filter to the population you talk about** before quoting a ratio: "14 % of items have
  several versions" was 0.06 % once extras were left out.
- **Re-verify every figure you quote** after each fix.
- When a measurement and a settled decision in CLAUDE.md disagree, the decision wins: apply it
  and record where the measurement differs.

## Comparing klapz with a reference

- Rebuild and restart before a live comparison: a server running an older binary shows nothing of
  the change.
- Run a private server on a port of its own over a **copy** of `klapz.db`, taken through
  SQLite's backup API if a server holds the file open. Sign in as `admin`
  (`tools/klapz_session.py`). Stop it and delete the copy afterwards.
- For the scanner: scan the folder the reference scanned into a throwaway database, then diff
  `Media`, `Part`, `Stream` and `Chapter` field by field, aligning tracks by `index`.
  `tools/capture_scan_samples.py` and `crates/db/tests/scan_samples.rs` are the committed form.
- Anything that runs ffprobe uses `KLAPZ_FFPROBE`'s (`ffmpeg -probe`), not a stock ffprobe:
  they disagree on languages (`fr` against `fre`, `eng` on Matroska video).
- New sample files: query the reference database read-only for the smallest file of each case,
  avoid names with a real or look-alike `:`, cut a large one without re-encoding
  (`ffmpeg -i X -map 0 -c copy -t 120 Y`). Lists of library paths are personal data: never
  commit them.
