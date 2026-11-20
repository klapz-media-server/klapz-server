"""Capture responses from a reference Jellyfin server, and scrub them into test fixtures.

  captures/jellyfin/                     raw bodies and headers. NOT committed: they carry the
                                         access token's user, file paths and server names.
  crates/api-jellyfin/tests/fixtures/    with --fixtures: the same bodies scrubbed of the
                                         server's and the user's identifiers, the token, the
                                         machine's paths and names.

Credentials come from the environment, so none is ever written into the repository:

    JELLYFIN_URL       default http://localhost:8096
    JELLYFIN_USER      required
    JELLYFIN_PASSWORD  required

Usage: JELLYFIN_USER=... JELLYFIN_PASSWORD=... python tools/capture_jellyfin.py [--fixtures]
       [--out DIRECTORY] [--all-items] [--writes]

`--writes` makes a playlist and a collection on the server, captures what they answer, and
deletes them.

Pointed at open-plex's own surface (`JELLYFIN_URL=http://127.0.0.1:32500/jellyfin`) with
`--out captures/jellyfin-ours`, the same requests capture our answers, for
`tools/compare_jellyfin.py` to set against the reference's.
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CAPTURES = ROOT / "captures" / "jellyfin"
FIXTURES = ROOT / "crates" / "api-jellyfin" / "tests" / "fixtures"

BASE = os.environ.get("JELLYFIN_URL", "http://localhost:8096").rstrip("/")
CLIENT = 'Client="open-plex capture", Device="capture", DeviceId="open-plex-capture", Version="1.0"'

# What scrubbing puts in place of the reference's own identifiers.
SERVER_ID = "5e1f0a0e5e1f0a0e5e1f0a0e5e1f0a0e"
USER_ID = "0a0e5e1f0a0e5e1f0a0e5e1f0a0e5e1f"

# A device profile like the one Jellyfin Web posts with PlaybackInfo: direct play of what a
# browser plays, HLS for the rest.
DEVICE_PROFILE = {
    "DeviceProfile": {
        "MaxStreamingBitrate": 120000000,
        "DirectPlayProfiles": [
            {"Container": "webm", "Type": "Video", "VideoCodec": "vp8,vp9,av1", "AudioCodec": "vorbis,opus"},
            {"Container": "mp4,m4v", "Type": "Video", "VideoCodec": "h264,vp9,av1", "AudioCodec": "aac,mp3,opus,flac"},
        ],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Video", "AudioCodec": "aac", "VideoCodec": "h264",
             "Context": "Streaming", "Protocol": "hls", "MaxAudioChannels": "2"},
        ],
        "ContainerProfiles": [],
        "CodecProfiles": [],
        "SubtitleProfiles": [{"Format": "vtt", "Method": "External"}],
    }
}


def request(method: str, path: str, token: str | None, body: object | None = None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, method=method, data=data)
    authorization = f"MediaBrowser {CLIENT}"
    if token:
        authorization += f', Token="{token}"'
    req.add_header("Authorization", authorization)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers), error.read()


def sign_in() -> dict:
    user = os.environ.get("JELLYFIN_USER")
    password = os.environ.get("JELLYFIN_PASSWORD")
    if not user or password is None:
        sys.exit("JELLYFIN_USER and JELLYFIN_PASSWORD are required")
    status, _, body = request(
        "POST", "/Users/AuthenticateByName", None, {"Username": user, "Pw": password}
    )
    if status != 200:
        sys.exit(f"sign-in refused: {status}")
    return json.loads(body)


def first(items: list[dict], **matching: str) -> dict:
    for item in items:
        if all(item.get(key) == value for key, value in matching.items()):
            return item
    raise SystemExit(f"nothing matches {matching}")


def captures(token: str, user: str) -> list[tuple[str, str, str, object | None]]:
    """(name, method, path, body), discovering the ids each step needs from the one before."""
    get = lambda path: json.loads(request("GET", path, token)[2])  # noqa: E731
    views = get(f"/UserViews?userId={user}")["Items"]
    movies = first(views, CollectionType="movies")["Id"]
    shows = first(views, CollectionType="tvshows")["Id"]
    movie = get(f"/Items?userId={user}&ParentId={movies}&Recursive=true&IncludeItemTypes=Movie&SortBy=SortName&Limit=1")["Items"][0]["Id"]
    series = get(f"/Items?userId={user}&ParentId={shows}&Recursive=true&IncludeItemTypes=Series&SortBy=SortName&Limit=1")["Items"][0]["Id"]
    season = get(f"/Shows/{series}/Seasons?userId={user}")["Items"][0]["Id"]
    episode = get(f"/Shows/{series}/Episodes?userId={user}&seasonId={season}")["Items"][0]["Id"]
    listing = "Fields=PrimaryImageAspectRatio,SortName,Path,MediaSourceCount&ImageTypeLimit=1&EnableImageTypes=Primary,Backdrop,Banner,Thumb"
    return [
        ("system_info_public", "GET", "/System/Info/Public", None),
        ("system_info", "GET", "/System/Info", None),
        ("system_ping", "GET", "/System/Ping", None),
        ("branding_configuration", "GET", "/Branding/Configuration", None),
        ("quick_connect_enabled", "GET", "/QuickConnect/Enabled", None),
        ("users_public", "GET", "/Users/Public", None),
        ("users_me", "GET", "/Users/Me", None),
        ("user_views", "GET", f"/UserViews?userId={user}", None),
        ("items_movies", "GET", f"/Items?userId={user}&ParentId={movies}&Recursive=true&IncludeItemTypes=Movie&SortBy=SortName,ProductionYear&SortOrder=Ascending&StartIndex=0&Limit=100&{listing}", None),
        ("items_series", "GET", f"/Items?userId={user}&ParentId={shows}&Recursive=true&IncludeItemTypes=Series&SortBy=SortName&SortOrder=Ascending&StartIndex=0&Limit=100&{listing}", None),
        ("item_movie", "GET", f"/Items/{movie}?userId={user}", None),
        ("item_series", "GET", f"/Items/{series}?userId={user}", None),
        ("item_season", "GET", f"/Items/{season}?userId={user}", None),
        ("item_episode", "GET", f"/Items/{episode}?userId={user}", None),
        ("legacy_user_item_movie", "GET", f"/Users/{user}/Items/{movie}", None),
        ("show_seasons", "GET", f"/Shows/{series}/Seasons?userId={user}&Fields=ItemCounts,PrimaryImageAspectRatio", None),
        ("show_episodes", "GET", f"/Shows/{series}/Episodes?userId={user}&seasonId={season}&Fields=ItemCounts,PrimaryImageAspectRatio,Overview", None),
        ("resume", "GET", f"/UserItems/Resume?userId={user}&Limit=12&Fields=PrimaryImageAspectRatio&MediaTypes=Video", None),
        ("latest_movies", "GET", f"/Items/Latest?userId={user}&ParentId={movies}&Limit=16&Fields=PrimaryImageAspectRatio", None),
        ("next_up", "GET", f"/Shows/NextUp?userId={user}&Limit=24&Fields=PrimaryImageAspectRatio", None),
        ("latest_shows", "GET", f"/Items/Latest?userId={user}&ParentId={shows}&Limit=16&Fields=PrimaryImageAspectRatio", None),
        ("items_episodes", "GET", f"/Items?userId={user}&ParentId={shows}&Recursive=true&IncludeItemTypes=Episode&SortBy=SortName&SortOrder=Ascending&StartIndex=0&Limit=100&{listing}", None),
        ("filters_movies", "GET", f"/Items/Filters?userId={user}&ParentId={movies}&IncludeItemTypes=Movie", None),
        ("filters2_movies", "GET", f"/Items/Filters2?userId={user}&ParentId={movies}&IncludeItemTypes=Movie", None),
        ("filters_shows", "GET", f"/Items/Filters?userId={user}&ParentId={shows}&IncludeItemTypes=Series", None),
        ("display_preferences", "GET", f"/DisplayPreferences/usersettings?userId={user}&client=emby", None),
        ("playback_info", "POST", f"/Items/{movie}/PlaybackInfo?userId={user}", DEVICE_PROFILE),
        ("item_unknown", "GET", "/Items/00000000000000000000000000000001", None),
        ("item_malformed", "GET", "/Items/zzz", None),
    ]


def capture_every_item(token: str, user: str) -> None:
    """Every film and episode as the clients ask for it, for comparing the 35 files one by one.

    `items/` holds each lookup, `legacy/` the same through `/Users/{user}/Items/{id}`,
    `playback/` its PlaybackInfo; `seasons/` and `episodes/` each series' children, `series/`
    and `season/` their lookups.
    """
    get = lambda path: json.loads(request("GET", path, token)[2])  # noqa: E731
    write = lambda folder, name, body: (CAPTURES / folder / f"{name}.json").write_bytes(body)  # noqa: E731
    for folder in ("items", "legacy", "playback", "seasons", "episodes", "series", "season"):
        (CAPTURES / folder).mkdir(parents=True, exist_ok=True)
    for kind in ("Movie", "Episode"):
        listing = get(f"/Items?userId={user}&Recursive=true&IncludeItemTypes={kind}&Limit=500")
        for item in listing["Items"]:
            write("items", item["Id"], request("GET", f"/Items/{item['Id']}?userId={user}", token)[2])
            write("legacy", item["Id"], request("GET", f"/Users/{user}/Items/{item['Id']}", token)[2])
            write("playback", item["Id"], request(
                "POST", f"/Items/{item['Id']}/PlaybackInfo?userId={user}", token, DEVICE_PROFILE
            )[2])
        print(f"{len(listing['Items'])} {kind} lookups")
    series = get(f"/Items?userId={user}&Recursive=true&IncludeItemTypes=Series&Limit=500")["Items"]
    fields = "Fields=ItemCounts,PrimaryImageAspectRatio,Overview,MediaSourceCount"
    for show in series:
        write("series", show["Id"], request("GET", f"/Items/{show['Id']}?userId={user}", token)[2])
        _, _, seasons = request("GET", f"/Shows/{show['Id']}/Seasons?userId={user}&{fields}", token)
        write("seasons", show["Id"], seasons)
        write("episodes", show["Id"], request(
            "GET", f"/Shows/{show['Id']}/Episodes?userId={user}&{fields}", token
        )[2])
        for season in json.loads(seasons).get("Items", []):
            write("season", season["Id"], request(
                "GET", f"/Items/{season['Id']}?userId={user}", token
            )[2])
    print(f"{len(series)} series")


def capture_writes(token: str, user: str, record) -> None:
    """What a playlist and a collection answer, made for the capture and deleted after it, and
    the pages of genres, people, studios and years, search hints and media segments."""
    get = lambda path: json.loads(request("GET", path, token)[2])  # noqa: E731
    views = get(f"/UserViews?userId={user}")["Items"]
    movies = first(views, CollectionType="movies")["Id"]
    films = get(f"/Items?userId={user}&ParentId={movies}&Recursive=true&IncludeItemTypes=Movie&SortBy=SortName")["Items"]
    ids = [film["Id"] for film in films]
    created = record("playlist_create", "POST", "/Playlists",
                     {"Name": "open-plex capture", "Ids": ids[:2], "UserId": user, "MediaType": "Video"})
    playlist = created["Id"]
    record("playlist_add", "POST", f"/Playlists/{playlist}/Items?ids={ids[2]}&userId={user}")
    record("playlist", "GET", f"/Playlists/{playlist}")
    entries = record("playlist_items", "GET", f"/Playlists/{playlist}/Items?userId={user}&Fields=PrimaryImageAspectRatio")
    record("playlist_users", "GET", f"/Playlists/{playlist}/Users")
    record("playlist_user", "GET", f"/Playlists/{playlist}/Users/{user}")
    record("playlists_listing", "GET", f"/Items?userId={user}&Recursive=true&IncludeItemTypes=Playlist")
    record("user_views_with_playlist", "GET", f"/UserViews?userId={user}")
    record("playlist_lookup", "GET", f"/Items/{playlist}?userId={user}")
    moved = entries["Items"][2]["PlaylistItemId"]
    record("playlist_move", "POST", f"/Playlists/{playlist}/Items/{moved}/Move/0")
    record("playlist_after_move", "GET", f"/Playlists/{playlist}/Items?userId={user}")
    record("playlist_remove", "DELETE", f"/Playlists/{playlist}/Items?entryIds={moved}")
    record("playlist_delete", "DELETE", f"/Items/{playlist}")

    collection = record("collection_create", "POST", f"/Collections?Name=open-plex%20capture&Ids={ids[0]},{ids[1]}")
    boxset = collection["Id"]
    record("collection_lookup", "GET", f"/Items/{boxset}?userId={user}")
    record("collection_items", "GET", f"/Items?userId={user}&ParentId={boxset}&Fields=PrimaryImageAspectRatio")
    record("collections_listing", "GET", f"/Items?userId={user}&Recursive=true&IncludeItemTypes=BoxSet")
    record("item_collections", "GET", f"/Items/{ids[0]}/Collections?userId={user}")
    record("user_views_with_collection", "GET", f"/UserViews?userId={user}")
    record("collection_add", "POST", f"/Collections/{boxset}/Items?ids={ids[2]}")
    record("collection_remove", "DELETE", f"/Collections/{boxset}/Items?ids={ids[2]}")
    record("collection_delete", "DELETE", f"/Items/{boxset}")

    record("genres", "GET", f"/Genres?userId={user}&ParentId={movies}&SortBy=SortName&SortOrder=Ascending&Recursive=true&Fields=PrimaryImageAspectRatio,ItemCounts&StartIndex=0")
    genre = get(f"/Genres?userId={user}&ParentId={movies}")["Items"][0]
    record("genre", "GET", f"/Genres/{urllib.parse.quote(genre['Name'])}?userId={user}")
    record("items_by_genre", "GET", f"/Items?userId={user}&GenreIds={genre['Id']}&Recursive=true&IncludeItemTypes=Movie")
    record("persons", "GET", f"/Persons?userId={user}&Limit=10&Fields=PrimaryImageAspectRatio")
    person = get(f"/Persons?userId={user}&Limit=1")["Items"][0]
    record("person", "GET", f"/Persons/{urllib.parse.quote(person['Name'])}?userId={user}")
    record("items_by_person", "GET", f"/Items?userId={user}&PersonIds={person['Id']}&Recursive=true&IncludeItemTypes=Movie,Series")
    record("studios", "GET", f"/Studios?userId={user}&ParentId={movies}&Fields=PrimaryImageAspectRatio")
    record("years", "GET", f"/Years?userId={user}&ParentId={movies}&Recursive=true&SortBy=SortName&SortOrder=Descending")
    record("year", "GET", f"/Years/1997?userId={user}")
    record("search_hints", "GET", f"/Search/Hints?userId={user}&searchTerm=doodle&Limit=20")
    record("search_items", "GET", f"/Items?userId={user}&searchTerm=doodle&Recursive=true&IncludeItemTypes=Movie&Limit=24&Fields=PrimaryImageAspectRatio")
    record("media_segments", "GET", f"/MediaSegments/{ids[0]}")


def scrub(text: str, server_id: str, user_id: str, server_name: str, username: str) -> str:
    text = text.replace(server_id, SERVER_ID).replace(user_id, USER_ID)
    text = re.sub(rf'"(Name|UserName)":\s*"{re.escape(username)}"', r'"\1": "user"', text)
    text = re.sub(r"(ApiKey|api_key)=[0-9a-f]+", r"\1=<token>", text)
    # The same ids in their dashed form, as `UserData.Key` writes them.
    dashed = lambda hex_id: f"{hex_id[:8]}-{hex_id[8:12]}-{hex_id[12:16]}-{hex_id[16:20]}-{hex_id[20:]}"  # noqa: E731
    text = text.replace(dashed(server_id), dashed(SERVER_ID)).replace(dashed(user_id), dashed(USER_ID))
    text = re.sub(r'"ServerName":\s*"[^"]*"', '"ServerName": "server"', text)
    text = text.replace(f'"Name": "{server_name}"', '"Name": "server"')
    text = re.sub(r'"traceId":\s*"[^"]*"', '"traceId": "00-0-0-00"', text)
    text = re.sub(r'"AccessToken":\s*"[^"]*"', '"AccessToken": "<token>"', text)
    text = re.sub(r'"(Id|SessionId)":\s*"[0-9a-f]{32}"(?=,\s*"UserId")', r'"\1": "<session>"', text)
    # Windows paths of the reference's machine, inside JSON strings (backslashes doubled).
    text = re.sub(r'[A-Z]:\\\\ProgramData\\\\Jellyfin\\\\Server', "/var/lib/jellyfin", text)
    text = re.sub(r'[A-Z]:\\\\Program Files\\\\Jellyfin\\\\Server', "/usr/lib/jellyfin", text)
    text = re.sub(r'[A-Z]:\\\\samples', "/media", text)
    text = re.sub(r'(?<=/)([^"\\]*)\\\\', r'\1/', text)
    while True:
        cleaned = re.sub(r'("(?:/media|/var/lib/jellyfin|/usr/lib/jellyfin)[^"]*?)\\\\', r'\1/', text)
        if cleaned == text:
            return text
        text = cleaned


def main() -> None:
    global CAPTURES
    write_fixtures = "--fixtures" in sys.argv
    if "--out" in sys.argv:
        CAPTURES = ROOT / sys.argv[sys.argv.index("--out") + 1]
    signed_in = sign_in()
    token = signed_in["AccessToken"]
    user = signed_in["User"]["Id"]
    server = signed_in["ServerId"]
    _, _, public = request("GET", "/System/Info/Public", None)
    server_name = json.loads(public)["ServerName"]
    CAPTURES.mkdir(parents=True, exist_ok=True)
    (CAPTURES / "authenticate_by_name.json").write_text(json.dumps(signed_in, indent=2, ensure_ascii=False), encoding="utf-8")
    outputs = [("authenticate_by_name", 200, json.dumps(signed_in))]

    def record(name: str, method: str, path: str, body: object | None = None) -> object | None:
        status, headers, raw = request(method, path, token, body)
        text = raw.decode("utf-8", errors="replace")
        try:
            answer = json.loads(text)
            text = json.dumps(answer, indent=2, ensure_ascii=False)
        except ValueError:
            answer = None
        (CAPTURES / f"{name}.json").write_text(text, encoding="utf-8")
        (CAPTURES / f"{name}.headers.json").write_text(
            json.dumps({"status": status, "method": method, "path": path, "headers": headers}, indent=2),
            encoding="utf-8",
        )
        print(f"{status} {method} {name}")
        outputs.append((name, status, text))
        return answer

    for name, method, path, body in captures(token, user):
        record(name, method, path, body)
    if "--writes" in sys.argv:
        capture_writes(token, user, record)
    if "--all-items" in sys.argv:
        capture_every_item(token, user)
    if write_fixtures:
        FIXTURES.mkdir(parents=True, exist_ok=True)
        for name, status, text in outputs:
            try:
                text = json.dumps(json.loads(text), indent=2, ensure_ascii=False)
            except ValueError:
                pass
            scrubbed = scrub(text, server, user, server_name, signed_in["User"]["Name"])
            if token in scrubbed:
                sys.exit(f"{name}: the token survived scrubbing")
            (FIXTURES / f"{name}.json").write_text(scrubbed + "\n", encoding="utf-8", newline="\n")
        print(f"fixtures written to {FIXTURES.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
