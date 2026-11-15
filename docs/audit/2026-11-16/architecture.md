# Ludex v0.4.14 — Cross-cutting Architecture Review

**Scope:** `/root/ludex` at `7199a2c` (chore(release): v0.4.14).
**Mode:** read-only. No repository files were modified.
**Audience:** whoever sequences the next 3 months of work.
**Companion reports:** `server-core.md`, `desktop.md`, `web.md`, `ui-ux.md`, `quality-infra.md` (this file is the strategy layer above them).

---

## 0. Method

Everything below was verified against the tree, not inferred from docs. Where the docs and the code disagree, the code wins and the doc is cited as drift. Line references are `path:line` at the reviewed commit.

The review answers five questions in order: *what is the shape of the system* → *where does that shape cost us* → *what must be true before we change it* → *in what order do we change it* → *what do we refuse to change*.

---

## 1. Baseline

| Component | Rust LOC | Modules / files | Notable concentration |
|---|---|---|---|
| `crates/ludex-core` | 8,759 | 10 modules | `archive.rs` 4,451 (single file) |
| `crates/ludex-server` | 66,000 (incl. 3.6k integration test) | 43 route files + 40 modules | `routes/games.rs` 3,334, `openapi.rs` 3,220, `models.rs` 2,685 |
| `apps/desktop/src-tauri` | 90,763 | 70 modules (flat namespace) | `install.rs` 7,995, `db.rs` 5,530, `commands.rs` 5,358 |
| `apps/web` | 95,672 (TS/TSX) | 35 screens, 127 components, 111 test files | `big-picture.tsx` 2,728, `types.ts` 4,169 |
| `e2e` | 279 | 5 specs | — |

Other numbers that matter for sequencing: **172 Tauri commands** (`apps/desktop/src-tauri/src/lib.rs:263-431`), **181 server routes** across 43 files, **401 exported API types** hand-maintained in `apps/web/src/lib/types.ts`, **796 desktop unit tests** that CI does not run, **596 web tests**, **250 server unit tests**, **32 server migrations** (SQLite + PostgreSQL mirror at parity), **20 desktop schema versions**, **14 locale catalogs** (46,923 lines).

Read that table as a shape: the *desktop* is the largest and least test-gated component, the *server* is wide but mechanically regular, the *web* is large but has real unit-test coverage, and *core* is small — which is the problem, see A1.

---

## 2. Verdict

The architecture is better than its line counts suggest. There is a real domain crate, a real API contract with a router↔OpenAPI equality test, a documented feature-parity matrix, a tested i18n gate, a measured extraction pipeline, and a scheduler (`pipeline::plan_next`) that is pure and unit-tested. Those are the things that usually rot first, and they haven't.

The fragility is concentrated in four places, in this order:

1. **No panic isolation + `panic = "abort"` + unlimited auto-resume + no attempt counter = a crash loop generator.** This is not stylistic. It is the one defect class that can brick a desktop install across launches.
2. **Desktop CI is `cargo check` only.** 91k lines and 796 unit tests gate nothing. The Windows-only surface (67 `cfg(windows)` sites across 15 modules) is compiled for the first time by the *edge bundle job*, i.e. after merge.
3. **The install pipeline is one 820-line function with four transport implementations, five HTTP clients and an error type that is `String`.** Every reliability defect in the last six releases (#203, #204, #205, #211, #216, #217, #221–#226, #231, #232) lives in or around it. It is not a module yet; it is an accumulation.
4. **Three independently-versioned artifacts share one contract with no negotiation** (server, desktop native, web bundle) while `types.ts` is hand-written and the desktop re-declares server DTOs locally (`install.rs:2397`).

Everything else — the god-object `AppState` (49 fields), `routes/games.rs`, the 9-provider React tree, the 5k-line mock API — is real debt but *stable* debt: it costs velocity, not correctness. Sequence it after 1–4.

---

## 3. Section A — Workspace and crate boundaries

### A1. `ludex-core` is a library of parsers, not a domain layer (medium risk, high leverage)

**Observation.** `ludex-core` owns the archive reader, filename parser, slug, ROM detection, redaction, gamepad math — and `models.rs` (663 lines), which is a *subset* of the API surface. It contains no rules. Every business invariant (version availability, `missing` vs `root_unavailable`, default version pinning, progress semantics) lives in the server; the desktop re-implements whatever it needs.

**Evidence.** `crates/ludex-core/src/lib.rs:1-11` states the intent ("everything the server and the desktop app share, and nothing that depends on a database, HTTP or an async runtime"). But `models.rs` is the only shared *domain* file and it is a DTO file. `crates/ludex-core/Cargo.toml` depends on `utoipa` — a documentation/codegen crate — which means the domain crate is already shaped by the API-doc requirement.

**Why it costs.** The contract has no single owner. `apps/web/src/lib/types.ts:1-4` is a hand-maintained mirror; `apps/desktop/src-tauri/src/install.rs:2388-2440` declares `GameDetail`/`VersionMetadata` locally with `#[serde(default)]` on every field so an older server cannot break an install; `crates/ludex-server/src/models.rs:1-6` says it "reuses the shared models" but re-declares ~30 row structs and several response DTOs. Three copies, one contract test — and the contract test (`crates/ludex-server/tests/openapi.rs`) only checks that the *server* document matches the *server* router.

**Recommendation.** Do **not** move business logic into core. Instead promote `ludex-core` to own the *wire* domain completely:
- Move every JSON-facing DTO currently in `crates/ludex-server/src/models.rs` (response types only, not `*Row`) into `ludex_core::models`, so `utoipa` schemas and TS generation have one source.
- Delete the desktop's shadow DTOs by importing the core types.
- Keep SQL rows in the server. Rows are a storage detail; DTOs are the contract.

This is mechanical, low-risk, and it is the prerequisite for the type generation in A2/B1.

### A2. Shared type generation exists for the server and stops there (high risk)

**Evidence.** `crates/ludex-server/src/openapi.rs:1-60` + `docs/OPENAPI.md` describe a generated OpenAPI 3.1 document with a router-equality test (`tests/openapi.rs`) and a live-mounting test. `docs/OPENAPI.md:57-58` then says: *"The client SDK types in `apps/web/src/lib/types.ts` mirror the same schemas"* — i.e. by hand. 401 exported types, 38 hand-written `USING_MOCKS ?` branches in `apps/web/src/lib/api.ts`, and a 4,155-line `mock.ts` that is a *third* implementation of the API.

**Why it costs.** Contract drift is invisible until runtime. Two live examples already in the tree:
- `archiveFormat` / `streamSupported` were added to version payloads in #226 and the desktop reads them "defensively" (`install.rs:2401-2410`) because it cannot trust the contract.
- `InstallJob.stream` / `.resumable` were added so the UI stops parsing English error text (`apps/web/src/lib/state/downloads.tsx:53-57` comment: "Use the persisted flags, never the English error message (#114)") — a flag added *because* the contract had no error taxonomy.

**Recommendation.** Generate `apps/web/src/lib/types.gen.ts` from `GET /api/openapi.json` with `openapi-typescript` in CI:
- committed output, `--check` job fails on diff (same pattern the repo already uses for `locales/en.json` and `themes.generated.css` — this team knows the pattern);
- hand-written file shrinks to aliases and UI-only types;
- the mock adapter keeps using the generated types, so it fails to compile on drift.

Do not generate a Rust HTTP client for the desktop (YAGNI, ~40 call sites); A1 covers the desktop instead.

### A3. The desktop links the entire server (medium risk, slow builds)

**Evidence.** `apps/desktop/src-tauri/Cargo.toml:34-35` depends on both crates; `ludex-server` is used for exactly three symbols: `Config::for_local` and `start` (`apps/desktop/src-tauri/src/server.rs:87,91`) and `ServerHandle` in state (`state.rs:317`). The dependency pulls `axum`, `sqlx` **with both sqlite and postgres features**, `utoipa`, `rust-embed`, `object_store`, `argon2`, the whole route tree — into a desktop binary whose release profile is `lto = true, codegen-units = 1, opt-level = "s"` (`Cargo.toml:96-99`).

**Why it costs.** Every server edit invalidates the desktop build (no shared target dir; `Cargo.toml:4` excludes the desktop from the workspace, and the comment at lines 6-7 explains it needs a different target directory). For a repo that ships 10+ patch releases per week, this is the single largest tax on iteration.

**Recommendation.** Introduce a `ludex-embed` crate (or a `local-mode` feature on `ludex-server`) that exposes only `start_local(games, data, media) -> { url, token, shutdown }`. The desktop depends on the thin facade, and the server's optional subsystems (`embed-web`, `object_store`, `oidc`, `virustotal`) sit behind features that local mode disables. Keep the desktop out of the root workspace (the comment's reason is real), but add a `cargo check` for `ludex-embed` in the main workspace job so the boundary is compiled on every PR.

