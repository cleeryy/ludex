# Ludex — server & core quality audit (v0.4.14)

Read-only audit of `/root/ludex` at tag `v0.4.14` (commit `7199a2c`).
Scope: `crates/ludex-server/src/**`, `crates/ludex-core/src/**`, `crates/ludex-server/migrations*`,
`crates/ludex-server/tests/**`, `crates/ludex-core` tests (inline only — the crate has no `tests/` dir).

Method: full read of the bootstrap/layering files (`lib.rs`, `state.rs`, `error.rs`, `db.rs`, `auth.rs`,
`routes/mod.rs`, `extract.rs`, `sql.rs`, `secrets.rs`, `webhooks.rs`, `security.rs`, `api_keys.rs`,
`media.rs`), structural grep across the rest, heuristic function-size and query-in-loop analysis.
`ripgrep` is not installed on this box, so counts below come from `grep -rF/-rcE` (equivalent results).

---

## 0. Headline numbers

| Metric | Value |
|---|---|
| `ludex-server/src/**` | 105 `.rs` files, 66 046 lines (≈57 200 production, ≈8 900 inline `#[cfg(test)]`) |
| `ludex-core/src/**` | 10 `.rs` files, 8 633 lines |
| `ludex-server/tests/**` | 40 files (39 suites + `common/mod.rs`), 19 172 lines |
| Total test functions | 607 (`#[test]` + `#[tokio::test]`): 294 inline server, 116 inline core, 197 integration |
| HTTP route registrations | 174 `.route(...)` calls → 175 method/path pairs, 181 unique paths, 231 documented operations |
| Async fns in server src | 820 (302 in `routes/`) |
| Published `pub fn` | 657 (server) + 64 (core) |
| SQL call sites | 526 `db::query*` sites, 567 SQL literals |
| Migrations | 32 SQLite + 32 PostgreSQL, 1 301 + 1 012 lines |
| `unwrap()` | 36 — **zero in production code** (all in `#[cfg(test)]` blocks or doctests) |
| `.expect(...)` | 735 (≈600 in test modules; the rest are startup-time invariants) |
| `todo!` / `unimplemented!` | 0 / 0 |
| `panic!` | 11 — 1 in production (`db.rs:811`), 10 in tests |
| `unsafe` | 0 real blocks (`core` has `#![forbid(unsafe_code)]`) |
| `TODO`/`FIXME`/`HACK` | 0 |
| Explicit DB transactions | 12 |
| `ON CONFLICT` upserts | 31 |
| Distinct issue references in comments | 53 (`#NNN`) |

Overall: this is an unusually disciplined Rust codebase — typed errors everywhere, consistent authz,
no `unwrap`/`todo` in production, heavy doc comments with issue traceability, and a real OpenAPI
contract test. The problems are **shape** problems (god modules, duplicated plumbing, one behavioural
SSRF gap), not discipline problems.

---

## 1. Layer map

```
main.rs / bin/ludex-backup.rs / examples/*
        │
        ▼
lib.rs  ── prepare() ──► config.rs (env) ──► db.rs (pool + migrations) ──► roots.rs
        │                                   └─► secrets.rs (AES-GCM at rest)
        │
        ├─► build_router() ──► middleware: security::rate_limit, auth::editor_only (ingest),
        │                      metrics::track, TraceLayer, CORS, DefaultBodyLimit
        │
        ├─► routes/*.rs (174 registrations)
        │       extract.rs  (ApiQuery/ApiPath/JsonBody/MultipartBody → ApiError)
        │       error.rs    (ApiError envelope)
        │       models.rs   (DTOs + SQL row shapes, utoipa schemas)
        │       auth.rs     (AuthUser extractor, JWT, tickets, sessions)
        │       api_keys.rs (ldx_ keys, scopes)
        │
        ├─► domain modules (no HTTP knowledge): scanner, metadata, providers, achievements,
        │   library_health, integrity, social, collections, share, saves, uploads, backup,
        │   extracted_cache, content_hash, virustotal, oidc, mfa/totp, desktop_handoff,
        │   emulators, gamevault, watcher, media_gc
        │
        ├─► infrastructure: storage.rs (local+S3), stream.rs (sync→async), events.rs (bus),
        │   webhooks.rs (delivery worker), metrics.rs, observability.rs, download_limits.rs,
        │   security.rs (limiters), state.rs (AppState)
        │
        └─► tokio background workers spawned in prepare(): scan, events, webhooks, integrity,
            media_gc, library_health, achievements, upload cleanup, extracted cache, ingest resume
        │
        ▼
ludex-core (db/HTTP/async-free): archive.rs, exe.rs, rom.rs, naming.rs, fingerprint.rs,
        slug.rs, redact.rs, gamepad.rs, models.rs
```

**Dependency direction is clean**: `ludex-core` has zero dependency on sqlx/axum/tokio
(`core/Cargo.toml` — serde, thiserror, chrono, uuid, zip, sevenz-rust2, unrar, tar, walkdir, sha2,
crc32fast, hex, utoipa). The server is the only place with DB and HTTP. Modules below `routes/`
take `&AppState` or `&Database`, never `Request`. Two exceptions worth noting:

1. `webhooks.rs` and `providers.rs` are infrastructure modules that are *also* domain logic; they
   hold their own SSRF policy. There is no shared `outbound.rs` — see §3.9.
2. `gamevault.rs` (1 479 lines) is a one-shot CLI importer that lives inside the server library
   crate and is exposed in `main.rs`. It is dead weight in the served binary.

### Responsibility per area

| Area | Files | Responsibility |
|---|---|---|
| Config | `config.rs` | 60-field `Config`, `from_env`, `for_local` (desktop) |
| DB | `db.rs`, `sql.rs` | `Database` enum (SQLite/Postgres), `DbQuery*` wrappers, `?`→`$n` rewrite, migrations, time helpers |
| Migrations | `migrations/`, `migrations_postgres/` | 32 numbered SQL pairs, applied by `sqlx::migrate!` at connect |
| Routes | `routes/*.rs` (44 files) | HTTP surface only; each exports `router()` |
| Auth | `auth.rs`, `api_keys.rs`, `security.rs`, `mfa.rs`, `totp.rs`, `oidc.rs` | JWT, API keys+scopes, throttles, 2FA, SSO |
| Models | `models.rs` | Response DTOs + `sqlx::FromRow` row shapes + utoipa `ToSchema` |
| Scanner | `scanner.rs`, `watcher.rs`, `roots.rs` | Walk roots, group into games, upsert versions, watch for changes |
| Archive | `core/archive.rs`, `storage.rs`, `stream.rs`, `extracted_cache.rs`, `uploads.rs`, `versions` route | ZIP/RAR/7z extraction, tar streaming, S3/local storage, extracted cache |
| Exe | `core/exe.rs` | Post-extraction executable discovery |
| Providers/metadata | `metadata.rs`, `providers.rs`, `game_metadata.rs`, `artwork.rs`, `media.rs` | IGDB/Steam/community sidecars, matching, artwork cache |
| Achievements | `achievements.rs` | Providers (Steam files/Galaxy API/Galaxy SQLite), definitions, unlocks |
| Saves | `saves.rs`, `routes/saves.rs` | Multipart cloud saves, snapshots, quota |
| Social | `social.rs`, `community.rs`, `collection_share.rs`, `share.rs` | Friends/presence/notifications, activity feed, public share links |
| Webhooks | `webhooks.rs`, `events.rs`, `audit.rs` | Event bus, HMAC-signed delivery with retries |
| Remote | `remote_installs.rs`, `desktop_download.rs`, `desktop_handoff.rs`, `devices.rs` | Mobile companion, desktop releases, SSO handoff, device registry |
| Uploads | `uploads.rs`, `routes/uploads.rs` | Resumable chunked archive uploads |
| VirusTotal | `virustotal.rs` | Hash lookups, quota, cache, verdicts |
| Ops | `metrics.rs`, `observability.rs`, `diagnostics.rs`, `backup.rs`, `integrity.rs`, `library_health.rs`, `media_gc.rs`, `content_hash.rs` | Ops surface |
| Tests | `tests/*.rs`, inline `#[cfg(test)]` | 607 tests |

