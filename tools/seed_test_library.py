"""Add a folder of media files to the development database as a library.

The file scanner is out of scope, and the imported library points at files that exist on
another machine, so nothing imported can be played here. This gives playback something real:
one film per file, with its tracks read by ffprobe and the subtitle files beside it
(`Film.fr.srt`, `Film.ass`) as sidecars, their location written as the reference writes it.

Development only. It writes straight to the tables, so these films are absent from the title
word index, whose only proper writer is `db::write::insert_items`: they list but do not search.

Usage: python tools/seed_test_library.py DIRECTORY [--database klapz.db] [--remove]
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import subprocess
import time
from pathlib import Path

SECTION_UUID = "0a11ce00-0000-4000-8000-0000000000aa"
SECTION_TITLE = "Test"
VIDEO_SUFFIXES = {".mkv", ".mp4", ".avi", ".m4v", ".mov", ".ts", ".webm"}
LANGUAGES = {"fre": "fr", "fra": "fr", "eng": "en", "ger": "de", "deu": "de", "spa": "es", "ita": "it"}
STREAM_KINDS = {"video", "audio", "subtitle"}
SIDECAR_SUFFIXES = {".srt", ".ass", ".ssa", ".vtt", ".smi"}


def next_id(database: sqlite3.Connection, table: str) -> int:
    return (database.execute(f"select max(id) from {table}").fetchone()[0] or 0) + 1


def remove(database: sqlite3.Connection) -> None:
    row = database.execute("select id from library_section where uuid=?", (SECTION_UUID,)).fetchone()
    if row is None:
        return
    section = row[0]
    items = "select id from metadata_item where library_section_id=?"
    media = f"select id from media where metadata_item_id in ({items})"
    parts = f"select id from media_part where media_id in ({media})"
    streams = f"select id from media_stream where media_part_id in ({parts})"
    # Foreign keys are off in sqlite3 by default: what cascades in the server is deleted here.
    database.execute(f"delete from subtitle_offset where media_stream_id in ({streams})", (section,))
    database.execute(f"delete from media_stream where media_part_id in ({parts})", (section,))
    database.execute(f"delete from media_part where media_id in ({media})", (section,))
    database.execute(f"delete from media where metadata_item_id in ({items})", (section,))
    database.execute("delete from metadata_item where library_section_id=?", (section,))
    database.execute("delete from section_root where section_id=?", (section,))
    database.execute("delete from library_section where id=?", (section,))


def sidecars(media: Path) -> list[tuple[Path, str | None]]:
    """The subtitle files beside a media file, with the language their name gives."""
    found = []
    for path in sorted(media.parent.iterdir()):
        if path.suffix.lower() not in SIDECAR_SUFFIXES or not path.name.startswith(media.stem + "."):
            continue
        middle = path.name[len(media.stem) + 1 : -len(path.suffix)]
        language = middle if 2 <= len(middle) <= 3 and middle.isalpha() else None
        found.append((path, LANGUAGES.get(language, language)))
    return found


def file_url(path: Path) -> str:
    """The location the reference stores for a sidecar: a `file://` URL, spaces escaped."""
    return "file:///" + path.as_posix().lstrip("/").replace("%", "%25").replace(" ", "%20")


def probe(path: Path) -> dict:
    output = subprocess.run(
        ["ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(path)],
        capture_output=True, text=True, check=True, encoding="utf-8",
    )
    return json.loads(output.stdout)


def frame_rate(stream: dict) -> float | None:
    numerator, _, denominator = stream.get("avg_frame_rate", "0/0").partition("/")
    return int(numerator) / int(denominator) if denominator and int(denominator) else None


# ffprobe's profile names, as Plex's analysis stores them.
PROFILES = {"dts": "dts", "dts-hd ma": "ma", "dts-es": "es", "dts-hd hra": "hra", "he-aac": "he-aac", "he-aacv2": "he-aacv2"}


def analysis(stream: dict) -> dict:
    """The columns our schema keeps from a media analysis, read off ffprobe, whose
    vocabulary Plex's analysis shares: `bt709`, `5.1(side)`, `left`."""
    profile = stream.get("profile")
    if profile:
        profile = PROFILES.get(profile.lower(), profile.lower())
    pixel_format = stream.get("pix_fmt", "")
    chroma = next((ratio for key, ratio in (("420", "4:2:0"), ("422", "4:2:2"), ("444", "4:4:4")) if key in pixel_format), None)
    field_order = stream.get("field_order")
    depth = stream.get("bits_per_raw_sample")
    return {
        "title": stream.get("tags", {}).get("title"),
        "profile": profile,
        "bit_depth": int(depth) if depth else (10 if "10" in pixel_format else 8 if pixel_format else None),
        "width": stream.get("width"),
        "height": stream.get("height"),
        "coded_width": stream.get("coded_width"),
        "coded_height": stream.get("coded_height"),
        "frame_rate": round(frame_rate(stream), 3) if stream["codec_type"] == "video" and frame_rate(stream) else None,
        "level": stream.get("level") if stream["codec_type"] == "video" else None,
        "ref_frames": stream.get("refs") if stream["codec_type"] == "video" else None,
        "scan_type": None if not field_order or field_order == "unknown" else ("progressive" if field_order == "progressive" else "interlaced"),
        "chroma_subsampling": chroma,
        "chroma_location": stream.get("chroma_location"),
        "color_range": stream.get("color_range"),
        "color_space": stream.get("color_space"),
        "color_primaries": stream.get("color_primaries"),
        "color_transfer": stream.get("color_transfer"),
        "sampling_rate": int(stream["sample_rate"]) if "sample_rate" in stream else None,
        "channel_layout": stream.get("channel_layout"),
    }