Do this *after* Wave 2, when the local-mode contract is stable. Doing it now would churn while the pipeline is being split.

### A4. `bridge.rs` is the right seam and is bypassed 9 times (low risk)

**Evidence.** `apps/desktop/src-tauri/src/bridge.rs:1-4` — "Everything the desktop app needs from the shared crates goes through this module so that API adjustments stay in one place." Seven modules use it; nine import `ludex_core::` directly (`adopt.rs`, `launch.rs`, `updates.rs`, `install.rs`, `stream_install.rs`, `game_executables.rs`, `roms.rs`, `diagnostics.rs`, `test_support.rs`).

**Recommendation.** Fold into A1: once DTOs live in core, `bridge.rs` becomes the re-export surface, and the rule "no `ludex_core::` outside `bridge.rs`" is enforceable with a grep in CI (cheap, mechanical, no behavior change).

### A5. Desktop module naming collides in two domains (low risk, real confusion cost)

**Evidence.**
- Saves: `saves_detect.rs` (4,106), `saves_registry.rs` (2,786), `saves_sync.rs` (2,643), `saves_manifest.rs` (1,163) = 10.7k lines, four modules, one domain, no module directory.
- Manifests: `manifest.rs` = installed-file manifests + verify/repair; `file_manifest.rs` = per-file download transport; `install_manifests` (DB table) vs `install_file_manifests` vs `install_stream_manifests`. Three manifest concepts, one word.
- Updates: `updater.rs` (app self-update), `updates.rs` (game updates), `update_notify.rs` (OS banners), `install_health.rs` (binary/build mismatch). Four modules named "update".

**Recommendation.** Pure `git mv` lane: `saves/{detect,registry,sync,manifest}.rs`, `install/transport/files.rs` (from `file_manifest.rs`), `update/{app,notify,game}.rs`. Zero logic change, one PR, and it makes every later review cheaper. Do it in Wave 1 so the pipeline split in Wave 3 lands in sane names.

### A6. `AppState` is a 49-field god object in both processes (medium risk, high merge-conflict cost)

**Evidence.** `crates/ludex-server/src/state.rs:19-80` (49 public fields: config, pool, jwt, scan, http, igdb, providers, oidc, events, webhooks, watcher, 5 `AtomicBool` busy flags, storage, metrics, ingest, 4 rate limiters, 4 attempt counters, transfer registry, extracted cache…). `apps/desktop/src-tauri/src/state.rs:312-345` mirrors the pattern with 17 fields including `Mutex<Connection>`, `AsyncMutex<Option<ServerHandle>>`, jobs, processes, secrets, overlay, diagnostics, runtimes.

**Why it costs.** Every feature edits `state.rs` and `lib.rs`; those are the two files this project already flags as lane conflicts (routing/merge pain scales with field count, not with feature count). It also makes construction untestable: you cannot build an `AppState` without a Tauri `AppHandle` or a real config.

**Recommendation.** Group into subsystem structs held as `Arc<_>`: `Auth { jwt, limiter, login_guard }`, `Catalog { scan, providers, igdb, content_hashes }`, `Transfer { storage, download_limits, extracted, transfers }`, `Integrations { oidc, virustotal, webhooks }`, `Ops { metrics, watcher, busy flags }`. `AppState` becomes 5-6 fields. Call sites change mechanically (`state.auth.jwt`), no behavior change. Expect ~200 touch points server-side; do it as a single `cargo fix`-style lane with clippy as the safety net.

### A7. Duplicated logic between server and desktop that must stay duplicated, and the one place it already is (decision required)

**Evidence.** `crates/ludex-core/src/gamepad.rs:22` documents it explicitly: *"the same rules are re-implemented in `apps/web/src/lib/controller-profiles.ts` and the two implementations are kept in sync by tests on both sides."* Also `apps/web/src/lib/install-queue.ts:5-8` mirrors the desktop scheduler's ordering rule in TypeScript.

**Recommendation.** Keep both (a wasm build of the mapping code is not worth the toolchain), but make the sync machine-checked: one `fixtures/controller-profiles.json` consumed by the Rust test *and* the Vitest test. Same pattern for install queue ordering: a small table of `(phase, position) → expected order` shared as JSON. This converts "kept in sync by discipline" into "kept in sync by CI" for ~1 day of work.

---

## 4. Section B — Data layer

### B1. Two databases per install in local mode, with no ownership rule (medium risk)

**Observation.** Local mode runs *two* SQLite files: the embedded server's `ludex.db` (games, versions, progress, play sessions, 55 tables, sqlx migrations) and the desktop index `desktop.db` (installs, manifests, jobs, sessions, 22 tables, hand-rolled migrations). They overlap: games/versions appear in both (the desktop caches `GameDetail`/`VersionMetadata`), and playtime exists as `sessions` locally and `progress`/`play_sessions` on the server.

**Evidence.** `crates/ludex-server/src/db.rs:1206,1232` (sqlx migrate) vs `apps/desktop/src-tauri/src/db.rs:133,615-725` (`SCHEMA_V1..V20`, `user_version`); the desktop's `install_jobs`/`installed_games` vs the server's `games`/`game_versions`; `apps/desktop/src-tauri/src/launch.rs:1251,1442` posts playtime to `/api/games/{id}/playtime` while writing local `sessions`.

**Is that wrong?** No. It is correct *asymmetry*: the server owns the catalog (what exists, what is missing, who may see it) and the desktop owns *this machine's filesystem truth* (what is installed where, what bytes are on disk). Trying to unify them is a trap — one of them is a multi-user server, the other is a per-machine cache with absolute paths that must never leave the machine (`saves_registry.rs:5-6` is explicit about this).

**Recommendation.** Keep two schemas; write the rule down as an ADR ("catalog vs local truth") and enforce three invariants in code:
1. The desktop never writes catalog tables; it reads the API.
2. The server never stores absolute local paths (already true — verify with a test that greps migrations for `_path` columns on `devices`/`remote_installs`).
3. Playtime: the desktop is the *writer*, the server is the *ledger*, and the desktop's `sessions.pending` is the outbox. Formalize the outbox (see D5).

### B2. The desktop migration runner has no downgrade guard (medium risk, cheap fix)