---

## 2. Per-file inventory

Scoring: 5 = exemplary, 4 = good with nits, 3 = works but needs restructuring, 2 = significant
problems, 1 = rewrite. Priority: **P1** = correctness/security, **P2** = maintainability, **P3** = polish.

### 2.1 Bootstrap & infrastructure (`crates/ludex-server/src/`)

| File | LOC | Role | Public surface | Coupling | Score | Issues | Prio |
|---|---|---|---|---|---|---|---|
| `lib.rs` | 674 | Entry (`run`/`start`), wiring, router assembly | `run`, `start`, `ServerHandle`, `Config` re-export | ⬆ everything | 3 | `prepare()` is 239 lines building a 42-field `AppState` inline (`lib.rs:167-406`); the same construction is copy-pasted in `state.rs:217-278` (`for_tests`) — two places to update per new field. `build_router` is in `lib.rs` (431) rather than `routes/mod.rs`. No `#![warn(missing_docs)]` unlike core. | P2 |
| `state.rs` | 278 | `AppState` god-struct + `ActiveTransfers` | `AppState`, `ActiveTransfers`, `TransferLease` | ⬆ all modules | 3 | 42 public fields; every feature adds a limiter/flag here (16× `Arc<AttemptCounter>`/locks). No sub-structs (`Security`, `Background`). `for_tests` duplicates `prepare` construction. | P2 |
| `error.rs` | 154 | `ApiError` envelope | `ApiError` + constructors + `From<sqlx/io/anyhow/serde_json>` | ⬇ none | 5 | `From<sqlx::Error>` always maps to 500 `internal_error` — unique/FK violations must be hand-mapped by callers (see §3.2). `From<serde_json::Error>` also 500 even for request bodies. | P3 |
| `db.rs` | 1 607 | Backend enum, pool, migrations, query wrappers | `Database`, `Backend`, `query*`, `DbRow`, `db_time`, `pg_placeholders` | ⬆ sqlx only | 3 | 4-arm match (`Pool×{Sqlite,Postgres}`, `Tx×{Sqlite,Postgres}`) repeated in **every** method — `execute`/`fetch_all`/`fetch_one`/`fetch_optional` × 4 query types ≈ 750 lines of boilerplate (`db.rs:609-1148`). `DbRow::get` **panics** in production on a decode error (`db.rs:805-813`), used at ~100 call sites. `pg_placeholders` (`db.rs:480`) ignores dollar-quoted strings and `E'...'`. | P2 |
| `sql.rs` | 35 | Bind helpers | `Bind`, `bind_query*` | ⬇ `db` | 4 | Trivial; `Bind = DbValue` alias adds no safety. | P3 |
| `config.rs` | 1 186 | Env config + desktop defaults | `Config`, `from_env`, `for_local` | ⬇ none | 3 | 60 flat fields; `from_env` is 400-820 (420 lines of `env_opt`/`env_dur` parsing); `for_local` re-states every default. No config validation step, no `Config::validate()`; invalid combos surface at first use. | P2 |
| `auth.rs` | 695 | Password hash, JWT, tickets, `AuthUser` | `AuthUser`, `issue_*`, `authenticate_token`, `start_session` | ⬇ db, api_keys, events | 4 | Solid. `authenticate_token` runs 1 query + 1 revocations sub-select per request — fine, indexed. `session_touch` map is bounded (10k). Legacy-token compatibility code (`jti: Option`) is permanent debt but documented. | P3 |
| `api_keys.rs` | 231 | `ldx_` keys, scopes | `Scope`, `generate`, `authenticate` | ⬇ db | 5 | SHA-256 + constant-time compare + 16-char lookup prefix: correct design for a high-entropy token. | — |
| `security.rs` | 546 | Rate limiter, login guard, attempt counter, UA parser | `RateLimiter`, `LoginGuard`, `AttemptCounter`, `describe_user_agent` | ⬇ state | 4 | Three overlapping limiters in one file, plus a fourth fork in `share.rs:629`. `describe_user_agent` (session labels) is unrelated and belongs in `models.rs`/`devices.rs`. | P2 |
| `secrets.rs` | 116 | AES-256-GCM + HKDF at rest | `encrypt`, `decrypt` | ⬇ none | 5 | Purpose-bound salt/info/AAD, random nonce per write, tested round-trip + tamper. Exemplary. | — |
| `extract.rs` | 93 | Contract-shaped extractors | `ApiQuery`, `ApiPath`, `JsonBody`, `MultipartBody` | ⬇ error | 5 | Small, single-purpose, kills axum's plain-text rejections. | — |
| `events.rs` | 144 | In-process bus | `channel`, `EventSink`, `Event`, `Actor`, `worker` | ⬇ db, webhooks | 5 | Clean bounded channel + worker. | — |
| `media.rs` | 198 | Artwork download + URL/name helpers | `download_media`, `media_url`, `safe_media_name`, `content_disposition` | ⬇ nothing (!) | 3 | **`download_media` (media.rs:41) fetches any provider-supplied URL with the shared `state.http` client — no scheme/private-IP check, redirects follow by default** → blind SSRF from a community provider's `coverUrl`. Size cap is enforced after `content_length` (good). | **P1** |
| `stream.rs` | 218 | Sync tar writer → async body | `tar_stream` | ⬇ core archive | 4 | Correct `spawn_blocking` + bounded channel back-pressure. | — |
| `web.rs` | 103 | Optional SPA serving | `attach` | ⬇ config | 3 | No security headers (no CSP/`X-Content-Type-Options`/`Referrer-Policy`) on the HTML shell; the `/api/docs` page is also unstyled-unsafe but static. | P2 |
| `metrics.rs` | 951 | Prometheus registry + health snapshot | `Metrics`, `snapshot`, `render_snapshot`, `track` | ⬇ db, state | 3 | Hand-rolled registry (951 lines) instead of `metrics` crate; `render_snapshot` is 162 lines (`metrics.rs:688`). `/metrics` handler has its own bearer/limit logic (`routes/metrics.rs:36`). | P3 |
| `observability.rs` | 671 | Admin dashboard aggregation | `snapshot`, `ObservabilityWindow` | ⬇ db | 4 | `snapshot` 145 lines, mostly query-and-map. | P3 |
| `locale.rs` | 184 | Locale registry | `SUPPORTED`, `normalize` | ⬇ none | 4 | — | — |
| `storage.rs` | 659 | Local + S3 archive storage | `ArchiveStorage`, `local_path`, `put/get/head` | ⬇ config | 4 | Two backends behind one enum, consistent errors, 23 `.expect` are all tests. S3 fallback policy in `routes/games.rs:1637`. | — |
| `content_hash.rs` | 502 | Background SHA-256 backfill | `ContentHasher`, `status`, `scheduled` | ⬇ db, storage | 4 | Buffered hashing on `spawn_blocking`. | — |
| `download_limits.rs` | 485 | Per-address/job/part admission | `DownloadLimiter`, `LimitRejection` | ⬇ none | 4 | Clear semantics, tested. | — |
| `roots.rs` | 424 | Multi-root registry | `bootstrap`, `list`, `LibraryRoot` | ⬇ db | 4 | — | — |
| `watcher.rs` | 414 | Debounced filesystem watcher | `start`, `WatcherHandle`, `WatcherStatus` | ⬇ notify | 4 | Reload-on-root-change handled via `AppState.watcher_handle`. | — |
| `media_gc.rs` | 365 | Orphan artwork GC | `run`, `scheduled_worker` | ⬇ db, storage | 4 | — | — |
| `devices.rs` | 454 | Device registry + heartbeat | `HEARTBEAT_LIMIT_PER_MINUTE`, routes helpers | ⬇ db | 5 | Clean. | — |
| `controller_profiles.rs` | 310 | Controller profiles | `list/save` | ⬇ db | 4 | — | — |
| `desktop_handoff.rs` | 383 | SSO handoff + exchange codes | `DesktopHandoffs`, `ExchangeCodes` | ⬇ none | 4 | TTL maps, per-account/address limits. | — |
| `desktop_download.rs` | 751 | GitHub release metadata proxy | `snapshot`, `DesktopAsset` | ⬇ http | 4 | `safe_url` scheme filter (`desktop_download.rs:336`), cached, no token. | — |
| `remote_installs.rs` | 759 | Mobile-triggered installs | `RemoteInstall`, `create/claim` | ⬇ db, storage | 4 | — | — |
| `avatar.rs` | 463 | Avatar upload/serve | `store`, `serve` | ⬇ media | 4 | Canonicalize-under-media guard (`avatar.rs:353`). | — |
| `artwork.rs` | 185 | Custom artwork upload | `MAX_ARTWORK_BYTES`, validation | ⬇ media | 4 | `detect_image` magic-byte validation. | — |
| `saves.rs` | 345 | Save storage + quota | `path`, `MAX_*` | ⬇ db | 4 | — | — |
| `collections.rs` | 404 | Smart-rule evaluation | `evaluate` | ⬇ db | 4 | — | — |
| `collection_share.rs` | 663 | Public collection links | `CollectionShare*` | ⬇ db | 4 | — | — |
| `share.rs` | 776 | Public game share links | `ShareLink`, `create/access` | ⬇ db | 3 | **Fourth rate-limiter implementation** (`share.rs:629 AttemptLimiter`) duplicating `security::AttemptCounter`, held in a process-global `OnceLock` (`share.rs:672`) — not injectable, not resettable, leaks between tests in one process. Password-hash dummy work factor duplicated (`share.rs:620`). | P2 |
| `community.rs` | 201 | Activity feed read/write | `record`, `feed` | ⬇ db | 4 | — | — |
| `audit.rs` | 200 | Audit trail | `actions::*`, `record` | ⬇ db | 4 | Action names are `pub const` — good. | — |
| `emulators.rs` | 706 | Emulator registry/ROM launch | `Emulator`, `validate` | ⬇ core rom | 3 | Third copy of `is_unique_violation` (`emulators.rs:567`). | P3 |
| `uploads.rs` | 295 | Resumable upload assembly | `assemble`, `cleanup_worker` | ⬇ games dir | 4 | `sanitize_file_name` + `unique_destination` tested against traversal. | — |
| `integrity.rs` | 841 | Deep archive verification | `IntegrityReport`, `scheduled_worker` | ⬇ db, core | 4 | `check_archives` 134 lines; hashing offloaded to `spawn_blocking`. | — |
| `backup.rs` | 934 | Tar backups + restore | `create`, `restore`, `verify` | ⬇ db, storage | 4 | Restore rejects unsafe tar paths (`backup.rs:787`); all tar IO in `spawn_blocking`. | — |
| `gamevault.rs` | 1 479 | One-shot GameVault import | `run`, `ImportOptions` | ⬇ db | 3 | CLI-only code shipped in the server lib; 28 fns, ~50 lines of tests. Could be a separate `xtask`/bin module. | P2 |
| `scanner.rs` | 1 728 | Library walk + upsert | `ScanRequest`, `worker`, `run_scan`, `index_uploaded_archive` | ⬇ db, core | 3 | `run_scan` is 192 lines (`scanner.rs:182`); `upsert_version` 133. Good use of `spawn_blocking` for walking/hashing. | P2 |
| `metadata.rs` | 1 955 | IGDB + Steam + matching + artwork | `IgdbClient`, `enrich_game`, `score_candidate` | ⬇ http, media | 3 | One file owns four concerns (IGDB client, Steam fallback, fuzzy matching, artwork cache). `enrich_game` 142 lines (`metadata.rs:641`). 18 `.expect` mostly fixtures. | P2 |
| `providers.rs` | 1 321 | Community metadata sidecars | `MetadataProviderConfig`, `search`, `resolve_target` | ⬇ http | 4 | Best-in-class SSRF policy for the *base URL* (resolve + pin + no redirects, `providers.rs:497-540`); the returned artwork URLs escape that check (see `media.rs`). | P1 (via media) |
| `achievements.rs` | 2 215 | Achievement providers + unlocks | `scan`, `for_game`, `to_public` | ⬇ db, fs | 3 | **N+1**: `upsert` (`achievements.rs:933`) does SELECT then UPDATE/INSERT per achievement, called per achievement in a loop (`achievements.rs:829`), with **no transaction** — 2-3 round trips × N per scan. `scan` 192 lines. Galaxy SQLite path builds table names by interpolation (`achievements.rs:527`, `LIMIT {const}` — const, not user input, OK). | P1/P2 |
| `library_health.rs` | 2 310 | Health Center findings | `run`, `apply_action`, `LibraryHealth*` | ⬇ db, fs, storage | 3 | `insert_findings` (`library_health.rs:657`) issues **one INSERT per finding** inside a transaction (no multi-row VALUES). `check_duplicates` 160 lines, `apply_action` 161, `check_archives` 141. | P2 |
| `virustotal.rs` | 2 527 | VT lookups/quota/cache | `Scanner`, `Verdict` | ⬇ http, db | 3 | Single feature, 35 fns, 2 527 lines — the largest single-feature module. `scan` 141 lines (`virustotal.rs:1561`). Honest-verdict design is excellent (404≠clean). | P2 |
| `oidc.rs` | 2 005 | OIDC + PKCE + settings + JWKS | `OidcClient`, `load_settings`, `resolve_settings` | ⬇ http, secrets | 3 | Correct security posture (state/nonce/PKCE S256, `aud`/`iss`/`exp` required, nonce constant-time compared, `oidc.rs:829-1140`), but 2 005 lines in one file mixing flow, settings crypto, discovery and JWKS fallback (RSA n/e path `oidc.rs:1064`). | P2 |
| `social.rs` | 1 807 | Friends, presence, notifications | `Friend`, `Presence`, `Notification` | ⬇ db | 3 | 38 fns; presence/notification preference tables re-read per request (`social.rs:956`, `1008`). | P3 |
| `extracted_cache.rs` | 1 542 | Server-side extracted cache | `ExtractedCache`, manifest | ⬇ core archive, storage | 3 | 11 fns for 1 542 lines — several very long functions; index rebuild at startup. | P2 |
| `mfa.rs` | 428 | TOTP challenges + recovery | `enroll`, `verify`, `redeem` | ⬇ db, totp | 4 | Replay prevented by `last_used_step` guard in the UPDATE (`mfa.rs:350`). | — |
| `totp.rs` | 409 | RFC 6238 implementation | `hotp`, `verify`, `base32_*` | ⬇ none | 5 | Local HMAC-SHA1 with RFC test vectors; recovery codes hashed. | — |
| `locale.rs`, `main.rs`, `bin/ludex-backup.rs`, `examples/*` | 184/178/198/251 | CLI, locale, tooling | — | — | 4 | `main.rs` hand-rolls arg parsing (fine, 3 flags). | — |

