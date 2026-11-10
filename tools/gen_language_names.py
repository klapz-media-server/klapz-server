"""Generate crates/api-plex/src/language_names.rs from CLDR 39.

The reference names a track's language in the client's language with ICU 69 (`icuuc69.dll`
in its install), which carries CLDR 39: the standard name, the region by its short name
(`Anglais (R.-U.)`), the region dropped when it is the language's likely one (`fr-FR` is
`Français`), and the first letter capitalised where the locale says to for a menu. Measured
identical on 2,516 tracks in fr, en and de. This extracts exactly the tables that needs, for
each of the reference's gettext catalogs.

Usage: python tools/gen_language_names.py   (downloads CLDR 39 JSON into a cache directory)
"""

from __future__ import annotations

import json
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCALES = ROOT / "files" / "Resources" / "locale"
OUT = ROOT / "crates" / "api-plex" / "src" / "language_names.rs"
CLDR = "https://raw.githubusercontent.com/unicode-org/cldr-json/39.0.0/cldr-json"
CACHE = Path(tempfile.gettempdir()) / "open-plex-cldr-39"

# Codes a stream can carry that are not languages of one country but still have names.
SPECIAL = ("mul", "und", "zxx", "mis")


def fetch(path: str) -> dict | None:
    target = CACHE / path
    if not target.is_file():
        target.parent.mkdir(parents=True, exist_ok=True)
        try:
            with urllib.request.urlopen(f"{CLDR}/{path}", timeout=60) as response:
                target.write_bytes(response.read())
        except urllib.error.HTTPError as error:
            if error.code == 404:
                return None
            raise
    return json.loads(target.read_text(encoding="utf-8"))


def main_section(package: str, locale: str, name: str) -> dict | None:
    data = fetch(f"{package}/main/{locale}/{name}.json")
    return None if data is None else data["main"][locale]


def cldr_locale(catalog: str) -> str:
    """The CLDR locale a catalog's directory name stands for: `pt_BR` is `pt`, the CLDR
    default for Portuguese, and `pt` the catalog of Portugal; `no` is Bokmål."""
    tag = catalog.replace("_", "-")
    special = {"pt": "pt-PT", "pt-BR": "pt", "no": "nb", "zh-CN": "zh"}
    if tag in special:
        return special[tag]
    for candidate in (tag, tag.split("-")[0]):
        if main_section("cldr-localenames-full", candidate, "languages") is not None:
            return candidate
    raise SystemExit(f"no CLDR locale for catalog {catalog}")


def rust_string(text: str) -> str:
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def pairs(entries: dict[str, str]) -> str:
    return "[" + ", ".join(f"({rust_string(k)}, {rust_string(v)})" for k, v in sorted(entries.items())) + "]"


