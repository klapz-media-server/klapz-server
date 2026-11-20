"""Set klapz's Jellyfin answers against the reference's, file by file and field by field.

Both sides come from `tools/capture_jellyfin.py --all-items`: the reference's in
`captures/jellyfin`, ours in `captures/jellyfin-ours`. The two servers number items differently,
so items are aligned by the file they play (a film's or an episode's), series and seasons by
the episodes they hold, libraries by their kind; every id is then rewritten into the name of
what it designates on both sides before values are compared.

For each of the files it compares the lookup (`/Items/{id}` and `/Users/{user}/Items/{id}`),
`PlaybackInfo` and the file's entry in its series' `/Shows/{id}/Episodes`, every field at
every depth, tracks by their index in the file. A difference is either explained — its field
matches a rule of `EXPLAINED`, which names the section of `docs/jellyfin/spec-gaps.md` that
measures it — or left unexplained, which is what this tool exists to drive to zero. The
answers that are not about one file (listings, shelves, filters) are compared by shape.

Usage: python tools/compare_jellyfin.py [--explained] [--file NAME]
"""

from __future__ import annotations

import json
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REFERENCE = ROOT / "captures" / "jellyfin"
OURS = ROOT / "captures" / "jellyfin-ours"

HEX_ID = re.compile(r"\b[0-9a-f]{32}\b|\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")

# Differences that are measured and documented: (pattern over the file's name, pattern over the
# field's path, the reason), the reason naming its section of `docs/jellyfin/spec-gaps.md`. The
# path is the field's keys joined by dots, arrays by `[]`, a track by its index.
EXPLAINED: list[tuple[str, str, str]] = [
    # A film alone in its folder is named after the folder, and a date in a name is a year, by
    # the naming rules measured on Plex, which the scan follows.
    (r"^(doodlebug|Le\.mal\.aime|Peur primale)", r"^(Name|SortName|ProductionYear)$", "naming"),
    # What an online metadata provider gave the reference; nothing here fetches it.
    ("", r"^(Overview|Taglines|Genres|GenreItems|People|Studios|ProviderIds|ExternalUrls|RemoteTrailers"
         r"|OfficialRating|CommunityRating|CriticRating|PremiereDate|EndDate|Status|AirDays|AirTime"
         r"|ProductionLocations|OriginalLanguage|ProductionYear|SeriesStudio|CustomRating|Tags)(\.|\[|$)",
     "metadata"),
    ("", r"^(Name|SortName|OriginalTitle|SeriesName|SeasonName|IndexNumber|ParentIndexNumber)$", "metadata"),
    # Artwork and its derivatives.
    ("", r"(Image|BlurHash|Backdrop|Logo|Thumb|Primary|Trickplay)", "artwork"),
    # Opaque or per-server values.
    ("", r"(^|\.)E[Tt]ag$", "opaque"),
    ("", r"^UserData\.Key$", "opaque"),
    ("", r"^(DateCreated|DateLastMediaAdded)$", "dates"),
    ("", r"^CanDelete$", "rights"),
    # A mapped drive is stored as the share it maps: roots are canonical.
    ("", r"(^|\.)Path$", "roots"),
    # The reference's prober states one reference frame for every video.
    ("", r"MediaStreams\[\d+\]\.RefFrames$", "ref-frames"),
    # A conversion's address is the binding's own; attachments and subtitles are not delivered.
    ("", r"\.TranscodingUrl$", "conversion"),
    ("", r"MediaAttachments\.\[\]\.DeliveryUrl$", "delivery"),
    ("", r"MediaStreams\[\d+\]\.(DeliveryMethod|DeliveryUrl|IsExternalUrl)$", "delivery"),
    # The reference keeps a folder per series and per film's directory; the scan groups as Plex.
    ("", r"^(ParentId|SeriesId|SeasonId)$", "grouping"),
    (r"^<(season|series)", r"^LocationType$", "grouping"),
    # A series found under two roots is one series here, two on the reference.
    (r"^<(series|season) of (5x01 - Jackpot|Maggie\.Simpson|\[SUF\] Steven)", "", "grouping"),
    # A series' running time is its provider's.
    (r"^<series", r"^(RunTimeTicks|CumulativeRunTimeTicks)$", "metadata"),
    # What the reference's prober read by decoding, and ffprobe-plex does not.
    (r"^(Peur primale|S01E41 - La cueillette)", r"MediaStreams\[\d+\]\.BitRate$", "decoded"),
    (r"^(Clone Wars S01E06|Daria\.S00E01)", r"MediaStreams\[[234]\]\.(Width|Height)$", "decoded"),
    (r"^S1957E06", r"MediaStreams\[1\]\.BitDepth$", "decoded"),
    (r"^Star\.Trek\.Discovery", r"MediaStreams\[0\]\.(Hdr10PlusPresentFlag|VideoRangeType)$", "decoded"),
]


