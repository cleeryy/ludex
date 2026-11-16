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

## Desktop SSO handoff (#157)

The Tauri app cannot receive an OIDC redirect, so the web page hands the session
back through a one-time code instead of a token:

- **No token in a URL.** The only response that ever contains a JWT is
  `POST /api/auth/oidc/exchange`; the handoff creation returns `{ expiresAt }`
  and the claim returns a single-use code.
- **State as a capability, never a credential.** `POST
  /api/auth/desktop-handoff` requires an authenticated web session with the
  `write` scope; the 256-bit state only binds that session to the desktop
  attempt. It is validated (43–128 chars, `A-Z a-z 0-9 - _`), compared in
  constant time and cannot be re-bound to another account (`409`).
- **One-time, short-lived.** The claim removes the entry, so a second claim is
  `410`. Both the handoff and its code expire after
  `LUDEX_DESKTOP_HANDOFF_TTL_SECONDS` (default 120 s, clamped 30–600).
- **No pending-signin oracle.** Unknown, expired and used states all answer
  `410 handoff_unavailable` with the same body; no user id or account detail is
  returned.
- **Bounded and in memory.** At most 1024 pending handoffs, lazily expired on
  every access. Nothing is persisted and a restart drops every pending attempt.
- **Rate limited.** 12 handoff creations per account per minute and 120 claims
  per address per minute (`429 handoff_throttled`, `Retry-After` set), on top of
  the global per-IP limiter.
- **Audited without secrets.** `oidc.handoff_created`, `oidc.handoff_claimed`
  and `oidc.handoff_failed` record the account (when known), the IP and a
  reason — never the state or the code.

The desktop poll runs for at most five minutes, logs only its terminal status,
and stops immediately on cancel. A browser that cannot be opened produces an
actionable error with the plain web login URL instead of a dead end.

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

## Collection share links (#173)

Collections are metadata shelves: a share link never transfers game files,
download tickets or archive paths. The public payload (`CollectionShareView`)
is built from a dedicated query and contains only game ids, titles, cover URLs,
release years and genres, plus the collection name/description and the owner's
display name when that profile is public.

- **Visibility.** `private` shelves cannot create a link (`409
  collection_private`) and are never reachable by token, listing, profile or
  feed. `unlisted` shelves are reachable only through a live token. `public`
  shelves are also listed in `/api/collections` and on the community profile.
  Switching a collection to `private` invalidates every existing token.
- **Lifecycle.** Optional expiry (`1..=8760` hours), optional Argon2id
  password, soft revocation (`revoked_at`), cascade on collection deletion.
  Expired/revoked/private links answer 404 on the public page and 410 on the
  access route where applicable.
- **Rate limiting.** Public page and access attempts share the game-share
  limiter (30 attempts per token per minute → 429 `rate_limited`).
- **Passwords.** The public page shows metadata but hides the game list until
  `POST /api/share/collection/{token}/access` verifies the password. A dummy
  Argon2 verification is spent for unprotected and unknown tokens, so response
  timing does not reveal whether a token exists.
- **Copy/import.** Only public shelves (by id) or live tokens (unlisted/public)
  can be imported. The copy is a new private collection owned by the caller;
  game files are never touched. Importing the same share twice merges missing
  games into the existing copy (`created: false`) instead of duplicating it.
