"""Run Plex's transcoder on a local file and record what it produces.

The reference server drives `Plex Transcoder` with a command line it writes to its own log.
This replays that shape without the server: it serves the media over HTTP, as the server does,
receives the transcoder's segment announcements and progress reports, and reports the segments
and calls it observed. Manifests are not produced here: on Plex the server writes them.

Usage: python tools/capture_transcoder.py FILE [--case burn|transcode|copy] [--segments 5]
                                          [--keep DIR]

Needs `files/Plex Transcoder.exe` and the codecs Plex downloaded under
`%LOCALAPPDATA%/Plex Media Server/Codecs`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import threading
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "files" / "Plex Transcoder.exe"
FONTS = ROOT / "files" / "Resources" / "Fonts"
CODECS = Path(os.environ["LOCALAPPDATA"]) / "Plex Media Server" / "Codecs"
CHUNK = 1 << 20


class Capture:
    """What the transcoder told the fake server, with when it said it."""

    def __init__(self) -> None:
        self.started = time.monotonic()
        self.lock = threading.Lock()
        self.calls: list[tuple[float, str, str, str]] = []

    def record(self, method: str, path: str, body: str) -> None:
        with self.lock:
            self.calls.append((time.monotonic() - self.started, method, path, body))


def handler_for(media: Path, capture: Capture) -> type[BaseHTTPRequestHandler]:
    size = media.stat().st_size

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_args: object) -> None:
            pass

        def do_GET(self) -> None:  # noqa: N802 - the name http.server dispatches on
            if not self.path.startswith("/file"):
                capture.record("GET", self.path, "")
                self.answer()
                return
            start, end = 0, size - 1
            wanted = re.match(r"bytes=(\d+)-(\d*)", self.headers.get("Range", ""))
            if wanted:
                start = int(wanted[1])
                end = int(wanted[2]) if wanted[2] else end
            self.send_response(206 if wanted else 200)
            self.send_header("Content-Type", "video/x-matroska")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(end - start + 1))
            if wanted:
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            self.end_headers()
            try:
                with media.open("rb") as source:
                    source.seek(start)
                    left = end - start + 1
                    while left > 0 and (data := source.read(min(CHUNK, left))):
                        self.wfile.write(data)
                        left -= len(data)
            except OSError:
                self.close_connection = True

        def read_body(self) -> None:
            if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
                while (line := self.rfile.readline().strip()) and (length := int(line, 16)):
                    capture.record(self.command, self.path, self.rfile.read(length).decode())
                    self.rfile.readline()
                self.rfile.readline()
                return
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            capture.record(self.command, self.path, body.decode())

        def answer(self) -> None:
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def do_POST(self) -> None:  # noqa: N802
            try:
                self.read_body()
                self.answer()
            except (OSError, ValueError):
                self.close_connection = True

        do_PUT = do_POST  # noqa: N815

    return Handler


def filter_path(path: Path) -> str:
    """A path as a value inside a filtergraph, escaped for both levels of parsing."""
    return path.as_posix().replace(":", "\\\\\\:").replace(" ", "\\\\\\ ")


def video_encode() -> list[str]:
    return [
        "-codec:0", "libx264", "-crf:0", "21", "-maxrate:0", "1297k", "-bufsize:0", "2594k",
        "-preset:0", "veryfast", "-x264opts:0", "subme=2:me_range=4:rc_lookahead=20:me=hex",
        "-force_key_frames:0", "expr:gte(t,n_forced*8)",
    ]


def audio_encode(index: int) -> list[str]:
    return [
        "-filter_complex", f"[0:{index}] aresample=async=1:ochl='stereo':osr=48000[a]",
        "-map", "[a]", "-codec:1", "aac_mf", "-b:1", "162k",
    ]


def case_arguments(case: str, audio: int, subtitle: int | None) -> list[str]:
    if case == "dash":
        return [
            "-map", "0:0", "-codec:0", "copy", "-filter_complex",
            f"[0:{audio}] aresample=async=1:ochl='stereo':osr=48000[a]", "-map", "[a]",
            "-codec:1", "aac_mf", "-b:1", "256k",
        ]
    if case == "copy":
        return ["-map", "0:0", "-codec:0", "copy", "-map", f"0:{audio}", "-codec:1", "copy"]
    scale = "[0:0]scale=w=720:h=404:force_divisible_by=4[0];[0]format=pix_fmts=yuv420p|nv12"
    if case == "burn":
        if subtitle is None:
            raise SystemExit("this file has no subtitle to burn")
        font = FONTS / "NotoSans-Medium.otf"
        burn = (
            f"{scale}[1];[1]inlineass=font_scale=1.0:font_path={filter_path(font)}:"
            f"fonts_dir={filter_path(FONTS)}:language=fr:outline=2.6:shadow=1.7:font_size=54[v]"
        )
        return [
            "-map_inlineass", f"0:{subtitle}", "-filter_complex", burn, "-map", "[v]",
            *video_encode(), *audio_encode(audio),
        ]
    return ["-filter_complex", f"{scale}[v]", "-map", "[v]", *video_encode(), *audio_encode(audio)]


def external_libs() -> str:
    """The codec directory, escaped the way the reference server's own command line does."""
    directory = next(path for path in CODECS.iterdir() if "windows" in path.name)
    escaped = str(directory).replace("\\", r"\\").replace(":", r"\:").replace(" ", r"\ ")
    slash = chr(92)
    return f"{slash * 4}?{slash * 2}{escaped}{slash * 2}"


