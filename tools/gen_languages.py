"""Generate the language table of `klapz-core` and the `Language` enum of the official API.

One language model for every language field: the languages CLDR 39 names in English (the
ISO 639-1 ones and the ISO 639-2/3 ones a name exists for, the special codes `und`, `mul`,
`zxx` and `mis` among them), each under its shortest code as BCP 47 writes it, with every
alias a file or a provider may use: ISO 639-2/T (`fra`), ISO 639-2/B (`fre`), the retired and
macrolanguage codes CLDR replaces (`iw`, `cmn`), and the English name (`French`). CLDR 39 is
the data of the ICU 69 the reference ships, and the cache is the one
`tools/gen_language_names.py` fills.

Writes `crates/core/src/language/table.rs` and `crates/api/src/language/values.rs`.

Usage: python tools/gen_languages.py
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from gen_language_names import ROOT, SPECIAL, fetch, main_section, rust_string  # noqa: E402

CORE = ROOT / "crates" / "core" / "src" / "language" / "table.rs"
API = ROOT / "crates" / "api" / "src" / "language" / "values.rs"

HEADER = [
    "//! **Generated** by `python tools/gen_languages.py` from CLDR 39. Do not edit; regenerate.",
    "",
]


def variant(code: str) -> str:
    return code[0].upper() + code[1:]


def doc(text: str) -> str:
    return text.replace("\\", "\\\\").rstrip(".") + "."


def main() -> None:
    aliases = fetch("cldr-core/supplemental/aliases.json")["supplemental"]["metadata"]["alias"]["languageAlias"]
    english = main_section("cldr-localenames-full", "en", "languages")["localeDisplayNames"]["languages"]

    # CLDR 39 names no `mis`; ISO 639-2 calls it this.
    english = {"mis": "Uncoded languages", **english}
    codes = sorted({code for code in english if code.isalpha() and 2 <= len(code) <= 3} | set(SPECIAL))
    known = set(codes)

    # ISO 639-2/T, reversed from the "overlong" aliases. Two languages have two: `ro` is
    # `ron` and the retired `mol`, `ak` is `aka` and `twi`; the one sharing the language's
    # first letter is the one ICU reports.
    terminological: dict[str, str] = {}
    bibliographic: dict[str, str] = {}
    for alias, entry in aliases.items():
        replacement = entry["_replacement"]
        if len(alias) != 3 or not alias.isalpha() or replacement not in known:
            continue
        if entry.get("_reason") == "overlong" and len(replacement) == 2:
            current = terminological.get(replacement)
            if current is None or (alias[0] == replacement[0] and current[0] != replacement[0]):
                terminological[replacement] = alias
        elif entry.get("_reason") == "bibliographic":
            bibliographic[replacement] = alias

    table: dict[str, str] = {code: code for code in codes}
    for code in codes:
        if code in terminological:
            table[terminological[code]] = code
        if code in bibliographic:
            table[bibliographic[code]] = code
    for alias, entry in aliases.items():
        replacement = entry["_replacement"]
        if alias.isalpha() and 2 <= len(alias) <= 3 and replacement in known and alias not in known:
            table.setdefault(alias, replacement)

    # English names longer than a code, so that `Ga` (Ga) never shadows `ga` (Irish).
    names: dict[str, str] = {}
    for code in codes:
        name = english[code].lower()
        if len(name) > 3 and name not in table:
            names.setdefault(name, code)

    native: dict[str, str] = {}
    for code in codes:
        own = main_section("cldr-localenames-full", code, "languages")
        if own is not None and code in own["localeDisplayNames"]["languages"]:
            native[code] = own["localeDisplayNames"]["languages"][code]

    core = [
        "//! The languages of [`super::Language`] and the codes and names each is known by.",
        "//!",
        *HEADER,
        "/// A language, by its shortest ISO 639 code: ISO 639-1 where it has one (`Fr`), else",
        "/// ISO 639-2 or 639-3 (`Fil`, `Yue`). `Und`, `Mul`, `Zxx` and `Mis` are ISO 639-2's",
        "/// undetermined, multiple, no linguistic content and uncoded languages.",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]",
        "pub enum Language {",
    ]
    for code in codes:
        core += [f"    /// {doc(english[code])}", f"    {variant(code)},"]
    core += [
        "}",
        "",
        "/// One language's codes and names, at the index of its variant.",
        "#[derive(Debug)]",
        "pub(super) struct Entry {",
        "    pub(super) code: &'static str,",
        "    /// ISO 639-2/T: `fra`. A language with no ISO 639-1 code is its own.",
        "    pub(super) terminological: Option<&'static str>,",
        "    /// ISO 639-2/B where it differs from 639-2/T: `fre`.",
        "    pub(super) bibliographic: Option<&'static str>,",
        "    /// In English: `French`.",
        "    pub(super) name: &'static str,",
        "    /// In itself: `français`.",
        "    pub(super) native_name: Option<&'static str>,",
        "}",
        "",
        f"pub(super) static ENTRIES: [Entry; {len(codes)}] = [",
    ]

    def option(value: str | None) -> str:
        return "None" if value is None else f"Some({rust_string(value)})"

    for code in codes:
        own_three = code if len(code) == 3 else terminological.get(code)
        core += [
            "    Entry {",
            f"        code: {rust_string(code)},",
            f"        terminological: {option(own_three)},",
            f"        bibliographic: {option(bibliographic.get(code))},",
            f"        name: {rust_string(english[code])},",
            f"        native_name: {option(native.get(code))},",
            "    },",
        ]
    core += [
        "];",
        "",
        "/// Every code a language is known by, its own included, sorted for a binary search.",
        f"pub(super) static CODES: [(&str, Language); {len(table)}] = [",
    ]
    core += [f"    ({rust_string(alias)}, Language::{variant(code)})," for alias, code in sorted(table.items())]
    core += [
        "];",
        "",
        "/// English names in lower case, sorted for a binary search: `french` is `fr`.",
        f"pub(super) static NAMES: [(&str, Language); {len(names)}] = [",
    ]
    core += [f"    ({rust_string(name)}, Language::{variant(code)})," for name, code in sorted(names.items())]
    core += ["];", ""]
    CORE.parent.mkdir(parents=True, exist_ok=True)
    CORE.write_text("\n".join(core), encoding="utf-8", newline="\n")

    api = [
        "//! The `Language` enum of the official API, the domain's own value for value.",
        "//!",
        *HEADER,
        "use async_graphql::Enum;",
        "",
        "/// A language, by its shortest ISO 639 code: ISO 639-1 where it has one (`FR`), else",
        "/// ISO 639-2 or 639-3 (`FIL`, `YUE`). `UND`, `MUL`, `ZXX` and `MIS` are ISO 639-2's",
        "/// undetermined, multiple, no linguistic content and uncoded languages; `MIS` also stands",
        "/// for a code this list does not hold, which `Locale.tag` keeps.",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]",
        '#[graphql(remote = "klapz_core::language::Language")]',
        "pub enum Language {",
    ]
    for code in codes:
        api += [f"    /// {doc(english[code])}", f"    {variant(code)},"]
    api += ["}", ""]
    API.parent.mkdir(parents=True, exist_ok=True)
    API.write_text("\n".join(api), encoding="utf-8", newline="\n")
    print(f"{len(codes)} languages, {len(table)} codes, {len(names)} names, {len(native)} native names")


if __name__ == "__main__":
    main()
