"""Give each scan sample what the reference Jellyfin publishes of its file, and what our scanner
reports of it, so that `crates/server/tests/jellyfin_samples.rs` replays the chain between them.

For every sample under `crates/scanner/tests/fixtures/samples` the reference holds:

  scanner.json   `klapz-scanner probe` on the file (no `--deep`): the report the scan stores.
  jellyfin.json  the reference's lookup of the file's item, scrubbed: the file-derived fields of the
                 item and its first media source, without ids, paths, entity tags or chapter images.

Reads the reference's lookups from `captures/jellyfin/items` (`tools/capture_jellyfin.py
--all-items`), and runs the scanner on the files themselves, which must be reachable at the
paths the reference stores; the scanner finds ffmpeg-plex as it does in the server
(`KLAPZ_TRANSCODER`).

Usage: KLAPZ_TRANSCODER=... python tools/capture_jellyfin_samples.py SCANNER
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SAMPLES = ROOT / "crates" / "scanner" / "tests" / "fixtures" / "samples"
ITEMS = ROOT / "captures" / "jellyfin" / "items"

# What an item says of its file, as opposed to what a metadata provider told the reference.
ITEM_FIELDS = ("Container", "RunTimeTicks", "HasSubtitles", "IsHD", "Width", "Height", "Chapters")
# What identifies the reference's own records rather than describes the file.
SOURCE_SCRUBBED = ("Id", "Path", "ETag")
# A chapter image the reference drew: where it keeps it, and when it drew it.
CHAPTER_SCRUBBED = ("ImagePath", "ImageTag", "ImageDateModified")


def file_name(path: str) -> str:
    return path.replace("\\", "/").rsplit("/", 1)[-1]


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    scanner = sys.argv[1]
    lookups = {}
    for path in ITEMS.glob("*.json"):
        item = json.loads(path.read_text(encoding="utf-8"))
        source = item["MediaSources"][0]
        lookups[file_name(source["Path"])] = item
    written = 0
    for sample in sorted(SAMPLES.iterdir()):
        probed = json.loads((sample / "ffprobe.json").read_text(encoding="utf-8"))
        name = file_name(probed["format"]["filename"]).removeprefix("file:")
        item = lookups.get(name)
        if item is None:
            continue
        source = dict(item["MediaSources"][0])
        file = source["Path"]
        for key in SOURCE_SCRUBBED:
            source.pop(key, None)
        published = {key: item[key] for key in ITEM_FIELDS if key in item}
        for chapter in published.get("Chapters", []):
            if "ImagePath" in chapter:
                for key in CHAPTER_SCRUBBED:
                    chapter.pop(key, None)
        library = "movies" if item["Type"] == "Movie" else "tvshows"
        answer = {"Library": library, "Item": published, "MediaSource": source}
        (sample / "jellyfin.json").write_text(
            json.dumps(answer, indent=1, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n"
        )
        report = subprocess.run(
            [scanner, "probe", "--", file], check=True, capture_output=True
        ).stdout
        (sample / "scanner.json").write_text(
            json.dumps(json.loads(report), indent=1, ensure_ascii=False) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        written += 1
    print(f"{written} samples given the reference's answer and the scanner's report")


if __name__ == "__main__":
    main()