### 2.2 Routes (`crates/ludex-server/src/routes/`) — 17 028 LOC, 44 modules

All modules follow the same shape (`router()` → handlers → `require_*`). Errors are uniformly
`ApiError`; there is **no handler returning a non-`ApiError` error type** (verified by grep).

| File | LOC | Routes | Score | Issues | Prio |
|---|---|---|---|---|---|
| `games.rs` | **3 334** | 19 | 3 | God module: listing+filters, detail, update, versions CRUD, downloads, Range/If-Range/ETag parsing, tar/zip streaming, `TransferLog` Drop guard, age gating. ~3 000 lines of logic. Search path loads **up to 5 000 rows** and ranks in Rust, then computes `total` from the truncated set (`games.rs:325-355`) → silently wrong totals on large libraries + 5 000-row memory spike. `binds` vector hard-codes two `user.id` binds to match `FROM_JOINS` (`games.rs:327`) — fragile positional coupling. | **P1/P2** |
| `openapi.rs`* | 104 | 2 | 5 | Thin wrapper; correct cache-control; docs page gated. | — |
| `collections.rs` | 1 040 | 8 | 3 | Import 178 lines (`collections.rs:541`); four `format!("SELECT {CONST}...")` builders with positional binds. | P2 |
| `admin_games.rs` | 982 | ~7 | 3 | Bulk operations; `list` 172 lines; job state in memory. | P2 |
| `game_metadata.rs` | 841 | ~6 | 3 | Manual-match apply 180 lines (`game_metadata.rs:533`). | P2 |
| `preferences.rs` | 679 | 3 | 4 | Local `is_unique_violation` (`preferences.rs:360`) — third copy. | P3 |
| `backups.rs` | 675 | 5 | 4 | Canonicalize-under-backup-dir guard (`backups.rs:289-297`). | — |
| `versions.rs` | 666 | 5 | 3 | Extracted-cache transport; `std::fs::canonicalize` inside async fn (`versions.rs:551-557`) — small blocking call per request. | P2 |
| `auth.rs` | 598 | 14 | 4 | Login/MFA/logout/sessions; `format!("UPDATE users SET {}")` from a static field list (`auth.rs:336`). | — |
| `admin_oidc.rs` | 479 | 4 | 4 | Hot reload of OIDC client. | — |
| `metadata_providers.rs` | 471 | 4 | 3 | **`map_unique_error` string-matches `"UNIQUE constraint failed"`** (`metadata_providers.rs:436-444`) → on PostgreSQL the same conflict returns **500** instead of 400. Also does a SELECT-then-INSERT name check (TOCTOU; unique index still protects). | **P1** |
| `share.rs` | 461 | 7 | 4 | Public access, password gate, throttling (delegates to `share.rs` limiter above). | P2 |
| `diagnostics.rs` | 451 | 3 | 4 | Public POST with double redaction, body/field caps. | — |
| `search.rs` | 448 | 2 | 3 | In-memory fuzzy match, `LIMIT 20`; shares `match_game` with `games.rs` (good) but the ranking algorithm is duplicated between list and search paths. | P2 |
| `saves.rs` | 444 | 3 | 3 | `upload` 183 lines (`saves.rs:72`); manual multipart field loop. | P2 |
| `invitations.rs` | 424 | 4 | 4 | `accept` 144 lines; unique-violation handled via shared helper. | — |
| `users.rs` | 406 | 4 | 3 | `update` 151 lines with `format!("UPDATE users SET {}")` from static subset. | P3 |
| `uploads.rs` | 376 | 4 | 4 | — | — |
| `library.rs` | 314 | 6 | 4 | — | — |
| `oidc.rs` | 310 | 5 | 4 | — | — |
| `community.rs` | 265 | 5 | 4 | `format!("UPDATE users SET {}")` from static list (`community.rs:122`). | P3 |
| `keys.rs` | 259 | 3 | 4 | — | — |
| `artwork.rs` | 258 | 4 | 4 | `kind.column()` interpolation is an enum method, safe. | — |
| `system.rs` | 239 | 5 | 4 | Owns the canonical `is_unique_violation` (`system.rs:234`) used by 6 modules by cross-import (`super::system::`). | — |
| `account.rs` | 221 | 3 | 4 | — | — |
| `social.rs` | 215 | 15 | 4 | 15 one-line handlers delegating to `social.rs`. | — |
| `achievements.rs` | 213 | 6 | 4 | — | — |
| `webhooks.rs` | 197 | 3 | 4 | `format!("UPDATE webhooks SET {}")` static list. | — |
| `emulators.rs` | 179 | 7 | 4 | — | — |
| `remote_installs.rs` | 176 | 3 | 4 | — | — |
| `mod.rs` | 159 | — | 4 | Contains `validate_username/password/email` — should live with auth. | P3 |
| `news.rs` | 154 | 3 | 4 | — | — |
| `virustotal.rs` | 147 | 4 | 4 | — | — |
| `library_health.rs` | 122 | 3 | 4 | — | — |
| `integrity.rs` | 120 | 4 | 4 | — | — |
| `metrics.rs` | 118 | 1 | 4 | Own auth path, 2 `unwrap()` are tests. | — |
| `admin_server.rs` | 95 | 2 | 4 | — | — |
| `audit.rs` | 93 | 1 | 4 | — | — |
| `devices.rs` | 90 | 2 | 4 | — | — |
| `controllers.rs` | 57 | 2 | 4 | — | — |
| `desktop.rs` | 55 | 1 | 4 | — | — |
| `media.rs` | 49 | 2 | 4 | — | — |
| `observability.rs` | 44 | 1 | 5 | Minimal, correct admin gate. | — |