- **Audit events.** `collection.shared`, `collection.share_revoked`,
  `collection.share_accessed`, `collection.share_denied` and
  `collection.imported` record ids and denial reasons; tokens and passwords
  never appear in audit metadata or logs.

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
- Every storage path built from a request goes through `paths::component` /
  `paths::child`: the upload id, the archive name and the save file name must
  be a single directory entry (no separator, no `.`/`..`, no control character,
  at most 255 characters). `paths::canonical_within` repeats the check after
  resolving symlinks before the assembly write, so a link planted inside the
  library root cannot redirect it.
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
- The desktop SSO handoff (#157) never logs its state or its one-time code; the
  poll loop records the terminal status only, and audit events carry the reason
  (`not_found`, `invalid_state`) without either value.
- Audit metadata never includes passwords, tokens or webhook secrets. Share
  audits record the game/version id and denial reason, not the token.
- Error responses are generic for 500s; database and IO errors are logged
  server-side with `tracing::error!`.
- `jwt.secret` is written with mode 0600; the JWT/API-key material is never
  returned by management endpoints (share tokens excepted, once, at creation).
- Archive passwords are never part of an error or a log line:
  `ArchiveError::{PasswordRequired, WrongPassword}` carry no data, and
  `ExtractOptions`' `Debug` impl prints `"N redacted"` instead of the password
  list (see `archive::tests::password_errors_and_options_never_echo_the_password`).
- The one deliberate exception is `ludex-server import` (GameVault): it prints
  the generated temporary passwords to the terminal exactly once, because the
  command has no other channel to reach the importing administrator. Nothing is
  logged, stored or audited; the print carries a `codeql[rust/cleartext-logging]`
  marker and is covered by `gamevault::tests::credentials_never_debug_their_password`.

## What did not change

- Role checks (`require_admin`, `require_editor`, scopes) are unchanged and are
  still enforced on every route that had them. The new routes reuse them.
- Existing JWT TTL and the API-key format are unchanged; the `jti` claim is
  additive and old tokens without it remain valid until `tokens_valid_after`
  says otherwise.
- CORS origins and body limits are unchanged. The upload chunk route adds a
  per-route limit of 65 MiB on top of the global JSON limit.

## Code scanning (#263, #266)

CodeQL runs on push, pull request and weekly (`security-and-quality`, Rust and
JavaScript/TypeScript, `build-mode: none`; `.github/workflows/codeql.yml`).
`.github/codeql/codeql-config.yml` only excludes generated output — test files
are scanned like production code, because a naive regexp in a test is a real
regression there too.

### Alert triage at v0.4.17

The 82 `rust/hard-coded-cryptographic-value` alerts were dismissed during the
initial triage (test fixtures, HKDF salts, AAD and dummy timing values). Of the
53 remaining alerts:

| Rule | Alerts | Disposition |
| --- | --- | --- |
| `rust/path-injection` | 35–50 (16) | **Fixed.** Upload ids, archive names and save file names are confined to one component by `paths::{component,child}`, and the assembly/publish paths are re-checked with `paths::canonical_within`. The remaining sinks carry a narrow `// codeql[rust/path-injection]` marker naming the test that proves the confinement (`paths::tests::*`, `uploads::tests::destinations_stay_inside_the_library`, `saves::tests::snapshot_names_cannot_escape_the_game_directory`). |
| `rust/request-forgery` | 51, 52 | Fixed in the parallel `media.rs`/`providers.rs` lane (provider URL allowlist + private-address guard); the two IGDB `metadata.rs` sinks are re-triaged in the final pass below. |
| `rust/cleartext-logging` | 29, 33, 34 | **Fixed.** 29: the debug print of a password error was removed and `ExtractOptions`' `Debug` redacts the password list. 33: the integration test no longer echoes the minted API key into an assertion message. 34: the CLI one-time password print is intentional and annotated. |
| `rust/cleartext-transmission` | 135 | **Fixed.** `HttpTransport::new` refuses a non-HTTPS base and falls back to the public endpoint; `validate_key` re-checks the scheme before sending the key in `/users/{key}` (`virustotal::tests::plaintext_api_base_is_refused`). |
| `js/xss-through-dom` | 1–16 | **Fixed at the boundary.** `lib/safe-url.ts` accepts only `https:`, `http:`, `data:`, `blob:` and `demo:` schemes plus single-slash relative paths, and `assetUrl` now returns a branded `SafeUrl`. Every `src`/`href`/`assign` in the reports goes through it; the SSO `desktopAddress` redirect additionally requires the configured server origin (`safeRedirectUrl`). False positives are dismissed with `safe-url.test.ts` as evidence. |
| `js/insecure-temporary-file` | 23, 138 | **Fixed.** `fs.mkdtemp` in `e2e/screenshots.mjs` (cleaned up in `finally`) and in `apps/web/scripts/i18n.mjs --pseudo`. |
| `js/file-system-race` | 21, 136 | **Fixed.** `vite.config.ts` writes `dist/sw.js` through a temp file + rename and no longer probes with `existsSync`; the screenshot manifest is written the same way. |
| `js/regex/missing-regexp-anchor` | 137 | **Fixed.** The YouTube route filter is host-anchored (`evilyoutube.com` no longer matches). |
| `js/missing-origin-check` | 22 | **Fixed.** `public/sw.js` ignores `postMessage` from a foreign origin. |
| `js/comparison-between-incompatible-types` | 26 | **Fixed.** The delete dialog is mounted only when a backup is selected, so it is unconditionally open. |
| `js/unused-local-variable` | 24, 25 | **Fixed.** Unused `pseudoText` import and `pluralIds` binding removed. |
| `rust/unused-variable` | 27, 28 | **Fixed.** Non-Windows `platform` parameter and the implicit `format!` capture replaced by explicit code. |
| `js/incomplete-multi-character-sanitization` | 20 | **Fixed.** `stripInlineMarkdown` decodes entities and strips tags in a fixpoint loop; the output is a React text node, never HTML. |
| `js/double-escaping` | 17 | **Fixed.** Entities are decoded before tag stripping, and no allowed entity can produce `<` or `>`. |
| `js/bad-tag-filter` | 19 | **Fixed.** The boot-script extractor is attribute-aware and case-insensitive. |
| `js/incomplete-sanitization` | 18 | **Fixed.** The navigation test escapes every regexp metacharacter. |

### Final triage (#266, 2026-11-16)

The first pass left 36 alerts open on `main` at `0bcea87`. Four were addressed
in code on the #266 follow-up lane (three real findings and one ambiguous
format capture):

