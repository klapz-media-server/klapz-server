---
name: reference-servers
description: How to measure behavior on the reference Plex Media Server and the reference Jellyfin without damaging them, and how to compare open-plex against them. Load before any capture, probe, experiment or comparison against 127.0.0.1:32400 or localhost:8096, before writing a capture or compare script, before briefing an agent that will touch a reference server, and whenever a question about "what Plex does" could be answered by measuring instead of guessing.
---

# Working against the reference servers

The reference servers are the arbiter of every behavioral question (CLAUDE.md, "Reference
server"). They are also the owner's real servers, holding real accounts and real watch state.
Measuring is cheap; repairing is not. This page is what earlier sessions learned the hard way.

## Where things are

**Plex** (`http://127.0.0.1:32400`, version 1.43.5):

| What | Where |
| --- | --- |
| Install | `C:\Program Files\Plex\Plex Media Server\` (and a copy of it in `files/`) |
| Data | `%LOCALAPPDATA%\Plex Media Server\` |
| Logs (DEBUG) | `Logs\Plex Media Server.log`, `Plex Media Scanner Analysis.log`, `… Deep Analysis.log`, `… Matcher.log`, `… Chapter Thumbnails.log` |
| Live database | `Plug-in Support\Databases\com.plexapp.plugins.library.db`, its blobs beside it |
| Artwork | `Metadata\{Movies,TV Shows,Collections}\<h>\<sha1(guid)>.bundle\Contents\_combined\…` |
| Token | registry, `HKCU:\Software\Plex, Inc.\Plex Media Server`, value `PlexOnlineToken` |

Open the live database with `?mode=ro` only. The snapshot under `db/` is older than the live
server (taken 2026-10-25): a difference on an item edited since is **snapshot drift**, a normal
category in every comparison, not a bug. Rule it out before suspecting a rule.

The reference does not restart after a reboot of the machine. Check it answers before measuring.

**Jellyfin** (`http://localhost:8096`, version 12.4.0, swagger at `/api-docs/swagger/index.html`):
credentials come from `JELLYFIN_USER` / `JELLYFIN_PASSWORD`, never from a file. Its libraries
read the samples share (mapped as `Z:`; `/Library/VirtualFolders` lists the roots). It has no
media-segment detector, so `/MediaSegments` is empty there for every file.

## The test libraries — the only things you may change

| Server | What | Ids |
| --- | --- | --- |
| Plex | section 27, "open-plex tests", films, agent `none` | Soul, item 577986 |
| Plex | section 28, "open-plex tests series", agent `none` | Bull, items 577987–577990 (episode 578173 among them) |
| open-plex (imported copy) | seeded `Test` library, section 21 | items 577336–577338 |

Both Plex test libraries read the owner's `Documents\TEST` folder: Soul 2020 (1080p H.264, two
EAC3 tracks fr/en, three SRT sidecars) and Bull 2016 S05E01–E02 (720p H.264, AAC). The main
libraries' files live on another machine: their `/decision` works from metadata alone, but a
transcode `start` answers `400` or dies. Because the agent is `none`, matching, intro and credits
detection answer `403`/`404` there; studying them would take a third library with a real agent,
which needs the owner's go-ahead.

Being tiny and without artwork, the test libraries are where a fixture can be **byte-identical**
to the reference, JSON and XML. Whole-library comparisons belong on a copy of our database.

## Rules for every request

- **Token in the `X-Plex-Token` header only**: never in a URL, a file, a printed line or an
  exception message. Read it from the registry inside the script. Catch exceptions and print the
  status alone: Git Bash once rewrote an argument, and the resulting error message printed the
  URL with the token in it. A token the owner pastes in chat is never reused.
- **Check every id's type before a write.** Plex shares one id space across kinds:
  `PUT /playlists/577986?title=x` renamed the *film* Soul.
- **Record the ids you create, and delete only those.** A cleanup that deleted "every history
  entry of type 1 in library 28" removed 41 entries that were not the agent's; they cannot be
  recreated.
- **Never write watch state, progress or ratings through the owner's real account** on a real
  item. Ratings may be synced to plex.tv.
- **Never change the reference's preferences, its library agents or its plex.tv registration.**
  Ask before proxying it.

## Calls that are forbidden outright

| Call | Why |
| --- | --- |
| `PUT /library/metadata/{id}/match`, `/unmatch`, fix-match, `/refresh` on an item that could share a guid with a real one | They move `metadata_item_settings` (every account's watch state) between guids **server-wide**. In a `none` library the move round-trips, so no API undoes it: 46 accounts' state on the real *Soul* ended up on test item 577986, repaired only by SQL with Plex stopped (see `docs/scanner/matching.md`, side effects). |
| `/library/sections/{id}/refresh?force=1` outside 27/28 | `path=` is ignored and the whole section's metadata is refreshed: episode 578173 lost its title and air date. Expect side effects even on 27/28. |

Some *reads* write too: `GET /library/metadata/{id}/posters` cached about 150 provider-image
taggings on a real item. Prefer the test libraries even for reads that look free.

## What cannot be undone, already left behind

Expect to meet these in captures: genre tags "Kung Fu", "Zorglub", "Aaa", "Bbb", "SciFi" and a
label "X"; an inert `at:offset=0` on stream 12262676; Bull's `lastViewedAt` moved (no API sets a
past one); 41 history entries of library 28 gone; play queues of test client identifiers (no API
deletes one); orphaned type-12 extras with no section. Record anything new you leave behind in
the "side effects" section of the `docs/scanner/*.md` page of the work.

