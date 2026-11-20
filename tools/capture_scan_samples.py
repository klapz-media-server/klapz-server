"""Capture what the scanner reads of sample files, and what the reference stored for them.

For each media file under SAMPLES, a directory of crates/scanner/tests/fixtures/samples/:

- `ffprobe.json`: the output of the scanner's own ffprobe command, the file's name kept and its
  directory dropped;
- `trace-N.txt`: the sequence parameter set `trace_headers` prints for H.264 track N;
- `file.json`: the file's identity and the layout the scanner reads from its bytes (MP4 index,
  MPEG-TS packets), from `open-plex-scanner probe`;
- `reference.json`: the reference's stored rows for the same file, found at REMOTE_ROOT plus its
  path below SAMPLES: the media's and part's columns, each embedded track's with its `ma:`
  analysis, and the item's chapters with where they came from, in the reference's own
  vocabulary. Identifiers, paths, hashes and dates are left out.

The fixtures are the reference's real data: `crates/db/tests/scan_samples.rs` replays ffprobe's
output through the scanner and the scan's storage, and compares with them.

Usage:
    python tools/capture_scan_samples.py SAMPLES --ffmpeg FFMPEG --scanner SCANNER
        [--reference DB] [--remote-root /mnt/acd/Plex/]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import subprocess
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "crates" / "scanner" / "tests" / "fixtures" / "samples"
MEDIA = (".mkv", ".mp4", ".m4v", ".avi", ".webm", ".flv", ".ts", ".m2ts", ".mpg", ".mov", ".wmv")

# The reference shows the rates its deep analysis measured on these, the declared ones on the
# rest, whose deep analysis a later normal one overwrote (docs/scanner/analysis.md, §2).
MEASURED = {
    "anatole-latuile-s1e35-la-chasse-au-tresor",
    "devil-in-ohio-s01e07-par-le-sang-1080p-dv-hdr-x265-amen",
    "ghost-in-the-shell-stand-alone-complex-s02e10-multi-ac3",
    "clone-wars-s01e06",
    "xspitfire911-boku-no-hero-academia-s06e16-bdrip-1080p-x265",
}

MEDIA_COLUMNS = (
    "width", "height", "display_aspect_ratio", "bitrate", "container", "video_codec",
    "audio_codec", "audio_channels", "frames_per_second", "duration",
)
PART_KEYS = (
    "ma:optimizedForStreaming", "ma:has64bitOffsets", "ma:hasThumbnail", "ma:packetLength",
    "ma:hasChapterTextStream",
)
STREAM_COLUMNS = ("stream_type_id", "index", "codec", "language", "channels", "bitrate", "default", "forced")
# Written by the deep analysis, which these fixtures do not replay.
DEEP_KEYS = {"ma:requiredBandwidths"}


def slug(name: str) -> str:
    ascii_name = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    name = re.sub(r"[^a-z0-9]+", "-", ascii_name.lower()).strip("-")
    # At most 60 characters, cut where a word ends.
    return name if len(name) <= 60 else name[:61].rsplit("-", 1)[0]


def ffprobe(ffmpeg: str, path: Path) -> dict:
    output = subprocess.run(
        [ffmpeg, "-probe", "-v", "error", "-print_format", "json", "-show_format", "-show_streams",
         "-show_chapters", "-i", f"file:{path}"],
        capture_output=True, check=True,
    ).stdout
    probed = json.loads(output)
    probed["format"]["filename"] = path.name
    return probed


def sequence_parameter_set(ffmpeg: str, path: Path, index: int) -> str:
    trace = subprocess.run(
        [ffmpeg, "-hide_banner", "-nostdin", "-v", "info", "-i", f"file:{path}", "-map", f"0:{index}",
         "-c", "copy", "-bsf:v", "trace_headers", "-frames:v", "1", "-f", "null", "-"],
        capture_output=True,
    ).stderr.decode("utf-8", "replace")
    kept, inside = [], False
    for line in trace.splitlines():
        if "[trace_headers @ " not in line:
            continue
        # The filter's address changes between runs; it says nothing.
        line = re.sub(r"\[trace_headers @ [0-9a-fA-Fx]+\]", "[trace_headers @ 0000000000000000]", line)
        title = line.split("] ", 1)[1]
        if title == "Sequence Parameter Set":
            if inside:
                break
            inside = True
        elif inside and not title[:1].isdigit():
            break
        if inside:
            kept.append(line)
    return "\n".join(kept) + "\n"


def layout(scanner: str, path: Path) -> dict:
    report = json.loads(subprocess.run([scanner, "probe", "--", str(path)], capture_output=True, check=True).stdout)
    keys = ("file", "optimizedForStreaming", "has64bitOffsets", "packetLength")
    return {key: report[key] for key in keys if key in report}


def reference(db: sqlite3.Connection, remote: str) -> dict | None:
    part = db.execute("select * from media_parts where file = ?", (remote,)).fetchone()
    if part is None:
        return None
    media = db.execute("select * from media_items where id = ?", (part["media_item_id"],)).fetchone()
    part_extra = json.loads(part["extra_data"] or "{}")
    streams = []
    for row in db.execute(
        "select * from media_streams where media_part_id = ? and coalesce(url, '') = '' order by [index]",
        (part["id"],),
    ):
        extra = json.loads(row["extra_data"] or "{}")
        stream = {column: row[column] for column in STREAM_COLUMNS if row[column] not in (None, "")}
        stream["analysis"] = {
            key: value for key, value in sorted(extra.items()) if key.startswith("ma:") and key not in DEEP_KEYS
        }
        streams.append(stream)
    item = db.execute("select extra_data from metadata_items where id = ?", (media["metadata_item_id"],)).fetchone()
    chapters = [
        {"index": row["index"], "start": row["time_offset"], "end": row["end_time_offset"], "title": row["tag"]}
        for row in db.execute(
            "select taggings.[index], time_offset, end_time_offset, tags.tag from taggings "
            "join tags on tags.id = taggings.tag_id and tags.tag_type = 9 "
            "where taggings.metadata_item_id = ? order by taggings.[index]",
            (media["metadata_item_id"],),
        )
    ]
    return {
        "chapterSource": json.loads(item["extra_data"] or "{}").get("ma:chapterSource"),
        "chapters": chapters,
        "media": {column: media[column] for column in MEDIA_COLUMNS if media[column] not in (None, "")},
        "part": {
            "size": part["size"],
            "duration": part["duration"],
            "analysis": {key: part_extra[key] for key in PART_KEYS if key in part_extra},
        },
        "streams": streams,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("samples", type=Path)
    parser.add_argument("--ffmpeg", required=True)
    parser.add_argument("--scanner", required=True)
    parser.add_argument(
        "--reference",
        default=os.path.join(
            os.environ.get("LOCALAPPDATA", ""),
            "Plex Media Server", "Plug-in Support", "Databases", "com.plexapp.plugins.library.db",
        ),
    )
    parser.add_argument("--remote-root", default="/mnt/acd/Plex/")
    arguments = parser.parse_args()

    db = sqlite3.connect(f"file:{arguments.reference}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    for path in sorted(arguments.samples.rglob("*")):
        if not path.is_file() or path.suffix.lower() not in MEDIA:
            continue
        relative = path.relative_to(arguments.samples).as_posix()
        stored = reference(db, arguments.remote_root + relative)
        if stored is None:
            print(f"not in the reference: {relative}")
            continue
        name = slug(path.stem)
        stored["rates"] = "measured" if name in MEASURED else "declared"
        target = OUT / name
        target.mkdir(parents=True, exist_ok=True)
        probed = ffprobe(arguments.ffmpeg, path)
        write(target / "ffprobe.json", probed)
        for stream in probed["streams"]:
            if stream.get("codec_name") == "h264" and not stream.get("disposition", {}).get("attached_pic"):
                trace = sequence_parameter_set(arguments.ffmpeg, path, stream["index"])
                (target / f"trace-{stream['index']}.txt").write_text(trace, encoding="utf-8", newline="\n")
        write(target / "file.json", layout(arguments.scanner, path))
        write(target / "reference.json", stored)
        print(f"{name}: {relative}")


def write(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, indent=1, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