| Alert | Fix |
| --- | --- |
| `js/unused-local-variable` 150 | unused `readFileSync` import removed from `e2e/console-screenshots.mjs` |
| `js/regex/missing-regexp-anchor` 137 | the thumbnail filter in `e2e/screenshots.mjs` is host- and `/vi/`-anchored, so `youtube.com` inside a foreign URL no longer matches |
| `js/bad-tag-filter` 142 | the boot test reads `<script>` bodies with a quote-aware scanner instead of a tag regexp; the new “inline script extraction” test covers case, attributes, `<scripted>` and end-tag noise |
| `rust/unused-variable` 139 | `downgrade_error` passes `version` as an explicit named format argument; `opening_a_newer_database_is_refused_with_a_recovery_path` asserts the rendered version |

The other 32 sit on a sink that is already confined, or on a deliberate
behaviour. Each was dismissed individually, with the sanitizer/fixture and the
test named in the dismissal comment:

| Rule | Alerts | Dismissed as | Evidence |
| --- | --- | --- | --- |
| `js/xss-through-dom` | 16 | false positive | every `src`/`href`/`assign` target is a branded `SafeUrl` from `assetUrl`/`toSafeUrl`/`safeRedirectUrl`; `apps/web/src/lib/safe-url.test.ts` |
| `rust/path-injection` | 9 | false positive | `paths::{component,child}` + canonical containment; `saves::tests::paths_stay_under_the_user_game_directory`, `saves::tests::snapshot_names_cannot_escape_the_game_directory`, `uploads::tests::destinations_stay_inside_the_library` |
| `rust/cleartext-logging` | 3 | false positive | `ExtractOptions`' `Debug` prints `"N redacted"` (`archive::tests::password_errors_and_options_never_echo_the_password`); the `saves.rs` sink is a test assertion message on a sanitised numeric-id path (`saves::tests::snapshot_names_cannot_escape_the_game_directory`) |
| `rust/cleartext-logging` | 1 | won't fix | the deliberate one-time `ludex-server import` password print, documented in `--help` and covered by `gamevault::tests::credentials_never_debug_their_password` |
| `rust/cleartext-transmission` | 1 | false positive | `is_secure_base` refuses a non-HTTPS base before `GET /users/{key}`; only loopback `http` is accepted, for the mock (`virustotal::tests::plaintext_api_base_is_refused`) |
| `rust/request-forgery` | 2 | false positive | `IgdbClient::new` uses the compile-time IGDB endpoints; `with_endpoints` is `#[cfg(test)]` and only the loopback fixture uses it (`metadata::tests::igdb_fetches_a_record_by_id`) |

Open alerts after the sweep: **0** (queried 2026-11-16). The four fixed alerts
were dismissed with the fix recorded in the comment; the next scan on `main`
would have closed them anyway. Any new alert is triaged in the PR that
introduces it: fix it, or dismiss it with a named test and a written reason.

### Dismissing an alert

Dismiss only when the fixing commit is on `main` — or, while a scan is still
pending, with the fix recorded in the comment — with the reason and the test
named in the comment:

```bash
gh api -X PATCH /repos/cleeryy/ludex/code-scanning/alerts/<number> \
  -f state=dismissed \
  -f dismissed_reason=false_positive \
  -f dismissed_comment="Out of scope at this sink: lib/safe-url.ts validates every \
  src/href/assign target (scheme allowlist + relative paths); evidence: \
  apps/web/src/lib/safe-url.test.ts. Fixed in #266."
```

Use `dismissed_reason=used_in_tests` for test-only fixtures and
`won't_fix` only with a written justification. There is no blanket dismissal:
every alert gets a per-alert reason naming a test, an annotation or a design
decision.

### Required checks

The open alert count is zero as of 2026-11-16, so the CodeQL jobs can be
required on `main`:

1. Settings → Branches → `main` → Require status checks to pass.
2. Add `CodeQL (rust)` and `CodeQL (javascript-typescript)` (the matrix job
   names) alongside the existing CI jobs.
3. Keep the weekly schedule (`41 5 * * 1`) so a new query release cannot
   introduce an alert silently between pulls.