\* `openapi.rs` counted twice: 104 in `routes/`, 3 220 at the crate root.

### 2.3 Crate-root big modules

| File | LOC | Role | Score | Issues | Prio |
|---|---|---|---|---|---|
| `openapi.rs` | **3 220** | Hand-declared route table + document builder | 3 | `ROUTES` is a 2 500-line literal array (`openapi.rs:135-2637`) with 231 entries; schema registration is a ~500-line `.schema_from::<T>()` chain (`openapi.rs:2950+`). Mitigated by `tests/openapi.rs` (parses `<file>.rs` text for `.route(` and probes each path), but that test is a **source-text parser** — a formatting change (`get(handler)` on a new line, raw strings) can make it silently under-report. | P2 |
| `models.rs` | 2 685 | All DTOs + row shapes | 3 | 100+ structs in one file (2 067-2 685 is tests). **`GameVersionInfo` (`models.rs:112`) is a field-for-field duplicate of `ludex_core::models::GameVersion` (`core/models.rs:255`)**, minus `indexed_at`; both derive `ToSchema` and serialize identically to the web client → guaranteed drift. `UserRow`, `AuditLogRow`, `WebhookRow` etc. each carry a `to_public`. | P2 |

`openapi.rs` at 3 220 lines is the **second-largest file in the repo** and contains zero fallible
logic — it is pure data. It inflates compile times and makes `cargo fmt` diffs noisy.

### 2.4 `ludex-core` (8 633 LOC)

| File | LOC | Role | Score | Issues |
|---|---|---|---|---|
| `archive.rs` | **4 451** | ZIP/RAR/7z detect+verify+extract, tar streaming, 7z binary discovery, progress | 3 | God module: 4 extractors + streaming + process control + progress in one file. Good tests (1 300+ lines, `expect` count 184 mostly there). `seven_zip_binary` runs `Command` — documented, only for local 7zz. `MAX_ZIP_WORKERS=8`, 4 MiB write buffers documented. Split candidates: `archive/zip.rs`, `archive/rar.rs`, `archive/sevenz.rs`, `archive/tarstream.rs`. |
| `exe.rs` | 865 | Executable discovery | 4 | Depth clamp, exclusion list. |
| `rom.rs` | 689 | ROM platform detection + launch commands | 4 | `%ROM%` substitution is quoted correctly (tested with `it's fine.nes`). |
| `models.rs` | 663 | Shared DTOs | 4 | Duplicate of the `GameVersionInfo` shape (see above). |
| `redact.rs` | 603 | Log/report redaction | 5 | Stable path hashes; used by diagnostics. |
| `naming.rs` | 561 | GameVault-compatible filename parser | 4 | — |
| `gamepad.rs` | 452 | Controller mapping/fingerprint | 4 | — |
| `fingerprint.rs` | 149 | Cheap archive fingerprint | 5 | 16 `unwrap()` — all in tests. |
| `slug.rs` | 146 | Slugs | 5 | — |
| `lib.rs` | 54 | Re-exports + `forbid(unsafe_code)` + `warn(missing_docs)` | 5 | — |

