---
name: windows-shell
description: The traps of this Windows machine — Git Bash, PowerShell, Python, cargo and a disk that is always nearly full — and the fixes that worked. Load before writing a script that edits files, before a shell command with quotes, backslashes or Plex paths in it, before building or running the test suite, and whenever a build fails strangely (linker errors, a rustc internal compiler error, "invalid metadata", a test run killed).
---

# Working on this machine

Windows 11, Git Bash for POSIX commands, PowerShell 5.1, Python 3.12. Every item here cost a
session time at least once.

## The disk is the first suspect

C: sits at 97–99 % full. A full disk does not say so: it shows up as

- `LNK1180` or `link.exe` exit codes `1318` / `0xc0000005` across dozens of targets;
- a rustc **internal compiler error** in a crate nobody touched, or "invalid metadata";
- `os error 112` ("espace insuffisant") writing the incremental cache;
- a test run that never finishes.

**Check `df -h /c` before re-running anything.** Then, in order:

1. `rm -rf target/debug/incremental` — regenerable, often 8 GB;
2. `rm -f target/debug/deps/*.pdb`, or `cargo clean -p <workspace crates>` — each test binary
   drags a 450 MB PDB, and `deps` reached 17 GB;
3. the `target/` of merged agent worktrees (`agent-delegation` skill);
4. say so in the report, and offer more cleanup when the disk is still above 97 %.

**Every cargo build or test runs light:**

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --all-features
```

`-j 4` or `-j 2` when rustc runs out of paging file. A shared `target/` grew to 73 GB once,
because `--all-features` and plain builds compile under different fingerprints.

## The full test suite

It takes longer than a harness timeout. Run it in the background with its output going to a log,
then read the result:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test --all-features > "$TEMP/fulltest.log" 2>&1; echo "exit $?"
grep -E "test result|FAILED|panicked|^error" "$TEMP/fulltest.log"
```

Trust the exit code and count `FAILED` lines too: an `awk` that summed only the
`test result: ok` lines once reported "99 passed, 0 failed" over two failures.

`cargo clippy` builds no binaries: run `cargo build` before regenerating fixtures with the
scanner binary, or they come from a stale one.

## Locked executables

A running `open-plex-server.exe` locks its file: `cargo build` and `cargo test` cannot relink
it. Stop the server first, check it is not running out of `target\debug` before a clean, and
never build `--release` while the live server runs from it. Redeploying the live server: stop it,
back up `open-plex.db`, build, restart detached (memory: long-running processes).

## Editing files

- **Write and edit files with the Write and Edit tools**, not heredocs. Heredocs broke on French
  apostrophes, Python triple quotes, `\n`, NUL bytes, and collapsed `\\` to `\` in Rust strings
  (`"D:\\Films"`, `\\?\`); a nested heredoc once put a `\x01` into YAML. `sed` replacements on
  paths with backslashes fail silently.
- **A script that patches files must assert each replacement happened.** `cargo fmt` reflows
  code, so a `str.replace` written against the old text silently did nothing, several times —
  once leaving a `SELECT` that imported zeros. Re-read the diff after any scripted change.
- **Python writes CRLF** in text mode (`write_text`, `open(..., "w")`): pass `newline="\n"`, or
  `sed -i 's/\r$//'` the files before committing. `core.autocrlf=true` in the owner's git config
  then warns on files that are really LF.
- Longer Python goes in a file in the scratchpad, run as `python -I -X utf8 file.py`. `-I`
  ignores `PYTHONIOENCODING`, so without `-X utf8` printing non-ASCII crashes in cp1252.
  PowerShell here-strings passed to `python -c` lose their line breaks.

## Git Bash rewrites arguments

MSYS path conversion turns an argument starting with `/` into a Windows path: `/:/prefs` became
`C:/Program Files/Git/:/prefs`, so a proxy meant to refuse that route refused nothing, and an HTTP
helper's error printed a URL with a token. Prefix with `MSYS_NO_PATHCONV=1`, or make the requests
from Python. Pass Windows executables Windows paths (`cygpath -w` / `cygpath -m`); a POSIX path
silently produced an empty comparison. Python cannot read Git Bash's `/tmp`: use the scratchpad.
`/usr/bin/time` does not exist; use the shell's `time`.

## Paths

`std::fs::canonicalize` returns verbatim paths: a mapped network drive becomes
`\\?\UNC\server\share\…`, a drive `\\?\C:\…`. Store them in their plain form
(`commands::displayed`); the standard library adds the prefix back itself when a path is long.
Windows forbids `:` in names, and Unicode look-alike colons make paths that other tools mangle.

## Network

- Windows reserves ranges of ports (`netsh interface ipv4 show excludedportrange protocol=tcp`):
  a bind inside one fails; pick another port.
- A Cloudflare quick tunnel buffers whole responses: a streamed body (SSE, a subtitle stream
  held open) arrives only at its end, over HTTP/2 or QUIC alike. Video segments are unaffected.
  Test a streamed response on the LAN, not through the tunnel.
- The server binds `127.0.0.1` by default: a test from another device needs
  `OPENPLEX_BIND=0.0.0.0:32500` at launch — and a look at what becomes reachable.
- The PowerShell console garbles UTF-8 output; check accented text through a file or a client
  that reads UTF-8.

## Git worktrees

- `git worktree remove` fails while a shell's current directory is inside it: leave it, or
  `rm -rf` it and `git worktree prune`.
- `git branch -d` judges "merged" against the current HEAD: check the branch is an ancestor of
  master (`git merge-base --is-ancestor`), then `-D`.

## Background tasks

- A harness background task is killed at its time limit (30 minutes by default). Servers and
  tunnels run detached through `Start-Process`; long builds and test runs either fit in a
  background task with a long timeout or are waited for explicitly.
- T3's `task_cancel` does not stop a Bash background task; `TaskStop` does.
- A `cloudflared` belonging to T3 runs on this machine: leave it alone.
- Docker Desktop is kept closed. `docker desktop stop` is unsupported here;
  `DockerCli.exe -Shutdown`, then stopping the process, works. Leave it as you found it.
