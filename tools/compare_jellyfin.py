"""Set open-plex's Jellyfin answers against the reference's, capture by capture.

Both sides come from `tools/capture_jellyfin.py`: the reference's in `captures/jellyfin`, ours
in `captures/jellyfin-ours`. For each capture it lists the keys one side writes and the other
does not, at every depth (the first element standing for each array), and the values whose
JSON types differ. Then, for the media sources both sides publish of the same file — matched
by file name, since the two servers number items differently — it compares the tracks field
by field.

Usage: python tools/compare_jellyfin.py [--values]
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REFERENCE = ROOT / "captures" / "jellyfin"
OURS = ROOT / "captures" / "jellyfin-ours"


def kind(value: object) -> str:
    return type(value).__name__


def compare(ours: object, reference: object, at: str, out: list[str]) -> None:
    if isinstance(ours, dict) and isinstance(reference, dict):
        for key, value in reference.items():
            if key not in ours:
                out.append(f"missing {at}.{key}")
            else:
                compare(ours[key], value, f"{at}.{key}", out)
        for key in ours:
            if key not in reference:
                out.append(f"extra   {at}.{key}")
    elif isinstance(ours, list) and isinstance(reference, list):
        if ours and reference:
            compare(ours[0], reference[0], f"{at}[0]", out)
    elif ours is None or reference is None:
        return
    elif kind(ours) != kind(reference) and not (
        isinstance(ours, (int, float)) and isinstance(reference, (int, float))
    ):
        out.append(f"type    {at}: {kind(ours)} where the reference has {kind(reference)}")


def load(path: Path) -> object | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def sources(answer: object) -> dict[str, dict]:
    """Every media source of an answer, by its file's name."""
    found: dict[str, dict] = {}

    def walk(value: object) -> None:
        if isinstance(value, dict):
            if "MediaStreams" in value and "Path" in value and "Protocol" in value:
                name = str(value["Path"]).replace("\\", "/").rsplit("/", 1)[-1]
                found[name] = value
            for child in value.values():
                walk(child)
        elif isinstance(value, list):
            for child in value:
                walk(child)

    walk(answer)
    return found


def main() -> None:
    show_values = "--values" in sys.argv
    reference_sources: dict[str, dict] = {}
    our_sources: dict[str, dict] = {}
    for reference_path in sorted(REFERENCE.glob("*.json")):
        if reference_path.name.endswith(".headers.json"):
            continue
        reference = load(reference_path)
        ours = load(OURS / reference_path.name)
        reference_sources.update(sources(reference))
        our_sources.update(sources(ours))
        if ours is None:
            print(f"== {reference_path.stem}: not captured on our side")
            continue
        differences: list[str] = []
        compare(ours, reference, "", differences)
        print(f"== {reference_path.stem}: {len(differences)} differences")
        for difference in differences:
            print(f"   {difference}")
    for path in (REFERENCE / "items").glob("*.json"):
        reference_sources.update(sources(load(path)))
    for path in (OURS / "items").glob("*.json"):
        our_sources.update(sources(load(path)))
    shared = sorted(set(reference_sources) & set(our_sources))
    print(f"\n== media sources of the same file: {len(shared)}")
    for name in shared:
        theirs = reference_sources[name]["MediaStreams"]
        mine = our_sources[name]["MediaStreams"]
        print(f"-- {name}: {len(mine)} tracks here, {len(theirs)} on the reference")
        for their_track, my_track in zip(theirs, mine):
            for key, value in their_track.items():
                if key not in my_track:
                    print(f"   track {their_track.get('Index')}: missing {key} ({value!r})")
                elif my_track[key] != value and show_values:
                    print(f"   track {their_track.get('Index')}: {key} {my_track[key]!r} vs {value!r}")


if __name__ == "__main__":
    main()
