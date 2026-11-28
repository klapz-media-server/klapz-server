# Running klapz

How to import, serve, configure and package the server. Linked from `CLAUDE.md`.

```sh
# Seed our schema from a real Plex database (idempotent; reads the source read-only).
cargo run -p klapz-server -- import db/com.plexapp.plugins.library.db

# Serve. The first start creates the `admin` account; without KLAPZ_ADMIN_PASSWORD its
# generated password is printed once on standard output.
KLAPZ_ADMIN_PASSWORD='a long passphrase' cargo run -p klapz-server
```

Each variable can also come from a `.env` file (gitignored): the one beside the executable,
else, under `cargo run`, the nearest one above the server crate — the repository's. A variable set
in the environment wins over the file, which wins over the default; the file read is logged at
startup.

| Variable | Default | Purpose |
| --- | --- | --- |
| `KLAPZ_DATABASE_URL` | `sqlite://klapz.db?mode=rwc` | Any SeaORM backend; migrations run on connect. |
| `KLAPZ_ADMIN_PASSWORD` | *(none)* | The password of the built-in `admin` owner, set at every start. Unset, a new `admin` gets a generated one, printed once on standard output, and an existing one keeps its own. Outside the password policy (8 to 256 characters), the server refuses to start. The tools sign in with it. |
| `KLAPZ_SESSION_SECRET` | *(drawn per process)* | The HS256 key sessions are signed with, 32 bytes at least. Without it, sessions end when the server stops. |
| `KLAPZ_INSECURE_COOKIES` | *(off)* | `1` drops `Secure` from the session cookie, for plain HTTP on a local network. |
| `KLAPZ_CORS_ORIGINS` | *(none)* | Origins of the web client besides `https://app.klapz.io` (and `localhost` / `127.0.0.1` on any port in a debug build), separated by commas: `https://klapz.example,http://192.168.1.2:5173`. They read every surface with the user's session, cookie included. A value that is not an origin stops the server. |
| `KLAPZ_MACHINE_ID` | placeholder | Must be stable across restarts or clients stop recognizing the server. |
| `KLAPZ_BIND` | `127.0.0.1:32500` | Listen address. Deliberately not 32400, so it can run beside the reference server. |
| `KLAPZ_JELLYFIN_WEB_DIR` | *(none)* | Jellyfin Web, served without a token at `/jellyfin/web`: a Jellyfin install's `jellyfin-web` directory. Not versioned: it is Jellyfin's code. |
| `KLAPZ_WEB_DIR` | `web` beside the server's executable | Plex Web, served without a token at `/web` as the reference serves it, read from disk: nothing of it is compiled into the server. Not versioned: it is Plex's code. No such directory (no `index.html` in it) means `/web` is not served. `/web` redirects to `/web/`, which serves the index, and a path naming no file answers the index, so a client routing in the browser works there; the reference redirects `/web` and `/web/` to `/web/index.html` instead. A client built for this prefix uses relative asset addresses (Vite's `base: './'`) or `/web/` ones. A browser opening `/` itself (`Sec-Fetch-Mode: navigate`, `GET` or `HEAD`) is answered `307` to `/web/` before any token is asked for, where the reference answers `401`; API clients, Plex Web's own calls included, still get the root's capabilities. Without a client, `/` stays as it was. |
| `KLAPZ_TRANSCODER` | `klapz-ffmpeg`, else `ffmpeg`, beside the executable (with `.exe` on Windows), else the `ffmpeg` on `PATH` (`klapz_scanner::transcoder`) | The patched ffmpeg that Direct Stream runs; its command lines use options a stock build lacks. Plex's sources rebuilt with its configuration, codecs included: `github.com/Maxou44/ffmpeg-plex` releases. A stream whose player chooses its rung needs `absolute_segments`, which cuts and numbers its segments on the media's timeline and its audio where each key frame is shown: v8.1.3-13 onward. A scan needs v8.1.3-21 onward for the MP4 layout, the MPEG-TS packet length, H.264's sequence header, Matroska's undetermined languages and fingerprints; an older build leaves them out. A file's hashes (`format.hashes`: the part's `content` hash and OpenSubtitles' `oshash`) need a build after v9.1-84; an older one stores none. A player given a picture subtitle (PGS, DVD, DVB) beside the stream rather than burned in, as TTML (`subtitleFormats: TTML`), needs v8.1.3-69 onward: its `imsc_image` encoder and muxer. A broadcast's teletext subtitles, given as text, need a build with libzvbi, whose ffprobe also prints the subtitle pages a scan stores (`teletext_pages`), the ones a stream carries undeclared in the releases after v9.1-84; an older one fails their extraction, and the player plays on without them. `files/Plex Transcoder.exe` also works for the rest, with the codecs below. |
| `KLAPZ_TRANSCODER_FONTS` | `Resources/Fonts` beside the transcoder | The fonts burned-in subtitles are drawn with: a Plex install's, `files/Resources/Fonts` here. Without them, a burn answers `501`. |
| `KLAPZ_FFPROBE` | `KLAPZ_TRANSCODER -probe` | The ffprobe a scan reads files with. ffmpeg-plex carries its own, run as `ffmpeg -probe` (first argument), from v8.1.3-15; with an older release or Plex's binary, a stock ffprobe. An ffmpeg that refuses `-probe` is named as such at startup and in the scan's failure, never taken for a corrupt file. |
| `KLAPZ_TRAY` | *(on, Windows only)* | `0` or `off` leaves the notification-area icon out. |
| `KLAPZ_TMDB_TOKEN` | the project's, built in | TMDB's v4 read access token, sent as a bearer header. Built into `klapz-agents::credentials`, the repo owner's decision (`security.md`); a blank value keeps the built-in one. |
| `KLAPZ_TVDB_API_KEY` | the project's, built in | TVDB's v4 key, built in likewise. |
| `KLAPZ_EXTRACTS_DIR` | `extracts` beside an SQLite database's file, else beside the executable | Where the analysis keeps what it makes out of a file's content, by its content hash: `ab/{hash}/subtitles/stream-{track}.{ext}` and `ab/{hash}/index-sd.bif`. A content kept there is not extracted again; what an unfinished run left (`*.partial`) is removed at startup. |
| `KLAPZ_IMAGES_DIR` | `images` beside an SQLite database's file, else beside the executable | Where provider images are kept, originals, `provider/ab/{sha256(url)}.{ext}`; served from there when the provider no longer is. |
| `KLAPZ_TVDB_PIN` | *(none)* | A TVDB subscriber's PIN, which a user-supported key requires. Never built in. Startup logs which providers have a key and from where, never the key. |
| `KLAPZ_TRANSCODER_CODECS` | *(none)* | Plex's separately distributed codecs (`%LOCALAPPDATA%/Plex Media Server/Codecs/<build>`), which Plex's own binary needs. The rebuilt one has them built in and needs none. |

`.github/workflows/package.yml` builds the server for Linux (x86_64, arm64), Windows and macOS
(arm64, x86_64) and publishes the binaries as a release on every push to the default branch:
the last `vX.Y.Z` tag with its patch raised (minor or major by running the workflow by hand),
stamped into the crate so the official API reports it. The desktop package (the `klapz`
repository) lays it beside the latest ffmpeg-plex release and the web client, where they find
each other with no variable set.

Imported items point at the paths of the server they were imported from: they play only where
those paths resolve. `python tools/plex_tunnel.py --origin http://LAN-IP:32500` opens a
Cloudflare quick tunnel and publishes its new address to plex.tv through `moveExtension`, signed
in as `admin`, with the server running; it refuses a loopback origin, where the transcoder's
callbacks answer without a token. A quick tunnel buffers whole responses, so a streamed body
arrives only at its end: test streamed responses without it.
`klapz-server library add --kind movie|show --title TITLE ROOT...` creates a library over
directories of this machine, and `klapz-server scan ID [--force]` scans it without serving,
waiting for its files to be read (a running server scans on `scanLibrary` or
`/library/sections/{id}/refresh`, and reads in the background); this supersedes
`python tools/seed_test_library.py DIRECTORY`, which writes a `Test` library straight to the
tables; `python tools/capture_transcoder.py` studies the transcoder alone.
To import again into a database the server has written to, `python tools/reimport.py`, server
stopped: the importer replaces every library, and with them the history, track choices,
locks and collections made here and the seeded libraries; the script backs the database up,
keeps those, imports, seeds again and puts back whatever still has its item.

