"""Dump the Plex library schema to docs/plex/db-schema.md.

Reads the pristine template database that ships with Plex Media Server rather than a
populated one: same schema, 318 KB instead of gigabytes, and no personal data to scrub.

Usage: python tools/dump_plex_schema.py
"""

from __future__ import annotations

import sqlite3
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEMPLATE_DB = ROOT / "files" / "Resources" / "com.plexapp.plugins.library.db"
OUT = ROOT / "docs" / "plex" / "db-schema.md"

# Tables on the read path of the API we reimplement. Listed first and in this order,
# because the rest of the schema is sync, plugin and full-text-search machinery that
# does not inform our own model.
CORE_TABLES = (
    "library_sections",
    "section_locations",
    "metadata_items",
    "media_items",
    "media_parts",
    "media_streams",
    "stream_types",
    "tags",
    "taggings",
    "accounts",
    "metadata_item_settings",
    "metadata_item_views",
    "play_queues",
    "play_queue_items",
    "play_queue_generators",
    "library_section_permissions",
    "devices",
    "preferences",
)

# Internal SQLite and extension tables: noise in a schema reference.
SKIP_PREFIXES = ("sqlite_", "fts4_", "spellfix_", "locations_")


def interesting(name: str) -> bool:
    return not name.startswith(SKIP_PREFIXES)


def main() -> None:
    connection = sqlite3.connect(f"file:{TEMPLATE_DB}?mode=ro", uri=True)

    all_tables = [
        row[0]
        for row in connection.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name"
        )
    ]
    other_tables = sorted(t for t in all_tables if interesting(t) and t not in CORE_TABLES)
    ordered = [t for t in CORE_TABLES if t in all_tables] + other_tables

    lines = [
        "# Plex library database schema",
        "",
        "Reference extracted from the pristine template database shipped with Plex Media "
        "Server (`files/Resources/com.plexapp.plugins.library.db`), not from a populated "
        "install — so it carries no personal data.",
        "",
        "**Generated** — regenerate with `python tools/dump_plex_schema.py`.",
        "",
        "This is a *reference*, not a target. klapz designs its own schema; this "
        "document exists to understand how Plex models its entities and which columns feed "
        "which API response attributes. See the notes at the end for the traps it reveals.",
        "",
        f"{len(all_tables)} tables in total; "
        f"{len(all_tables) - len(ordered)} internal SQLite, FTS and R-tree tables omitted.",
        "",
        "## Contents",
        "",
    ]
    lines += [f"- [`{t}`](#{t.replace('_', '-')})" for t in ordered]
    lines += ["- [Notes](#notes)", ""]

    for table in ordered:
        columns = list(connection.execute(f"PRAGMA table_info({table})"))
        indexes = [
            row[1]
            for row in connection.execute(f"PRAGMA index_list({table})")
        ]
        is_core = table in CORE_TABLES
        lines += [
            f"## `{table}`",
            "",
            ("On the API read path." if is_core else "Not on the API read path."),
            "",
            "| Column | Type | Constraints |",
            "| --- | --- | --- |",
        ]
        for _cid, name, column_type, not_null, default, pk in columns:
            constraints = []
            if pk:
                constraints.append("primary key")
            if not_null:
                constraints.append("not null")
            if default is not None:
                constraints.append(f"default `{default}`")
            # `index` and other reserved words appear as column names; quote them so the
            # reference is copy-pasteable into SQL.
            quoted = f"`\"{name}\"`" if name in {"index", "order", "default"} else f"`{name}`"
            lines.append(
                f"| {quoted} | `{column_type or 'blob'}` | {', '.join(constraints) or '—'} |"
            )
        lines += ["", f"{len(indexes)} index(es)." if indexes else "No indexes.", ""]

    lines += [
        "## Notes",
        "",
        "What this schema tells us, and why klapz does not clone it:",
        "",
        "- **Types are integers, the API emits strings.** `metadata_items.metadata_type` and "
        "`library_sections.section_type` are integer codes, while the API exposes "
        "`type=\"movie\"`. The mapping lives at the binding boundary.",
        "- **Timestamps are `datetime`, the API emits epoch seconds.** Another conversion the "
        "binding owns; `core` should hold one unambiguous time type.",
        "- **Packed columns.** `extra_data` and `user_fields` hold encoded key/value blobs "
        "rather than real columns. They are unqueryable and version-fragile — the kind of "
        "design decision we inherit nothing from.",
        "- **`index` is a column name**, and a reserved SQL word. A reminder that identifiers "
        "from this schema cannot be interpolated naively into queries.",
        "- **Denormalized tag columns.** `tags_genre`, `tags_director`, `tags_star` duplicate "
        "what `taggings` already models, as comma-joined strings for display speed.",
        "- **`changed_at` / `resources_changed_at`** are change counters driving sync, not "
        "timestamps in the usual sense.",
        "",
    ]

    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"{OUT.relative_to(ROOT)}: {len(ordered)} tables documented of {len(all_tables)}")


if __name__ == "__main__":
    main()
