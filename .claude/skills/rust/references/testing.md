# Testing

## Contents

- [Two levels](#two-levels)
- [Unit tests](#unit-tests)
- [Driving the router](#driving-the-router)
- [Spec-driven route coverage](#spec-driven-route-coverage)
- [Endpoint conformance tests](#endpoint-conformance-tests)
- [Fixtures](#fixtures)
- [Field completeness against the reference database](#field-completeness-against-the-reference-database)
- [Tooling](#tooling)

## Two levels

Low-level unit tests catch logic errors. Endpoint tests catch compatibility errors. They fail on
different things, and neither substitutes for the other: a correct transcode decision wired to a
response that omits an attribute real clients need will pass every unit test and break every
client.

## Unit tests

Colocated in the file under test, under `#[cfg(test)] mod tests`. They get access to private
items, and keep verification next to the logic.

The right targets are the pure functions where a mistake is silent:

- path resolution against a library root, including the escape attempts from `security.md`;
- identifier and enum conversions (integer `metadata_type` to API `type` string);
- timestamp and duration conversions;
- serializing one type in isolation, especially absent-field behavior;
- pagination arithmetic, where `size` and `totalSize` must not coincide.

`unwrap()` is fine here: a panicking test is a failing test.

## Driving the router

Endpoint tests exercise the real router without opening a socket, through
`tower::ServiceExt::oneshot`. One helper builds a configured app, and every test uses it so the
setup stays in one place.

```rust
/// Builds the app over an in-memory database seeded from fixtures.
///
/// Migrations run here rather than against a prebuilt schema file, so every test run
/// exercises them too.
async fn test_app() -> Router { /* ... */ }

async fn get(app: &Router, uri: &str) -> Response {
    app.clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}
```

## Spec-driven route coverage

The cheapest high-value test in the project: walk `docs/openapi.json` and assert that every
operation resolves to a route. It fails the moment a spec path is missing or misspelled — a
whole class of error that is otherwise found by a client months later.

```rust
/// Every operation in the spec must resolve to a route.
///
/// A missing route answers 404; a declared-but-unimplemented one answers 501. That
/// distinction is the whole point, so stubs must never return 404.
#[tokio::test]
async fn every_spec_operation_is_routed() {
    let app = test_app().await;
    let mut missing = Vec::new();

    for operation in spec_operations() {
        let uri = operation.path_with_sample_parameters();
        let status = request(&app, operation.method, &uri).await.status();

        if status == StatusCode::NOT_FOUND {
            missing.push(format!("{} {}", operation.method, operation.path));
        }
    }

    assert!(missing.is_empty(), "unrouted spec operations:\n{}", missing.join("\n"));
}
```

Two details that decide whether this test is useful or merely noisy. Path parameters must be
filled with plausible sample values, since `{sectionId}` is not a URI — derive them from the
parameter name and type. And the deferred operations from `docs/ENDPOINTS.md` need to be excluded
by an explicit list, so the test tracks real intent rather than failing on the 37 scanner
endpoints that are out of scope on purpose.

Report every failure at once rather than asserting inside the loop. A test that stops at the
first missing route makes you re-run it once per fix.

## Endpoint conformance tests

Per implemented endpoint, assert what a client actually depends on:

- the status code;
- the negotiated `Content-Type` — XML by default, JSON only on `Accept: application/json`;
- the presence and value of the `MediaContainer` attributes;
- **the absence** of attributes that must be omitted;
- the `X-Plex-Container-*` headers agreeing with the container's own counters;
- the same logical response in both formats, since both come from one model.

The absent-attribute assertion is the one most often skipped and the most valuable. An attribute
wrongly present is invisible when eyeballing a response, and clients do read the difference
between an absent `viewOffset` and one set to zero.

Authorization deserves its own tests, not an afterthought: for each endpoint requiring `admin`,
assert that a user-scope token gets refused. That test is what prevents a privilege escalation
from shipping unnoticed.

## Fixtures

Fixtures are real captured responses, never invented data. Invented data proves our serializer
agrees with itself; captures prove it agrees with Plex.

They live in `tests/fixtures/`, named after the endpoint they came from, and are compared as-is.
Scrub tokens, machine identifiers and personal paths before committing — this applies to anything
derived from the reference database or from traffic captures (see `security.md`, "Data at rest").

When a comparison needs normalization — a timestamp, a generated session id — normalize the
specific field explicitly rather than loosening the comparison. A fuzzy comparison stops catching
the thing it was written for.

## Field completeness against the reference database

The reference database under `db/` is a real populated library. Its purpose in testing is to
answer a question no hand-written fixture can: **are we emitting every field that has data?**

The method: for each entity on the read path, enumerate the columns that are actually populated
in the reference data, and check each one either maps to an API attribute we emit or is
deliberately excluded with a reason. A column with values in thousands of rows that appears
nowhere in our output is a compatibility gap, and it is exactly the kind that stays invisible
until a specific client needs that field.

Two cautions. This is a real library, so nothing extracted from it is committed without
scrubbing, and the aggregate analysis (which columns are populated, at what rate) is safe to keep
while row content is not. And a populated column is not automatically an emitted attribute —
Plex has internal columns that never surface. The output of this analysis is a reviewed list, not
an automatic requirement.

## Tooling

```
cargo test --all-features              # the suite, per the closing procedure
cargo nextest run                      # faster, clearer output on a large suite
cargo llvm-cov --all-features --html   # coverage
```

Treat coverage as a way to find untested branches, not as a target to hit. A high percentage on
a suite that never asserts an absent attribute means nothing here; the endpoint conformance
tests are what measure whether this project works.
