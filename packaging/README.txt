open-plex
=========

A media server speaking the Plex Media Server API.

Contents
--------

  open-plex-server    The server. Run it with no argument to serve on 127.0.0.1:32500.
  open-plex-scanner   Reads one media file for the server's library scans.
  ffmpeg              ffmpeg-plex, the patched ffmpeg the server streams with and the
                      scanner reads files with (as `ffmpeg -probe`).

The server finds the scanner and ffmpeg beside its own executable; OPENPLEX_SCANNER and
OPENPLEX_TRANSCODER override them.

Getting started
---------------

  OPENPLEX_ADMIN_PASSWORD='a long passphrase' ./open-plex-server

The first start creates the database (open-plex.db in the working directory, or
OPENPLEX_DATABASE_URL) and the built-in `admin` account; without OPENPLEX_ADMIN_PASSWORD,
its generated password is printed once on standard output. Owners manage users at /admin.

  ./open-plex-server library add --kind movie --title Films /path/to/films
  ./open-plex-server scan 1

create a library over directories of this machine and scan it without serving; a running
server scans on request.

Burning subtitles into a video needs fonts, which this package does not carry: point
OPENPLEX_TRANSCODER_FONTS at a directory of them, or put them in Resources/Fonts beside
ffmpeg.

Licenses
--------

open-plex is MIT licensed.

ffmpeg is FFmpeg built with --enable-gpl --enable-version3, and is distributed under the
GNU General Public License, version 3. It runs as a separate program; its exact build options
are in ffmpeg-configure.txt, and its source code is at the address in FFMPEG-SOURCE.txt.