**Evidence.** `apps/desktop/src-tauri/src/db.rs:615-725`. The runner reads `user_version`, applies `if version < N` blocks, then sets `user_version = SCHEMA_VERSION` (20). There is no checksum table, no `version > SCHEMA_VERSION` branch, and no backup before migrating.

**Why it costs.** A user who runs an `edge` build (version bumped automatically by `ci.yml:392-405`) and then goes back to stable opens a v20 database with a v19 binary. Every query still "works" against the newer schema, so nothing fails loudly — until a column the old code does not know is `NOT NULL` without default, or a rename lands. The failure mode is silent corruption, not an error.

**Recommendation.** Three lines of defence, ~1 day:
1. Refuse to open when `user_version > SCHEMA_VERSION`: return a typed error the UI renders as "This library was opened by a newer Ludex. Update, or restore a backup." (Never auto-downgrade.)
2. `VACUUM INTO desktop-backup-<version>.db` before applying any migration that bumps the version; keep the last two.
3. Number the desktop schema as files (`schema/0021_*.sql` + `include_str!`) instead of one 5,530-line Rust file with 20 `const &str` blocks. Mechanical, and it lets the migration test iterate the directory like the server does.

**Do not** migrate the desktop to sqlx. `rusqlite` with a held `Mutex<Connection>` is the right call for a single-process app and gives synchronous migrations and `VACUUM INTO` for free.

### B3. Multi-statement writes are not transactional (medium risk)

**Evidence.** 12 `.begin()` call sites across 66k server lines (`routes/games.rs` 3, `gamevault.rs` 2, `db.rs` 2, plus one each in `system.rs`, `metadata_providers.rs`, `invitations.rs`, `library_health.rs`, `emulators.rs`). The scanner inserts a game then upserts versions as separate statements (`scanner.rs:494,745`); `remove_game` deletes files then rows (`routes/games.rs:899-1030`); collection membership and bulk admin operations (`routes/admin_games.rs` 982 lines) are multi-statement.

**Why it costs.** On SQLite the window is small; on PostgreSQL with a network round-trip between statements, a mid-flight failure leaves a game row with no versions, or a version row pointing at a deleted file. The scanner will "fix" the first on the next scan and never fix the second.

**Recommendation.** Add `db::with_tx(pool, |tx| …)` used by: scan upsert (game+versions+root flags), game delete (rows only, files before), collection create+bind, bulk admin jobs, backup restore. That is ~8 call sites, mechanical, and it is exactly the kind of change the existing `tests/postgres.rs` can validate (it already runs against a real postgres:16).

### B4. Backup is SQLite-only and desktop has none (low-medium risk, documented)

**Evidence.** `crates/ludex-server/src/backup.rs:305-310` bails on PostgreSQL with a "use pg_dump" message; `docs/POSTGRES.md:227-235` documents it. The desktop has no index backup at all — the install manifests (`install_manifests`, `install_file_manifests`) are the only record linking files on disk to versions, and nothing snapshots them.

**Recommendation.** Server: keep the design (a SQLite snapshot with a manifest and SHA-256 is a good artifact); add a CI check that `ludex-backup` prints the pg_dump pointer instead of failing at the first file. Desktop: reuse the same three-line rule as B2 (2) — `VACUUM INTO` on startup at most once per day, keep two, ignore failures. ~4 hours and it converts an unrecoverable state into a recoverable one.

### B5. Dual-backend SQL discipline is real and should be preserved (no action)

`crates/ludex-server/src/db.rs:236` (`RowType` requiring `FromRow` for both `SqliteRow` and `PgRow`) plus `db.rs:475` (placeholder rewriting) plus only **2** `is_postgres()` branches outside `db.rs` is a genuinely good design: the dialect difference is confined to one file and one trait bound. Do not "simplify" this by dropping PostgreSQL, and do not replace the rewriting with a query builder. Add a lint instead: a test that fails when `$n` appears in a non-`db.rs` string literal, and one that fails when `?` appears inside a raw SQL literal passed to `sqlx::query` (bypassing the wrapper).

---

## 5. Section C — Desktop runtime architecture

This is where the risk is.

### C1. Crash loop: `panic = "abort"` + unlimited auto-resume + no attempt counter (critical)

**The chain, verified end to end:**
1. `apps/desktop/src-tauri/Cargo.toml:99` — `panic = "abort"` in the release profile. Any panic in any thread aborts the process.
2. `apps/desktop/src-tauri/src/diagnostics.rs:533-555` — the panic hook records a report, then calls the previous hook, which aborts.
3. `apps/desktop/src-tauri/src/db.rs:2200-2211` — `mark_interrupted_jobs` rewrites every non-terminal job to `phase = 'error'` with a fixed message at startup.
4. `apps/desktop/src-tauri/src/install.rs:2314-2380` — `auto_resume_on_startup` selects exactly those rows (`INTERRUPTED_MESSAGE` / `LEGACY_INTERRUPTED_MESSAGE`) and resumes them, unconditionally, every launch.
5. `apps/desktop/src-tauri/src/db.rs:166-180` — the `install_jobs` table has **no attempts / resume-count column**. Nothing remembers that a job has already failed this way.

**Consequence.** A job that deterministically panics (corrupt archive header, a slice index in the extractor, an arithmetic overflow in a size calculation) aborts the app on startup, the next launch marks it interrupted, resumes it, aborts again. The user sees "Ludex closes right after opening", with no way out through the UI because the crash happens before the window is interactive. Secondary variant without panics: a job whose URL is permanently dead but whose failure is classified as retryable is resumed forever, occupying a download slot and hiding the real error behind "Resuming…".

**Required before any refactor.** Three independent guards, any one of which breaks the loop:
- `install_jobs.resume_attempts INTEGER NOT NULL DEFAULT 0` and `last_resume_bytes INTEGER`; auto-resume refuses after 3 attempts that made no byte progress, leaving the job `paused` with an explicit "needs attention" message.
- `panic = "unwind"` in the desktop release profile, and every worker (`install::run`, `stream_install`, `runtime`, `saves_sync`) spawned under `catch_unwind`. A worker panic becomes a job error, not a process exit. Keep `abort` only in the server binary, where a panic really is fatal for the request.
- A startup "safe mode": if the app exited abnormally twice within 10 minutes, skip auto-resume for that launch and surface a banner. This is the only guard that survives a *new* panic site nobody anticipated.

**Acceptance gate.** A chaos test (D8) that kills the process mid-download and mid-extract 20 times must end with every job terminal or resuming successfully, and with a panic-injected job producing exactly one error row and a live window.

### C2. One 820-line function with four transports (high risk, high effort)

**Evidence.** `apps/desktop/src-tauri/src/install.rs:2513-3333` — `execute()`: resolve install root, compute slug/version paths, hash metadata lookup, local-library shortcut, file-manifest transport, per-entry stream install, classic resumable download, verification, extraction, fingerprinting, rename/copy into place, manifest recording. 47 control-flow statements and 36 `?` in one body. Around it: `manifest_install` (`install.rs:3699`), `stream_install::install` (called at `install.rs:5598`, implemented in `stream_install.rs:697-1180`), the classic loop inside `execute`, and `runtime/install.rs:257` for provider artifacts.

Five separate HTTP clients/loops with their own retry, stall and progress logic:
- `install.rs:4598` `HttpTransport` (classic)
- `install.rs:4117` `HttpFileTransport` (file manifests)
- `install.rs:4024` `HttpManifestTransport`
- `stream_install.rs:121` `HttpTransport` (per-entry, with its own `RetryTuning`, backoff and stall timeout at `stream_install.rs:51-95`)
- `remote_play.rs:597` `HttpTransport` (IGD/UPnP, unrelated but same name)

