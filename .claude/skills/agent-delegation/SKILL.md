---
name: agent-delegation
description: How to hand open-plex work to parallel agents in their own worktrees and bring it back — the brief that works, what agents reliably get wrong, and the merge checklist. Load before spawning or briefing any agent (delegate_task, Agent, a T3 thread) on this repo, before merging an agent's branch into master, and when an agent's report comes back.
---

# Delegating open-plex work

The owner likes parallel workers, one per feature, merged into master as each finishes, with a
French report after each merge. What follows is what made those runs succeed, and what had to be
redone when it was missing.

## The brief

Every brief carries the same **context and rules** block. Briefs built from it got the same safe
behavior from every agent; free-form briefs made each one rediscover the rules, differently.

1. **Goal and scope**: the feature, the files it may touch, the public signatures it must keep.
   Narrow tasks — pure functions behind frozen signatures, an allowed list of files, "do not
   contact the reference, do not open `db/`" — came back needing no follow-up.
2. **The project's rules**: load the `rust` skill; GraphQL first, the adapter translates and
   holds no domain logic ("ne fait que un binding graphql"); unit tests in each file plus route
   tests against captured fixtures, refusal cases included, in the same commit as the behavior.
   Route tests go in one test file per domain (`tests/play_queues.rs`, `tests/markers.rs`…),
   which is the real convention, not `conformance.rs`.
3. **Isolation**: its own worktree; a **migration number reserved** for it; a **test port** of
   its own (32511 and up); a copy of the database, never the live `open-plex.db`, never the
   server on 32500 or the tunnel.
4. **The reference**, when it is involved: the rules of the `reference-servers` skill, verbatim —
   test libraries only, the forbidden calls by name, token in the header, ids checked before each
   write, every change left behind reported. A research agent damaged real watch state because
   its brief did not name `match`/`unmatch`. Two agents on the same test libraries at once also
   collided: give each its own items, or serialize them.
5. **Environment**: a test server needs `OPENPLEX_ADMIN_PASSWORD` and a sign-in.
   `OPENPLEX_TOKEN` and `OPENPLEX_ALLOWED_NETWORKS` no longer exist; old briefs still set them.
   Low-disk builds in every cargo command (`windows-shell` skill), never `--release`,
   `cargo clean` of its own worktree after committing. Say whether it may touch other worktrees'
   `target/` (one did, unasked).
6. **Generated files** it must regenerate (list below), and the closing procedure.
7. **Finishing**: "Do not end your turn while a background job runs: wait for it in the same
   turn." Agents that launched an import or the test suite in the background and ended their turn
   left thousands of lines uncommitted, twice. Then commit on its branch, and report.
8. **The report**: branch name read from `git branch --show-current` at the end (T3 can rename a
   branch mid-run, and a brief can name one that already exists), commit hashes, the closing
   procedure's real results, what it changed on the reference, and a short "decisions for the
   owner" section.

For listing or search work, add a timing pass on real data over the database copy: tests never
showed the 4-second tag page or the 2.7-second `/hubs/search` that the real library did.

## When a report comes back

- A "completed" notification can be intermediate. If the report reads like a progress note ("I'm
  waiting for the test run"), inspect the worktree: `git -C <worktree> status`, `git log`. Resume
  the agent in its own thread rather than redoing its work.
- Verify before relaying: the claims, the test counts, the branch. Say what you checked.
- Agents leave decisions for the owner. Do not treat their defaults as settled, and do not pass on
  a list of thirty: the owner rarely answers long lists. Keep the few that change shipped
  behavior.

## Merging

- `git merge --ff-only` when the branch starts from master's head: nothing to re-test.
- Otherwise merge, then run `cargo test --all-features --no-fail-fast` on the merged tree. The
  conflicts are predictable:
  - **migration numbers**: renumber so they follow on without a gap;
  - **fields one branch adds** to `ApiContext` or to a struct, missing from the other branch's
    test initializers (`E0063`);
  - **two branches defining the same GraphQL type name**: the `sdl` example panics;
  - **`clippy::too_many_lines`** once both sides add to one function;
  - resolving by **concatenating both sides** breaks braces; taking one side wholesale for a test
    file drops the other's fixture changes.
- **Generated files**: keep master's, then regenerate on the merged tree:
  - `python tools/gen_endpoints.py`: it carries over only `partial` and `done` with their notes,
    from the existing `docs/ENDPOINTS.md`. Copy the branch's changed rows across first, by
    (method, path), or they fall back to `todo`. Regenerate twice and diff: a malformed row (a
    doubled `|`) is silently emptied.
  - `python tools/gen_stub_routes.py` whenever a stub became real, then `cargo fmt` (its output
    fails `fmt --check`). Skipping it crashes the server at startup on the duplicate route.
  - `cargo run -p open-plex-api --example sdl > docs/schema.graphql` and
    `--example introspect > docs/introspection.json`: agents forget the second.
  - `python tools/gen_jellyfin_endpoints.py` for the Jellyfin tracker.
  - Strip CRLF from whatever a Python script wrote.

## After

Delete merged worktrees and their branches: each weighs about 4 GB with its `target/`, and 24
piled up once. Worktrees live under `%USERPROFILE%\.t3\worktrees\open-plex\` and lack the
untracked folders (`files/`, `db/`, `web/`, `.work/`): read those from the main checkout.

T3 Code's checkpoints (`refs/t3/…`) snapshot every file of the folder, gitignored ones included:
they once held the real Plex database, backups of ours and `files/`, 5 GB in `.git`. After heavy
agent use, check `git count-objects -vH` and `git for-each-ref refs/t3 | wc -l`; deleting them
costs T3 its "revert to this turn", so ask first.
