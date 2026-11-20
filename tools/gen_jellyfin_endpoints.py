"""Generate docs/jellyfin/ENDPOINTS.md from the reference Jellyfin's OpenAPI document.

Statuses are derived on every run, never carried over: an operation is `served` when
`crates/api-jellyfin` declares a route for its path and method, `todo` otherwise. Paths are
compared without regard to case or parameter names, as the reference routes. What a served
operation leaves out is in NOTES, and in docs/jellyfin/spec-gaps.md.

The document itself comes from the reference (`/api-docs/openapi.json`); refresh it with
`python tools/gen_jellyfin_endpoints.py --fetch` while one runs at JELLYFIN_URL.

Usage: python tools/gen_jellyfin_endpoints.py [--fetch]
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.request
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "docs" / "jellyfin" / "openapi.json"
OUT = ROOT / "docs" / "jellyfin" / "ENDPOINTS.md"
SOURCES = ROOT / "crates" / "api-jellyfin" / "src"
METHODS = ("get", "post", "put", "patch", "delete", "head")

# What a served operation does not do. Keyed by "METHOD /path" as the spec writes it.
NOTES = {
    "GET /Items": "Filters the official API cannot express answer 501; one sort key.",
    "GET /Items/{itemId}/PlaybackInfo": "Direct play decided by MediaFile.readableAsIs; HLS otherwise.",
    "POST /Items/{itemId}/PlaybackInfo": "Direct play decided by MediaFile.readableAsIs; HLS otherwise.",
    "GET /Videos/{itemId}/stream": "static=true only: a remux to another container answers 501.",
    "GET /Videos/{itemId}/stream.{container}": "static=true only.",
    "HEAD /Videos/{itemId}/stream": "static=true only.",
    "HEAD /Videos/{itemId}/stream.{container}": "static=true only.",
    "GET /Items/{itemId}/Images/{imageType}": "No artwork is served: 404.",
    "GET /Items/{itemId}/Images/{imageType}/{imageIndex}": "No artwork is served: 404.",
    "GET /Items/{itemId}/Images": "Empty: no artwork.",
    "POST /UserFavoriteItems/{itemId}": "Refused 501: favorites are not kept.",
    "DELETE /UserFavoriteItems/{itemId}": "Refused 501: favorites are not kept.",
    "GET /QuickConnect/Enabled": "false: Quick Connect is not built.",
    "POST /DisplayPreferences/{displayPreferencesId}": "Accepted, not kept.",
    "GET /DisplayPreferences/{displayPreferencesId}": "The reference's defaults.",
    "GET /System/Info": "The reference's own paths are left empty.",
    "GET /Users/Public": "Empty, as the reference's hidden users.",
    "GET /Items/{itemId}/Similar": "Films and series, by the official API's measure.",
    "GET /Items/{itemId}/ThemeMedia": "Empty: no theme media is held.",
    "GET /SyncPlay/List": "Empty: no group is held.",
    "GET /Items/Filters": "Below a library or across the caller's; no tags (keywords are a provider's).",
    "GET /Items/Filters2": "Languages as tracks state them; Live TV flags answer 501.",
    "POST /Playlists": "The caller's own: sharing (Users, IsPublic) answers 501.",
    "POST /Playlists/{playlistId}": "Name and items; sharing answers 501.",
    "GET /Playlists/{playlistId}": "OpenAccess false, no shares: a playlist is its owner's.",
    "POST /Playlists/{playlistId}/Items": "Appended; a position before the end answers 501.",
    "GET /Playlists/{playlistId}/Users": "Empty: shared with nobody.",
    "POST /Playlists/{playlistId}/Users/{userId}": "501: playlists are not shared.",
    "DELETE /Playlists/{playlistId}/Users/{userId}": "501: playlists are not shared.",
    "DELETE /Items/{itemId}": "Playlists and collections; media files answer 501.",
    "POST /Collections": "In its first item's library; no item, ParentId or IsLocked answers 501.",
    "GET /Genres": "searchTerm completes a word of the name; name ranges answer 501.",
    "GET /Persons": "With searchTerm, or one role at a time; every role unsearched answers 501.",
    "GET /Studios": "A studio's id is a hash of its name; StudioIds on /Items answers 501.",
    "GET /Search/Hints": "Items then people, first page only; Live TV and media filters answer 501.",
}


def fetch() -> None:
    base = os.environ.get("JELLYFIN_URL", "http://localhost:8096").rstrip("/")
    with urllib.request.urlopen(f"{base}/api-docs/openapi.json", timeout=60) as response:
        spec = json.load(response)
    spec.pop("servers", None)
    SPEC.parent.mkdir(parents=True, exist_ok=True)
    SPEC.write_text(json.dumps(spec, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")


def shape(path: str) -> str:
    """A path as routing sees it: lower case, every parameter alike."""
    return re.sub(r"\{[^}]+\}", "{}", path.lower()).rstrip("/")


def declared_routes() -> set[tuple[str, str]]:
    """(method, path shape) for every route the crate declares. A GET route answers HEAD."""
    routes: set[tuple[str, str]] = set()
    pattern = re.compile(r'\.route\(\s*"([^"]+)",\s*([^)]*\)(?:\.[a-z]+\([^)]*\))*)', re.S)
    for source in SOURCES.glob("*.rs"):
        for path, handlers in pattern.findall(source.read_text(encoding="utf-8")):
            for method in re.findall(r"\b(get|post|put|patch|delete|head)\(", handlers):
                routes.add((method, shape(path)))
                if method == "get":
                    routes.add(("head", shape(path)))
    return routes


def served(method: str, path: str, routes: set[tuple[str, str]]) -> bool:
    wanted = shape(path)
    if (method, wanted) in routes:
        return True
    # `/videos/{id}/{name}` serves `stream`, `stream.{container}` and `master.m3u8`.
    if re.fullmatch(r"/videos/\{\}/(stream(\.\{\})?|master\.m3u8)", wanted):
        return (method, "/videos/{}/{}") in routes
    return False


def main() -> None:
    if "--fetch" in sys.argv:
        fetch()
    spec = json.loads(SPEC.read_text(encoding="utf-8"))
    routes = declared_routes()
    by_tag: dict[str, list[tuple[str, str, str, str]]] = defaultdict(list)
    totals = {"served": 0, "todo": 0}
    for path, operations in sorted(spec["paths"].items()):
        for method in METHODS:
            operation = operations.get(method)
            if not operation:
                continue
            status = "served" if served(method, path, routes) else "todo"
            totals[status] += 1
            tag = (operation.get("tags") or ["Other"])[0]
            note = NOTES.get(f"{method.upper()} {path}", "")
            by_tag[tag].append((method.upper(), path, status, note))
    total = sum(totals.values())
    lines = [
        "# Jellyfin endpoints",
        "",
        f"Tracker for the {total} operations of the reference Jellyfin's OpenAPI document "
        f"(`docs/jellyfin/openapi.json`, {spec['info']['version']}), served below `/jellyfin`.",
        "",
        "**Generated** — `python tools/gen_jellyfin_endpoints.py`. A status is derived from the "
        "routes `crates/api-jellyfin` declares: `served` has a handler, `todo` answers the "
        "surface's `404`. Notes say what a served operation leaves out; the measured "
        "differences are in `docs/jellyfin/spec-gaps.md`.",
        "",
        "## Progress",
        "",
        "| Status | Operations | Share |",
        "| --- | --: | --: |",
    ]
    for status, count in totals.items():
        lines.append(f"| `{status}` | {count} | {100 * count / total:.1f} % |")
    lines += ["", "## By domain", "", "| Domain | Operations | `served` |", "| --- | --: | --: |"]
    for tag in sorted(by_tag):
        operations = by_tag[tag]
        done = sum(1 for operation in operations if operation[2] == "served")
        lines.append(f"| {tag} | {len(operations)} | {done} |")
    for tag in sorted(by_tag):
        lines += ["", f"## {tag}", "", "| Method | Path | Status | Notes |", "| --- | --- | --- | --- |"]
        for method, path, status, note in by_tag[tag]:
            lines.append(f"| {method} | `{path}` | `{status}` | {note} |")
    lines += [
        "",
        "## Served outside the document",
        "",
        "Routes the reference answers that its document leaves out, which older clients call:",
        "",
        "| Path | As |",
        "| --- | --- |",
        "| `/Users/{userId}/Items`, `/Users/{userId}/Items/{itemId}` | `/Items`, `/Items/{itemId}` |",
        "| `/Users/{userId}/Items/Latest`, `/Users/{userId}/Items/Resume` | `/Items/Latest`, `/UserItems/Resume` |",
        "| `/Users/{userId}/Views` | `/UserViews` |",
        "| `/Users/{userId}/PlayedItems/{itemId}` | `/UserPlayedItems/{itemId}` |",
        "| `/Users/{userId}/Items/{itemId}/Intros`, `/Users/{userId}/Items/{itemId}/SpecialFeatures`, `/Users/{userId}/Items/{itemId}/LocalTrailers` | the same, without the user |",
        "| `/Videos/{itemId}/master.m3u8` | the address `PlaybackInfo` gives a conversion |",
        "",
    ]
    OUT.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    print(f"{totals['served']} of {total} operations served")


if __name__ == "__main__":
    main()
