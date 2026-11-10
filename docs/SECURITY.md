# Security model

This document covers the hardening added with #19 (rate limiting, JWT
revocation, sessions), #9 (share links) and #21 (resumable uploads), and the
threat model behind each control. It complements `docs/OIDC.md` and the
operator guide in `docs/DEPLOY.md`.

## Credentials and tokens

- Passwords are hashed with Argon2id (`auth::hash_password`), verified with the
  same routine. Unknown usernames are verified against a dummy hash so login
  timing does not reveal whether an account exists.
- Access tokens are HS256 JWTs signed with the server secret
  (`LUDEX_JWT_SECRET` or `{data}/jwt.secret`, mode 0600). They now carry a
  `jti` and are paired with an `auth_sessions` row.
- The default lifetime is 7 days (`LUDEX_JWT_TTL_MINUTES`), unchanged from the
  historical value. Sessions also have a server-side `expires_at`, and every
  authenticated request re-reads the account from SQLite (role and activation
  are authoritative, never the token claim).
- API keys (`ldx_...`) are unchanged: SHA-256 at rest, `prefix` lookup,
  constant-time comparison, scopes (`read`/`write`/`admin`). They are not
  session-bound and cannot be logged out of; revoke the key instead.

### Revocation

| Action | Effect |
| --- | --- |
| `POST /api/auth/logout` | Inserts the current `jti` into `token_revocations` and marks the session revoked |
| `DELETE /api/auth/sessions/{id}` | Same, for one session (own, or any as admin) |
| `POST /api/me/password` | Verifies the current password, then revokes every *other* session (keeps the caller) |
| `DELETE /api/users/{id}/sessions` | Admin: revokes all sessions and sets `users.tokens_valid_after`, which rejects every older JWT, including tokens issued before sessions existed |

`authenticate_token` checks, in one query, whether the `jti` is revoked, the
session is revoked, or the token was issued before `tokens_valid_after`.
Revoked credentials get 401 `session_revoked` and clients must sign in again.

## Brute-force protection

### Login throttle

Key: client IP + submitted username (case-insensitive), held in memory.

- The first two failures never delay the next attempt (a typo or a
  just-changed password must not feel broken).
- From the third consecutive failure the delay doubles: 500 ms, 1 s, 2 s, …
  capped at 60 s (`LUDEX_LOGIN_BACKOFF_MS` sets the base).
- At `LUDEX_LOGIN_MAX_ATTEMPTS` (default 5) the key is locked out for
  `LUDEX_LOGIN_LOCKOUT_SECONDS` (default 15 min).
- Every rejection is 429 `login_throttled` with `Retry-After`; the message
  includes the number of seconds. Successful login clears the counter.
- Failures are audited as `login.failed` with the reason (never the password).

Counters are process-local by design: the target deployment is one SQLite-backed
server. Behind a reverse proxy, the client IP is the proxy's address; apply a
per-real-IP limit at the proxy as well. Never trust a forwarded header for the
limit unless the proxy is configured to overwrite it (Ludex does not read
`X-Forwarded-For`).

### Global request limit

`security::rate_limit` is a per-IP token bucket applied to the whole `/api`
router (`LUDEX_RATE_LIMIT_RPS`, burst `LUDEX_RATE_LIMIT_BURST`). Health,
`/api/openapi.json`, `/api/docs` and CORS preflights are exempt so monitoring
and discovery never consume a budget. 429s carry `Retry-After`. Set
`LUDEX_RATE_LIMIT_RPS=0` to disable (the desktop local server does this).

## Share links (#9)

A share link is a capability URL: whoever holds the token (and the optional
password) can fetch one game archive without an account. The implementation is
`share.rs` + `routes/share.rs` with migration `0011_share_links.sql`.

- Tokens are 32 random bytes rendered as `lsx_<hex>`. Only the SHA-256 is stored
  (`token_hash`), so a database leak cannot be replayed; the plaintext token is
  returned exactly once in the creation response and there is no “show again”
  endpoint.
- Optional passwords are Argon2id hashes. The password travels as JSON in
  `POST /api/share/{token}/access` (`{ "password": "..." }`), never in a query
  string or log line.
