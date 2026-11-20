klapz
=====

A media server speaking the Plex Media Server API.

Contents
--------

  klapz-server    The server. Run it with no argument to serve on 127.0.0.1:32500.
  klapz-scanner   Reads one media file for the server's library scans.
  ffmpeg          ffmpeg-plex, the patched ffmpeg the server streams with and the
                  scanner reads files with (as `ffmpeg -probe`).

The server finds the scanner and ffmpeg beside its own executable; KLAPZ_SCANNER and
KLAPZ_TRANSCODER override them. A web client put in a `web` directory beside it (one with
an index.html, such as Plex Web) is served at /web; without one, /web is not served.
KLAPZ_WEB_DIR names another directory.

Getting started
---------------

  KLAPZ_ADMIN_PASSWORD='a long passphrase' ./klapz-server

The first start creates the database (klapz.db in the working directory, or
KLAPZ_DATABASE_URL) and the built-in `admin` account; without KLAPZ_ADMIN_PASSWORD,
its generated password is printed once on standard output. Owners manage users at /admin.

  ./klapz-server library add --kind movie --title Films /path/to/films
  ./klapz-server scan 1

create a library over directories of this machine and scan it without serving; a running
server scans on request.

Burning subtitles into a video needs fonts, which this package does not carry: point
KLAPZ_TRANSCODER_FONTS at a directory of them, or put them in Resources/Fonts beside
ffmpeg.

Licenses
--------

klapz is MIT licensed.

ffmpeg is FFmpeg built with --enable-gpl --enable-version3, and is distributed under the
GNU General Public License, version 3. It runs as a separate program; its exact build options
are in ffmpeg-configure.txt, and its source code is at the address in FFMPEG-SOURCE.txt.
