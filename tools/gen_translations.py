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
    "related_movies": "Related Movies",
    "related_shows": "Related Shows",
    "more_with": "Actor\x04More with {1}",
    "more_by": "Director\x04More by {1}",
    "movies": "Movies",
    "tv_shows": "TV Shows",
    "seasons": "Seasons",
    "episodes": "Episodes",
    "genre": "Genre",
    "year": "Year",
    "decade": "Decade",
    "content_rating": "Content Rating",
    "director": "Director",
    "actor": "Actor",
    "writer": "Writer",
    "producer": "Producer",
    "country": "Country",
    "studio": "Studio",
    "resolution": "Resolution",
    "sort_title": "Title",
    "date_added": "Date Added",
    "release_date": "Release Date",
    "critic_rating": "Critic Rating",
    "audience_rating": "Audience Rating",
    "duration": "Duration",
    "date_viewed": "Date Viewed",
    "randomly": "Randomly",
    "last_episode_date_added": "Last Episode Date Added",
    "unwatched": "Unwatched",
    "in_progress": "In Progress",
    "recently_released_movies": "Recently Released Movies",
    "top_unwatched_movies": "Top Unwatched Movies",
    "recently_watched_movies": "Recently Watched Movies",
    "recently_released_episodes": "Recently Released Episodes",
    "start_watching": "Start Watching",
    "top_rated_tv": "Top Rated TV",
    "recently_watched_episodes": "Recently Watched Episodes",
    "top_movies_in": "Genre\x04Top Movies in {1}",
    "top_movies_by": "Director\x04Top Movies by {1}",
    "top_movies_with": "Actor\x04Top Movies with {1}",
    "rediscover": "Rediscover",
    "visibility": "Visibility",
    "visibility_summary": "Restrict where content from this library should appear.",
    "visibility_everywhere": "Include in home screen and global search",
    "visibility_no_home": "Exclude from home screen",
    "visibility_nowhere": "Exclude from home screen and global search",
    "adult_matching": "Allow matching to explicit content",
    "adult_matching_summary": "Allow matching to adult/pornographic titles and returning adult titles in fix match results. This may hide the title from some features like your Watchlist.",
    # The shelves of a search, `/hubs/search`; films and episodes reuse the strings above.
    "shows": "Shows",
    "collections": "Collections",
    "artists": "Artists",
    "albums": "Albums",
    "tracks": "Tracks",
    "photo_albums": "Photo Albums",
    "automatic_tags": "Automatic Tags",
    "photos": "Photos",
    "tags": "Tags",
    "actors": "Actors",
    "directors": "Directors",
    "genres": "Genres",
    "playlists": "Playlists",
    "shared": "Shared",
    "places": "Places",
    # The words a track's label adds after its language.
    "unknown": "Unknown",
    "forced": "Forced",
    "sdh": "SDH",
    "descriptive": "Descriptive",
    "external": "External",
    # The shelves at the end of a playback, the home screen's, and an item's music shelves.
    "up_next": "Up Next",
    "up_next_in": "Up Next in {1}",
    "on_deck": "On Deck",
    "collection_shelf": "Collection{1} Collection",
    "more_in_genre": "GenreMore in {1}",
    "recently_added_movies": "Recently Added Movies",
    "recently_added_tv": "Recently Added TV",
    "recently_added_music": "Recently Added Music",
    "recently_added_photos": "Recently Added Photos",
    "recently_added_videos": "Recently Added Videos",
    "most_played": "Most Played",
    "most_popular_tracks": "Most Popular Tracks",
    "music_videos": "Music Videos",
    # The filter menu, `/library/sections/{id}/filters` and `includeMeta`.
    "collection": "Collection",
    "video_codec": "Video Codec",
    "audio_codec": "Audio Codec",
    "subtitle_codec": "Subtitle Codec",
    "audio_layout": "Audio Layout",
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
    lines += ["};", "", f"pub static CATALOGS: [Catalog; {len(catalogs)}] = ["]
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
