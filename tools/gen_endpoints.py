"""Generate docs/ENDPOINTS.md from the Plex OpenAPI spec.

Re-runnable. Statuses split in two: `partial` and `done` record a judgement a human made
and are carried over from the previous file, while `todo`, `stub` and `deferred` are derived
from the spec and from this module's own predicates on every run. Deriving them is what
keeps the tracker honest — an earlier version carried every status over, so the 211 routes
that became stubs in one pass stayed recorded as `todo` and the file quietly lied.

Usage: python tools/gen_endpoints.py
"""

from __future__ import annotations

import json
import re
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "docs" / "openapi.json"
OUT = ROOT / "docs" / "ENDPOINTS.md"

METHODS = ("get", "post", "put", "patch", "delete", "head", "options")

# Status vocabulary.
#   todo     - not started
#   stub     - route exists, returns 501
#   partial  - works for the common path, known gaps noted in the Notes column
#   done     - conformant, covered by tests
#   deferred - deliberately out of scope for now, see DEFERRED below
STATUSES = ("todo", "stub", "partial", "done", "deferred")

# Recomputed on every run rather than read back from the file; see the module docstring.
DERIVED_STATUSES = frozenset({"todo", "stub", "deferred"})

# Paths served by a real handler in crates/api-plex. Registering one twice makes Axum panic
# at startup, so the stub generator needs the same list — it imports it from here.
# Path-level rather than per-operation: Axum panics on two registrations of the same path,
# so a path with a real handler for one method cannot keep stubs for the others. The
# remaining methods show as `todo` in the tracker, which is what they are.
IMPLEMENTED = frozenset(
    {
        "/",
        "/identity",
        "/library/sections",
        "/library/sections/all",
        "/library/sections/{sectionId}/all",
        "/library/metadata/{ids}",
        # Singular. The spec declares only the plural form, which a real server 404s;
        # listed so the stub generator does not register a second route on this path.
        "/library/sections/{sectionId}/firstCharacter",
        # Not in the spec either; the spec declares only `/hubs/search`.
        "/search",
        "/media/providers",
        "/status/sessions",
        "/:/prefs",
        "/updater/status",
        "/updater/check",
    }
)

# Served by a real handler but absent from the spec, so the loop over the spec never sees them.
# Status and notes are written here rather than carried over from the file, since there is no
# spec row for the carry-over to key on. Each is described in docs/spec-gaps.md.
OFF_SPEC = (
    (
        "done",
        "GET",
        "/library/sections",
        "user",
        "Library sections",
        "Byte-identical to the reference. The spec declares only its `/all` alias.",
    ),
    (
        "partial",
        "GET",
        "/library/metadata/{id}/children",
        "user",
        "Seasons of a series, episodes of a season",
        "Zero differing fields against the reference on seven parents across two libraries; "
        "`400` on a film or an episode, as the reference. Gaps: the leading `All episodes` "
        "`Directory` is omitted while `/allLeaves` answers 501, so `size` is one short.",
    ),
    (
        "partial",
        "GET",
        "/library/metadata/{id}/grandchildren",
        "user",
        "Every episode of a series, season by season",
        "Same field fidelity as `/children`. Gaps: those of `/library/metadata/{ids}`.",
    ),
    (
        "partial",
        "GET",
        "/search",
        "user",
        "Global search across libraries",
        "Matches exactly as the reference (word prefix over title, sort title, original "
        "title) but returns matches in title order, unranked and uncut. The spec declares "
        "only `/hubs/search`.",
    ),
)

# Scanning, matching and media analysis live in a separate binary on Plex
# (Plex Media Scanner / Plex Transcoder), so the endpoints that only drive them are
# out of scope until that component exists. Matched as substrings against the path.
DEFERRED_TAGS = frozenset({"Metadata Agents"})
DEFERRED_PATHS = (
    "/refresh",
    "/analyze",
    "/analyzeLoudness",
    "/match",
    "/unmatch",
    "/split",
    "/merge",
    "/addetect",
    "/voiceActivity",
    "/chapterThumbs",
    "/library/optimize",
    "/library/clean/bundles",
    "/library/caches",
    "/indexes",
    "/intros",
)

ROW_RE = re.compile(
    r"^\|\s*`(?P<status>todo|stub|partial|done|deferred)`\s*\|"  # status
    r"\s*`(?P<method>[A-Z]+)`\s*\|"                          # method
    r"\s*`(?P<path>[^`]+)`\s*\|"                             # path
    r"[^|]*\|"                                               # scope
    r"[^|]*\|"                                               # summary
    r"\s*(?P<notes>[^|]*?)\s*\|\s*$"                         # notes
)


def load_previous(path: Path) -> dict[tuple[str, str], tuple[str, str]]:
    """Map (method, path) -> (status, notes) from a previously generated file."""
    if not path.exists():
        return {}
    previous = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        match = ROW_RE.match(line)
        if match:
            key = (match["method"], match["path"])
            previous[key] = (match["status"], match["notes"])
    return previous


def unroutable(path: str) -> str | None:
    """Why this path cannot be a route pattern, or None when it can.

    Two shapes in the spec are not patterns. A trailing `.*` stands for "any extension",
    and a segment like `{streamId}.{ext}` packs two parameters into one segment, which no
    router can split unambiguously. Both need handlers that parse the segment themselves,
    so they stay `todo` rather than becoming stubs.
    """
    if "*" in path:
        return "wildcard in path"
    for segment in path.split("/"):
        if segment.count("{") > 1:
            return f"two parameters in segment `{segment}`"
    return None