**Why it costs.** Each reliability fix in the changelog touched one or two of these and had to reason about the others. #211 ("honour If-Range, 1 MiB chunks"), #217 ("parallel jobs get their own N-aware budget"), #226 ("staging on the install volume") are all cross-cutting changes that had to be applied by hand in 3–4 places.

**Recommendation (Wave 3).** Split by *responsibility*, not by transport, into a directory:

```
install/
  job.rs         InstallJob, InstallPhase, JobEntry, persistence, events
  plan.rs        paths, space plan, staging policy (from execute's first 60 lines)
  scheduler.rs   dispatch, queue positions, slots (from install.rs:827-1440)
  transport/
    mod.rs       trait Transport { fetch(plan, sink, cancel) -> Artifact }
    classic.rs   resumable archive download
    stream.rs    per-entry ZIP (from stream_install.rs)
    files.rs     file manifest (from file_manifest.rs)
  verify.rs      SHA-256 gate (from archive_verify.rs) + sidecar
  commit.rs      rename/copy, staging cleanup, manifest recording
  extract.rs     bridge to ludex-core archive
```

and one shared `net/` module: `RetryPolicy` (stall timeout, backoff, attempt budget, retry-now signal), `ProgressSink` (bytes, speed, ETA — the web already has `lib/eta.ts` doing this; do not duplicate the smoothing), `CancelSet` (cancel + pause + retry-now as one value instead of three tokens passed around).

The existing `Failure` enum (`install.rs:541-556`) becomes the transport error type with a `retry: RetryClass` field (`Transient`, `Permanent`, `NeedsSpace`, `Cancelled`, `Paused`). That kills the string matching in C3.

**Effort:** 15–25 focused days. Do it as ~8 sequential PRs, each with the tests unchanged, on a lane branch, merged only when `install.rs` shrank and no test was rewritten to fit.

### C3. Error taxonomy is `String` on the desktop and codes on the server (high risk, cheap to start)

**Evidence.** 512 `Result<_, String>` signatures in `apps/desktop/src-tauri/src`; zero `thiserror`/`anyhow` in the desktop crate. Decision logic then parses the English text:
- `stream_install.rs:961-973` `is_permanent_stream_failure()` matches substrings `"Not enough free space"`, `"unsafe entry path"`, `"Could not create the staging folder"`, …
- `install.rs:5716,5743` classify disk-full by matching `"no space left"` / `"disk full"` / `"enospc"`.

Meanwhile the server has a clean taxonomy (`crates/ludex-server/src/error.rs:1-120`: `ApiError { status, code, message, retry_after }`, codes like `invalid_credentials`, `confirmation_required`, and a stable `{"error":{code,message}}` envelope that the web already consumes at `apps/web/src/lib/api.ts:227-231`).

**Why it costs.** Any copy edit, any i18n pass on error strings, any platform whose OS error text differs (`fr_FR` Windows says "Espace insuffisant") silently changes a retry decision into a give-up or a give-up into an infinite retry (see C1).

**Recommendation (Wave 0, small):** introduce `enum InstallError { Cancelled, Paused, NeedsSpace { required, available, path }, Transient { message, status }, Permanent { message, status }, Corrupt { expected, actual } }` in the install module and convert the *decision* sites only (4 functions: `is_permanent_stream_failure`, the disk-full classifier, the fallback decision, the UI "Resume vs Restart" flag). `impl From<InstallError> for String` keeps all 512 signatures compiling. Full conversion of the 512 sites is *not* worth it; the boundary is what matters.

### C4. Local-mode resume uses a stale loopback URL (high risk, small fix)

**Evidence.** `apps/desktop/src-tauri/src/settings.rs:697` — `server_port: 0` by default, i.e. the OS picks a port at every launch. `apps/desktop/src-tauri/src/server.rs:88-90` honours a pinned port only when `server_port != 0`, and **never writes the chosen port back**. Install jobs persist `server_url` with the port (`install.rs:482`, `InstallParams::from_record`) and resume with it verbatim (`install.rs:1493-1522`). `is_local_server()` (`install.rs:4417-4427`) then classifies a stale loopback URL as "local" and the local-file shortcut issues an HTTP request to the dead port.

**Consequence.** Every interrupted *local-mode* install fails at the first request after a restart with a network error, and — combined with C1 — is retried on every subsequent launch.

**Fix (Wave 0, ~half a day, two options):**
- (a) pin the port: on first successful start, persist the bound port into `server_port`; on a full restart with the same port, resume works as-is. Risk: collisions; handle by falling back to 0 and rewriting.
- (b) rewrite at resume: if `params.server_url` is loopback and `settings.mode == Local`, replace it with the current `settings.server_url` whenever the URL differs. This is strictly more robust and 15 lines its b(2) is a prerequisite for any "resume everywhere" confidence.

Recommend **both**: (b) makes resume correct, (a) makes the port stable for humans and for the Sunshine/Moonlight handoff.

### C5. State ownership across the API boundary is implicit (medium risk)

**Observation.** The UI keeps a *copy* of install state: `apps/web/src/state/downloads.tsx` (662 lines) holds an optimistic queue, mirrors the desktop scheduler (`lib/install-queue.ts`), and invents phases (`"interrupted"` at `downloads.tsx:42` is not a desktop phase — `InstallPhase` has `error`, `cancelled`, `paused`). Events arrive per job (`install://progress`, `install://jobs`, `install://done`, `install://error`, plus `install://manifest-progress`, `move-progress`, `verify-progress`, `repair-progress`), so the UI must reconcile an event stream with a poll (`list_install_jobs`).

**Event protocol inventory.** No registry exists; names are string literals on both sides: `install://*` (8), `overlay://*` (4), `ui://*` (2), `cloud-save://*` (3), `update://*` (3), `runtime://progress`, `game://exited`, `game://focus`, `local-server://status`, `save-manifest://updated`. The web subscribes in `apps/web/src/lib/native.ts:791-1489`. Nothing checks that both sides agree.

**Recommendation.** Two small moves, both mechanical:
1. One Rust module (`events.rs`) exporting `pub const INSTALL_PROGRESS: &str = "install://progress";` etc., re-exported through `bridge.rs`; the TypeScript side gets a generated constants file from the same list (a 20-line build script). This makes a typo a compile error instead of a silent no-op.
2. Make `GET /api/...`-style truth explicit: *desktop is authoritative, events are hints*. The UI should treat an event as "refetch this job id", not as the state itself. `downloads.tsx` already mostly does this; codify it and delete the duplicated phase enum.

### C6. Startup sequence is a 45-line sequential `await` chain with no failure policy (medium risk)

**Evidence.** `apps/desktop/src-tauri/src/lib.rs:216-260`: `install_health::startup_check` → `server::autostart` → `install::auto_resume_on_startup` → `install::rebuild_space_ledger` → `install::sweep_stale_staging` → `launch::sync_pending` → 5 worker starts → `saves_manifest::auto_update` → 2 updater checks. Each step's failure is either `let _ =` (ignored) or logged; ordering is load-bearing and documented only in comments (`lib.rs:228-241` explains why the sweep must run after resume). `rebuild_space_ledger` runs after resume but `dispatch` (called by resume) already consults the ledger.

**Recommendation.** Extract `startup::run(app)` returning `Vec<StartupIssue>` and an explicit ordered list of `StartupStep { name, critical: bool, run }`, with the ledger rebuilt *before* resume. Surface issues in the existing diagnostics panel. This is a refactor of ~120 lines that makes C1's safe mode trivial and gives the UI something honest to show when the embedded server fails to boot.