def seed(database: sqlite3.Connection, directory: Path) -> None:
    now = int(time.time())
    section = next_id(database, "library_section")
    database.execute(
        "insert into library_section (id, kind, title, title_sort, language, uuid, created_at,"
        " updated_at, scanned_at, content_changed_at, hidden) values (?,?,?,?,?,?,?,?,?,?,0)",
        (section, "movie", SECTION_TITLE, SECTION_TITLE, "fr-FR", SECTION_UUID, now, now, now, now),
    )
    database.execute(
        "insert into section_root (id, section_id, path, created_at) values (?,?,?,?)",
        (next_id(database, "section_root"), section, str(directory), now),
    )
    for path in sorted(directory.iterdir()):
        if path.suffix.lower() not in VIDEO_SUFFIXES:
            continue
        details = probe(path)
        streams = [s for s in details["streams"] if s["codec_type"] in STREAM_KINDS]
        video = next(s for s in streams if s["codec_type"] == "video")
        audio = next((s for s in streams if s["codec_type"] == "audio"), None)
        duration = int(float(details["format"]["duration"]) * 1000)
        bitrate = int(details["format"].get("bit_rate", 0))

        item = next_id(database, "metadata_item")
        title = path.stem.replace(".", " ")
        database.execute(
            "insert into metadata_item (id, library_section_id, kind, guid, title, title_sort,"
            " sort_key, duration_ms, added_at, updated_at) values (?,?,?,?,?,?,?,?,?,?)",
            (item, section, "movie", f"local://{item}", title, title, title.casefold(), duration, now, now),
        )
        media = next_id(database, "media")
        database.execute(
            "insert into media (id, metadata_item_id, width, height, aspect_ratio, duration_ms,"
            " bitrate, container, video_codec, audio_codec, audio_channels, frame_rate, interlaced,"
            " optimized_for_streaming, created_at, updated_at)"
            " values (?,?,?,?,?,?,?,?,?,?,?,?,0,0,?,?)",
            (media, item, video.get("width"), video.get("height"),
             video["width"] / video["height"], duration, bitrate, path.suffix[1:].lower(),
             video["codec_name"], audio and audio["codec_name"], audio and audio.get("channels"),
             frame_rate(video), now, now),
        )
        part = next_id(database, "media_part")
        database.execute(
            "insert into media_part (id, media_id, position, path, size_bytes, duration_ms,"
            " created_at, updated_at) values (?,?,0,?,?,?,?,?)",
            (part, media, str(path), path.stat().st_size, duration, now, now),
        )
        seen: set[str] = set()
        for stream in streams:
            kind = stream["codec_type"]
            language = stream.get("tags", {}).get("language")
            measured = analysis(stream)
            columns = ", ".join(measured)
            database.execute(
                "insert into media_stream (id, media_part_id, kind, position, codec, language,"
                f" channels, bitrate, is_default, is_forced, {columns})"
                f" values (?,?,?,?,?,?,?,?,?,0,{','.join('?' * len(measured))})",
                (next_id(database, "media_stream"), part, kind, stream["index"],
                 "srt" if stream["codec_name"] == "subrip" else stream["codec_name"],
                 LANGUAGES.get(language, language), stream.get("channels"),
                 int(stream["bit_rate"]) if "bit_rate" in stream else None, int(kind not in seen),
                 *measured.values()),
            )
            seen.add(kind)
        for sidecar, language in sidecars(path):
            codec = sidecar.suffix[1:].lower()
            database.execute(
                "insert into media_stream (id, media_part_id, kind, position, codec, language,"
                " is_default, is_forced, external_path, subtitle_format)"
                " values (?,?,'subtitle',0,?,?,0,0,?,?)",
                (next_id(database, "media_stream"), part, codec, language, file_url(sidecar), codec),
            )
            print(f"  sidecar {sidecar.name}")
        print(f"item {item}, part {part}: {[(s['codec_type'], s['codec_name']) for s in streams]}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path, nargs="?")
    parser.add_argument("--database", default="klapz.db")
    parser.add_argument("--remove", action="store_true")
    args = parser.parse_args()
    database = sqlite3.connect(args.database, timeout=30)
    with database:
        remove(database)
        if not args.remove:
            seed(database, args.directory.resolve())


if __name__ == "__main__":
    main()