def segment_output(base: str, case: str, subtitle: int | None) -> list[str]:
    return [
        "-segment_format", "mpegts", "-f", "ssegment", "-individual_header_trailer", "0",
        "-segment_time", "8", "-segment_start_number", "0", "-segment_time_delta", "0.0625",
        "-segment_list", f"{base}/manifest", "-segment_list_type", "csv",
        "-segment_list_size", "5", "-segment_list_separate_stream_times", "1",
        "-segment_list_unfinished", "1", "-max_delay", "5000000",
        "-avoid_negative_ts", "disabled", "-map_metadata", "-1", "-map_chapters", "-1",
        "media-%05d.ts",
        *(["-map", f"0:{subtitle}", "-f", "null", "-codec", "ass", "nullfile"]
          if case == "burn" else []),
    ]


def dash_output(base: str) -> list[str]:
    return [
        "-f", "dash", "-seg_duration", "5", "-dash_segment_type", "mp4",
        "-init_seg_name", "init-stream$RepresentationID$.m4s",
        "-media_seg_name", "chunk-stream$RepresentationID$-$Number%05d$.m4s",
        "-window_size", "5", "-delete_removed", "false", "-skip_to_segment", "1",
        "-manifest_name", f"{base}/manifest?X-Plex-Http-Pipeline=infinite",
        "-avoid_negative_ts", "disabled", "-map_metadata", "-1", "-map_chapters", "-1", "dash",
    ]


def probe(media: Path) -> list[dict]:
    output = subprocess.run(
        ["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(media)],
        capture_output=True, text=True, check=True,
    )
    return json.loads(output.stdout)["streams"]


def describe(segment: Path) -> str:
    output = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "stream=codec_name,width,height,start_time:"
         "format=duration,start_time", "-of", "compact=p=0:nk=1", str(segment)],
        capture_output=True, text=True,
    )
    return " | ".join(output.stdout.split())


class QuietServer(ThreadingHTTPServer):
    """A transcoder dropping its connection when it stops is normal, not worth a traceback."""

    def handle_error(self, request: object, client_address: object) -> None:
        pass


def announced_bodies(capture: Capture) -> list[str]:
    return [body for _, _, path, body in capture.calls if path.startswith("/manifest") and body]


def report_dash(output: Path) -> None:
    """The chunks of each stream, timed by joining them to their init segment."""
    for stream in (0, 1):
        chunks = sorted(output.glob(f"chunk-stream{stream}-*.m4s"))
        if not chunks:
            continue
        init = output / f"init-stream{stream}.m4s"
        print(f"  stream {stream}: init {init.stat().st_size if init.exists() else None} B, {len(chunks)} chunks")
        for chunk in chunks[:6]:
            joined = output / "joined.mp4"
            joined.write_bytes(init.read_bytes() + chunk.read_bytes())
            print(f"    {chunk.name} {chunk.stat().st_size:>9} B  {describe(joined)}")
            joined.unlink()


def main() -> None:
    sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser()
    parser.add_argument("file", type=Path)
    parser.add_argument("--case", choices=("burn", "transcode", "copy", "dash"), default="transcode")
    parser.add_argument("--segments", type=int, default=5)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--keep", type=Path)
    args = parser.parse_args()

    streams = probe(args.file)
    index_of = lambda kind: next((s["index"] for s in streams if s["codec_type"] == kind), None)
    audio, subtitle = index_of("audio"), index_of("subtitle")

    capture = Capture()
    server = QuietServer(("127.0.0.1", 0), handler_for(args.file, capture))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"

    with tempfile.TemporaryDirectory() as scratch:
        output = args.keep or Path(scratch)
        output.mkdir(parents=True, exist_ok=True)
        environment = dict(os.environ, FFMPEG_EXTERNAL_LIBS=external_libs())
        command = [
            str(BINARY), "-codec:0", "h264", "-analyzeduration", "20000000",
            "-probesize", "20000000", "-i", f"{base}/file.mkv", "-start_at_zero", "-copyts",
            "-y", "-nostats", "-loglevel", "error", "-progressurl", f"{base}/progress",
            *case_arguments(args.case, audio, subtitle),
            *(dash_output(base) if args.case == "dash" else segment_output(base, args.case, subtitle)),
        ]
        process = subprocess.Popen(
            command, cwd=output, env=environment, stderr=subprocess.PIPE, text=True,
            encoding="utf-8", errors="replace",
        )
        deadline = time.monotonic() + args.timeout
        while process.poll() is None and time.monotonic() < deadline:
            if len(list(output.glob("media-*.ts"))) + len(list(output.glob("chunk-stream0-*"))) > args.segments:
                break
            time.sleep(0.2)
        elapsed = time.monotonic() - capture.started
        still_running = process.poll() is None
        process.terminate()
        _, errors = process.communicate(timeout=20)

        print(f"case {args.case}: ran {elapsed:.1f} s, "
              f"{'stopped by us' if still_running else f'exit code {process.returncode}'}")
        if errors.strip():
            print("stderr:", errors.strip()[:1200])
        report_dash(output)
        for segment in sorted(output.glob("media-*.ts"))[: args.segments]:
            print(f"  {segment.name} {segment.stat().st_size:>9} B  {describe(segment)}")

    if args.case == "dash" and announced_bodies(capture):
        print("first manifest:", announced_bodies(capture)[0][:1500])
        print("last manifest:", announced_bodies(capture)[-1][:1500])
    print("calls:", dict(Counter((method, path.split("?")[0]) for _, method, path, _ in capture.calls)))
    announced = [c for c in capture.calls if c[2].startswith("/manifest")]
    for moment, method, _, body in announced[:8]:
        print(f"  {moment:6.1f} s {method} manifest: {body.strip()[:150]!r}")
    reports = [c for c in capture.calls if c[2].startswith("/progress")]
    for moment, method, path, body in reports[:2] + reports[-1:]:
        print(f"  {moment:6.1f} s {method} progress: {(path + ' ' + body)[:300]!r}")


if __name__ == "__main__":
    main()