- Public routes need no account:
  - `GET /api/share/{token}` returns metadata only (`ShareView`: public store
    fields, version label/size, expiry, password flag, remaining downloads).
    Unknown, revoked, expired and exhausted links all answer the same 404 so the
    route cannot probe the library.
  - `POST /api/share/{token}/access` verifies the password and exchanges the
    link for a 15-minute download ticket (`ShareAccessResponse { url,
    expiresAt }`). The ticket is a normal scoped download ticket for
    `/api/games/{id}/download`, so Range and object-storage behaviour are
    identical to authenticated downloads.
- Access attempts are limited to 30 per token per minute (`share::allow_attempt`),
  and exactly one Argon2 verification runs per attempt — including unknown
  tokens — so response time does not reveal whether a link exists.
- `max_downloads` is reserved atomically (`claim_access`) before the ticket is
  issued, so concurrent requests cannot exceed the cap. A client that abandons
  the download after receiving the ticket still consumes a slot; the ticket
  itself expires in 15 minutes.
- Expiry and revocation are checked before the claim and re-read if the atomic
  claim loses a race. Revocation is a soft delete (`revoked_at`) so the audit
  trail keeps the row.
- Management is editor-only: `POST /api/games/{id}/shares`, `GET /api/shares`
  and `DELETE /api/shares/{id}` all call `require_editor` (API keys need the
  `write` scope). Share labels are editor-facing and never shown publicly.
- Audit events: `share.created`, `share.revoked`, `share.access` and
  `share.access_denied` record the game/version id and denial reason, never the
  token or password.
- Public payloads deliberately exclude progress, bookmarks, playtime, file
  paths and other games.

Threat model: a leaked link grants read access to one archive until it expires,
is revoked or hits its cap. It does not grant library browsing, metadata edits,
progress changes or access to other games. A weak password is equivalent to no
password; prefer expiry/cap or a strong password.

## Resumable uploads (#21)

- Admin/editor only (`require_editor`, which also enforces the API-key `write`
  scope). A non-owner editor cannot inspect or complete another editor's
  upload; admins can.
- File names are sanitised to a single path component; only `.zip`, `.rar` and
  `.7z` are accepted. The destination is always inside the ingest library root,
  and an existing file is never overwritten (a numeric suffix is added).
- Declared size is bounded by `LUDEX_UPLOAD_MAX_BYTES`; each chunk body is
  limited to 65 MiB and must match the exact expected length. Parts are written
  to a temporary file and renamed, so a crash leaves either the old or the new
  chunk, never a torn file.
- Completion re-reads every chunk's length, assembles into a `.tmp` file in the
  library root, verifies total size and (when provided) SHA-256 before renaming
  and indexing. A mismatch returns 422 and leaves the upload open for retry.
- Chunks live under `{data}/uploads/{id}` and are removed on complete/cancel;
  stale uploads are removed after `LUDEX_UPLOAD_TTL_HOURS` by an hourly worker
  and lazily when a new upload starts. Orphan directories without a row are
  removed too.
- Uploading is an authenticated, audited operation (`upload.completed` with
  game/version/size/hash). It does not bypass the library scanner: the archive
  is indexed through the same `process_candidate` path as a normal scan.

## Secrets in logs

- The HTTP trace span records the method and path only, never the query string,
  so download tickets (`?token=...`) do not reach logs. Share passwords travel
  in a JSON body that is never logged.
- Audit metadata never includes passwords, tokens or webhook secrets. Share
  audits record the game/version id and denial reason, not the token.
- Error responses are generic for 500s; database and IO errors are logged
  server-side with `tracing::error!`.
- `jwt.secret` is written with mode 0600; the JWT/API-key material is never
  returned by management endpoints (share tokens excepted, once, at creation).

## What did not change

- Role checks (`require_admin`, `require_editor`, scopes) are unchanged and are
  still enforced on every route that had them. The new routes reuse them.
- Existing JWT TTL and the API-key format are unchanged; the `jti` claim is
  additive and old tokens without it remain valid until `tokens_valid_after`
  says otherwise.
- CORS origins and body limits are unchanged. The upload chunk route adds a
  per-route limit of 65 MiB on top of the global JSON limit.