### 2.5 Migrations

| Path | Count | LOC | Issues |
|---|---|---|---|
| `migrations/` (SQLite) | 32 | 1 301 | All `IF NOT EXISTS`; FKs declared; every file has a header comment naming the issue. `0032_virustotal.sql` shows the standard: table doc, index list, rationale. |
| `migrations_postgres/` | 32 | 1 012 | Mirrors by hand: `INTEGER PRIMARY KEY AUTOINCREMENT` → `BIGSERIAL`, `TEXT` timestamps kept (deliberate, see §3.3). Divergence is only caught by a **count** assertion (`tests/postgres.rs:193`) plus a few hand-written inserts — column-level drift (missing index, different default) is undetected. |

Migrations are embedded via `sqlx::migrate!` in `db::connect_with` (`db.rs:1207`, `db.rs:1236`), so an
edited migration file silently fails checksum validation at startup — good, but it means the only
safe fix for a released migration is a new number. Next free number: **0033**.

### 2.6 Tests (`crates/ludex-server/tests/`)

| File | LOC | Tests | Uses `common`? | Assessment |
|---|---|---|---|---|
| `api.rs` | **3 596** | 24 | no | God test file covering list/detail/update/download/share/news/... — the de-facto integration suite. Splitting by area would halve wall-clock noise. |
| `integrations.rs` | 788 | 6 | no | Mock HTTP servers. |
| `virustotal.rs` | 786 | 9 | yes | — |
| `extracted_cache.rs` | 770 | 6 | yes | — |
| `oidc.rs` | 756 | 7 | no | — |
| `saves.rs` | 706 | 6 | no | — |
| `account_security.rs` | 700 | 8 | yes | — |
| `metadata_match.rs` | 617 | — | yes | Pure matching logic. |
| `postgres.rs` | 592 | 1 (skip-if-unset) | no | Covers only setup/login/list/progress/saves. |
| `collections_share.rs` | 571 | — | yes | — |
| `social.rs` | 543 | — | yes | — |
| `achievements.rs` | 538 | — | no | — |
| `library_ops.rs` | 537 | 7 | no | — |
| `admin_games.rs` | 507 | 7 | yes | — |
| `download_limits.rs` | 445 | — | no | — |
| `avatar.rs` | 401 | 6 | no | — |
| `preferences.rs` | 395 | 6 | yes | — |
| `game_versions.rs` | 393 | — | yes | — |
| `desktop_download.rs` | 392 | 6 | no | — |
| `remote_installs.rs` | 389 | — | yes | — |
| `devices.rs` | 382 | — | yes | — |
| `metadata_providers.rs` | 378 | — | yes | — |
| `migration.rs` | 367 | — | no | **Misnamed**: the file documents itself as "GameVault import tests (#6)" (`migration.rs:1`) — it is `gamevault` tests, not migration tests. |
| `backup.rs` | 348 | 6 | no | — |
| `uploads.rs` | 291 | — | yes | — |
| `desktop_handoff.rs` | 290 | 10 | yes | — |
| `parental.rs` | 283 | — | no | — |
| `admin_backups.rs` | 276 | — | yes | — |
| `artwork.rs` | 269 | — | no | — |
| `openapi.rs` | 248 | — | no | Contract test (source-text parser). |
| `api_security.rs` | 232 | — | yes | — |
| `locale.rs` | 219 | 6 | yes | — |
| `content_hash.rs` | 201 | — | yes | — |
| `core_gaming.rs` | 190 | — | no | — |
| `admin_observability.rs` | 180 | — | yes | — |
| `diagnostics.rs` | 165 | — | yes | — |
| `onboarding.rs` | 159 | — | yes | — |
| `library_health.rs` | 137 | — | yes | — |
| `observability.rs` | 135 | — | no | — |
| `common/mod.rs` | 253 | — | — | `TestServer` + `setup/login/create_user/add_game/wait_for_scan`. |

**16 of 39 suites ignore `common/mod.rs`** and re-implement server boot/HTTP helpers; that is the
largest source of duplication in the test tree (~1 500 duplicated lines, estimated).

---

## 3. Cross-cutting analysis

### 3.1 Layering

Clean, with three leaks:

1. `routes/versions.rs:551-557` uses `std::fs::canonicalize` / `symlink_metadata` directly in async
   handlers (blocking syscalls). Trivial in cost, but it is the only place where HTTP code touches
   the filesystem without going through `storage.rs` — and it breaks the "routes are thin" rule.
2. `routes/system.rs:234` exports `is_unique_violation`, which `users`, `invitations`, `collections`,
   `preferences`(copy), `emulators`(copy), `oidc` import via `super::system::` / `crate::routes::system::`.
   A SQL-error predicate living in a **route** module and imported by domain modules inverts the
   dependency (`. → routes`).
3. `gamevault.rs` (domain) is only reachable from `main.rs`; it drags `std::fs` dump parsing into the
   server crate. Fine for now, but it is the only module that exists purely for a CLI.

### 3.2 Error taxonomy

`ApiError { status, code, message, retry_after }` → `{"error":{"code","message"}}` is used
uniformly; the custom extractors (`extract.rs`) ensure axum rejections never leak plain text. Gaps:

- `impl From<sqlx::Error> for ApiError` (`error.rs:122`) collapses **every** DB error to
  `500 internal_error`. Constraint violations therefore need manual mapping, and three modules
  forgot or did it three different ways:
  - `system.rs:234` (canonical), `preferences.rs:360`, `emulators.rs:567` → **three copies**;
  - `metadata_providers.rs:436` → **string match on the SQLite message**, wrong on Postgres.
- `From<serde_json::Error>` → 500 even when the failure is a malformed *request* body (should be 400).
  In practice handlers pre-validate, but a `.json::<T>()` failure inside `providers.rs` would 500.
- No `IntoResponse` for domain error enums (`ArchiveError`, `StorageError`, `ProviderError`) — each
  call site maps manually. That is ~40 hand-written match arms across routes; a `thiserror` →
  `ApiError` bridge would remove them.

### 3.3 SQLite / PostgreSQL parity

Architecture is sound: one SQL text, `?` → `$n` rewrite (`pg_placeholders`), one `Database` enum,
`RETURNING id` everywhere instead of `last_insert_rowid` (documented `db.rs:609`). Only **2**
dialect branches exist (`routes/games.rs:139` and `:148`, JSON array containment) — impressively low.

Remaining parity risks, in order of severity:

1. **`metadata_providers.rs:436-444`** — message sniffing breaks on PG (see above). P1.
2. **Timestamps are `TEXT` on both engines** (`db_time` = RFC 3339 seconds). Comparisons like
   `expires_at > ?` (`auth.rs:340`) rely on fixed-width UTC strings; that holds today, but the schema
   forbids using native `timestamptz`, so no range index benefit and no type-level protection
   against a non-`Z` write. Worth a documented invariant + a test.
3. **Migration content parity is unverified** — only file count is asserted (`tests/postgres.rs:193`).
4. **Coverage**: `tests/postgres.rs` exercises setup/login/list/progress/saves only. Achievements,
   social, collections, library health, backups, webhooks, extracted cache, uploads and VirusTotal
   have **never run on Postgres** in CI.
5. `foreign_keys(true)` (`db.rs:1256`) is SQLite-only; PG relies on declared REFERENCES — fine because
   the PG migrations do declare them, but it is another hand-maintained asymmetry.

### 3.4 Transactions

Only 12 explicit `begin()` sites. Most single-statement writes are fine, but the following
multi-statement invariants are **not** transactional:

- `achievements::upsert` (`achievements.rs:933-1000`): SELECT → UPDATE or INSERT, per achievement,
  in a loop, no transaction → a crash mid-scan leaves partially updated definitions, and concurrent
  scans can race into the unique index (mapped to a 500).