def derived_status(path: str, tag: str) -> tuple[str, str]:
    """Status and note for an operation nobody has recorded a judgement on."""
    if tag in DEFERRED_TAGS or any(fragment in path for fragment in DEFERRED_PATHS):
        return "deferred", "scanner/analysis, separate binary"
    if path in IMPLEMENTED:
        return "todo", ""
    if reason := unroutable(path):
        return "todo", f"needs a hand-written handler: {reason}"
    return "stub", ""


def scope_of(operation: dict, spec_default: list) -> str:
    """Narrowest security scope the operation needs, or '-' when unauthenticated."""
    security = operation.get("security", spec_default)
    if security == []:
        return "public"
    scopes = {s for requirement in security for s in next(iter(requirement.values()), [])}
    if not scopes:
        return "token"
    return "admin" if "admin" in scopes and len(scopes) == 1 else "user"


def first_sentence(text: str, limit: int = 110) -> str:
    """Collapse a description to one short line fit for a table cell."""
    flat = " ".join(text.split())
    flat = flat.split(". ")[0].rstrip(".")
    flat = flat.replace("|", "\\|")
    return flat if len(flat) <= limit else flat[: limit - 1].rstrip() + "…"


def main() -> None:
    spec = json.loads(SPEC.read_text(encoding="utf-8"))
    previous = load_previous(OUT)
    spec_security = spec.get("security", [])

    by_tag: dict[str, list[dict]] = defaultdict(list)
    for path, item in spec["paths"].items():
        for method, operation in item.items():
            if method not in METHODS:
                continue
            summary = operation.get("summary") or operation.get("description") or ""
            tag = (operation.get("tags") or ["Untagged"])[0]
            status, notes = derived_status(path, tag)
            recorded = previous.get((method.upper(), path))
            if recorded:
                # A recorded note survives either way: it is the only place the gaps behind
                # a `partial` are written down.
                notes = recorded[1] or notes
                if recorded[0] not in DERIVED_STATUSES:
                    status = recorded[0]
            by_tag[tag].append(
                {
                    "method": method.upper(),
                    "path": path,
                    "scope": scope_of(operation, spec_security),
                    "summary": first_sentence(summary),
                    "status": status,
                    "notes": notes,
                }
            )

    total = sum(len(rows) for rows in by_tag.values())
    counts = defaultdict(int)
    for rows in by_tag.values():
        for row in rows:
            counts[row["status"]] += 1

    lines = [
        "# Endpoints",
        "",
        f"Implementation tracker for the {total} operations in `docs/openapi.json` "
        f"(Plex Media Server {spec['info']['version'].strip()}).",
        "",
        "**Generated** — regenerate with `python tools/gen_endpoints.py` after a spec update. "
        "`partial` and `done` are judgements, carried over from the previous file along with "
        "every note; `todo`, `stub` and `deferred` are derived from the spec on each run, so "
        "they cannot drift out of date.",
        "",
        "## Status vocabulary",
        "",
        "| Status | Meaning |",
        "| --- | --- |",
        "| `todo` | Not started |",
        "| `stub` | Route declared, answers `501 Not Implemented` |",
        "| `partial` | Works on the common path, gaps listed in Notes |",
        "| `done` | Conformant and covered by tests |",
        "| `deferred` | Out of scope for now: scanning, matching and media analysis are a "
        "separate binary on Plex |",
        "",
        "## Progress",
        "",
        "| Status | Operations | Share |",
        "| --- | --: | --: |",
    ]
    for status in STATUSES:
        n = counts[status]
        lines.append(f"| `{status}` | {n} | {n / total * 100:.1f} % |")
    lines += ["| **Total** | **" + str(total) + "** | |", ""]

    lines += [
        "## By domain",
        "",
        "| Domain | Operations | `done` | `partial` | `stub` | `todo` | `deferred` |",
        "| --- | --: | --: | --: | --: | --: | --: |",
    ]
    for tag in sorted(by_tag, key=lambda t: (-len(by_tag[t]), t)):
        rows = by_tag[tag]
        per = defaultdict(int)
        for row in rows:
            per[row["status"]] += 1
        anchor = re.sub(r"[^a-z0-9]+", "-", tag.lower()).strip("-")
        lines.append(
            f"| [{tag}](#{anchor}) | {len(rows)} | {per['done']} | {per['partial']} "
            f"| {per['stub']} | {per['todo']} | {per['deferred']} |"
        )
    lines.append("")

    lines += [
        "## Outside the spec",
        "",
        "Served, but absent from `docs/openapi.json` and therefore from the counts above.",
        "",
        "| Status | Method | Path | Scope | Description | Notes |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for status, method, path, scope, summary, notes in OFF_SPEC:
        lines.append(f"| `{status}` | `{method}` | `{path}` | {scope} | {summary} | {notes} |")
    lines.append("")

    for tag in sorted(by_tag, key=lambda t: (-len(by_tag[t]), t)):
        rows = sorted(by_tag[tag], key=lambda r: (r["path"], r["method"]))
        lines += [
            f"## {tag}",
            "",
            "| Status | Method | Path | Scope | Description | Notes |",
            "| --- | --- | --- | --- | --- | --- |",
        ]
        for row in rows:
            lines.append(
                f"| `{row['status']}` | `{row['method']}` | `{row['path']}` "
                f"| {row['scope']} | {row['summary']} | {row['notes']} |"
            )
        lines.append("")

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"{OUT.relative_to(ROOT)}: {total} operations across {len(by_tag)} domains")
    print("carried over:", sum(1 for k in previous if k), "previous rows")


if __name__ == "__main__":
    main()