Repairing the database itself is SQL with Plex **stopped**: back the file up beside the
original, write with `Plex SQLite.exe`, `PRAGMA quick_check`, restart, verify through the API.
Wait for any agent still working against the server.

## Where answers come from when the API is silent

1. **The reference's own log.** The exact `Plex Transcoder` command line of a session is logged
   even when the job dies at once; so are the reasons for its `400`s and its plex.tv calls.
2. **The binary.** `files/` keeps SQL, log formats and URL templates as plain strings. There is
   no `strings` tool here: extract printable runs with a few lines of Python, and search for the
   whole call, not fragments, before calling something unknown.
3. **Plex Web**, the bundle in `web/`: which settings and attributes a client actually reads
   (10 of 162 server settings, 2 of a library's), and the rules it applies.
4. **The spec**, last: it is wrong often enough (`docs/spec-gaps.md`).

The reference account has no subscription (`myPlexSubscription="0"`): the success path of
gated features — markers, terminate (`401`), the provider's extras — cannot be measured, and its
shape has to come from the binary, Plex Web or the spec. Say so where it is used.

## Capturing tricks

- **A conversion is started with `decision` first, then `start`**, with Plex Web's full parameter
  set (`addDebugOverlay`, `autoAdjustQuality` included): `start` alone answers `400`. The ffmpeg
  arguments are then in the log — read them rather than infer them from another mode (an HLS
  seek's `-ss` and `-segment_start_number` were wrong until the log was read).
- **A shelf drawn at random is cached a while**: add a dummy parameter (`?nonce=N`) per request
  to get independent draws.
- **Files added to the test folder are copies** (truncated ones are fine), never hard links:
  Plex's scanner dropped an episode that was a hard link.
- Restore whatever a test changed besides the library: watch state, track choices, and any
  setting changed for it (`LanNetworksBandwidth` was).

## Measuring a rule

- **Simulate it over whole libraries, then sweep its constant.** The recently-added batching was
  established by reproducing 200 of 200 entries, and by showing any window from 1156 to 1215
  minutes gives the same result — which is what makes "20 hours" a measurement, not a guess.
  Scripts go in `.work/` or the scratchpad.
- **Replay a trace.** For a stateful surface (play queues), record a sequence of mutations on the
  reference, replay it on ours, diff ids, versions and order.
- **Cover every kind in the sample.** An attribute order derived from 480 listing items was wrong
  for episodes, which listings never contain. Use `/children` and `/grandchildren` too.
- **Filter to the population you talk about** before quoting a ratio: "14 % of items have
  several versions" was 0.06 % once extras were left out.
- **Re-verify every figure you quote**, after each fix. Two rules written as "measured" had to be
  corrected on whole-library data.
- When a measurement and the owner's instruction disagree, the instruction wins: apply it, say
  where the measurement differs, record it in CLAUDE.md as their decision.

## Comparing open-plex with a reference

- Rebuild and restart before a live comparison: a server running a binary built before the
  change shows nothing of it.
- Run a private server on a port of its own (325xx, never the live 32500) over a **copy** of
  `open-plex.db`, taken through SQLite's backup API since the live server holds the file open.
  Sign in as `admin` (`tools/open_plex_session.py`). Stop it and delete the copy afterwards.
- Database copies made from the 2026-11-11 backup predate test libraries 27/28 on our side:
  take a fresh one.
- For the scanner: scan the folder the reference scanned into a throwaway database, then diff
  `Media`, `Part`, `Stream` and `Chapter` field by field, aligning tracks by `index`.
  `tools/capture_scan_samples.py` and `crates/db/tests/scan_samples.rs` are the committed form.
- Anything that runs ffprobe must use `OPENPLEX_TRANSCODER`'s (`ffmpeg -probe`), not a stock
  ffprobe on `PATH`: they disagree on languages (`fr` against `fre`, `eng` on Matroska video).
- New sample files: query the reference database read-only for the smallest file of each case,
  avoid names with a real or look-alike `:`, cut a large one without re-encoding
  (`ffmpeg -i X -map 0 -c copy -t 120 Y`). The lists hold library paths: keep them in the
  scratchpad, never in git.
- On Windows, `curl … | python` reads stdin as cp1252: mojibake there is the pipe's, not the
  server's. Check the raw bytes first.