- `scanner::upsert_version` (`scanner.rs:653`) + `publish_archive` (`scanner.rs:791`) + `ensure_game`
  (`scanner.rs:436`): game row, version row and event are separate statements.
- `auth::revoke_other_sessions` (`auth.rs:296-345`): three statements (count, insert revocations,
  update sessions) without a transaction — a crash between 2 and 3 leaves sessions that are
  "revoked" in `token_revocations` but still listed active in `auth_sessions`.
- `routes/games.rs:1093` delete cascade does use a transaction — the exception, not the rule.

`DbTransaction` exists and is used correctly where it is used (backup, library health, game delete,
emulator seed, gamevault import).

### 3.5 Migrations discipline

Good: numbered pairs, `IF NOT EXISTS`, issue refs, comments explaining *why*, embedded via
`sqlx::migrate!` (checksum-protected), `PRAGMA`/WAL configured in code. Weak: no down-migrations
(acceptable for this project), no schema snapshot/doc generation, PG mirror is hand-maintained, and
the only migration test is misnamed and unrelated (`tests/migration.rs` = GameVault tests).

### 3.6 Pagination

Two conventions coexist:

- `Paginated<T>` (`core/models.rs:438`) with `page`/`limit` for games, collections, audit,
  diagnostics, admin games, search — all clamp `limit` (`1..=200`, search `1..=20`).
- Bare arrays for social (friends/notifications use internal `LIMIT ?` but no cursor),
  achievements (capped by `MAX_DEFINITIONS`), share links, devices, emulators.

No cursor/keyset pagination anywhere; `OFFSET` on `games` with 100k rows will degrade. More urgent:
the **search path silently truncates** at `LIMIT 5000` and reports the truncated length as `total`
(`routes/games.rs:325-355`).

### 3.7 API contract / OpenAPI drift

Best-in-class *intent*: `tests/openapi.rs` parses every `.route(` in `src/routes/*.rs` and
`src/ingest/mod.rs` and fails if the router and `ROUTES` disagree, then boots a server and probes
each documented path. Schemas come from `utoipa::ToSchema` derives, so field drift is impossible.

