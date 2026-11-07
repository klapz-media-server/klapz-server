"""Compare our /library/sections/{id}/all and /library/metadata/{id} against the reference.

Field-by-field over real items, which is the only thing that proves compatibility: a
hand-written fixture only proves we agree with ourselves.

Reports three classes separately, because they call for different work.
  missing   - the reference sends it, we do not. A gap.
  differing - both send it, with different values. A bug, unless explained.
  extra     - we send it, the reference does not. Usually a deliberate deviation.

Usage: PLEX_TOKEN=... OPENPLEX_TOKEN=... python tools/compare_items.py [section...]
"""

from __future__ import annotations

import json
import os
import sys
import urllib.request
from collections import defaultdict

REFERENCE = "http://127.0.0.1:32400"
OURS = "http://127.0.0.1:32500"

# Values that cannot match and should not be reported as differences. Each is either a
# timestamp of when *this* server started or a counter the reference advances on its own.
VOLATILE = {"mediaTagVersion"}


def fetch(base: str, token: str, path: str) -> dict:
    url = f"{base}{path}{'&' if '?' in path else '?'}X-Plex-Token={token}"
    request = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(request) as response:
        return json.loads(response.read().decode("utf-8"))


def walk(node: object, path: str, into: dict[str, object]) -> None:
    """Flattens a container into `path -> scalar`, indexing lists by position."""
    if isinstance(node, dict):
        for key, value in node.items():
            walk(value, f"{path}/{key}", into)
    elif isinstance(node, list):
        for index, value in enumerate(node):
            walk(value, f"{path}[{index}]", into)
    else:
        into[path] = node


def align(container: dict) -> dict:
    """Re-keys a container's items by `ratingKey`.

    Ordering and per-field fidelity are two separate questions, and comparing by position
    conflates them: one item sorted differently makes every field of every later item look
    wrong. Ordering is reported on its own, above.
    """
    aligned = {k: v for k, v in container.items() if k != "Metadata"}
    for item in container.get("Metadata", []):
        aligned[f"item:{item['ratingKey']}"] = item
    return aligned


def compare(reference: dict, ours: dict) -> tuple[dict, dict, dict]:
    left: dict[str, object] = {}
    right: dict[str, object] = {}
    walk(align(reference["MediaContainer"]), "", left)
    walk(align(ours["MediaContainer"]), "", right)

    # Only items both servers returned: an item outside our page is an ordering difference,
    # already reported, not a field gap.
    shared = {k.split("/")[1] for k in left} & {k.split("/")[1] for k in right}
    left = {k: v for k, v in left.items() if k.split("/")[1] in shared}
    right = {k: v for k, v in right.items() if k.split("/")[1] in shared}

    missing = {k: v for k, v in left.items() if k not in right}
    extra = {k: v for k, v in right.items() if k not in left}
    differing = {
        k: (left[k], right[k])
        for k in left
        if k in right and left[k] != right[k] and k.rsplit("/", 1)[-1] not in VOLATILE
    }
    return missing, differing, extra


def summarize(label: str, rows: dict, show: int = 6) -> None:
    """Groups by field name rather than by path: one missing field on 50 items is one gap."""
    by_field: dict[str, list] = defaultdict(list)
    for path, value in rows.items():
        by_field[path.rsplit("/", 1)[-1].split("[")[0]].append((path, value))

    print(f"\n{label}: {len(by_field)} distinct fields over {len(rows)} paths")
    for field, entries in sorted(by_field.items(), key=lambda kv: -len(kv[1])):
        sample = entries[0][1]
        text = json.dumps(sample, ensure_ascii=False)
        if len(text) > 90:
            text = text[:87] + "..."
        print(f"  {field:28s} x{len(entries):<5d} e.g. {text}")
        if show and len(by_field) <= show:
            for path, value in entries[:2]:
                print(f"      {path}")


def main() -> None:
    reference_token = os.environ.get("PLEX_TOKEN")
    our_token = os.environ.get("OPENPLEX_TOKEN")
    if not reference_token or not our_token:
        sys.exit("set PLEX_TOKEN and OPENPLEX_TOKEN")

    sections = [int(a) for a in sys.argv[1:]] or [4, 5]
    size = 10

    for section in sections:
        query = f"/library/sections/{section}/all?X-Plex-Container-Start=0&X-Plex-Container-Size={size}"
        print(f"\n{'=' * 78}\n/library/sections/{section}/all\n{'=' * 78}")
        reference = fetch(REFERENCE, reference_token, query)
        ours = fetch(OURS, our_token, query)

        ref_items = reference["MediaContainer"].get("Metadata", [])
        our_items = ours["MediaContainer"].get("Metadata", [])
        ref_keys = [m["ratingKey"] for m in ref_items]
        our_keys = [m["ratingKey"] for m in our_items]
        print(f"ordering identical: {ref_keys == our_keys}")
        if ref_keys != our_keys:
            print(f"  reference: {ref_keys}")
            print(f"  ours:      {our_keys}")

        missing, differing, extra = compare(reference, ours)
        summarize("missing (reference has, we do not)", missing)
        summarize("extra (we have, reference does not)", extra)
        print(f"\ndiffering: {len(differing)} paths")
        by_field: dict[str, list] = defaultdict(list)
        for path, (left, right) in differing.items():
            by_field[path.rsplit("/", 1)[-1].split("[")[0]].append((path, left, right))
        for field, entries in sorted(by_field.items(), key=lambda kv: -len(kv[1])):
            path, left, right = entries[0]
            print(f"  {field:28s} x{len(entries):<5d} ref={left!r} ours={right!r}")

        # One item in depth, where streams and the per-item library attributes appear.
        if ref_keys and ref_keys == our_keys:
            key = ref_keys[0]
            print(f"\n{'-' * 78}\n/library/metadata/{key}\n{'-' * 78}")
            reference = fetch(REFERENCE, reference_token, f"/library/metadata/{key}")
            ours = fetch(OURS, our_token, f"/library/metadata/{key}")
            missing, differing, extra = compare(reference, ours)
            summarize("missing", missing)
            summarize("extra", extra)
            print(f"\ndiffering: {len(differing)} paths")
            for path, (left, right) in sorted(differing.items()):
                print(f"  {path}\n      ref={left!r}\n      ours={right!r}")


if __name__ == "__main__":
    main()
