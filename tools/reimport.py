"""Import a Plex database again into an existing development database, keeping its own data.

The importer replaces every library: sections cascade to their items, and everything the server
wrote about those items goes with them. This wraps it so that nothing the server wrote is lost:

1. it refuses while the database is in use (stop the server first);
2. it backs the database up, whole and consistent, beside it;
3. it keeps the rows the cascade would delete that the importer does not bring back — viewing
   history, track choices, field locks, collections made here — and the libraries added by
   `seed_test_library.py`, which are not in the Plex database;
4. it runs the importer (`open-plex-server import`);
5. it seeds those libraries again, then puts the kept rows back, each one whose parent still
   exists: item ids are Plex's, and the seeder numbers from the largest id, so both come back
   under the same numbers.

Usage: python tools/reimport.py [--source db/com.plexapp.plugins.library.db]
                                [--database open-plex.db]
                                [--server target/release/open-plex-server.exe]
"""

from __future__ import annotations

import argparse
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "tools" / "seed_test_library.py"
SEEDED_UUID = "0a11ce00-0000-4000-8000-0000000000aa"

# What the cascade deletes and the importer does not write back, parents before children.
# Imported rows come back by themselves; a kept row they cover is skipped as a duplicate.
KEPT_TABLES = (
    "viewing_history",
    "track_choice",
    "item_field_lock",
    "collection",
    "collection_entry",
    "collection_word",
)

# Shown before and after, to see the import did what it should.
COUNTED_TABLES = (
    "library_section",
    "metadata_item",
    "media_part",
    "tag",
    "item_marker",
    "collection",
    "collection_entry",
    "viewing_history",
    "track_choice",
    "item_field_lock",
    "watch_state",
    "playlist_entry",
)


def connect(database: Path) -> sqlite3.Connection:
    connection = sqlite3.connect(database, timeout=1)
    connection.execute("pragma foreign_keys = on")
    return connection


def refuse_if_in_use(database: Path) -> None:
    connection = connect(database)
    try:
        connection.execute("begin exclusive")
        connection.rollback()
    except sqlite3.OperationalError:
        sys.exit(f"{database} is in use: stop the server first")
    finally:
        connection.close()


def back_up(database: Path) -> Path:
    target = database.with_name(f"{database.name}.{time.strftime('%Y%m%d-%H%M%S')}.bak")
    source = sqlite3.connect(database)
    copy = sqlite3.connect(target)
    with copy:
        source.backup(copy)
    copy.close()
    source.close()
    return target


def tables(connection: sqlite3.Connection) -> set[str]:
    return {row[0] for row in connection.execute("select name from sqlite_master where type='table'")}


def counts(connection: sqlite3.Connection) -> dict[str, int]:
    present = tables(connection)
    return {
        table: connection.execute(f'select count(*) from "{table}"').fetchone()[0]
        for table in COUNTED_TABLES
        if table in present
    }


def keep(connection: sqlite3.Connection) -> dict[str, tuple[list[str], list[tuple]]]:
    present = tables(connection)
    kept = {}
    for table in KEPT_TABLES:
        if table not in present:
            continue
        columns = [row[1] for row in connection.execute(f'pragma table_info("{table}")')]
        rows = connection.execute(f'select * from "{table}"').fetchall()
        kept[table] = (columns, rows)
    return kept


def seeded_directories(connection: sqlite3.Connection) -> list[str]:
    return [
        row[0]
        for row in connection.execute(
            "select root.path from section_root root join library_section section"
            " on section.id = root.section_id where section.uuid = ?",
            (SEEDED_UUID,),
        )
    ]


def run_import(server: Path, source: Path, database: Path) -> None:
    environment = dict(os.environ, OPENPLEX_DATABASE_URL=f"sqlite://{database.as_posix()}?mode=rwc")
    began = time.time()
    subprocess.run([str(server), "import", str(source)], cwd=ROOT, env=environment, check=True)
    print(f"import finished in {time.time() - began:.0f} s")


def seed(directory: str, database: Path) -> None:
    subprocess.run([sys.executable, str(SEED), directory, "--database", str(database)], cwd=ROOT, check=True)


def put_back(connection: sqlite3.Connection, kept: dict[str, tuple[list[str], list[tuple]]]) -> None:
    for table, (columns, rows) in kept.items():
        names = ", ".join(f'"{column}"' for column in columns)
        marks = ", ".join("?" for _ in columns)
        restored = duplicates = orphans = 0
        for row in rows:
            try:
                cursor = connection.execute(f'insert or ignore into "{table}" ({names}) values ({marks})', row)
            except sqlite3.IntegrityError:
                # Its parent is gone: an item or a file the new import no longer holds.
                orphans += 1
                continue
            if cursor.rowcount:
                restored += 1
            else:
                duplicates += 1
        connection.commit()
        print(f"  {table}: {restored} put back, {duplicates} already there, {orphans} whose parent is gone")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--source", type=Path, default=ROOT / "db" / "com.plexapp.plugins.library.db")
    parser.add_argument("--database", type=Path, default=ROOT / "open-plex.db")
    parser.add_argument("--server", type=Path, default=ROOT / "target" / "release" / "open-plex-server.exe")
    arguments = parser.parse_args()
    for path in (arguments.source, arguments.database, arguments.server):
        if not path.exists():
            sys.exit(f"{path} does not exist")

    refuse_if_in_use(arguments.database)
    print("backup:", back_up(arguments.database))

    connection = connect(arguments.database)
    before = counts(connection)
    kept = keep(connection)
    directories = seeded_directories(connection)
    connection.close()
    print("kept:", {table: len(rows) for table, (_, rows) in kept.items()}, "| seeded libraries:", directories)

    run_import(arguments.server, arguments.source, arguments.database)
    for directory in directories:
        seed(directory, arguments.database)

    connection = connect(arguments.database)
    put_back(connection, kept)
    after = counts(connection)
    connection.close()
    print(f"{'table':20} {'before':>10} {'after':>10}")
    for table, count in after.items():
        print(f"{table:20} {before.get(table, 0):>10} {count:>10}")


if __name__ == "__main__":
    main()
