# Media analysis and derived assets on the reference

What Plex Media Server 1.43.5.11029 computes from a media file once the scanner has
attached it to an item, how, where it keeps the result, and where that result belongs in our
schema. This document is the basis for the analysis half of an open-plex scanner: a separate
binary that reads files and writes through the GraphQL API.

Every statement carries its source:

- **measured**: observed on the live reference (`127.0.0.1:32400`), in its logs, its database
  (opened read-only) or its files, or by running its binaries on the test libraries 27 and 28;
- **binary**: read from strings in `Plex Media Scanner.exe` / `Plex Media Server.exe` (help text,
  log formats, command templates, symbol names), not observed in action;
- **inferred**: a conclusion drawn from the above that no test settled.

Population figures come from the live database on 2026-11-17: 241,295 parts, 240,416 video,
379,889 audio and 321,490 subtitle tracks. Most of the real library's files sit on an
unreachable network mount, so every job the reference attempts on them today fails; what it
holds was computed when they were reachable.

## Overview

| Task | Runs in | Trigger | Writes | Our schema |
| --- | --- | --- | --- | --- |
| Media analysis | `Plex Media Scanner --analyze` (in-process libav) | new or changed file, `PUT …/analyze`, butler *UpgradeMediaAnalysis* | `media_items`, `media_parts`, `media_streams` columns and `ma:` keys; chapters (taggings type 9) | `media`, `media_part`, `media_stream`, `stream_dolby_vision`; chapters: **none** |
| Thumbnails / art from video | same process, after analysis | analysis without `--no-thumbs` | bundle `Thumbnails/thumbN.jpg`, `Art/artN.jpg`, `user_thumb_url` / `user_art_url` | no artwork kept |
| Deep analysis | `Plex Media Scanner --analyze-deeply` | butler *DeepMediaAnalysis* | per-track `bitrate`, `ma:requiredBandwidths`, `ma:deepAnalysisVersion`, `GoP-N.xml` | `media_stream.required_bandwidths`, `bitrate` |
| Video preview thumbnails (BIF) | PMS spawns `Plex Transcoder` | `PUT …/index`, after analysis (`GenerateBIFBehavior`), butler *GenerateMediaIndexFiles* | `Indexes/index-sd.bif`, `mi:indexes`, `mi:failureBIF*` | **none** |
| Chapter thumbnails | `Plex Media Scanner --generate --chapter-thumbs-only` | `PUT …/chapterThumbs`, butler *GenerateChapterThumbs* | `Chapters/chapterN.jpg`, `taggings.thumb_url` | **none** |
| Intro detection | PMS + `Plex Media Fingerprinter` (Chromaprint) | `PUT …/intro` on a season or show, butler *GenerateIntroMarkers* | blob type 5 per part, taggings type 12 `intro`, `pv:intros` | `item_marker` (imported) |
| Credits detection | `Plex Media Scanner` + `Plex Transcoder` + OpenCV EAST model | `PUT …/credits`, butler *GenerateCreditsMarkers* | taggings type 12 `credits`, `pv:credits`, `pv:creditsDetectionErrorCount` | `item_marker` (imported) |
| Loudness | `Plex Media Scanner --analyze-loudness` (libebur128) | butler *LoudnessAnalysis*, `PUT …/analyzeLoudness` | `ln:` keys on audio tracks | `stream_loudness` |
| Voice activity | PMS, Silero VAD (ONNX) | butler *GenerateVoiceActivity* (disabled here), `PUT …/voiceActivity` | `pv:voiceActivityDetectionVersion` | none, never ran |
| Ad (commercial) markers | `Plex Commercial Skipper` (comskip) | `PUT …/addetect`; DVR recordings only by default | taggings type 12 `commercial` | `item_marker` kind `commercial` |

## 1. Media analysis (`--analyze`)

### When it runs

**Measured.** `PUT /library/metadata/{id}/analyze` makes PMS spawn, with
`FFMPEG_EXTERNAL_LIBS` (the codec pack directory) and `X_PLEX_TOKEN` in the environment:

```
Plex Media Scanner.exe --analyze --manual --server-action index,chapterThumbs,intro,addetect --item 578173 --log-file-suffix " Analysis"
```

When it finishes, the scanner calls back PMS for each `--server-action`: `PUT …/addetect`,
`PUT …/index?force=0`, `PUT …/chapterThumbs?force=0&manual=1` and `PUT …/intro?force=0`. A
scan of new files runs `--analyze --item a,b,…` without `--manual`. During the run, the
scanner calls `/:/metadata/notify/changeItemState?…&mediaState=analyzing`, then `thumbnailing`,
then `idle`, so that clients can show progress, and calls `/library/changestamp` before every
save (**measured**).

Analysis on an already-analyzed file whose size and mtime are unchanged rewrote nothing
except `metadata_items.changed_at` (**measured**, item 578173).

