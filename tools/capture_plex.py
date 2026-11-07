"""Capture responses from a reference Plex server and derive the wire-format map.

Produces two things from the same captures:

  captures/           raw XML and JSON bodies plus response headers. NOT committed: they
                      carry library content, file paths and the token used to fetch them.
  docs/wire-format.md the per-field type map and element-name table. Safe to commit —
                      field names and types only, never values.

Credentials come from the environment so no token is ever written into the repo:

    PLEX_URL    default http://127.0.0.1:32400
    PLEX_TOKEN  required

Usage: PLEX_TOKEN=... python tools/capture_plex.py
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.request
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CAPTURES = ROOT / "captures"
OUT = ROOT / "docs" / "wire-format.md"

BASE = os.environ.get("PLEX_URL", "http://127.0.0.1:32400").rstrip("/")
TOKEN = os.environ.get("PLEX_TOKEN")

# Endpoints worth capturing, in read-path order. `{...}` placeholders are filled from
# earlier captures by `discover`, so the list stays declarative.
ENDPOINTS = (
    "/identity",
    "/",
    "/library/sections",
    "/library/sections/{section}",
    "/library/sections/{section}/all",
    "/library/sections/{section}/all?includeMeta=1",
    "/library/sections/{section}/filters",
    "/library/sections/{section}/sorts",
    # Spec says `firstCharacters`; the real server only answers the singular form.
    "/library/sections/{section}/firstCharacter?type=1",
    "/library/metadata/{item}",
    "/library/metadata/{item}/tree",
    "/library/metadata/{item}/related",
    "/hubs",
    "/hubs/continueWatching",
    "/status/sessions",
    "/playQueues",
    "/:/prefs",
)

# The web client sends this; matching it keeps captures representative.
JSON_ACCEPT = "application/json, text/plain, */*"


def fetch(path: str, accept: str | None) -> tuple[int, dict[str, str], bytes]:
    separator = "&" if "?" in path else "?"
    url = f"{BASE}{path}{separator}X-Plex-Token={TOKEN}"
    request = urllib.request.Request(url)
    if accept:
        request.add_header("Accept", accept)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers), error.read()


def slug(path: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", path.lower()).strip("_") or "root"


def discover() -> dict[str, str]:
    """Pick a real section and item id so the templated endpoints resolve."""
    _, _, body = fetch("/library/sections", JSON_ACCEPT)
    sections = json.loads(body)["MediaContainer"].get("Directory", [])
    if not sections:
        sys.exit("no library sections on the reference server")
    # Prefer a movie section: the richest metadata on the read path.
    section = next((s for s in sections if s.get("type") == "movie"), sections[0])
    section_key = str(section["key"])

    _, _, body = fetch(f"/library/sections/{section_key}/all", JSON_ACCEPT)
    items = json.loads(body)["MediaContainer"].get("Metadata", [])
    if not items:
        sys.exit(f"section {section_key} is empty")
    return {"section": section_key, "item": str(items[0]["ratingKey"])}


class TypeMap:
    """Accumulates observed JSON types and presence counts per structural path."""

    def __init__(self) -> None:
        self.scalars: dict[str, dict[str, set[str]]] = defaultdict(lambda: defaultdict(set))
        self.containers: dict[str, set[str]] = defaultdict(set)
        self.seen: Counter[str] = Counter()
        self.present: dict[str, Counter[str]] = defaultdict(Counter)

    def walk(self, node: object, path: str) -> None:
        if not isinstance(node, dict):
            return
        self.seen[path] += 1
        for key, value in node.items():
            child = f"{path}/{key}"
            if isinstance(value, dict):
                self.containers[child].add("object")
                self.walk(value, child)
            elif isinstance(value, list):
                self.containers[child].add("array")
                for element in value:
                    self.walk(element, child)
            else:
                self.scalars[path][key].add(type(value).__name__)
                self.present[path][key] += 1


JSON_TYPE_NAMES = {"str": "string", "int": "number", "float": "number", "bool": "boolean",
                   "NoneType": "null"}


def main() -> None:
    if not TOKEN:
        sys.exit("PLEX_TOKEN is not set; refusing to run (see this file's docstring)")

    CAPTURES.mkdir(exist_ok=True)
    ids = discover()
    type_map = TypeMap()
    elements: dict[str, list[str]] = {}
    statuses: list[tuple[str, int, str, str]] = []

    for template in ENDPOINTS:
        path = template.format(**ids)
        name = slug(template)

        status, headers, xml_body = fetch(path, None)
        _, json_headers, json_body = fetch(path, JSON_ACCEPT)

        (CAPTURES / f"{name}.xml").write_bytes(xml_body)
        (CAPTURES / f"{name}.json").write_bytes(json_body)
        (CAPTURES / f"{name}.headers.txt").write_text(
            "".join(f"{k}: {v}\n" for k, v in headers.items()), encoding="utf-8"
        )

        statuses.append(
            (template, status, headers.get("Content-Type", "-"),
             json_headers.get("Content-Type", "-"))
        )

        # XML child element names, for the element-to-key table.
        found = sorted(set(re.findall(rb"<([A-Z][A-Za-z0-9]*)", xml_body)))
        elements[template] = [e.decode() for e in found]

        if json_headers.get("Content-Type", "").startswith("application/json"):
            try:
                type_map.walk(json.loads(json_body), "")
            except json.JSONDecodeError:
                pass

    lines = [
        "# Wire format",
        "",
        "Per-field type map derived from a live reference server. **Generated** — regenerate "
        "with `PLEX_TOKEN=... python tools/capture_plex.py`.",
        "",
        "Field names and types only; no captured values appear here. The raw bodies land in "
        "`captures/`, which is not committed because it carries library content and the token "
        "used to fetch it.",
        "",
        "## Endpoints captured",
        "",
        "| Endpoint | Status | Default `Content-Type` | With JSON `Accept` |",
        "| --- | --: | --- | --- |",
    ]
    for template, status, xml_type, json_type in statuses:
        lines.append(f"| `{template}` | {status} | `{xml_type}` | `{json_type}` |")

    lines += [
        "",
        "## XML child elements per endpoint",
        "",
        "Element names present in the XML response. Compare against the JSON keys below: the "
        "mapping is irregular and per-endpoint.",
        "",
        "| Endpoint | Elements |",
        "| --- | --- |",
    ]
    for template, found in elements.items():
        rendered = ", ".join(f"`{e}`" for e in found) or "—"
        lines.append(f"| `{template}` | {rendered} |")

    lines += [
        "",
        "## JSON containers: object or array",
        "",
        "A key listed as `object` is a single nested value in JSON even though XML represents "
        "it as a child element like any other. This cannot be inferred from the XML.",
        "",
        "| Path | Shape |",
        "| --- | --- |",
    ]
    for path in sorted(type_map.containers):
        shapes = ", ".join(sorted(type_map.containers[path]))
        lines.append(f"| `{path}` | {shapes} |")

    lines += ["", "## Scalar fields by level", ""]
    for path in sorted(type_map.scalars):
        total = type_map.seen[path]
        lines += [
            f"### `{path or '/'}`",
            "",
            f"Observed on {total} node(s).",
            "",
            "| Field | JSON type | Present |",
            "| --- | --- | --: |",
        ]
        for key in sorted(type_map.scalars[path]):
            observed = sorted(type_map.scalars[path][key])
            rendered = ", ".join(JSON_TYPE_NAMES.get(t, t) for t in observed)
            count = type_map.present[path][key]
            lines.append(f"| `{key}` | {rendered} | {count}/{total} |")
        lines.append("")

    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"captures -> {CAPTURES.relative_to(ROOT)}/ ({len(statuses)} endpoints)")
    print(f"type map -> {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