### C7. Fullscreen/console/window policy is spread across four layers (medium risk — the "fullscreen" item)

**Evidence.** Native: `console.rs:39-102` (`fullscreen` flag from argv), `console.rs:232-260` (`apply_window`: undecorated + `set_fullscreen`), `chrome.rs:1-79` (native vs custom title bar), `lib.rs:137-162` (close-request policy: tray vs overlay vs console), `window_focus.rs` (620 lines, focus stealing/restoring). Web: `apps/web/src/lib/window-chrome.ts` + `components/shell/window-controls.tsx:41-108` (its own `fullscreen` state, polled), `components/shell/app-shell.tsx:359` (topbar hidden when fullscreen), `console` entry redirect (`app.tsx:229-248`).

**Why it costs.** "Fullscreen" means three different things (Tauri window is fullscreen; the game is exclusive-fullscreen and hides our overlay; the console shell is active) and each layer infers it independently. That is why #227 (promote `--console` on a second instance) and #228 (overlay diagnostics) both needed bespoke fixes, and why the overlay panel has to *explain* fullscreen failure modes to the user (`components/settings/overlay-panel.tsx:472,584-586`).

**Recommendation (Wave 0, ~3–4 days).** One `WindowPolicy` resolved in Rust from `{argv, session mode, settings}` producing `{ decorations, fullscreen, tray, close_action, overlay_host }`, applied in exactly one place, and emitted to the web as a single `window://state` payload on every transition. The web then *reads* policy instead of inferring it (delete the polling in `window-controls.tsx`). Add a truth-table unit test (console × windowed × close × overlay) — this is exactly the kind of thing the repo already tests well (`console.rs:400-490`).

---

## 6. Section D — Server runtime

### D1. `routes/games.rs` (3,334 lines) hides five transfer implementations (medium risk)

**Evidence.** In one file: `download` (`:1532`), `download_local` (`:1653`), `download_from_storage` (`:2004`), `stream_version` (`:2150`), `stream_manifest` (`:2342`), `stream_entry` (`:2452`), plus `download_ticket` (`:1372`), `local_version_path` (`:1964`), `stage_object` (`:2123`). Range/If-Range handling, ticket auth, per-user limits (`download_limits.rs`) and SHA-256 headers (`content_hash.rs`) are applied per path.

