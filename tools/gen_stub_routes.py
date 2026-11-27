"""Generate crates/api-plex/src/stubs.rs from the Plex OpenAPI spec.

Every spec operation that is neither deferred nor already implemented gets a route that
answers 501. That is what makes the spec-coverage test meaningful: a missing route answers
404, a known-unimplemented one answers 501, and only the first is a bug.

Generated rather than hand-written because there are over two hundred, and generated into a
*file* rather than a macro so every path stays greppable.

Emitted as a table walked by one short function rather than as a 200-link `.route()` chain.
A chain of that size is a function `clippy::too_many_lines` rightly rejects, and splitting it
into chunks meant predicting how `cargo fmt` wraps each link — which is guesswork. A table
has no such coupling: one path per line, whatever rustfmt does.

Usage: python tools/gen_stub_routes.py
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from gen_endpoints import (  # noqa: E402
    DEFERRED_PATHS,
    DEFERRED_TAGS,
    IMPLEMENTED,
    unroutable,
)

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "docs" / "plex" / "openapi.json"
OUT = ROOT / "crates" / "api-plex" / "src" / "stubs.rs"

METHODS = ("get", "post", "put", "patch", "delete", "head")


def collect(spec: dict) -> tuple[dict[str, set[str]], list[tuple[str, str]]]:
    """Routable stub operations by path, plus the ones no router can express."""
    by_path: dict[str, set[str]] = defaultdict(set)
    skipped: list[tuple[str, str]] = []

    for path, item in sorted(spec["paths"].items()):
        if path in IMPLEMENTED:
            continue
        for method, operation in item.items():
            if method not in METHODS:
                continue
            tag = (operation.get("tags") or ["Untagged"])[0]
            if tag in DEFERRED_TAGS or any(p in path for p in DEFERRED_PATHS):
                continue
            if reason := unroutable(path):
                skipped.append((f"{method.upper()} {path}", reason))
                continue
            by_path[path].add(method)

    return by_path, skipped


def render(by_path: dict[str, set[str]], skipped: list[tuple[str, str]]) -> str:
    used = [method for method in METHODS if any(method in m for m in by_path.values())]
    total = sum(len(methods) for methods in by_path.values())

    lines = [
        "//! Routes declared by the spec but not implemented yet.",
        "//!",
        "//! **Generated** — regenerate with `python tools/gen_stub_routes.py`.",
        "//!",
        "//! Each answers `501 Not Implemented`. That is what gives the spec-coverage test",
        "//! something to assert: a route the spec declares and we never registered answers",
        "//! `404`, which is a bug, while one we have deliberately not written yet answers",
        "//! `501`, which is not. Without the distinction the test could only check that the",
        "//! router exists.",
        "//!",
        "//! Replacing a stub means deleting its row from `STUBS` and adding the real handler",
        "//! to the module that owns that domain.",
        "//!",
        "//! These are added onto the caller's router rather than merged in from a second one.",
        "//! `merge` re-validates the incoming paths with Axum's default rules, which reject",
        "//! the segments Plex writes as a literal colon — so the routes have to be declared on",
        "//! the one router that has those checks turned off.",
        "",
        "use axum::routing::MethodRouter;",
        "use axum::Router;",
        "",
        "use crate::error::ApiError;",
        "use crate::PlexState;",
        "",
        "/// Placeholder for routes that exist in the spec but have no handler yet.",
        "async fn not_implemented() -> ApiError {",
        "    ApiError::NotImplemented",
        "}",
        "",
        "/// Methods a stub answers, as a mask so each row of `STUBS` stays one line.",
        "mod verb {",
    ]
    lines += [
        f"    pub const {method.upper()}: u8 = 1 << {index};"
        for index, method in enumerate(used)
    ]
    lines += [
        "}",
        "",
        "/// Builds the method router for one mask.",
        "///",
        "/// A mask rather than a handler per method because every method answers the same way;",
        "/// what varies is only which ones are declared at all. A method the spec does not",
        "/// declare must stay undeclared, so it answers `405` instead of `501`.",
        "fn stub(methods: u8) -> MethodRouter<PlexState> {",
        "    let mut router = MethodRouter::new();",
    ]
    for method in used:
        lines += [
            f"    if methods & verb::{method.upper()} != 0 {{",
            f"        router = router.{method}(not_implemented);",
            "    }",
        ]
    lines += [
        "    router",
        "}",
        "",
        f"/// The {total} stub operations, across {len(by_path)} paths.",
        "const STUBS: &[(&str, u8)] = &[",
    ]
    for path in sorted(by_path):
        mask = " | ".join(
            f"verb::{method.upper()}" for method in used if method in by_path[path]
        )
        lines.append(f'    ("{path}", {mask}),')
    lines += [
        "];",
        "",
        f"/// Adds {total} stub operations across {len(by_path)} paths.",
        "pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {",
        "    STUBS",
        "        .iter()",
        "        .fold(router, |router, (path, methods)| {",
        "            router.route(path, stub(*methods))",
        "        })",
        "}",
        "",
    ]

    if skipped:
        lines += [
            "// Not routable as written, and needing real handlers regardless:",
            *[f"//   {operation} — {reason}" for operation, reason in skipped],
            "",
        ]

    return "\n".join(lines)


def main() -> None:
    spec = json.loads(SPEC.read_text(encoding="utf-8"))
    by_path, skipped = collect(spec)
    OUT.write_text(render(by_path, skipped), encoding="utf-8")
    print(
        f"{OUT.relative_to(ROOT)}: "
        f"{sum(len(v) for v in by_path.values())} operations, {len(by_path)} paths, "
        f"{len(skipped)} skipped"
    )


if __name__ == "__main__":
    main()
