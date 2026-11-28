---
paths:
  - "crates/core/src/access.rs"
  - "crates/core/src/account.rs"
  - "crates/core/src/auth.rs"
  - "crates/core/src/identity.rs"
  - "crates/api/src/access.rs"
  - "crates/api/src/account.rs"
  - "crates/server/src/auth.rs"
  - "crates/server/src/admin.rs"
  - "crates/server/src/cors.rs"
  - "crates/server/src/tray.rs"
  - "crates/server/src/remote_tokens.rs"
  - "crates/server/src/web.rs"
  - "crates/api-plex/src/auth.rs"
  - "crates/api-plex/src/plex_tv.rs"
  - "crates/api-jellyfin/src/auth.rs"
  - "crates/api-jellyfin/src/users.rs"
  - "crates/db/src/access_repository.rs"
  - "crates/api/tests/extensions.rs"
  - "crates/db/tests/access.rs"
  - "tools/klapz_session.py"
---

# Accounts, authentication and rights

Our own model; Plex's sharing model (accounts table, home/restricted flags,
`library_section_permissions`) is not reproduced.

## Model

- `users` (unique lower-case `username`: 3–32 of `a-z0-9._-`, starting alphanumeric, no `@`;
  optional unique `email`; role `owner` or `user`), `library_permission` (`viewer` or `admin` on
  one library), `user_extension` (one protocol, `plex` or `jellyfin`, with its external id and
  on/off). **A connection has no token of its own.**
- An owner reaches every library as admin (`LibraryAccess::All`) and manages users, roles,
  permissions, activation, extensions, sessions. `setUserRole` is owners-only.
- Built-in `admin` (`account::ensure_admin`, at startup): always exists, active, owner, no
  address; cannot be demoted, deactivated or deleted (`BUILT_IN_ACCOUNT`). Its password is
  `KLAPZ_ADMIN_PASSWORD`, else drawn once and printed on stdout only, never logged.
- `signUp` creates an inactive user (Argon2id, `user_password`); an inactive user opens nothing,
  its tokens and sessions included. `signIn`: `INVALID_CREDENTIALS` alike for unknown user and
  wrong password (decoy hash for timing), `INACTIVE` only with the right password, five failures
  per identifier per fifteen minutes.

## Transport (`server::auth`)

- Every request is a user's: a plex.tv token resolved through `extension_account`, a Jellyfin
  device token (below `/jellyfin` only), or a session. No operator, no admin token, no trusted
  network. Precedence: Plex token, bearer, cookie.
- Sessions: HS256 JWT (`KLAPZ_SESSION_SECRET`, ≥ 32 bytes) in `klapz_session`, `HttpOnly;
  SameSite=Lax`, `Secure` unless `KLAPZ_INSECURE_COOKIES=1`, 30 days, nothing stored; the user's
  current state is checked on every request. API clients send it as `Authorization: Bearer`.
- `/graphql` accepts JSON only (a same-site form could post as the signed-in user) and admits
  no principal; every other route answers `401` without one.
- Unknown plex.tv tokens: the answer (refusal included) is cached five minutes, at most 60
  lookups a minute, so the server cannot be used to flood plex.tv.
- CORS: Plex's domains on the Plex surface without credentials; `https://app.klapz.io`,
  `KLAPZ_CORS_ORIGINS` and (debug builds) localhost with credentials; nothing else.
- `/admin` is the owners' page: served without a token, holds no data, driven by `/graphql`
  with the session cookie.
- `/identity` without a token shows the server published at the address the request reached,
  when only one is (`PublishedServer`, which opens nothing).
- Passwords: `setPassword(userId:)` lets an owner set one without a current password; a user
  changing its own gives its current one, failures counted with `signIn`'s.
- Tray "Open web app": one-time ticket in `/web/?ticket=…` (`Sessions::issue_sign_in_ticket`,
  stored as a digest, 60 s, `signInWithTicket`), never a session in a URL.

## Pairing and extensions

`pairExtension`, `moveExtension`, `unpairExtension` are open to owners and the user named;
switching Plex on/off stays the owners'; Jellyfin's the user switches itself
(`Extension::connected_by_its_user`). plex.tv is reached only through the `AccountDirectory`
port (`api-plex::plex_tv`). The account token is stored in clear (the server must present it);
addresses are republished at startup and hourly.

Tokens travel in Plex query strings: never log a URL, header or body that may carry one.