`thumbOffset` and `artOffset` on `PUT …/analyze` become the scanner's `--thumbOffset` and
`--artOffset <percent>` (**binary**, help text). `--no-thumbs` skips image generation.

### How

**Measured.** The scanner opens the file **in process** with Plex's libav DLLs (`[MI] Opening
input file`, then `[FFMPEG] …` debug lines from `avformat_find_stream_info`). It reads only the
file's headers and the probe window: 2 ms for a 20-second file, 10 ms for a 1 GB episode.
It logs one summary line per part:

```
[MI]  * 26c23ba303ef7c749c5576d8617a445ab1ec2f07 [mkv h264/aac] Duration: 2467160ms, 1280x720 (1.777778) 25.000fps, bitrate=3208kbps, audio channels=2, optimizedForStreaming=0
```

The hex value is `media_parts.hash`. `[MI] Chapters had generic titles, clearing.` drops
chapter titles of the form *Chapter N* (**measured**; regex `(?:chapter)?\.? ?(\d+)` in the
binary). The binary also logs `[MI] Aborting chapter import because there are more than %d
chapters`.

### What it writes, and what ffprobe gives for it

All measured on the three test files (two 720p H.264/AAC episodes and one 1080p H.264 film with
two E-AC-3 tracks and three SRT tracks), compared with `ffprobe -show_streams -show_format`
from the patched ffmpeg-plex 8.1.3 and the `trace_headers` bitstream filter.

**`media_items`** (our `media`):

| Reference | Value | ffprobe source |
| --- | --- | --- |
| `container` | `mkv` | `format_name` mapped (`matroska,webm` → `mkv`) |
| `duration` | ms | `format.duration` |
| `bitrate` | 7726375 (bps) | `format.bit_rate`, exact, before deep analysis |
| `width`, `height` | 1920×1080 | the first video track |
| `display_aspect_ratio` | 1.7777777910 (float32) | width / height × SAR |
| `frames_per_second` | 23.976024627685547 | `r_frame_rate` 24000/1001 as float32 |
| `video_codec`, `audio_codec`, `audio_channels` | `h264`, `eac3`, 6 | first video, first (default) audio |
| `color_trc` | `bt709` | video `color_transfer` |
| `media_analysis_version` | 8 | the analysis' version (see below) |
| `extra_data` `ma:videoProfile`, `ma:audioProfile` | `high`, `lc` | track profiles, lower-cased |

**`media_parts`** (our `media_part`): `hash` (40 hex), `open_subtitle_hash`, `size`,
`duration`, `updated_at` (equals the file's mtime: 1620467892 for an episode whose mtime is
1620467892). `open_subtitle_hash` is the OpenSubtitles hash: file size plus the 64-bit
little-endian words of the first and last 64 KiB, which reproduces `84af7a8426a039b5` exactly.
`hash` matched none of SHA-1 over the first 64 KiB, the first MiB, the first and last 64 KiB,
or the path; this is the scanner's identity hash, which `docs/scanner/matching.md` covers.
Part `extra_data`:

| Key | Count | Meaning |
| --- | --- | --- |
| `ma:container` | 240,381 | as on the media |
| `ma:videoProfile`, `ma:audioProfile` | 155,788 / 54,916 | as on the media |
| `ma:optimizedForStreaming` | 28,714 | MP4 `moov` before `mdat` |
| `ma:has64bitOffsets` | 8,558 | MP4 `co64` |
| `ma:hasThumbnail` | 10,051 | embedded cover art |
| `ma:packetLength` | 272 | MPEG-TS 188/192 |
| `ma:hasChapterTextStream` | 19 | chapters carried as a text track |
| `pv:chapters` | 157,199 | JSON `{"Chapters":{"Chapter":[{"name","start","end"}…]}}`, seconds as floats; `{"Chapters":{}}` without chapters (62,823 parts have some) |

Our `media_part` already carries `has_thumbnail`, `has_64bit_offsets`,
`optimized_for_streaming`, `packet_length`, `has_chapter_text_stream`, `content_hash`
and `open_subtitles_hash`.

**`media_streams`** (our `media_stream`): `codec` (`subrip` → `srt`), `index` (ffmpeg's
stream index, our `position`), `language` (ISO 639-2 → 639-1: `fre` → `fr`; a Matroska video
track without a language tag gets `en`, Matroska's default), `channels`, `bitrate`, `default`,
`forced`. Video `extra_data`, every value a string:

| Key | Test value | ffprobe source | Reproduced |
| --- | --- | --- | --- |
| `ma:profile` | `high` | `profile` lower-cased | yes |
| `ma:level` | `40`, `31` | `level` | yes |
| `ma:width`, `ma:height` | 1920, 1080 | `width`, `height` | yes |
| `ma:codedWidth`, `ma:codedHeight` | 1920, **1088** | SPS `(pic_height_in_map_units_minus1+1)×16`; ffprobe's `coded_height` says 1080 | via `trace_headers` |
| `ma:refFrames` | 4, 2 | SPS `max_num_ref_frames`; ffprobe's `refs` says 1 | via `trace_headers` |
| `ma:hasScalingMatrix` | 0 | SPS `seq_scaling_matrix_present_flag` | via `trace_headers` |
| `ma:bitDepth` | 8 | `bits_per_raw_sample` | yes |
| `ma:chromaSubsampling` | `4:2:0` | from `pix_fmt` | yes |
| `ma:chromaLocation`, `ma:colorRange`, `ma:colorSpace`, `ma:colorPrimaries`, `ma:colorTrc` | `left`, `tv`, `bt709`… | same names | yes |
| `ma:frameRate` | `23.976`, `25.000` | `r_frame_rate`, three decimals | yes |
| `ma:scanType` | `progressive` | `field_order` | yes |
| `ma:requiredBandwidths` | eight values | deep analysis (§2) | no |

Other video keys in the library: `ma:title` (30,935), `ma:streamIdentifier` (8,846),
`ma:pixelAspectRatio` + `ma:anamorphic` (7,856), `ma:codecID` (6,368, the FourCC),
`ma:original` / `ma:dub`, `ma:closedCaptions` (1,182), `ma:headerCompression`,
`ma:separateFields`, `ma:HDR10PlusPresent` (173) and the eight `ma:DOVI*` keys (2,651:
`DOVIPresent`, `DOVIVersion`, `DOVIProfile`, `DOVILevel`, `DOVIBLCompatID`, `DOVIBLPresent`,
`DOVIELPresent`, `DOVIRPUPresent`). ffprobe exposes the last two groups as `side_data_list`
entries (*DOVI configuration record*, HDR10+ via frame side data, which requires reading frames,
not only headers: **inferred**).

Audio: `ma:samplingRate`, `ma:audioChannelLayout` (`5.1(side)`, `stereo`: ffprobe's
`channel_layout` verbatim), `ma:profile` (`lc`, `dolby digital plus + dolby atmos`: ffprobe's
`profile` lower-cased), `ma:bitDepth` (lossless only), `ma:title`, `ma:visualImpaired`,
`ma:hearingImpaired`, `ma:comment`. Subtitles: `ma:title`, `ma:format` (sidecars),
`ma:embeddedInVideo` (EIA-608 in the video), `ma:hearingImpaired`. All of these already have
typed columns in `media_stream` and `stream_dolby_vision`.

**Conclusion (measured):** ffprobe from ffmpeg-plex, plus `trace_headers` for three SPS fields,
reproduces every per-track value of the normal analysis on the test files. Two `media_items`
values differ by origin: `bitrate` is the container's, and the video track's `bitrate` is
`format.bit_rate` minus the audio tracks' declared bitrates (Soul: 7726375 − 256000 − 768000
= 6702375, stored exactly). Bull's video track stored the format bitrate itself (3208969),
because its AAC track declares no bitrate in the header (**inferred**).

### Versions

`media_items.media_analysis_version`: 8 on 17,129 items, 7 on 134,070, 6 on 5,991 and 0 on 84,091
(never analyzed: files unreachable when added) (**measured**). The butler's
*UpgradeMediaAnalysis* re-analyzes anything below the current version (**binary**: task name;
the version comparison was not observed).

### Chapters

**Measured.** Chapters are stored twice: in the part's `pv:chapters` JSON (seconds) and as
taggings of **tag type 9**, one per chapter, `index` from 1, `time_offset` / `end_time_offset` in
ms (rounded to 10 ms: 1248.664 s → 1248660), the tag's `tag` being the chapter title and
`thumb_url` `/library/media/{media_id}/chapterImages/{index}` when a thumbnail exists (418,442
of 519,141 rows on 64,251 items). `GET /library/metadata/{id}?includeChapters=1` publishes
`<Chapter id="{tag id}" filter="thumb={tag id}" tag=… index=… startTimeOffset=…
endTimeOffset=…/>` and `chapterSource="media"` on the item. **Our schema has no chapters**: a
`chapter` table (part or media, index, start/end ms, title) is needed, and `pv:chapters` is the
importable source.

## 2. Deep analysis (`--analyze-deeply`)

**Measured** on episode 578173 (`--analyze-deeply --force --item 578173`; without `--force` it
did nothing, the part being at `ma:deepAnalysisVersion` 6):

- It **demuxes every packet without decoding**: 170,000 packets of a 41-minute 1 GB file in
  0.3 s (`Deep Analysis: Read N packets at T`), then sorts them and runs a *file bitrate
  analysis* and a *stream bitrate analysis* per track.
- It sets each track's `bitrate` to the measured average: video 3208969 → **3110461**
  (Matroska's `BPS` tag says 3110461, identical), subtitle 77 → 76, audio unchanged at 96000.
  The media's `bitrate` became 3206539.
- It writes `ma:requiredBandwidths` on the part and on every track: eight kilobit values, one
  per buffer size from the smallest (for 1-kbps subtitles, `1,1,1,1,1,1,1,1`; values of
  2147483647 occur for a buffer no bandwidth fills). The forced run recomputed the same values.
- It saves `GoP-{stream index}.xml` in the part's bundle (`Contents/GoP-0.xml`), one `<Group
  pts=… duration=… size=…/>` per GoP in ms and bytes under `<VideoStream timebaseNumerator="1"
  timebaseDenomenator="1000">` (the misspelling is Plex's). The forced run rewrote it
  byte-identical.
- `pv:deepAnalysisDate` is **not** the date of the analysis: it equals the file's mtime on all
  three test files (1620467892, 1620468101, 1610180026) and did not move with the forced run.
- `ma:deepAnalysisVersion` is `6` on all 156,677 deeply analyzed parts.

The transcoder decides whether to copy the video by comparing a client's bandwidth with
`requiredBandwidths` (`docs/spec-gaps.md`, *Quality follows a fixed ladder*; **binary**:
*Cannot direct play this item. Required bandwidth is not known and only {2}kbps is available.*).

**Not reproduced.** The buffer sizes behind the eight values were not found. On Soul, video
stored `10639,9885,9182,9033,9033,9033,9033,9033` against an average of 6700 kbps; neither a
startup-delay model (`max_t S(t)/(t+d)`), nor sliding windows, nor a leaky bucket of fixed size
(`max_t (S(t)−B)/t`) gives a sequence that levels off at 9033 for the five largest buffers. Our
tools do have everything needed (`ffprobe -show_entries packet=stream_index,pts_time,size`
dumps Soul's 525,138 packets in 7 s), so this remains a question of finding the rule; see the
open questions.

## 3. Thumbnails and art taken from the video

**Measured** (analysis log): for an item without artwork, the analysis grabs three frames at
**25 %, 50 % and 75 %** of the duration, decodes each in process and writes
`Contents/Thumbnails/thumb1.jpg` … `thumb3.jpg`, fitted inside 720×512 (720×405 for 16:9). For
a film it also writes `Contents/Art/art1.jpg` … `art3.jpg` at 1280×720: the art and thumbnail
of the same number are the same frame (identical PSNR against any reference frame). The film's
frames did not match exact or key-frame seeks at 25/50/75 % (PSNR 12–26 dB), so the film's
offsets are not settled: `--thumbOffset` / `--artOffset` exist for that purpose.

The bundle is the part's: `Media/localhost/{hash[0]}/{hash[1:]}.bundle/Contents/…`, referenced
from `metadata_items.user_thumb_url` / `user_art_url` as `media://{hash[0]}/{hash[1:]}.bundle/
Contents/Thumbnails/thumb3.jpg`. On all three test items the chosen image was the **smallest
JPEG** of the three (thumb3 of 24014/21697/14858 bytes, thumb1 of 17930/19243/18373, art3 of
61669/76534/28864): a coincidence on three cases, not a rule.

The blur hashes and UltraBlur corner colors (`pv:thumbBlurHash`, `pv:artBlurHash`,
`pv:ultraBlur*`) are computed from the chosen image afterwards, by PMS (already in
`item_corner_colors`; `transcoder::backdrop` reproduces the algorithm).

Our server keeps no artwork today; an open-plex scanner would produce these with ffmpeg-plex
(`-ss`, one frame, scaled) and needs somewhere to store images.

## 4. Video preview thumbnails (BIF)

### Generation

**Measured.** `PUT /library/metadata/{id}/index?force=1` schedules a *BaseIndexFrameFile* job in
PMS (`building index (320x240) for parts…`), which **skips silently while the file
`index-sd.bif` exists**, even with `force=1`. With the file removed, PMS spawned:

```
"Plex Transcoder.exe" -codec:v h264 -skip_frame:v nokey -i <file> -threads 0 -nostats -q 3 -loglevel quiet
  -filter_complex "[0:V:0] fps=fps=0.500000:round=up,scale=w=320:h=240:force_original_aspect_ratio=decrease [out]"
  -map [out] "<bundle>\Contents\Indexes\tmp\img-%06d.jpg"
  -progressurl http://127.0.0.1:32400/video/:/transcode/session/bif/<activity uuid>/progress