Weakness: the test's parser is **textual** (`tests/openapi.rs:34-110`). Any of these silently reduces
coverage: a route registered through a loop/`nest`/helper, `include_str!`, or formatting that puts
the path literal off the same line. Also nothing asserts that every `ToSchema` type in `models.rs`
is *registered* — an unregistered-but-referenced schema would only show as a dangling `$ref`.
The 2 500-line `ROUTES` literal is also a merge-conflict magnet (every lane that adds a route edits
the same block, and `openapi.rs` is not in the project's conflict list).

### 3.8 Security

Strong overall: Argon2id, JWT with `jti` + server-side revocation, API-key scopes with
`has_scope`/`require_*` helpers applied consistently (spot-checked 12 admin routes), constant-time
comparisons for tokens and nonces, replay-proof TOTP, AES-GCM at rest with HKDF purpose separation,
path-traversal guards via canonicalize+`starts_with` in games/versions/achievements/artwork/avatar/
backup, and an exemplary SSRF policy for webhook and provider *base* URLs (resolve, pin the IP,
`redirect::Policy::none`, `webhooks.rs:359-382`).

Concrete gaps:

1. **SSRF via artwork URL (P1).** `providers.rs` validates the base URL, then returns arbitrary
   `coverUrl`/`backgroundUrl`/`screenshots` from the provider's JSON. `metadata::cache_artwork`
   (`metadata.rs:873`) → `media::download_media` (`media.rs:41`) fetches them with the shared
   client: no `resolve_target`, no private-IP check, and reqwest's default redirect policy follows up
   to 10 hops. A malicious or compromised community provider can make the server issue GETs to
   `http://10.1.50.103:8006/...` or `http://169.254.169.254/...`. Exfiltration is limited (the body
   must pass `detect_image` to be stored) but this is a blind SSRF and a port-scan oracle via
   timing/status. Fix: route artwork fetches through `providers::resolve_target` (or a shared
   `outbound` module) with `redirect(none)` and a per-hop check.
2. **No HTTP security headers** on the SPA shell (`web.rs`): no CSP, `X-Content-Type-Options`,
   `Referrer-Policy`, or frame-ancestors. The app is same-origin and token-based, so impact is
   limited, but CSP is the cheapest defence for the docs page and any XSS in the bundle.
3. `db.rs:811` panic on decode error — a schema/row-shape mismatch in production becomes a panicked
   task (no response, connection dropped) rather than a 500. ~100 call sites use the panicking getter.
4. `routes/metrics.rs` allows unauthenticated scrapes when `LUDEX_METRICS_TOKEN` is unset (rate
   limited to 30/min). Documented and intentional; worth calling out in deployment docs.
5. Rate limiting is applied to `/api` only — `/metrics` has its own, and the `ingest` router gets no
   limiter (it is editor-only, so acceptable).
6. `share.rs` global limiter is process-wide and not resettable → tests share budget, and a
   multi-instance deployment would not share it.

### 3.9 Test gaps

Coverage is broad (247 distinct `/api/...` paths appear in tests vs 181 documented), but these areas
have **no dedicated suite** and only incidental coverage:

| Area | Suite | Note |
|---|---|---|
| `audit`, `webhooks`, `keys`, `media`, `news`, `search`, `users`, `share`, `community`, `controllers`, `emulators`, `game_metadata`, `integrity`, `invitations` | none | exercised only via `api.rs` / `api_security.rs` / `collections_share.rs`, if at all |
| Postgres | 1 conditional test | see §3.3 |
| Migration parity/content | none | `tests/migration.rs` is GameVault |
| `openapi` schema registration completeness | none | only route presence |
| `config.rs` `from_env` | inline unit tests only | no test for invalid combinations |
| Concurrency/races | none | no test for two simultaneous scans, uploads or backups (guards exist but are untested under contention) |

Positives: `download_limits.rs` (445 lines), `extracted_cache.rs` (770), `virustotal.rs` (786) and
`desktop_handoff.rs` (290) are genuinely adversarial suites. `ingest/tests.rs` (712 lines inline)
covers resume/range logic.

### 3.10 Feature flags

There is **no** Cargo/runtime feature-flag framework beyond `embed-web` (`Cargo.toml`). Feature
availability is expressed as:
- `Config` booleans (`extracted_cache_enabled`, `watcher_enabled`, `openapi_docs`, metrics token),
- `FeatureFlags` in `/api/config` (`models.rs:635`) — a *client hint* struct with two fields that are
  hardcoded `true` ("always true in this build": `steam`, `archive_sha256`).

That is fine for a single-binary self-hosted app, but the `FeatureFlags` struct is already lying in
two fields and will accumulate more. Either compute them or drop the constants.

### 3.11 Performance

Hot spots found by inspection:

1. `routes/games.rs:325` — search loads ≤5 000 rows and ranks/sorts/paginates in Rust. O(N log N) per
   request with N up to 5 000, plus 5 000 `GameListRow` allocations. This is the single biggest
   per-request allocation in the app.
2. `achievements.rs:829` — 2-3 queries per achievement per scan, no transaction (N+1).
3. `library_health.rs:657` — one INSERT per finding (thousands on a broken library).
4. `metadata.rs:873` + `:912` — artwork/screenshot downloads are `buffered(4)` (good) but unbounded in
   count per game; a game with 30 screenshots issues 30 requests inside the enrichment path.
5. `routes/games.rs:1653` — 1 MiB copy chunks (documented measurement, deliberate).
6. `db.rs:1207` — SQLite pool default 8 with `acquire_timeout(30s)`; `busy_timeout(5s)`; WAL on.
   Reasonable, but `max_connections` is not validated against PG's limits (8 default is low for PG).
7. `versions.rs:551` — blocking `canonicalize` per extracted-cache request.

Non-issues verified: `spawn_blocking` is used consistently for hashing, walking, tar/zip IO and
backup (~20 sites); no `block_in_place`; no `std::thread::sleep`; no query inside a `for` loop in
`routes/*` (outside the two N+1 cases above); `SELECT *` appears only twice.

---

## 4. Quantification

### 4.1 Top 20 largest files (production LOC)

| # | File | LOC |
|---|---|---|
| 1 | `crates/ludex-server/src/routes/games.rs` | 3 334 |
| 2 | `crates/ludex-server/src/openapi.rs` | 3 220 |
| 3 | `crates/ludex-server/src/models.rs` | 2 685 |
| 4 | `crates/ludex-server/src/virustotal.rs` | 2 527 |
| 5 | `crates/ludex-server/src/library_health.rs` | 2 310 |
| 6 | `crates/ludex-server/src/achievements.rs` | 2 215 |
| 7 | `crates/ludex-server/src/oidc.rs` | 2 005 |
| 8 | `crates/ludex-server/src/metadata.rs` | 1 955 |
| 9 | `crates/ludex-server/src/social.rs` | 1 807 |
| 10 | `crates/ludex-server/src/scanner.rs` | 1 728 |
| 11 | `crates/ludex-server/src/db.rs` | 1 607 |
| 12 | `crates/ludex-server/src/extracted_cache.rs` | 1 542 |
| 13 | `crates/ludex-server/src/gamevault.rs` | 1 479 |
| 14 | `crates/ludex-server/src/providers.rs` | 1 321 |
| 15 | `crates/ludex-server/src/config.rs` | 1 186 |
| 16 | `crates/ludex-server/src/ingest/manager.rs` | 1 172 |
| 17 | `crates/ludex-server/src/routes/collections.rs` | 1 040 |
| 18 | `crates/ludex-server/src/routes/admin_games.rs` | 982 |
| 19 | `crates/ludex-server/src/metrics.rs` | 951 |
| 20 | `crates/ludex-server/src/backup.rs` | 934 |
| — | `crates/ludex-core/src/archive.rs` | 4 451 (largest in repo) |

20 files = 34 752 LOC = 53 % of the server crate. `archive.rs` alone is 52 % of `ludex-core`.

Largest functions (auto-measured):

| Lines | Location | Function |
|---|---|---|
| 456 | `openapi.rs:2742` | `document` |
| 239 | `lib.rs:167` | `prepare` |
| 212 | `ingest/manager.rs:583` | `attempt_once` |
| 192 | `scanner.rs:182` | `run_scan` |
| 192 | `achievements.rs:735` | `scan` |
| 183 | `routes/saves.rs:72` | `upload` |
| 180 | `routes/game_metadata.rs:533` | `apply` |
| 178 | `routes/collections.rs:541` | `import` |
| 172 | `routes/admin_games.rs:184` | `list` |
| 162 | `metrics.rs:688` | `render_snapshot` |

### 4.2 Hygiene counts (grep)

| Pattern | Count | Comment |
|---|---|---|
| `unwrap()` | 36 | 0 production; `core/fingerprint.rs` 16, `core/rom.rs` 10, rest tests |
| `.expect(` | 735 | ≈600 in `#[cfg(test)]`; production ones are startup invariants (`expect("HMAC accepts any key length")`, `expect("32 bytes is a valid HKDF output length")`) |
| `panic!` | 11 | 1 production (`db.rs:811`), 10 tests |
| `todo!`/`unimplemented!` | 0/0 | — |
| `unsafe` | 0 blocks | `#![forbid(unsafe_code)]` in core only |
| `TODO`/`FIXME`/`HACK` | 0 | — |
| `#[allow(dead_code)]` | 9 | 6 in `openapi.rs` (const-fn helpers), 1 `db.rs`, 1 `oidc.rs`, 1 test |
| `SELECT *` | 2 | — |
| `LIMIT ?` | 26 | consistent |
| `OFFSET` | 4 | games, collections, audit, diagnostics |
| `format!` SQL with user data | 0 | all interpolations are static column lists / enum-derived |
| `.begin().await` | 12 | — |
| `ON CONFLICT` | 31 | — |
| `spawn_blocking` | 20 | — |
| `is_postgres()` branches | 2 | — |

### 4.3 Duplicated helper clusters

| Cluster | Sites | Cost |
|---|---|---|
| `is_unique_violation` | `routes/system.rs:234`, `routes/preferences.rs:360`, `emulators.rs:567` + message-sniffing variant `routes/metadata_providers.rs:436` | 4 copies, 1 wrong on PG |
| Rate limiting | `security::RateLimiter` (token bucket), `security::LoginGuard` (backoff+lockout), `security::AttemptCounter` (fixed window), `download_limits::DownloadLimiter` (multi-bucket), `share::AttemptLimiter` (sliding window, static), `metrics::Metrics::allow_scrape` | 6 policies, 4 vocabularies |
| `UPDATE ... SET {}` dynamic builders | `routes/games.rs:1315`, `routes/users.rs:243`, `routes/auth.rs:336`, `routes/community.rs:122`, `routes/collections.rs:245`, `routes/game_metadata.rs:670`, `routes/news.rs:90`, `routes/webhooks.rs:136` | 8 near-identical blocks |
| `to_public` row→DTO | 16 impls | by design, but `GameVersionInfo` duplicates a core model instead |
| `SELECT {COLUMNS} FROM ... WHERE id = ?` | 9 constant-column fragments (`LIST_COLUMNS`, `DETAIL_COLUMNS`, `PROFILE_COLUMNS`, `AUDIT_COLUMNS`, `KEY_COLUMNS`, `SHARE_COLUMNS`, `PUBLIC_GAME_COLUMNS`, `COLLECTION_COLUMNS`, `PROVIDER_COLUMNS`) | acceptable; the bind-position coupling is the risk |
| 4-arm executor match in `db.rs` | 12 method bodies | ~750 lines |
| Test bootstrap | 16 suites without `common/mod.rs` | ~1 500 lines |
| Schema registration chain | `openapi.rs:2950+` (~250 entries) | 500 lines, merge-conflict-prone |

---

## 5. Top 20 refactors (impact / effort / risk)

| # | Refactor | Impact | Effort | Risk | Notes |
|---|---|---|---|---|---|
| 1 | **Fix SSRF in artwork download** — route `media::download_media` through `providers::resolve_target`, pin the address, `redirect::Policy::none`, re-check every hop; add a `--allow-private` test knob | High (security) | S (½ day) | Low | `media.rs:41`, `metadata.rs:873/896/912`. Reuse `webhooks.rs:359-382` pattern. |
| 2 | **Single `is_unique_violation`** in `db.rs` (or `sql.rs`) using `DatabaseError::is_unique_violation`, replace all 4 copies; delete the string match | High (PG correctness) | S | Low | `metadata_providers.rs:436` is the actual bug. |
| 3 | **Wrap achievement scan in one transaction + batch upsert** — load existing keys once, then `INSERT ... ON CONFLICT DO UPDATE` per achievement (or multi-VALUES chunks of 50) | High (perf + atomicity) | M (1-2 d) | Medium | `achievements.rs:829`, `:933`. Same for `library_health::insert_findings` (`:657`). |
| 4 | **Split `routes/games.rs`** into `games/list.rs`, `games/detail.rs`, `games/versions.rs`, `games/download.rs`, `games/transport.rs` (Range/ETag/tar/zip helpers) | High (maintainability) | M (2 d) | Medium | 3 334-line god module with 3 000 lines of logic; transport helpers are reusable by `versions.rs`. |
| 5 | **Fix search pagination** — either `EXISTS`-based SQL scoring with `LIMIT/OFFSET`, or drop the `5000` cap, stream and cap `total` explicitly with a `truncated` flag | High (correctness + memory) | M (1 d) | Medium | `routes/games.rs:325-355`. Cheap interim: report `total = min(count, 5000)` and add `truncated: true`. |
| 6 | **Collapse `db.rs` 4-arm duplication** behind an internal macro or a `PgExecutor`/`SqliteExecutor` trait object | Medium (maintainability) | M (1-2 d) | Medium (unsafe to get wrong) | `db.rs:609-1148`. Pair with replacing `DbRow::get` panics by `try_get` + `ApiError` at the ~100 call sites (or at least the ~20 in request paths). |
| 7 | **De-duplicate `GameVersionInfo`** — re-export `ludex_core::models::GameVersion` (add optional `indexed_at`) | Medium (contract drift) | S (½ day) | Low | `models.rs:112` vs `core/models.rs:255`. Touches web types but the JSON is already identical. |
| 8 | **`AppState` decomposition** — `SecurityState { rate, login, diagnostics, invitations, mfa, handoff_*, friend, device, download }`, `BackgroundState { integrity_busy, media_gc_busy, ... }`, `ContentState { storage, extracted, hashes }`; one `AppState::new(config)` used by both `prepare` and `for_tests` | Medium (single place to wire features) | M (1 d) | Low-Med | `state.rs:20-120` (42 public fields), `lib.rs:167-406`, `state.rs:217-278`. |
| 9 | **Move `is_unique_violation` + validators out of `routes/system.rs`/`routes/mod.rs`** into `db.rs`/`auth.rs` | Medium (layering) | S | Low | Fixes the `. → routes` import direction; 6 importers. |
| 10 | **Split `ludex-core/src/archive.rs`** into `archive/{mod,zip,rar,sevenz,tarstream,progress}.rs` | Medium (compile + review) | M (1-2 d) | Low (pure move) | 4 451 lines, 52 % of core. Keep `lib.rs` re-exports so nothing downstream changes. |
| 11 | **Generate the OpenAPI route table** from a proc-macro/attribute on handlers, or at least split `ROUTES` per tag into 40 `const &[RouteDoc]` slices concatenated at build | Medium (merge conflicts, 3 220-line file) | L (2-3 d) | Medium | `openapi.rs:135-2637`. Interim: one `mod routes_doc::<area>` per area — mechanical and safe. |
| 12 | **Extract `outbound.rs`** — one place for resolve+pin+no-redirect+size-cap+timeout, used by webhooks, providers, artwork, desktop downloads, ingest | Medium (one policy, one test suite) | M | Low | Today each of the 5 has its own variant; 3 of them are correct. |
| 13 | **Typed domain errors → `ApiError`** (`ArchiveError`, `StorageError`, `ProviderError`, `OidcError`) | Medium (removes ~40 match arms) | M | Low | `error.rs` gains 4 `From` impls; handlers shrink. |
| 14 | **Fold `share::AttemptLimiter` into `security::AttemptCounter`** and move it into `AppState` | Medium (test isolation, consistency) | S | Low | `share.rs:629-676`. |
| 15 | **Split `tests/api.rs` (3 596)** into `api_library.rs`, `api_download.rs`, `api_share.rs`, `api_news.rs` and make all suites use `common/mod.rs` | Medium (CI signal, review) | M (1-2 d) | Low | 16 suites duplicate bootstrap. |
| 16 | **Rename `tests/migration.rs` → `tests/gamevault.rs`** and add a real migration test: parse both migration dirs, assert same file set and same table/column names | Medium (protects PG parity) | S | Low | `tests/migration.rs:1`. The count-only assertion is `tests/postgres.rs:193`. |
| 17 | **`Config::validate()`** called from `prepare` — reject zero/absurd values, unknown locale, missing dirs, `saves_max_bytes > upload_max_bytes`, etc. | Medium (fail fast) | S-M | Low | `config.rs:400-820`. |
| 18 | **Split `oidc.rs`** into `oidc/{flow,settings,jwks}.rs` (2 005 lines) | Low-Med | M | Low | Security-critical code is easier to review in 3 files. |
| 19 | **Move `gamevault.rs` behind a `bin` feature or `examples/`** | Low (binary size, clarity) | S | Low | Only reachable from `main.rs`'s subcommand. |
| 20 | **Add security headers middleware** (CSP, `nosniff`, `Referrer-Policy`, frame-ancestors) for non-API responses | Low-Med (defence in depth) | S | Low | `web.rs:60-103`. |

### Quick wins (≤2 hours each, low risk)

1. `metadata_providers.rs:436` → `db::is_unique_violation` (fixes a real PG 500).
2. `routes/games.rs:325` → set `total` from the count query instead of `ranked.len()` and document the 5 000 cap.
3. `share.rs:629` → use `security::AttemptCounter` (delete 50 lines).
4. Rename `tests/migration.rs` → `tests/gamevault.rs`.
5. Add `#[allow(clippy::too_many_lines)]`-free split of `lib.rs::prepare` into `build_state(config) -> AppState` — one function, ~30 lines saved, removes the `for_tests` drift.
6. `error.rs` — add `impl From<sqlx::Error>` variant that maps `RowNotFound` → 404? **No** (too magic); instead add `ApiError::conflict_from_db(err)` helper next to `is_unique_violation`.
7. `openapi.rs` — assert in `tests/openapi.rs` that every `ToSchema` type referenced by `ROUTES` exists in `components.schemas` (currently only dangling refs would show at runtime).
8. `routes/mod.rs` — move `validate_username/password/email` to `auth.rs` (3 import sites).
9. `web.rs` — add `X-Content-Type-Options: nosniff` and a minimal CSP.
10. `db.rs:811` — add `# Panics` doc + a comment pointing at `try_get`, or migrate the 20 request-path call sites.

---

## 6. Five most important findings

1. **Blind SSRF through provider artwork URLs.** `media::download_media` (`crates/ludex-server/src/media.rs:41`) fetches any URL supplied by a community metadata provider's JSON (`metadata.rs:873`) with the shared HTTP client — no private-address check and default redirect following. `providers.rs` validates only the provider *base* URL, so a malicious sidecar can drive GETs at `10.1.x.x`/link-local/metadata endpoints. Fix by routing artwork through the existing `resolve_target` + `redirect::Policy::none` pattern (`webhooks.rs:359-382`).

2. **PostgreSQL parity bug: unique-violation detection by string sniffing.** `routes/metadata_providers.rs:436-444` matches `"UNIQUE constraint failed"`, which only exists on SQLite; on PostgreSQL the same conflict returns 500 instead of 400. Three other modules each carry their own copy of `is_unique_violation` (`routes/system.rs:234`, `routes/preferences.rs:360`, `emulators.rs:567`) — a shared `db::is_unique_violation` removes the bug and the duplication.

3. **N+1 writes in the two heaviest background jobs.** `achievements::upsert` (`achievements.rs:933`) does SELECT + UPDATE/INSERT per achievement inside a loop with no transaction (`achievements.rs:829`), and `library_health::insert_findings` (`library_health.rs:657`) issues one INSERT per finding. On a large library that is thousands of round trips per run plus partial-state risk on crash; batching with `ON CONFLICT DO UPDATE` inside one transaction fixes both.

4. **God modules and a duplicated contract type.** `routes/games.rs` (3 334 LOC, 19 routes + all Range/ETag/tar/zip transport), `openapi.rs` (3 220 LOC of literal route data), `models.rs` (2 685) and `core/archive.rs` (4 451) hold 53 % of the server crate. Inside `models.rs`, `GameVersionInfo` (`models.rs:112`) is a field-for-field clone of `ludex_core::models::GameVersion` (`core/models.rs:255`) — two schemas the web client cannot tell apart, guaranteed to drift.

5. **Search pagination is silently wrong and memory-heavy.** `routes/games.rs:325-355` loads up to 5 000 rows, ranks in Rust and reports `total = ranked.len()`, so a library larger than 5 000 matching games gets an incorrect `total` and missing pages, with a 5 000-row allocation per query. Score in SQL (or expose `truncated`) before this becomes a support ticket.