**Recommendation.** Extract `transfer/` with a `TransferSource` enum (`LocalFile | ObjectStore | ZipStream`), one `serve_range()` used by all of them, and thin handlers that resolve the source and delegate. Keep `stream_manifest`/`stream_entry` in `transfer/zip.rs` (they are the one genuinely different protocol). Target: `routes/games.rs` < 1,200 lines, with the four download handlers reduced to <60 lines each. This is also where the last four "download" defects (#211, #217, #226, #231) would have been fixed once.

### D2. Background workers are spawned ad hoc with `AtomicBool` busy flags (low risk, known)

`lib.rs:372-394` spawns 9 workers; five subsystems serialize with `Arc<AtomicBool>` (`state.rs:48-58`). It works (single process, cooperative) but there is no observability beyond the flag and no cancellation on shutdown beyond `watch::Sender<bool>` (`lib.rs:411-426`). Recommendation: leave it. It is correct for a single-node server and rewriting it into a job system is exactly the kind of complexity that does not earn its keep (see §11).

### D3. No push channel to clients (low risk, decision needed)

There is an in-process event bus (`events.rs:1-6`) used for audit + webhooks only; no SSE/WebSocket anywhere. The web polls via TanStack Query with `staleTime: 20_000` (`app.tsx:63-71`); the phone↔PC install flow polls (`remote_installs.rs:1-20`, desktop heartbeat every 25 s, `remote_install.rs:25-27`).

**Assessment.** For a homelab-scale product this is the right call: polling is simple, cacheable, and survives proxies. Recommendation: keep polling, but *document the intervals as a contract* (they already are, in code comments) and make the remote-install claim idempotent under a 2× slower poll. Do not add SSE for its own sake; the only feature that would justify it is live progress on the phone, and that can be a 3-second poll on one endpoint.

---

## 7. Section E — Frontend architecture

### E1. Nine nested providers around the entire router (low risk, medium readability cost)

**Evidence.** `apps/web/src/app.tsx:256-265`: `QueryClientProvider > SettingsProvider > ToastProvider > AuthProvider > PreferencesProvider > DownloadsProvider > UploadProvider > I18nProvider > UpdateProvider > ConfigSyncDefaults > HashRouter`. Every provider re-renders its subtree when its value changes; the ordering matters (Auth above Preferences above I18n) and is only explained by inline comments.

**Recommendation.** Not urgent, and *do not* rewrite into a global store. Two safe reductions:
- Merge `I18n` into `Preferences` (locale is already an account preference; `ConfigSyncDefaults` at `app.tsx:189-221` exists only because they are separate).
- Move `DownloadsProvider`'s server-derived state into TanStack Query (`state/downloads.tsx` already uses `useQuery` + `useQueryClient`), leaving a thin context for optimistic queue actions only.

Target: 5 providers, no behavior change, ~2 days, covered by existing screens.

### E2. The mock API is a third implementation of the contract (medium risk)

**Evidence.** `apps/web/src/lib/mock.ts` (4,155) + `mock-install.ts` (905) + `data/demo.ts` (853) = 5,913 lines, selected by 38 inline `USING_MOCKS ?` branches in `api.ts` (`api.ts:263-…`) and `flags.ts:5-7`.

**Recommendation.** Invert the branch: build one `ApiClient` interface, then `export const api: ApiClient = USING_MOCKS ? mockApi : httpApi;`. That removes 38 decisions from the hot file, lets Vite tree-shake `mock.ts` and `demo.ts` out of production bundles entirely (today it is only *dead* at runtime, not at build time, for the `demo.ts` string data), and makes the mock implement the same interface so drift is a type error. Keep the mock — it is a genuine asset for demos and for `vitest`. Do *not* replace it with MSW (bigger change, no additional safety here).

### E3. `big-picture.tsx` (2,728 lines) is a screen, a shell, and an interaction model (medium risk)

**Evidence.** `screens/big-picture.tsx` 2,728; `components/big-picture/` holds `controller.ts` (584), `motion.ts` (574), plus layout/sound modules. The design work here is genuinely good (documented motion vocabulary in `docs/big-picture/31-bp-motion.md`, interruptible springs, hash-guarded WAVs). The *structure* is the problem: one component owns rail navigation, focus management, panels, and the launch handoff.

**Recommendation.** Mechanical split into `console/{shell,rail,tile,panel,handoff}.tsx` + `useConsoleFocus()` hook; keep `motion.ts`, `controller.ts`, sound specs untouched. This is the single highest-value frontend split (it is also the surface with the most user-visible bugs in the changelog: #218, #220, #227, #229).

### E4. Design system is healthy; leave it (no action)

`components/ui` is 1,100 lines across 7 files (controls, dialog, popover, select, toast, tooltip, elevation-confirm) — correctly small. Tokens are generated (`styles/themes.generated.css`, 182 CSS vars) and **tested** (`styles/tokens.test.ts`, `no-hardcoded-colors.test.ts`, `console-material.test.ts`, `page-scroll.test.ts`), i18n is gettext-style with a parity/coverage/plural/RTL gate and a pseudo-locale (`lib/i18n.tsx:1-30`, `apps/web/scripts/i18n.mjs`, 14 catalogs). This is above the norm for a project this age; touching it would be pure churn.

The one gap: `lib/i18n.tsx` (701) and `lib/pseudo.ts` are correct but the *copy* lives inside TSX as `t("…")` string literals — that is fine and intentional. No action.

### E5. Screen-level data fetching has no key registry (low risk, cheap)

170 `useQuery`/`useMutation` calls; query keys are string tuples written ad hoc (`["game", …]`, `["installed"]`, `["games"]`). Invalidation is by literal string (`apps/web/src/state/downloads.tsx` and `components/game/*` both invalidate `["installed"]`). One typo = a stale screen.

**Recommendation.** One `lib/query-keys.ts` with `keys.game(id)`, `keys.installed()`, etc. Mechanical, ~1 day, no behavior change, and it makes the "which screens go stale" question answerable.

---

## 8. Section F — Release / CI / security

### F1. Desktop tests do not run in CI (critical, cheapest fix in this document)

**Evidence.** `ci.yml:146-179` — `Desktop (cargo check)` runs `cargo check`. `ci.yml:183-204` — `Desktop (macOS)` runs `cargo check`. `cargo test --workspace --all-targets` (`ci.yml:40`) covers only `ludex-core` + `ludex-server`, because the desktop is excluded from the workspace (`Cargo.toml:4`). There are **796** `#[test]`/`#[tokio::test]` in `apps/desktop/src-tauri/src`.

**Why it costs.** The largest, riskiest component in the repo has zero test gating on merge. It also means the tests' *compile* errors are the only thing CI catches, which is why so much logic was written test-first but is verified only by hand before a release.

**Recommendation (Wave 0, ~2 hours).** Add to `desktop-check`: `cargo test --lib --offline` (the crate has no integration test dir; `test_support.rs` provides the HTTP fakes at `test_support.rs:306-379`). Keep it on `ubuntu-24.04`. Expect platform-gated tests to skip; that is fine. Then add `cargo clippy --all-targets -- -D warnings` for the desktop in the same job — the workspace clippy at `ci.yml:37` does not see it either.

### F2. Windows is compiled for the first time after merge (high risk)

**Evidence.** `ci.yml:339-352` — the edge job builds on `windows-latest` for pushes to main (`edge` prerelease), and `release.yml:180-215` on tag. There is no Windows `cargo check` on pull requests. 67 `cfg(windows)` sites across 15 modules (`elevate.rs` 1,700, `overlay.rs` 2,298, `paths.rs`, `saves_registry.rs`, `window_focus.rs`, `steam.rs`, `chrome.rs`…) never see a compiler until then.

**Recommendation.** Add a `windows-latest` matrix leg to `desktop-check` running `cargo check --target x86_64-pc-windows-msvc` (build-only, ~10 min with the existing rust-cache; no bundling). Gate it on `pull_request` paths touching `apps/desktop/**`. This is the difference between "release build fails" and "PR is red".

### F3. No dependency/license/security automation (medium risk)

**Evidence.** No `deny.toml`, no `cargo-deny`/`cargo-audit`/`cargo-licenses` job anywhere in `.github/workflows/`; no Dependabot config visible; `SECURITY.md` documents the disclosure process (good) but not dependency policy. The project is AGPL-3.0-or-later with a mixed dependency tree (`unrar` — non-free-ish, worth a licensing look; `sevenz-rust2`, `zip`, `gilrs`, `object_store`).

**Recommendation.** One `cargo-deny` job (bans, advisories, licenses, sources) with an allow-list matching the AGPL choice, plus `pnpm audit --prod` as advisory. Do it before the next release, not after.

### F4. Release pipeline is unusually mature; two gaps (low risk)

**Strengths to preserve:** E2E gate before `create-release` (`release.yml:14-67`), per-platform desktop matrix with Linux/macOS/Windows, updater key presence check (`release.yml:246-258`), Windows installer payload verification against the stamped commit (`release.yml:326-340`, `scripts/verify-windows-install.ps1`), `includeUpdaterJson` merging, Docker build-only in CI and push on tag, edge channel with auto-bumped pre-release version (`ci.yml:392-405`).

**Gaps:**
1. `macOS` bundles are ad-hoc signed when no certificate secret exists (`release.yml:262-273`) — Gatekeeper will warn. Acceptable, but the *updater* on macOS with an unsigned app is a support burden; document it as a known limitation in `RELEASING.md` rather than letting users discover it.
2. The desktop release has no post-install smoke on Windows/Linux (the edge job runs `verify-windows-install.ps1` only for the payload hash; nothing launches the app). The `desktop-smoke` job (`ci.yml:276`) is `continue-on-error: true`, so it can rot silently. Recommendation: keep it advisory but alert on 3 consecutive failures; a permanently-red advisory job is worse than none.

### F5. Secret handling and crypto are sound (no action, one note)

Argon2id, HS256 with a generated 0600 secret (`lib.rs:508-547`), AES-GCM at rest for provider secrets/OIDC/VirusTotal keys, `#![forbid(unsafe_code)]` in core, and a documented "never commit secrets" discipline. Note: `ludex-server/src/secrets.rs` and the desktop's `secrets.rs` (OS keychain + SQLite fallback) are two unrelated modules with the same name — rename the desktop one to `token_store.rs` during the A5 lane.

---

## 9. Top 10 architectural decisions

Each is a *decision to take*, with a recommendation. Ordering is by leverage, not urgency (urgency is §10).

| # | Decision | Recommendation | Why it is architectural |
|---|---|---|---|
| 1 | **Is the desktop allowed to crash?** | No. `panic = "unwind"` + worker `catch_unwind` + job-level attempt budget + safe mode on repeated abnormal exits. | Changes the failure model of the largest component. Everything else in the runtime assumes "a job fails, the app lives". |
| 2 | **Is local mode two databases or one?** | Two, with an explicit ADR: server = catalog, desktop = local truth. Enforce with three invariants (B1). | Prevents a multi-month unification project that would buy nothing. |
| 3 | **Who owns the wire contract?** | `ludex-core::models` owns DTOs; OpenAPI is generated; `types.gen.ts` is generated and CI-diffed; hand-written types shrink to aliases. | One source → three consumers. Today there are three sources and one consumer test. |
| 4 | **Do we keep the desktop↔server coupling as-is?** | Keep the dependency, but behind a `ludex-embed` facade with feature-gated subsystems. Do not fork the server for local mode. | Determines build times and whether local mode can diverge. |
| 5 | **What is the install pipeline's unit of composition?** | A `Transport` trait + one `net` retry/progress/cancel core; `execute()` disappears. | The pipeline is the product's core loop; its shape dictates how many places a reliability fix touches. |
| 6 | **How do desktop errors cross the Tauri boundary?** | Typed `InstallError`/`LaunchError` with `code` + `retry` class; `String` only at serialization. Stop parsing messages. | Retry/fallback correctness currently depends on English copy. |
| 7 | **Is the UI state authoritative or derived?** | Desktop is authoritative; native events are invalidation hints; the web keeps no phase enum of its own. | Removes a class of "Downloads shows something the desktop doesn't agree with" bugs. |
| 8 | **Push or poll?** | Poll. Document intervals; keep the server event bus for audit/webhooks only. | Adding SSE here would be complexity that does not earn its keep at this scale. |
| 9 | **Do we keep the dual Rust/TS implementations (gamepad, queue order)?** | Keep, but bind them with shared JSON fixtures in both test suites. | Cheap insurance; a wasm build is not worth the toolchain. |
| 10 | **Is the desktop in the workspace?** | No (keep the exclusion), but gate it in CI with tests + clippy + a Windows check. | The exclusion is justified by target dir and platform deps; the *consequence* (no gating) is not. |

---

## 10. Refactor program

Effort is in focused engineer-days for one senior dev with agent assistance (this repo's actual mode of work), excluding review latency. Every wave lands on `main` behind green CI; no wave may contain both a move and a behavior change.

### Wave 0 — Stop the bleeding (4–6 d) · no dependencies · ship immediately

| Item | Files | Gate |
|---|---|---|
| 0.1 Desktop tests + clippy in CI | `ci.yml:146-179` | All 796 tests run and pass; clippy `-D warnings` |
| 0.2 Windows `cargo check` on PR | `ci.yml` new leg | Compiles on `windows-latest` for desktop changes |
| 0.3 Resume attempt budget + safe mode | `db.rs:166` (new column, schema v21), `install.rs:2314`, `lib.rs:216` | Chaos test (§12) passes; injected panic → one error row, app alive |
| 0.4 `panic = "unwind"` + worker isolation | `Cargo.toml:99`, `install.rs:1942`, `stream_install`, `runtime`, `saves_sync` | Panic in a worker does not exit the process |
| 0.5 Local-mode URL refresh at resume | `install.rs:1493`, `settings.rs:697`, `server.rs:88` | Restart with 10 interrupted local jobs → all resume, none hit a dead port |
| 0.6 `InstallError` at the 4 decision sites | `install.rs:541`, `stream_install.rs:961` | Retry decisions no longer read English text |
| 0.7 `WindowPolicy` single source | `console.rs`, `chrome.rs`, `lib.rs:137`, `window-chrome.ts` | Truth-table test; overlay/console matrix manual check |

Why this order: 0.3 and 0.5 are the two bugs that make everything else look unreliable; 0.1/0.2 are the safety net that makes the rest of the program safe to attempt. If only one wave ships this quarter, ship this one.

### Wave 1 — Contract and hygiene (6–9 d) · after 0.4 (unwind makes moves safe)

1.1 Move response DTOs to `ludex_core::models`; delete desktop shadow DTOs (`install.rs:2388-2440`) — 3 d.
1.2 Generate `types.gen.ts` from OpenAPI + `--check` CI job; shrink `types.ts` — 3 d.
1.3 Module renames via `git mv`: `saves/`, `update/`, `install/transport/files.rs`, desktop `secrets.rs` → `token_store.rs` — 1 d.
1.4 `bridge.rs` becomes the only `ludex_core::` path; add the grep guard — 0.5 d.
1.5 Shared JSON fixtures for controller profiles and queue ordering (A7) — 1 d.

Gate: no file content changes beyond moves/renames in 1.3; `types.gen.ts` diff job green; all tests untouched and passing.

### Wave 2 — Data safety (6–10 d) · parallel to Wave 1 (different files)

2.1 Desktop downgrade guard + pre-migration backup (`db.rs:615-725`) — 1 d.
2.2 Desktop schema as numbered `include_str!` files — 2 d.
2.3 `db::with_tx` + 8 transactional call sites server-side — 3 d, validated by `tests/postgres.rs`.
2.4 Playtime outbox formalized (desktop `sessions` → server `progress`), with a bounded retry worker like `saves_sync` already has — 2 d.
2.5 `ludex-backup` PostgreSQL message test — 0.5 d.

Gate: kill-the-app-mid-migration test restores from the pre-migration snapshot; postgres tests green; no query changes.

### Wave 3 — Install pipeline decomposition (15–25 d) · after 0.3–0.6, 1.3

Strictly sequential PRs, each shippable:

3.1 Extract `install/job.rs` (job struct, phases, persistence, events) — 3 d.
3.2 Extract `install/plan.rs` + `install/scheduler.rs` from `dispatch`/`resume`/`reorder` — 4 d. *(Touches the part most likely to regress: keep `pipeline.rs` tests as the oracle.)*
3.3 Introduce `net::{RetryPolicy, ProgressSink, CancelSet}` and port the classic loop onto it — 4 d.
3.4 Port the per-entry stream loop (`stream_install.rs:697-1180`) onto `net` — 4 d.
3.5 Port the file-manifest loop (`install.rs:3699`+) onto `net` — 3 d.
3.6 Extract `install/verify.rs` + `install/commit.rs`; `execute()` ceases to exist — 4 d.
3.7 Optional: port `runtime/install.rs:257` onto `net` — 2 d.

Gate per PR: `install.rs` (or its successor) strictly smaller; **no test rewritten**; chaos test green; a 10 GB install produces byte-identical output before/after (compare manifest SHA-256 of the installed tree).

### Wave 4 — Server decomposition (12–20 d) · parallel to Wave 3 (different crate)

4.1 `AppState` → subsystem `Arc`s (A6) — 5 d.
4.2 `transfer/` module; four download handlers reduced to delegators (D1) — 6 d.
4.3 Split `routes/games.rs` by resource — 3 d.
4.4 Split `models.rs` (DTOs moved in 1.1; rows vs queries remain) — 2 d.
4.5 `openapi.rs` route table is per-module (`RouteDoc` beside each router) — 3 d, optional.

Gate: `openapi.json` byte-identical before/after (this is a superb refactor oracle and it already exists); `tests/api.rs` untouched and green.

### Wave 5 — Frontend structure (12–18 d) · after 1.2 and 0.7

5.1 `api` adapter chosen once; delete 38 branches (E2) — 2 d.
5.2 `lib/query-keys.ts` + mechanical adoption (E5) — 2 d.
5.3 Merge `I18n` into `Preferences`; slim Downloads context (E1) — 3 d.
5.4 Split `big-picture.tsx` into `console/` (E3) — 6 d.
5.5 One `useInstallAction()` hook shared by Downloads, Game detail, Console — 3 d.
5.6 Extract the demo/mock data behind a dynamic import so it leaves the production bundle — 1 d.

Gate: bundle size not increased; every screen's e2e/unit behavior unchanged; `tsc --noEmit` clean.

### Wave 6 — Platform & ecosystem (8–12 d) · optional, after Waves 2–4

6.1 `ludex-embed` facade + feature gating (A3) — 4 d.
6.2 `cargo-deny` + advisory job (F3) — 1 d.
6.3 Contract negotiation: `X-Ludex-Contract` in `/api/health` + desktop warning on mismatch — 2 d.
6.4 Docs: ADRs for decisions 1–6, and a `docs/ARCHITECTURE.md` refresh that finally describes the *event* protocol and the two-database model — 2 d.

### Dependency graph

```
0.1 0.2 ──────────────┐ (safety net for everything)
0.3 0.4 0.5 0.6 0.7 ──┼──> 1.x ──> 2.x (data, parallel)
                      │        └──> 3.1..3.7 ──> 6.1
                      └──────────> 4.x ──> 6.3
                               1.2 ──> 5.x
```

Calendar: single-track ≈ 9–13 weeks. With two lanes (desktop / server+web) and the existing worktree workflow, ≈ 6–7 weeks, provided the shared files (`routes/mod.rs`, `lib.rs`, `state.rs`, `models.rs`, `lib/types.ts`, `CHANGELOG.md`) stay serialized as they already are.

---

## 11. How to avoid a big-bang rewrite

**Rules that fit this repo's actual practice:**
1. **Move before change.** Every `git mv`/extract PR must have an empty diff in behavior; the test suite is not edited in a move PR. This is verifiable: `git diff -M --stat` should show renames and line counts, not logic.
2. **Shim, then migrate, then delete.** New module exports the old API (`pub use install::InstallJob;`), callers migrate file-by-file, the old path is deleted in a third PR. Never both at once.
3. **One lane per wave, merged sequentially.** The repo already uses `/root/.local/share/opencode/worktree/…/lane-*`; keep the discipline that shared files (`state.rs`, `lib.rs`, `routes/mod.rs`, `CHANGELOG.md`, `lib/types.ts`) are touched by one lane at a time.
4. **Refactor oracles beat review.** `openapi.json` equality (4.x), the installed-tree manifest hash (3.x), `themes.generated.css` + `locales/en.json` drift checks (1.2/5.x), and the existing `pipeline.rs` unit tests (3.2) are all mechanical truth. Use them instead of asking a reviewer to eyeball 3,000 lines.
5. **Feature-freeze the pipeline while 3.x runs.** No new install behavior in the same weeks; bugfixes land as small patches on the old code and get re-applied after the split. (This is the only way an 820-line function gets smaller.)

---

## 12. How to measure success

**Gates (must be green, CI-enforced):**
- G0: desktop `cargo test` + clippy + Windows `cargo check` on every PR.
- G1: chaos gate — a scripted harness kills the desktop mid-download/mid-extract/mid-verify 20× (and once with `panic!` injected into a transport): 100% of jobs end terminal or resume; zero orphan staging folders; zero duplicate installs; the window is always alive at the end. This is the single most valuable new test in the program and it must exist before Wave 3.
- G2: `types.gen.ts` / `openapi.json` / `themes.generated.css` / `locales/en.json` diff jobs.
- G3: local-mode resume across a port change (0.5) stays green.
- G4: per-wave oracles above (manifest hash, openapi equality).

**Trends to track (not gates):**
- `install.rs` LOC (target < 1,500 by end of Wave 3) and file count under `install/`.
- Number of files a "download" bugfix touches (today 3–4; target 1–2).
- CI wall-clock (must not exceed ~35 min; the desktop test leg adds ~6–10 min).
- Crash-free launch rate from the local diagnostics store (`diagnostics.rs`, already records panics — it just never aggregates; a `diagnostics_status` count by kind is enough).
- E2E count and duration (279 lines today; target one spec per critical flow: install, launch, resume, console, overlay, admin restore).
- Mean time from issue to patch release (currently ~1 day — do not regress this; it is the project's main quality signal).

---

## 13. What explicitly not to refactor

1. **i18n.** Gettext-style ids, plural categories, coverage gate, pseudo-locale, RTL coherence. It is better than most commercial products. Touch nothing.
2. **Theme/token system.** Generated CSS vars + 4 token tests including a no-hardcoded-colors lint. Same verdict.
3. **`pipeline::plan_next`.** Pure, injected capacity provider, tested. It is the model the rest of the runtime should copy, not something to "improve".
4. **`space.rs` ledger.** In-memory, rebuilt at startup, per-volume reservations with a safety margin. Correct by construction for a single process.
5. **`ludex-core::archive`.** Freshly parallelized and *measured* (3.8× ZIP, 4.8× 7z) with a documented rejection of zlib-ng. Do not touch for at least two releases.
6. **The desktop's `rusqlite` + held `Mutex<Connection>`.** Single process, synchronous migrations, `VACUUM INTO`. Moving to sqlx would be a net loss.
7. **The two-database model.** See decision 2. Unifying it would be a multi-month project that violates the local-truth invariant.
8. **`ludex-backup` format.** Versioned manifest + per-file SHA-256 + VACUUM INTO snapshot. Extend it; do not redesign it.
9. **The dual Rust/TS controller mapping.** Bind with fixtures (1.5), don't unify.
10. **The 172 Tauri commands.** The count is fine — it is a flat, discoverable IPC surface with typed wrappers in `lib/native.ts`. Splitting them into "domains" would add indirection without reducing surface.

---

## 14. Risk register (residual, after the plan)

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Wave 3 regresses a transport edge case (If-Range, resume, fallback) | Medium | High | Chaos gate G1 + installed-tree hash oracle + no-test-rewrite rule; keep `stream_install` behind a setting until the split proves clean |
| A new panic site appears in code written after 0.4 | Medium | High | `catch_unwind` at worker boundary + safe mode (0.3/0.4) survive *unknown* panic sites, which no audit can enumerate |
| `ludex-embed` (6.1) re-couples the crates | Low | Medium | Do it last; feature-gate by subsystem; verify desktop build time and binary size before/after |
| Generated types break hand-written ergonomics (`string` vs union literals) | Medium | Low | Map OpenAPI enums to TS unions explicitly in the codegen config; keep a thin `types.ts` for UI-only shapes |
| Concurrent lanes collide on `state.rs`/`lib.rs` again | High | Medium | Wave 4.1 (subsystem split) is the structural fix; until then serialize as today |
| Desktop tests are flaky on CI (fs, timing, ports) | Medium | Low | They already use `memory_database()` and injected transports; quarantine a flaky test explicitly rather than `continue-on-error` on the whole leg |

---

## 15. Evidence index (selected)

- Workspace: `Cargo.toml:1-5`; desktop exclusion `Cargo.toml:4`; desktop release profile `apps/desktop/src-tauri/Cargo.toml:96-99`; server dep `apps/desktop/src-tauri/Cargo.toml:34-35`.
- Embedded server: `apps/desktop/src-tauri/src/server.rs:87-91`, `state.rs:317`, `crates/ludex-server/src/lib.rs:136-155`.
- Shadow DTOs: `apps/desktop/src-tauri/src/install.rs:2388-2440`.
- Install pipeline: `install.rs:541` (`Failure`), `:827` (`dispatch`), `:1493` (`entry_from_persisted`), `:1942-1952` (worker), `:2314` (auto-resume), `:2513-3333` (`execute`), `:3699` (`manifest_install`), `:4024/:4117/:4598` (transports), `stream_install.rs:121,697,961`, `runtime/install.rs:257`, `remote_play.rs:597`.
- Crash loop inputs: `apps/desktop/src-tauri/Cargo.toml:99`; `diagnostics.rs:533`; `db.rs:166-180` (no attempts column), `:2200` (`mark_interrupted_jobs`); `install.rs:2314-2380`.
- Local-mode URL: `settings.rs:697`; `server.rs:88-90`; `install.rs:482,571-577,1493-1522,4417-4427`.
- Startup: `lib.rs:174` (panic hook), `:201-260` (sequence), `:263-431` (172 handlers).
- Window/console: `console.rs:39-102,232-275`; `chrome.rs:1-79`; `lib.rs:137-162`; `window_focus.rs`; web `window-chrome.ts`, `components/shell/window-controls.tsx:41-108`, `app.tsx:229-248`.
- Server data layer: `db.rs:236` (`RowType`), `:475` (rewrite), `:1174-1240` (migrations), `error.rs:1-120`; `backup.rs:305-310`; `docs/POSTGRES.md:227-235`.
- Route surface: `routes/mod.rs:49-110`; `routes/games.rs:1372,1532,1653,2004,2150,2342,2452`; `models.rs` (2,685).
- Contract: `openapi.rs`; `tests/openapi.rs`; `docs/OPENAPI.md:1-60`; `apps/web/src/lib/types.ts:1-4`; `apps/web/src/lib/api.ts:263+`; `mock.ts` (4,155); `docs/CONTRACT.md` (4,963).
- Frontend: `app.tsx:63-71,134-168,256-265`; `state/downloads.tsx:36-57`; `lib/install-queue.ts:1-40`; `components/big-picture/{controller.ts,motion.ts}`; `styles/*.test.ts`; `lib/i18n.tsx:1-30`.
- CI/release: `ci.yml:37,40,71,146-179,183-204,276,339-405`; `release.yml:14-67,180-215,246-258,326-340`; no `deny.toml`.
- Shared-logic note: `crates/ludex-core/src/gamepad.rs:22`.