```

with the working directory `Contents/Indexes`. It took 6 s for a 41-minute 720p episode, and the
result was **byte-identical** to the BIF made on 2026-11-11. `-skip_frame:v nokey` follows
`GenerateBIFKeyframesOnly=1`, `fps=0.5` follows `GenerateBIFFrameInterval=2`, and
`-codec:v` names the source codec. PMS then packs the JPEGs into the BIF and deletes `tmp`
(**inferred**: the pack step is not logged). The job registers an activity *Generating video
preview thumbnails*.

Preferences (**measured**, `/:/prefs`): `GenerateBIFBehavior` (`never` | `scheduled` | `asap`, here
`asap`), `GenerateBIFFrameInterval` 2, `GenerateBIFKeyframesOnly` 1,
`GenerateIndexFilesDuringAnalysis` 0, `ButlerTaskGenerateMediaIndexFiles` 0, and per library
`enableBIFGeneration`.

### Format

**Measured** on three BIFs. Roku's BIF:

| Offset | Content |
| --- | --- |
| 0 | magic `89 42 49 46 0D 0A 1A 0A` |
| 8 | version, u32 LE: 0 |
| 12 | image count, u32 LE: 1234 for 2467 s, 3016 for 6031 s (`ceil(duration / 2 s)`) |
| 16 | frame-wise separation, u32 LE: **0**, which per Roku means 1000 ms |
| 20–63 | zero |
| 64 | index: `count + 1` pairs of u32 LE (timestamp, byte offset); the timestamp is in seconds (0, 2, 4…), the last pair `(0xFFFFFFFF, file length)` |
| … | the JPEGs, back to back: 320×180 for 16:9 sources (`yuvj420p`) |

### Serving

**Measured** on part 5571720:

- `GET /library/parts/{part}/indexes/sd` answers the file as stored, `application/octet-stream`.
  `?interval=ms` rebuilds a BIF with timestamps every `interval/1000` s, each picking the
  stored frame nearest the time, ties upward (3 s: 3 → frame at 4, 9 → 10; 5 s: 5 → 6, 15 →
  16), count `ceil(last timestamp / interval)`, without a `Content-Type`.
  `interval=1000` answered the stored file.
- `GET /library/parts/{part}/indexes/sd/{offset ms}` answers one JPEG (`image/jpeg`), frame
  `floor(offset / 2000)`: 3999 → the frame at 2 s, 5000 → 4 s. **The last frame is never
  served**: offsets from 2,464,000 to 9,999,999 all answer the frame at 2464 s, not the one at
  2466 s that the file holds.
- `hd` is `404`, as is an unknown part.

Part `extra_data` records the state: `mi:indexes` = `sd` (783 parts), or on failure
`mi:failureBIF` = the codec pack's version (`a336ba9-16a9aaf440ee9a92b75f070b`),
`mi:failureBIFResultCode` (3) and `mi:failureBIFRetryCount` (156,665 parts, all from the
unreachable mount). PMS skips a part *previously failed … with the same FFMPEG version*
(**measured**, log). The Plex surface publishes them as `Part` attributes `indexes="sd"`,
`failureBIF`, `failureBIFResultCode`, `failureBIFRetryCount` (**measured**, item 28132).

**Reproducible:** completely with ffmpeg-plex (same command, our own packer). Our schema has
no place for the index state or the file; a BIF belongs in storage keyed by part, and
`mi:indexes` / failure state on `media_part`.

## 5. Chapter thumbnails

**Measured**: `PUT …/chapterThumbs?force=0&manual=1` makes PMS spawn `Plex Media Scanner.exe
--generate --chapter-thumbs-only --item N --force` (activity *Generating video chapter
thumbnails*); for a file without chapters it logs `Generated new chapter thumbnails` and
writes nothing. None of the reference's files are reachable, so no chapter image exists on
disk (0 files under `Chapters/`).

**Binary**: images are `Contents/Chapters/chapter%d.jpg`, logged as `Created chapter thumbnail
for metadata item %d, media item %d for chapter number %zu:%d at %s`, refused above a limit
(`Chapter count exceeds thumnail limit of %d`), served at `/library/media/%d/chapterImages/%d`
(the `thumb_url` stored on the chapter's tagging). Part keys `chapterImageSeekFailure`,
`chapterImagesStale` record failures. `GenerateChapterThumbBehavior` (`asap` here) and the
butler's *GenerateChapterThumbs* drive it. The frame's position within the chapter, its size
and the limit were not measured.

## 6. Intro detection

**Binary.** Intro detection runs in PMS (`IntroDetector`) on a **season** or a show, never on
one episode: `PUT /library/metadata/{episode}/intro` answers `404` (**measured**), and
`IntroDetector: Unable to run intro detection for item %d (missing item, or unexpected type)`
exists. Its steps, from the binary's log strings:

1. *Intro Detection Flac*: each episode's audio is transcoded to FLAC.
2. `Plex Media Fingerprinter.exe -length N <flac>`: the Fingerprinter **is** Chromaprint's
   `fpcalc` 1.5.1 (**measured**: `-version` prints `fpcalc version 1.5.1 (FFmpeg Lavc60.31.102…)`,
   same options, default algorithm 2).
3. The fingerprint is stored in the blobs database, then episodes are compared pairwise.
4. `IntroDetector: Less than half of episodes have an intro. Clearing intros for [%d]`: an
   intro is kept only when at least half the season shares it.

**Blob type 5 (measured)**, `linked_type='media_part'`, `linked_id` = part id, 131,863 rows, one
per part: gzip of comma-separated **unsigned 32-bit Chromaprint raw values**. Running the
Fingerprinter with `-raw -length 1234` on a FLAC of Bull E01's first 1234 s gave 9945 values
in the same textual form (≈ 8.075 per second, one per 4096/3 samples at 11,025 Hz = 123.83 ms).
On 120 sampled blobs, the fingerprint covers **the first half of the episode** (values ×
0.12383 s ÷ duration = 0.499 median), and **600 s** for shorter ones (a constant 4825 values on
11- and 14-minute episodes; whole episodes under 10 minutes give a ratio of 0.99). So `-length
= max(duration / 2, 600)`, capped at the duration (**inferred** from the measurement).
Consecutive episodes of one series share their first values (`3252763646,3287431470,…`).

**Markers (measured)**: taggings of tag type 12 (one tag, id 189893 for credits on item 28132),
`text='intro'`, `time_offset` / `end_time_offset` in ms, `extra_data` `pv:version=5`: 80,228
rows, all on episodes, at most one per episode (80,207 episodes). Intro starts are multiples of
the Chromaprint step: 990 ms (11,895 intros), 1114, 1238, 1361, 1485… = k × 123.83 ms with **k
≥ 8**; the first eight values are never used. The part's `pv:intros` repeats them as
`{"MediaPartMarkersArray":{"attributeName":"intros","version":5,"MediaPartMarker":[{"startTimeOffset":990,"endTimeOffset":25345}]}}`;
`""` (10,514 parts) means detected with no intro. The spec's `threshold` parameter on
`PUT …/intro` feeds the comparison; its default and the matching algorithm were not found.

Enabling: library setting `enableIntroMarkerGeneration`, which libraries using the `none` agent
(both test libraries) do not have, and server setting `GenerateIntroMarkerBehavior` (absent from
`/:/prefs` here). The test season could therefore not be run, and **no intro run was observed**.

**Reproducible:** the fingerprints, exactly, with ffmpeg-plex (FLAC) and `fpcalc` 1.5.1 or the
Chromaprint library (algorithm 2, raw); the pairwise matching is to be designed, against
80,228 stored intros, using the 131,863 stored fingerprints as test input.
`item_marker` (kind `intro`, `version`) already holds the imported results; fingerprints have no
place in our schema and need not be kept beyond a season's comparison.

## 7. Credits detection

**Binary.** Credits detection runs in `Plex Media Scanner` (`CreditsDetector`,
`--creditsTempDataPath`, log suffix *Credits*), in a temporary directory `PlexCreditsDetection-…`:

1. `Plex Transcoder.exe -codec:v … -skip_frame … -threads …` extracts frames with
   `[in]fps=0.5,scale=w=320:h=320:force_original_aspect_ratio=increase,showinfo[out]` into
   `thumb-%05d.jpeg`, with `ffmpeg.log` parsed for `Parsed_showinfo` `n`, `pts`, `pts_time`.
   Whether the whole film or only its end is extracted was not found.
2. Each frame goes through `model_v1.pb`, downloaded from
   `https://s3.amazonaws.com/files.plexapp.com/storage_ml/credits-detection/model_v1.pb`
   (*Model hasn't been downloaded*, *Could not download model file*) and run with OpenCV
   4.5.5's `dnn` (`readNet`, `blobFromImage`, `forward`). Its output layer
   `feature_fusion/Conv_7/Sigmoid` is the score map of **EAST**, the scene-text detector: the
   model finds text, it does not recognize credits.
3. Per frame it writes a row to `thumbnail_data.csv`: `index, pts, ptsTimeMs, log, entropy,
   histogramPeakRatio, numTextDetections, textXCenter, textYCenter`, then a decision to
   `result.json`: `status, hasCredits, timestamps, indices, version, errorReason`.

`onnxruntime.dll` and `Music.tflite` (TensorFlow Lite, the *Music Analysis* task's sonic model)
are not part of credits detection.

**Measured**, the stored results: taggings type 12 `text='credits'`, `pv:version=4`; 137,635
rows: 116,922 on 113,435 episodes and 20,713 on 17,815 films (of 18,333). `pv:final=1` marks the
credits that run to the end (124,636 rows); a film or episode can have several credits markers,
the final one last. Part keys: `pv:credits`
(`{"MediaPartMarkersArray":{"attributeName":"credits","version":4,"MediaPartMarker":[{"startTimeOffset":1333808,"endTimeOffset":1433120,"final":true}]}}`,
an array without `MediaPartMarker` when none was found) and `pv:creditsDetectionErrorCount` (1 to
4; at 4 *has failed too many times, we will not retry again*). Credits on films start at 79 %
of the duration at the earliest (credits markers without `final`) and 82 % (final ones).

Settings: `GenerateCreditsMarkerBehavior` with an online option: *Credits markers can be
generated locally and/or retrieved via an online database … if this preference is set to
'both' then any locally detected markers are submitted anonymously back* (**binary**). The
library setting `enableCreditsMarkerGeneration` is absent from the test libraries: `PUT
…/credits?force=1&manual=1` answered `403` (**measured**), and no credits run was observed.

**Reproducible:** frame extraction and EAST (OpenCV `dnn`, or ONNX after conversion) are
available, but the decision rule from the CSV features to a start time is unknown, and
open-plex keeps decoding out of process (EAST would run in the scanner binary on JPEGs, which
the image rule allows only if the owner extends it). Results go to `item_marker` (`credits`,
`is_final`, `version`).

## 8. Loudness

**Binary.** `--analyze-loudness` (`LoudnessScannerGroup`), libebur128 (`EBUR128_DUAL_MONO only
works with mono files!`), log lines `Loudness: Track loudness %.2f gain %.2f dB, peak %.6f,
range: %.2f` and `Album loudness … min track gain …`, `Saved %zu loudness points in %zu bytes`
(the per-100 ms curve behind `/library/streams/{id}/levels` and `/loudness`), loudness ramps
(`includeLoudnessRamps`, *Start ramp*, *End ramp*) for smart transitions, and
`LoudnessReplayGainStyle`. Settings: `LoudnessAnalysisBehavior` (`asap` here),
`LoudnessAnalysisThreads`, library `enableLoudnessAnalysis`. It is a music feature: *loudness
leveling and smart transitions*.

**Measured.** 89 audio tracks carry `ln:` keys, all on films, from an older run:
`ln:loudness` (integrated, LUFS), `ln:lra` (LU), `ln:peak` (true peak; 0.08, 1.47), `ln:threshold`,
`ln:gainOffset`, `ln:loudnessAnalysisVersion` `0.02`, two decimals each; already imported into
`stream_loudness`. `GET /library/streams/{id}/levels` and `/loudness` answer `404` even for those
89 tracks, so no curve is stored for them. `--analyze-loudness --force --item 578173` on an
episode exited at once and changed nothing, and `PUT …/analyzeLoudness` answered `403` on the test
film. Without a music library, no loudness run was observed.

**Reproducible:** ffmpeg's `ebur128` filter (`-af ebur128=peak=true`) gives integrated, LRA,
threshold and true peak by the same standard; `gainOffset` against which target (`-18`? the
89 tracks give `-22.32` → `0.84`, which no common target explains) is open.

## 9. Voice activity

**Binary.** `VoiceActivityDetectionManager`, *Voice Activity Detection Flac*, Silero VAD v4
(`https://files.plexapp.com/storage_ml/silero_vad/v4/silero_vad.onnx`, onnxruntime), part key
`pv:voiceActivityDetectionVersion`, used for *subtitle auto-sync*. The butler task
*ButlerTaskGenerateVoiceActivity* is disabled here and no part carries the key (**measured**).
Where the activity data goes was not found.

## 10. Ad markers

**Measured.** `analyze` logs *Invoking ad detection for Library 28 whose scan option is set to
[DVR files]* and calls `PUT …/addetect`, scheduled in an *AdDetection* job manager; for files
that are not DVR recordings nothing follows. Library setting `enableAdMarkerGeneration` (`0`
disabled, `1` recorded items, the default, `2` all items), server `GenerateAdMarkerBehavior`.
**Binary**: `Plex Commercial Skipper.exe` is comskip, with `Resources/comskip.ini`
(`detect_method=43`: black frame, logo, aspect ratio and the fuzzy combination; commercial
breaks between 25 and 600 s). No Live TV here, so out of scope with the DVR.

## 11. Butler

**Measured**, `GET /butler` (interval in days, window `ButlerStartHour` 2 to `ButlerEndHour` 5):

| Task | Every | Enabled | Analysis it triggers |
| --- | --- | --- | --- |
| `DeepMediaAnalysis` | 1 | yes | `--analyze-deeply --item N` per item without `ma:deepAnalysisVersion` (log: *Scheduling deep analysis for: …*) |
| `UpgradeMediaAnalysis` | 1 | yes | `--analyze` of items below the current `media_analysis_version` |
| `GenerateMediaIndexFiles` | 1 | yes | BIF (§4), when `GenerateBIFBehavior` is not `never` |
| `GenerateChapterThumbs` | 1 | yes | `--generate --chapter-thumbs-only --item N` (§5) |
| `ButlerTaskGenerateIntroMarkers` | 1 | yes | intro detection for new seasons and older versions (*Upgrading intro detection for %d items*) |
| `ButlerTaskGenerateCreditsMarkers` | 1, randomized | yes | credits (*Adding new credits markers*, *Upgrading credits markers*) |
| `ButlerTaskGenerateVoiceActivity` | 1, randomized | no | voice activity |
| `ButlerTaskGenerateAdMarkers` | 1 | yes | ad markers (*no libraries are enabled for ad marker generation, aborting task*) |
| `LoudnessAnalysis` | 1 | yes | `--analyze-loudness` |
| `MusicAnalysis` | 1 | yes | sonic analysis of tracks (`Music.tflite`) |
| `RefreshLocalMedia` | 3 | yes | local assets |
| `RefreshPeriodicMetadata` | 1, randomized | yes | agents (scanner side) |
| `ProcessAssets` | 1 | yes | not analysis |
| `GarbageCollectBlobs` | 7 | yes | orphaned blobs (fingerprints, subtitles) |
| `CleanOldBundles` | 7 | yes | orphaned `Media/` bundles |
| `GarbageCollectLibraryMedia`, `CleanOldCacheFiles`, `OptimizeDatabase`, `BackupDatabase`, `RefreshEpgGuides`, `RefreshLibraries` (off), `AutomaticUpdates` (off), `ButlerTaskCleanSupplementalLogFiles` (off) | | | maintenance, not analysis |

## What our tools reproduce

| Output | With | Status |
| --- | --- | --- |
| Track and container fields of the analysis | `ffprobe` (ffmpeg-plex) + `trace_headers` for `refFrames`, `codedHeight`, `hasScalingMatrix` | measured identical on the test files |
| Track bitrates of deep analysis | packet sizes from `ffprobe -show_entries packet` | matches Matroska's `BPS` (3110461) |
| `requiredBandwidths` | packet sizes | rule unknown |
| GoP file | packet flags and sizes | format known, not compared |
| OpenSubtitles hash | 128 KiB read | exact |
| Part `hash` | — | unknown here, see `matching.md` |
| BIF | ffmpeg-plex, same command, own packer | command and format measured, file byte-identical between runs |
| Analysis thumbnails and art | ffmpeg-plex | positions approximately known, choice rule unknown |
| Chapter list | ffprobe `-show_chapters` | known; no chapter storage in our schema |
| Intro fingerprints | Chromaprint 1.5.1 algorithm 2 | format and coverage measured |
| Intro and credits markers | — | algorithms unknown; results imported |
| Loudness | ffmpeg `ebur128` | standard known, gain target unknown |

## Gaps in our schema

- Chapters (taggings type 9 / `pv:chapters`): no table.
- Analysis state: `media_analysis_version`, `ma:deepAnalysisVersion`, `pv:deepAnalysisDate`
  (an mtime), BIF state (`mi:indexes`, `mi:failureBIF*`), `pv:creditsDetectionErrorCount`,
  the detection versions per part: not stored. A scanner deciding what to redo needs them.
- Generated images (thumbnails, art, BIF, chapter images): no storage.
- `media.color_trc` exists on the reference media row; ours has it per track only.

## Open questions

- The buffer sizes behind `requiredBandwidths`, and the rule that levels off for the larger
  buffers.
- Which of the three analysis frames becomes the poster or backdrop (smallest file on three of
  three cases), and the film offsets, which did not reproduce at 25/50/75 %.
- How BIF JPEGs are packed (in PMS, after the transcoder): the quality is `-q 3`; whether
  anything else is applied is unknown, though the result is byte-stable.
- The intro matcher: comparison of fingerprints across episodes, the `threshold`, how the end
  is chosen, and the 8-value start floor.
- Credits: which part of the video is sampled, and the decision from the EAST features to a start.
- Loudness: the `gainOffset` target, and where the per-100 ms points are stored (no blob type
  for them exists here).
- Voice activity: where its output goes.
- Chapter thumbnails: frame position within the chapter, size, the chapter limit.

## Left changed on the reference

- Episode 578173 (section 28): forced deep analysis rewrote its track `bitrate`s (video 3208969 →
  3110461, subtitle 77 → 76), the media `bitrate` (3208969 → 3206539), the tracks' `updated_at`
  and its `GoP-0.xml` (same content); analysis bumped `changed_at`. Its `index-sd.bif` was
  deleted and regenerated byte-identical.
- New log file `Logs/Plex Media Scanner Loudness.log` (the loudness run's suffix).
- Requests that changed nothing: `PUT …/577986/credits`, `/voiceActivity`, `/analyzeLoudness`
  (`403`), `PUT …/577988/intro` (`404`).
