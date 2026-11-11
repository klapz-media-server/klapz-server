# Errors at the HTTP boundary

## Contents

- [One type per layer](#one-type-per-layer)
- [ApiError](#apierror)
- [Status mapping](#status-mapping)
- [Body shape](#body-shape)
- [What never reaches the client](#what-never-reaches-the-client)
- [Extractor rejections](#extractor-rejections)

## One type per layer

`db` and `core` have their own `thiserror` enums, described in `SKILL.md`. The binding has exactly
one error type, `ApiError`, and it is the only thing in the repo that knows about HTTP status
codes.

This split is what lets `core` stay HTTP-free. A `core` function returning
`Result<_, LibraryError>` says what went wrong in domain terms; translating "this section does
not exist" into `404` is a protocol decision, and protocol decisions live in the binding.

## ApiError

```rust
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(Cow<'static, str>),
    #[error("not implemented")]
    NotImplemented,
    #[error("internal error")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}
```

Domain errors convert through `From`, which is what makes `?` usable in handlers without a
`map_err` on every call:

```rust
impl From<LibraryError> for ApiError {
    fn from(error: LibraryError) -> Self {
        match error {
            LibraryError::SectionNotFound(_) => Self::NotFound,
            // A referenced file being gone is a server-side inconsistency, not a client
            // mistake: the client asked for something we told it existed.
            LibraryError::MissingFile { .. } => Self::Internal(Box::new(error)),
            LibraryError::Db(e) => Self::Internal(Box::new(e)),
        }
    }
}
```

Write the `match` exhaustively rather than with a catch-all arm. When a new domain variant is
added, the compiler then forces a decision about its status code — which is exactly the moment to
make that decision, instead of silently defaulting to `500`.

## Status mapping

| `ApiError` | Status | When |
| --- | --- | --- |
| `Unauthorized` | `401` | No token, unknown token, or expired token |
| `Forbidden` | `403` | Valid token, insufficient scope or no access to the object |
| `NotFound` | `404` | Unknown id, or an id the bearer may not see (see below) |
| `BadRequest` | `400` | Malformed parameter that an extractor could not reject earlier |
| `NotImplemented` | `501` | Spec route declared but not implemented yet |
| `Internal` | `500` | Anything else |

The `403`/`404` choice is a real decision, not a detail. Returning `403` for an object that
exists but is not shared with the bearer confirms its existence, which leaks the library contents
of other users. Prefer `404` for an object the bearer may not see, and reserve `403` for a
scope failure on an object they can see.

`501` matters for a different reason: during the skeleton phase it is what distinguishes
"declared but unimplemented" from "missing route". The spec-coverage test relies on that
distinction (see `testing.md`), so a stub must never answer `404`.

## Body shape

Settled by capture against a real server (1.43.5): errors return a **minimal HTML document**,
with `Content-Type: text/html` and no charset.

```
HTTP/1.1 401 Unauthorized
X-Plex-Protocol: 1.0
Content-Type: text/html
Content-Length: 91

<html><head><title>Unauthorized</title></head><body><h1>401 Unauthorized</h1></body></html>
```

The body is exactly `<html><head><title>{reason}</title></head><body><h1>{status} {reason}</h1></body></html>`,
on a single line with no trailing newline — which is why `Content-Length` is 91 for
`401 Unauthorized` and 85 for `404 Not Found`.

**Errors ignore content negotiation entirely.** A request sending `Accept: application/json`
still receives HTML on an error. This is counter-intuitive enough to be worth stating plainly: do
not route error responses through the `PlexResponse` negotiation path, or we will emit JSON
where Plex emits HTML and diverge on every failure.

No `MediaContainer` is involved, and there is no error code, machine-readable reason, or detail
field to populate. Clients branch on the status code alone, which is the only thing that carries
information here.

Keep the HTML emitter in one place and derive it from the status code, so that a new variant
cannot produce a differently-shaped body.

## What never reaches the client

`Internal` carries its source error for logging, and deliberately renders as a flat
`"internal error"`. Paths, SQL fragments, ffmpeg command lines and backtraces are diagnostics for
the server's logs, not for the client — leaking them hands an attacker the filesystem layout and
schema.

The pairing to get right: log the full error at the boundary, return the opaque one.

```rust
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let Self::Internal(source) = &self {
            // The only place internal detail is allowed to surface.
            tracing::error!(error = %source, "request failed");
        }
        // ...
    }
}
```

Beware of `#[error(transparent)]` reaching `ApiError`: it forwards the inner message, which is
how a database error message ends up in a client response. `Internal` wraps rather than
forwards for exactly that reason.

## Extractor rejections

Axum's built-in extractors have their own rejection types, and their default messages do not
match this error model — a `Path` rejection returns plain text with a framework-flavored message.

Have custom extractors return `ApiError` directly. For built-in ones, wrap them where the
difference is visible to clients, so that every response from the binding has one consistent
shape. This is also why authentication is an extractor returning `ApiError` rather than a
middleware: the rejection path goes through the same formatting as everything else.