def explanation(name: str, path: str) -> str | None:
    for file_pattern, field_pattern, reason in EXPLAINED:
        if re.search(file_pattern, name) and re.search(field_pattern, path):
            return reason
    return None


def load(path: Path) -> object | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def file_name(path: str) -> str:
    return str(path).replace("\\", "/").rsplit("/", 1)[-1]


class Side:
    """One server's captures, with every id it publishes named after what it designates."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.items: dict[str, dict] = {}
        self.by_file: dict[str, str] = {}
        self.names: dict[str, str] = {}
        for path in sorted((root / "items").glob("*.json")):
            item = load(path)
            if not isinstance(item, dict) or "MediaSources" not in item:
                continue
            name = file_name(item["MediaSources"][0]["Path"])
            self.items[item["Id"]] = item
            self.by_file[name] = item["Id"]
            self.names[item["Id"]] = f"<file {name}>"
        signed_in = load(root / "authenticate_by_name.json") or {}
        if signed_in:
            self.names[signed_in["User"]["Id"]] = "<user>"
            self.names[signed_in["ServerId"]] = "<server>"
        for view in (load(root / "user_views.json") or {}).get("Items", []):
            self.names[view["Id"]] = f"<library {view.get('CollectionType')}>"

    def name_parents(self) -> None:
        """Series and seasons, named after the first of their episodes by file name."""
        for item_id, item in sorted(self.items.items(), key=lambda pair: self.names[pair[0]]):
            for key, kind in (("SeriesId", "series"), ("SeasonId", "season")):
                parent = item.get(key)
                if parent and parent not in self.names:
                    self.names[parent] = f"<{kind} of {self.names[item_id][6:-1]}>"

    def normalized(self, value: object) -> object:
        if isinstance(value, dict):
            return {key: self.normalized(child) for key, child in value.items()}
        if isinstance(value, list):
            return [self.normalized(child) for child in value]
        if isinstance(value, str):
            return HEX_ID.sub(lambda match: self.names.get(match.group(0).replace("-", ""), "<id>"), value)
        return value


def leaves(value: object, at: str = "") -> dict[str, object]:
    """Every scalar of an answer by its path; tracks keyed by their index in the file."""
    found: dict[str, object] = {}
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "MediaStreams" and isinstance(child, list):
                for track in child:
                    index = track.get("Index") if isinstance(track, dict) else None
                    found.update(leaves(track, f"{at}{key}[{index}]."))
            else:
                found.update(leaves(child, f"{at}{key}."))
        if not value:
            found[at.rstrip(".")] = {}
    elif isinstance(value, list):
        if not value:
            found[at.rstrip(".")] = []
        for position, child in enumerate(value):
            found.update(leaves(child, f"{at}[{position}]."))
    else:
        found[at.rstrip(".")] = value
    return found


def generic(path: str) -> str:
    return re.sub(r"\[\d+\]", "[]", path)


def compare(ours: object, reference: object) -> tuple[int, list[tuple[str, object, object]]]:
    """Fields alike, and the differing ones as (path, ours, reference)."""
    mine, theirs = leaves(ours), leaves(reference)
    same = 0
    differing = []
    for path in sorted(set(mine) | set(theirs)):
        a, b = mine.get(path, "<absent>"), theirs.get(path, "<absent>")
        if a == b or (isinstance(a, (int, float)) and isinstance(b, (int, float)) and float(a) == float(b)):
            same += 1
        else:
            differing.append((path, a, b))
    return same, differing


def episode_entries(side: Side) -> dict[str, dict]:
    """Each episode's entry in its series' `/Shows/{id}/Episodes`, by file name."""
    entries: dict[str, dict] = {}
    for path in (side.root / "episodes").glob("*.json"):
        for entry in (load(path) or {}).get("Items", []):
            item = side.items.get(entry.get("Id"))
            if item:
                entries[file_name(item["MediaSources"][0]["Path"])] = entry
    return entries


class Tally:
    """Fields alike and differing, overall and for the subject being compared."""

    def __init__(self, show_explained: bool) -> None:
        self.show_explained = show_explained
        self.totals: Counter = Counter()
        self.unexplained: Counter = Counter()
        self.explained: Counter = Counter()

    def judge(self, subject: str, label: str, mine: object, theirs: object) -> tuple[int, Counter, list[str]]:
        same, differing = compare(mine, theirs)
        reasons: Counter = Counter()
        lines: list[str] = []
        for path, a, b in differing:
            reason = explanation(subject, path if "MediaStreams" in path else generic(path))
            if reason:
                reasons[reason] += 1
                self.explained[(reason, generic(path))] += 1
                if self.show_explained:
                    lines.append(f"   {label}: {path}: {a!r} vs {b!r}  [{reason}]")
            else:
                self.unexplained[generic(path)] += 1
                lines.append(f"   {label}: {path}: {a!r} vs {b!r}")
        self.totals["same"] += same
        self.totals["explained"] += sum(reasons.values())
        self.totals["unexplained"] += sum(1 for line in lines if not line.endswith("]"))
        return same, reasons, lines

    def report(self, subject: str, parts: list[tuple[int, Counter, list[str]]]) -> None:
        same = sum(part[0] for part in parts)
        reasons = sum((part[1] for part in parts), Counter())
        lines = [line for part in parts for line in part[2]]
        unexplained = sum(1 for line in lines if not line.endswith("]"))
        named = ", ".join(f"{reason} {count}" for reason, count in sorted(reasons.items()))
        print(f"== {subject}: {same} identical, {unexplained} unexplained, explained: {named or 'none'}")
        for line in lines:
            print(line)


def by_index(listing: object) -> dict[object, dict]:
    return {entry.get("IndexNumber"): entry for entry in (listing or {}).get("Items", [])}


def main() -> None:
    show_explained = "--explained" in sys.argv
    only = sys.argv[sys.argv.index("--file") + 1] if "--file" in sys.argv else None
    reference, ours = Side(REFERENCE), Side(OURS)
    reference.name_parents()
    ours.name_parents()
    tally = Tally(show_explained)

    print("# Each file: its lookups, its PlaybackInfo, its entry in its series' episodes")
    their_entries, my_entries = episode_entries(reference), episode_entries(ours)
    for name in sorted(reference.by_file):
        if only and only not in name:
            continue
        if name not in ours.by_file:
            print(f"== {name}: not on our side")
            continue
        their_id, my_id = reference.by_file[name], ours.by_file[name]
        parts = []
        for label, folder in (("lookup", "items"), ("legacy", "legacy"), ("playback", "playback")):
            theirs = reference.normalized(load(REFERENCE / folder / f"{their_id}.json"))
            mine = ours.normalized(load(OURS / folder / f"{my_id}.json"))
            parts.append(tally.judge(name, label, mine, theirs))
        if name in their_entries and name in my_entries:
            parts.append(tally.judge(
                name, "episodes",
                ours.normalized(my_entries[name]), reference.normalized(their_entries[name]),
            ))
        tally.report(name, parts)

    print("\n# Each series and season both servers hold, named by their first episode's file")
    mine_by_name = {named: key for key, named in ours.names.items()}
    for key, named in sorted(reference.names.items(), key=lambda pair: pair[1]):
        kind = "series" if named.startswith("<series") else "season" if named.startswith("<season") else None
        if not kind or named not in mine_by_name or (only and only not in named):
            continue
        mine_key = mine_by_name[named]
        parts = [tally.judge(
            named, "lookup",
            ours.normalized(load(OURS / kind / f"{mine_key}.json")),
            reference.normalized(load(REFERENCE / kind / f"{key}.json")),
        )]
        if kind == "series":
            their_seasons = by_index(load(REFERENCE / "seasons" / f"{key}.json"))
            my_seasons = by_index(load(OURS / "seasons" / f"{mine_key}.json"))
            for index in sorted(set(their_seasons) | set(my_seasons), key=str):
                parts.append(tally.judge(
                    named, f"season {index}",
                    ours.normalized(my_seasons.get(index)), reference.normalized(their_seasons.get(index)),
                ))
        tally.report(named, parts)

    print(
        f"\n== {tally.totals['same']} identical, {tally.totals['explained']} explained, "
        f"{tally.totals['unexplained']} unexplained"
    )
    print("\n== unexplained, by field")
    for path, count in tally.unexplained.most_common():
        print(f"   {count:4} {path}")
    print("\n== explained, by field")
    for (reason, path), count in sorted(tally.explained.items()):
        print(f"   {count:4} {path} [{reason}]")


if __name__ == "__main__":
    main()