def main() -> None:
    supplemental = fetch("cldr-core/supplemental/aliases.json")["supplemental"]["metadata"]["alias"]["languageAlias"]
    likely = fetch("cldr-core/supplemental/likelySubtags.json")["supplemental"]["likelySubtags"]
    english = main_section("cldr-localenames-full", "en", "languages")["localeDisplayNames"]["languages"]

    # Canonical replacements a stored tag may need: `fre` and `fra` are `fr`, `iw` is `he`.
    canonical = {
        code: alias["_replacement"]
        for code, alias in supplemental.items()
        if alias.get("_reason") in ("overlong", "bibliographic", "macrolanguage", "legacy")
        and alias["_replacement"].isalpha()
    }
    # ISO 639-2/T codes, reversed from the "overlong" aliases. Two languages have two: `ro`
    # is `ron` and the retired `mol`, `ak` is `aka` and `twi`; the code sharing the
    # language's first letter is the one ICU reports.
    alpha3: dict[str, str] = {}
    for code, alias in supplemental.items():
        replacement = alias["_replacement"]
        if alias.get("_reason") == "overlong" and len(code) == 3 and len(replacement) == 2 and replacement.isalpha():
            if replacement not in alpha3 or (code[0] == replacement[0] and alpha3[replacement][0] != replacement[0]):
                alpha3[replacement] = code

    named = sorted({code for code in english if code.isalpha() and len(code) == 2} | set(alpha3.values()) | set(SPECIAL))
    named = [code for code in named if code in english]
    regions_of_language = {
        language: tag.split("-")[-1]
        for language, tag in likely.items()
        if "-" not in language and language in named
    }

    endonyms = {}
    for code in named:
        own = main_section("cldr-localenames-full", code, "languages")
        if own is not None and code in own["localeDisplayNames"]["languages"]:
            endonyms[code] = own["localeDisplayNames"]["languages"][code]

    catalogs = sorted(p.name for p in LOCALES.iterdir() if (p / "LC_MESSAGES" / "plex.mo").is_file())
    blocks = []
    for catalog in ["en"] + [c for c in catalogs if c != "en"]:
        locale = cldr_locale(catalog)
        languages = main_section("cldr-localenames-full", locale, "languages")["localeDisplayNames"]["languages"]
        territories = main_section("cldr-localenames-full", locale, "territories")["localeDisplayNames"]["territories"]
        # A regional locale that does not say inherits its language's rule, as ICU resolves
        # it: `pt-PT` capitalises because `pt` does.
        menu = None
        for source in (locale, locale.split("-")[0]):
            transforms = main_section("cldr-misc-full", source, "contextTransforms") or {}
            menu = transforms.get("contextTransforms", {}).get("languages", {}).get("uiListOrMenu")
            if menu is not None:
                break
        names = {code: languages[code] for code in named if code in languages}
        regions = {
            code: territories.get(f"{code}-alt-short", name)
            for code, name in territories.items()
            if "-alt-" not in code
        }
        blocks.append((catalog, locale, menu == "titlecase-firstword", names, regions))

    lines = [
        "//! Language and region names, in every language the reference names them in.",
        "//!",
        "//! **Generated** by `python tools/gen_language_names.py` from CLDR 39, the data of the",
        "//! ICU 69 the reference ships. Do not edit; regenerate. Every table is sorted by key,",
        "//! for binary search.",
        "",
        "/// One client language's names.",
        "#[derive(Debug)]",
        "pub struct LocaleNames {",
        "    /// The gettext catalog it belongs with: `fr`, `pt_BR`.",
        "    pub catalog: &'static str,",
        "    /// Whether a name opening a menu entry takes a capital: `Anglais`, not `anglais`.",
        "    pub titlecase: bool,",
        "    pub languages: &'static [(&'static str, &'static str)],",
        "    /// By their short name where there is one: `R.-U.` rather than `Royaume-Uni`.",
        "    pub regions: &'static [(&'static str, &'static str)],",
        "}",
        "",
        "/// Each language's name in itself: `fr` is `français`.",
        f"pub static ENDONYMS: [(&str, &str); {len(endonyms)}] = {pairs(endonyms)};",
        "",
        "/// The region a language is assumed to be from when a tag names none: `pt` is `BR`.",
        f"pub static LIKELY_REGIONS: [(&str, &str); {len(regions_of_language)}] = {pairs(regions_of_language)};",
        "",
        "/// Deprecated and three-letter codes, by the code they stand for: `fre` is `fr`.",
        f"pub static CANONICAL: [(&str, &str); {len(canonical)}] = {pairs(canonical)};",
        "",
        "/// ISO 639-2/T codes: `fr` is `fra`.",
        f"pub static ALPHA3: [(&str, &str); {len(alpha3)}] = {pairs(alpha3)};",
        "",
        f"pub static LOCALES: [LocaleNames; {len(blocks)}] = [",
    ]
    for catalog, locale, titlecase, names, regions in blocks:
        lines += [
            f"    // CLDR `{locale}`",
            "    LocaleNames {",
            f"        catalog: {rust_string(catalog)},",
            f"        titlecase: {'true' if titlecase else 'false'},",
            f"        languages: &{pairs(names)},",
            f"        regions: &{pairs(regions)},",
            "    },",
        ]
    lines += ["];", ""]
    OUT.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    print(f"{OUT.relative_to(ROOT)}: {len(blocks)} locales, {len(named)} languages, {len(endonyms)} endonyms")


if __name__ == "__main__":
    main()
