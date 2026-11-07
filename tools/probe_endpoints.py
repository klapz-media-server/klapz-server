"""Probe the reference server to find which spec operations actually exist.

The official spec is not perfectly faithful — `/library/sections/{id}/firstCharacters` is
declared but the real server only answers the singular `firstCharacter`. Implementing from
the spec alone therefore produces routes no client calls and misses routes clients use.

Only GET operations are probed, and paths matching the deferred scanner/analysis patterns
are skipped: nothing here mutates the reference server.

    PLEX_URL    default http://127.0.0.1:32400
    PLEX_TOKEN  required

Usage: PLEX_TOKEN=... python tools/probe_endpoints.py
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.request
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from gen_endpoints import DEFERRED_PATHS, DEFERRED_TAGS  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "docs" / "openapi.json"
OUT = ROOT / "docs" / "endpoint-probe.md"

BASE = os.environ.get("PLEX_URL", "http://127.0.0.1:32400").rstrip("/")
TOKEN = os.environ.get("PLEX_TOKEN")

# Path parameters filled with real ids where the name is recognizable, so a 404 means
# "route absent" rather than "id does not exist". Anything unmatched gets `1`.
PARAM_HINTS = {
    "sectionid": "section",
    "ids": "item",
    "ratingkey": "item",
    "metadataid": "item",
    "key": "item",
}

# Enumerated path segments: substituting a number yields a 404 that says nothing about
# whether the route exists.
PARAM_LITERALS = {"transcodetype": "video", "element": "thumb", "ext": "jpg"}

# Some operations need a query parameter to resolve at all.
REQUIRED_QUERY = {
    "/library/sections/{sectionId}/firstCharacter": "type=1",
    "/library/sections/{sectionId}/all": "type=1",
}


def fetch_status(path: str) -> int:
    separator = "&" if "?" in path else "?"
    url = f"{BASE}{path}{separator}X-Plex-Token={TOKEN}"
    try:
        with urllib.request.urlopen(url, timeout=20) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code
    except (urllib.error.URLError, TimeoutError):
        return 0


def discover() -> dict[str, str]:
    request = urllib.request.Request(f"{BASE}/library/sections?X-Plex-Token={TOKEN}")
    request.add_header("Accept", "application/json")
    with urllib.request.urlopen(request, timeout=20) as response:
        sections = json.load(response)["MediaContainer"].get("Directory", [])
    section = next((s for s in sections if s.get("type") == "movie"), sections[0])

    request = urllib.request.Request(
        f"{BASE}/library/sections/{section['key']}/all?X-Plex-Token={TOKEN}"
    )
    request.add_header("Accept", "application/json")
    with urllib.request.urlopen(request, timeout=20) as response:
        items = json.load(response)["MediaContainer"].get("Metadata", [])
    return {"section": str(section["key"]), "item": str(items[0]["ratingKey"])}


def concretize(path: str, ids: dict[str, str]) -> str:
    def replace(match: re.Match[str]) -> str:
        name = match.group(1).lower()
        if literal := PARAM_LITERALS.get(name):
            return literal
        hint = PARAM_HINTS.get(name)
        return ids[hint] if hint else "1"

    return re.sub(r"\{([^}]+)\}", replace, path)


def main() -> None:
    if not TOKEN:
        sys.exit("PLEX_TOKEN is not set; refusing to run")

    spec = json.loads(SPEC.read_text(encoding="utf-8"))
    ids = discover()

    rows = []
    for path, item in sorted(spec["paths"].items()):
        operation = item.get("get")
        if operation is None:
            continue
        tag = (operation.get("tags") or ["Untagged"])[0]
        if tag in DEFERRED_TAGS or any(p in path for p in DEFERRED_PATHS):
            continue

        probe_path = concretize(path, ids)
        if query := REQUIRED_QUERY.get(path):
            probe_path = f"{probe_path}?{query}"

        status = fetch_status(probe_path)
        rows.append((path, tag, status, probe_path))

    counts = Counter(verdict(status) for _, _, status, _ in rows)

    lines = [
        "# Live endpoint probe",
        "",
        "Which `GET` operations of `docs/openapi.json` the reference server actually answers. "
        "**Generated** — regenerate with `PLEX_TOKEN=... python tools/probe_endpoints.py`.",
        "",
        "The spec is not perfectly faithful, so this informs what is worth implementing. "
        "Write operations are not probed, and deferred scanner/analysis paths are skipped.",
        "",
        "**Read `absent` carefully.** It means *not served for the identifiers this probe "
        "could synthesize*, which is not the same as *the route does not exist*. Path "
        "parameters like `collectionId`, `playlistId`, `partId`, `streamId`, `mediaId`, "
        "`updatedAt` and `changestamp` are filled with `1`, so a 404 on those is usually a "
        "wrong id rather than a missing route. `/:/websocket/notifications` also 404s "
        "because it expects a WebSocket upgrade, not a plain GET. Treat `absent` as a "
        "shortlist to verify by hand, and confirm each one the way "
        "`/library/sections/{sectionId}/firstCharacters` was confirmed — by finding the form "
        "the server does answer.",
        "",
        "| Verdict | Meaning |",
        "| --- | --- |",
        "| `exists` | 2xx — route present and answered |",
        "| `absent` | 404 — declared in the spec, not served |",
        "| `needs-params` | 4xx other than 404 — route exists but our probe was incomplete |",
        "| `error` | 5xx or no response |",
        "",
        "| Verdict | Count |",
        "| --- | --: |",
    ]
    for key in ("exists", "absent", "needs-params", "error"):
        lines.append(f"| `{key}` | {counts.get(key, 0)} |")
    lines += [
        f"| **Total** | **{len(rows)}** |",
        "",
        "## Results",
        "",
        "| Verdict | Status | Path | Domain |",
        "| --- | --: | --- | --- |",
    ]
    for path, tag, status, _probe in sorted(rows, key=lambda r: (verdict(r[2]), r[0])):
        lines.append(f"| `{verdict(status)}` | {status or '—'} | `{path}` | {tag} |")

    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"{OUT.relative_to(ROOT)}: probed {len(rows)} GET operations")
    for key in ("exists", "absent", "needs-params", "error"):
        print(f"  {key:13} {counts.get(key, 0)}")


def verdict(status: int) -> str:
    if 200 <= status < 300:
        return "exists"
    if status == 404:
        return "absent"
    if 400 <= status < 500:
        return "needs-params"
    return "error"


if __name__ == "__main__":
    main()
