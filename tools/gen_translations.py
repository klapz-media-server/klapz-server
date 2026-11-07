"""Generate crates/api-plex/src/translations.rs from the reference's gettext catalogs.

The reference translates the few strings it writes itself — season names, hub titles — into
the client's `X-Plex-Language`, from the catalogs it ships under
`Resources/locale/<language>/LC_MESSAGES/plex.mo`. This extracts exactly the strings the
adapter writes, for every catalog, so the wording matches what a client already shows.

Usage: python tools/gen_translations.py   (reads files/, which is not versioned)
"""

from __future__ import annotations

import gettext
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCALES = ROOT / "files" / "Resources" / "locale"
OUT = ROOT / "crates" / "api-plex" / "src" / "translations.rs"

# Field name -> gettext message id. `\x04` separates a context from the message, as gettext
# encodes `pgettext`; `{1}` is the reference's placeholder.
STRINGS = {
    "season": "Season {1}",
    "specials": "Specials",
    "continue_watching": "Continue Watching",
    "recently_added_in": "Library\x04Recently Added in {1}",
    "recent_playlists": "Recent Playlists",
}


def english(message_id: str) -> str:
    return message_id.split("\x04", 1)[-1]


def rust_string(text: str) -> str:
    escaped = text.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def main() -> None:
    catalogs = []
    for directory in sorted(LOCALES.iterdir()):
        mo = directory / "LC_MESSAGES" / "plex.mo"
        if not mo.is_file():
            continue
        with mo.open("rb") as handle:
            catalog = gettext.GNUTranslations(handle)._catalog
        # A string the catalog does not translate falls back to English, as gettext does.
        values = {field: catalog.get(mid) or english(mid) for field, mid in STRINGS.items()}
        catalogs.append((directory.name, values))

    lines = [
        "//! Strings the server writes itself, in every language the reference translates them.",
        "//!",
        "//! **Generated** by `python tools/gen_translations.py` from the reference's gettext",
        "//! catalogs (`Resources/locale/*/LC_MESSAGES/plex.mo`). Do not edit; regenerate.",
        "//! `{1}` is the placeholder the catalogs use.",
        "",
        "/// One language's strings.",
        "#[derive(Debug)]",
        "pub struct Catalog {",
        "    /// The catalog's directory name: `fr`, `pt_BR`.",
        "    pub code: &'static str,",
    ]
    for field in STRINGS:
        lines.append(f"    pub {field}: &'static str,")
    lines += ["}", "", "/// English, the catalogs' source language and the fallback for every other.",
              "pub const ENGLISH: Catalog = Catalog {", '    code: "en",']
    for field, mid in STRINGS.items():
        lines.append(f"    {field}: {rust_string(english(mid))},")
    lines += ["};", "", f"pub const CATALOGS: [Catalog; {len(catalogs)}] = ["]
    for code, values in catalogs:
        lines.append("    Catalog {")
        lines.append(f"        code: {rust_string(code)},")
        for field in STRINGS:
            lines.append(f"        {field}: {rust_string(values[field])},")
        lines.append("    },")
    lines += ["];", ""]
    OUT.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    print(f"{OUT.relative_to(ROOT)}: {len(catalogs)} catalogs, {len(STRINGS)} strings")


if __name__ == "__main__":
    main()
