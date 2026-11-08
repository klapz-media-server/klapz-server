# Security

## Contents

- [Threat model](#threat-model)
- [Path traversal](#path-traversal)
- [Executing ffmpeg](#executing-ffmpeg)
- [Fetching a URL a client names](#fetching-a-url-a-client-names)
- [Tokens and authentication](#tokens-and-authentication)
- [Unauthenticated access (allowedNetworks)](#unauthenticated-access-allowednetworks)
- [SQL](#sql)
- [XML](#xml)
- [Resource limits](#resource-limits)
- [Logging](#logging)
- [Data at rest](#data-at-rest)
- [Dependencies](#dependencies)

## Threat model

Three trust boundaries, worth keeping distinct while writing an endpoint.

**Authenticated but hostile client.** This is the default case and the most important: a valid
token does not make a request harmless. A legitimate shared user can ask for a `partId` they
have no access to, a file path outside any library, or a thousand transcode sessions. Every
endpoint authorizes *the requested object*, not just *the token bearer*.

**Unauthenticated local network.** Plex deliberately exposes part of its surface without a token
(`/identity`, GDM discovery) and allows exempting address ranges. Anything reachable without a
token must be treated as public.

**The internet.** A self-hosted server ends up exposed, through port forwarding or a tunnel.
Nothing may rely on the assumption that the network is trusted.

What is *not* in the model: defending against the machine's administrator. Whoever runs the
server already has the files.

## Path traversal

The project's most likely vulnerability. Several endpoints turn client input into a disk access —
serving media parts, artwork, sidecar subtitles, chapter images.

Filtering `..` by string inspection is off the table: it is bypassable through percent encoding,
alternate UTF-8 encodings, Windows separators (`\`), UNC paths (`\\?\`), and above all through
symlinks, which contain no `..` at all. The only correct approach is to resolve, then compare.

```rust
/// Resolves a client-supplied path against a library root.
///
/// Canonicalization is what makes this safe: it collapses `..`, resolves symlinks and
/// normalizes separators, so the prefix check below cannot be fooled by encoding tricks.
/// It requires the target to exist, which is fine here — a missing file is a 404 anyway.
pub fn resolve_within(root: &Path, requested: &Path) -> Result<PathBuf, PathError> {
    let root = root.canonicalize().map_err(PathError::UnreadableRoot)?;
    let candidate = root.join(requested).canonicalize().map_err(|_| PathError::NotFound)?;

    if candidate.starts_with(&root) {
        Ok(candidate)
    } else {
        Err(PathError::Escapes)
    }
}
```

Two things to know about this pattern. `Path::starts_with` compares component by component
rather than byte by byte, which is what we want — `/media/movies2` does not pass for
`/media/movies`, whereas a string comparison would let it through. And `root.join(requested)`
discards `root` entirely if `requested` is absolute: canonicalization plus the prefix check
still catch that case, but it is the check that protects, not the `join`.

Better still when possible: **never accept a path from the client**. Media parts have a database
identifier; the endpoint takes a `PartId` and the path comes from the database. The `filename`
present in some Plex URLs is cosmetic — it lets the client name its download, and must not
participate in resolution.

That the API *publishes* paths (`MediaFile.path`, `Part.file`) does not weaken this. The two
directions are unrelated: outbound, a path is a fact about the library; inbound, it is an
attacker's free choice of filesystem location. A reviewer seeing a path in a response has no
licence to add a route that takes one.

## Executing ffmpeg

Plex does not encode in-process, it executes `ffmpeg`. Transcode parameters come from the client:
resolution, bitrate, codecs, offsets, track selection. That is a command line built from
untrusted input.

```rust
// NO — a shell, and interpolated client values: arbitrary command execution
Command::new("sh").arg("-c").arg(format!("ffmpeg -i {input} -b:v {bitrate} ..."))

// YES — no shell, one argument per call, values validated upstream
let mut cmd = Command::new(&self.transcoder_path);
cmd.arg("-i").arg(&input_path)          // OsStr, never interpreted
   .arg("-b:v").arg(bitrate.as_ffmpeg_arg())
   .arg("-c:v").arg(video_codec.as_ffmpeg_arg());
```

`Command::arg` hands arguments straight to `execve`/`CreateProcess`: there is no shell to
interpret `;`, `|` or `$()`. So the rule is simple: **never `sh -c`, never `format!` to merge
several arguments into one**.

That alone is not enough, because a single argument can still be dangerous if ffmpeg gives it
meaning:

- A value starting with `-` is read as an option. Validate numeric values by parsing them into a
  numeric type (`u32`, `u64`) rather than relaying a string — a `u32` cannot become `-loglevel`.
- Codecs, containers and profiles come from an `enum`, not a client string. Conversion happens
  through exhaustive matching, which structurally forbids an unexpected value.
- ffmpeg's input protocols are an SSRF surface: if an input path could read `http://…` or
  `concat:…`, ffmpeg would go fetch it. The input comes from the database and is a verified
  local path — never a client value.

On the lifecycle side, a poorly supervised transcode process is a self-inflicted denial of
service:

- Kill the child when the session ends or the client disappears. A `Drop` impl that kills the
  process covers every exit path, including errors.
- Read `stdout` and `stderr` continuously. A full pipe blocks the child indefinitely, and it
  surfaces as a "frozen transcode" that is painful to diagnose.
- Cap concurrent sessions, and bound the lifetime of an idle one.
- Create segment directories with restrictive permissions and clean them up, including after an
  abrupt shutdown (sweep at startup).

## Fetching a URL a client names

`/photo/:/transcode` takes a URL and the server goes and gets it: an SSRF surface. Host
allowlist, TLS only, default port, re-checked on **every redirect hop** — `photo/source.rs` is
the pattern. Compare on the parsed `Url`, never on the string (`https://plex.tv@evil.example/`).
A `reqwest` error quotes its URL, which may hold a token: `without_url()` before logging it.

Bytes fetched that way are decoded in process by the `image` crate: only JPEG, PNG and WebP are
accepted, and the declared canvas is checked before anything is allocated for it.

## Tokens and authentication

Plex accepts `X-Plex-Token` **as a header and as a query parameter**. The spec only declares the
header, but real Plex URLs (artwork, streams) carry the token in the query string and clients
depend on it. Both must be supported, with the consequences accepted.

Token comparison is **constant-time**. A `==` on strings stops at the first differing byte, which
leaks the length of the correct prefix and allows reconstructing a token byte by byte. Use a
dedicated primitive (`subtle::ConstantTimeEq`) or compare digests.

```rust
use subtle::ConstantTimeEq;

fn token_matches(presented: &[u8], expected: &[u8]) -> bool {
    presented.ct_eq(expected).into()
}
```

Because the token sometimes travels in the URL, it ends up in access logs, `Referer` headers and
browser history. Consequences: no full URL in logs (see [Logging](#logging)), and a bounded token
lifetime with revocation.

The spec's scopes (`shared user` / `admin`) are not decorative. Destructive or configuration
operations — `DELETE /library/sections/{id}`, `PUT /:/prefs`, `/status/sessions/terminate`,
Butler tasks, the Updater — require `admin`. A shared-token bearer able to wipe a library is a
privilege escalation. And since the project has no paid tier, **no authorization depends on a
subscription**: scopes are the only authorization axis, and they are role scopes.

Finally, authorization is about the object. A valid `ratingKey` or `partId` does not mean the
bearer is entitled to it: check membership in a library shared with them, in the SQL query
rather than afterwards — a forgotten filter is invisible, while a missing join fails the test.

## Unauthenticated access (allowedNetworks)

Plex lets an operator declare address ranges exempt from authentication. It is convenient, and it
is an authentication bypass by design: treat it as one.

The address to compare is the **socket** address, not a header. `X-Forwarded-For` and
`X-Real-IP` are client-supplied and forgeable: read them only when the server is explicitly
configured behind a trusted proxy, and only from that proxy's address. An
`X-Forwarded-For: 127.0.0.1` must never grant access.

Comparing ranges correctly needs a dedicated type (`ipnet`), not textual prefix matching. Watch
for IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`), which must be reduced to their IPv4 form
before comparison or the rule silently fails to match.

Even when exempt from authentication, a client is not an administrator: the exemption grants a
read scope, never `admin`.

## SQL

Parameterized queries, exclusively. Never `format!` or concatenation to inject a value into SQL,
including an integer — the discipline must be exceptionless, because that is the only version of
it that can be verified by reading.

```rust
// NO
format!("SELECT title FROM metadata_items WHERE id = {id}")

// YES — SeaORM builds the statement, values are bound
MetadataItems::find_by_id(id.get()).one(db).await?
```

The Plex API exposes dynamic sorting and filtering (`sort=titleSort:desc`, per-field filters).
Those are SQL identifiers, which cannot be bound as parameters. They therefore go through a
**mapping to a hardcoded value** — an `enum` of sortable columns, not a sanitized string.
Sanitizing an identifier is a gamble; exhaustive matching is a proof.

## XML

The server **emits** XML, so the risk is injection into attribute values. A movie title
containing `"` or `<` must be escaped by the serializer. Never assemble XML with `format!`; go
through `quick-xml`/serde, which escapes correctly.

If the server ever **parses** XML (import, incoming request), keep entity expansion disabled:
that is the entry point for XXE (local file read, SSRF) and for expansion bombs. `quick-xml` does
not expand external entities by default — do not re-enable that behavior.

## Resource limits

Everything the client controls in size or count needs a server-side cap. Without one, a single
client exhausts the machine, and no malicious intent is required.

- `X-Plex-Container-Size` / `containerSize`: clamp the effective value. A request for 500,000
  items serializes hundreds of megabytes of XML in memory.
- Request body size: `tower_http::limit::RequestBodyLimitLayer`.
- Timeouts on requests (`TimeoutLayer`) and on transcode processes.
- Concurrent transcode sessions: a semaphore, not a hope.
- Depth and cost of recursive metadata queries (`/tree`, `allLeaves` on a large show): bound the
  depth and paginate.
- Library scan jobs: one at a time per section, otherwise a client can trigger a hundred
  concurrent scans through `/library/sections/{id}/refresh`.

## Logging

What must never appear in a log: tokens, full URLs containing a query string, `Authorization`
headers, and user file paths at a default verbosity level.

Since the token travels in the query string, logging `request.uri()` verbatim leaks credentials.
Log the path alone plus a redacted view of the parameters. If you use `tower_http::trace`,
configure explicitly what is captured rather than accepting the default.

Conversely, what must be logged: authentication failures, authorization denials, and path escape
attempts — with the source address. That is what makes an attack detectable, and it is worthless
if drowned in request noise.

## Data at rest

Tokens are stored as digests, never in clear: an exfiltrated database must not grant access.
Machine identifiers and server keys are generated with a cryptographic generator
(`rand::rngs::OsRng`), never with a clock-seeded PRNG.

Configuration files and the database are created with restrictive permissions. No secret, key or
real token in the repository — including in test fixtures: fixtures captured from a real server
must be scrubbed before being committed. The same applies to the reference SQLite database and to
traffic captures.

## Dependencies

`unsafe_code = "forbid"` at the workspace level. If a dependency forces `unsafe` into our code,
that is a decision to document, not a quiet `allow`.

Check advisories and licenses continuously:

```
cargo audit
cargo deny check
```

Before adding a dependency that touches networking, parsing or cryptography, check that it is
maintained, and prefer an established library over one that happens to fit exactly. Do not write
cryptography — use the RustCrypto ecosystem or `ring`.
