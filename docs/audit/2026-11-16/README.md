# Ludex quality audit — 2026-11-16

Baseline: `7199a2c` (`chore(release): v0.4.14`). Six read-only audits run in parallel; no source file was changed by the audit itself.

| Report | Scope | Lines |
|---|---|---|
| [server-core.md](server-core.md) | `crates/ludex-server`, `crates/ludex-core`, SQL/migrations, routes, auth, security | 635 |
| [desktop.md](desktop.md) | `apps/desktop/src-tauri`: install, stream, DB, overlay, console, launch | 460 |
| [web.md](web.md) | `apps/web/src`: screens, components, state, lib, types, tests | 672 |
| [ui-ux.md](ui-ux.md) | Design system, Console, flows, accessibility, 10-foot UI | 324 |
| [quality-infra.md](quality-infra.md) | Tests, CI/CD, docs, deps, scripts, release | 388 |
| [architecture.md](architecture.md) | Cross-cutting boundaries, data, runtime, refactor waves | 558 |

## Verdict

The architecture is better than the line count suggests: a real domain crate, a router↔OpenAPI equality test, a tested i18n/theme gate, a pure scheduler (`pipeline::plan_next`), and a measured extraction pipeline. The fragility is concentrated, not diffuse.

**Four things must be fixed before any large refactor:**

1. **The desktop can crash-loop on startup.** `panic = "abort"` (`apps/desktop/src-tauri/Cargo.toml:99`) + unlimited `auto_resume_on_startup` (`install.rs:2314`) + no attempt counter = a job that panics kills every launch. This is #234.
2. **The desktop is not gated by CI.** `apps/desktop/src-tauri` is a separate workspace; CI runs `cargo check` only (`ci.yml:145-205`). 839 desktop tests pass in 15.8 s locally and never run; Windows-only code is first compiled after merge.
3. **The install pipeline is one 820-line `execute()`** with four transports and `String` errors; reliability fixes touch it every release. It needs the Wave 3 split, not a rewrite.
4. **The contract has three copies** (server DTOs, desktop shadow DTOs, hand-written `types.ts`) with no negotiation. Move DTO ownership to `ludex-core`, generate `types.gen.ts`, CI-diff it.

## Highest-impact findings

### Critical / data loss / security
- **Crash loop**: `panic = "abort"` + auto-resume; safe mode/attempt budget required (#234).
- **Fullscreen trap**: window-state restores fullscreen; Big Picture only exits fullscreen if it set it; no F11/menu escape (#233).
- **Ludusavi cap**: official manifest is 53,549 games; parser rejects above 50,000 (`saves_detect.rs:51,1890`), so save detection always fails (#236).
- **Blind SSRF**: `media.rs:41` fetches provider-supplied artwork URLs with the shared client, no private-IP check, follows redirects.
- **Desktop settings save is destructive**: `settings.rs:1452-1470` removes the destination before rename; a crash loses all settings.
- **Desktop migrations are non-transactional**: a partial failure leaves schema ahead of `user_version` and re-runs fail forever; no downgrade guard.
- **No request timeout in the web API client**: a server that accepts and never answers leaves the app on the boot screen.
- **Two console commands do not exist**: `quit_app` and `system_sleep` are invoked by the web but not registered; Sleep/Quit always toast "Not available".

### Correctness / reliability
- Local-mode resume persists `server_port: 0` (ephemeral) and reuses it after restart; interrupted local installs fail.
- `WaitingForSpace` only re-dispatches on job completion or manual "Check again"; freeing space externally leaves the queue parked.
- Search pagination reports a `total` from a truncated 5,000-row set.
- PostgreSQL parity: unique-violation handling string-matches SQLite text; only 2 PG tests.
- N+1 writes in achievements and library health.
- `MAX_MANIFEST_GAMES` is a hard failure, not a warning; see #236.

### Frontend
- 1 Hz `installBridge.list()` + `setJobs(newArray)` re-renders every `useDownloads()` consumer, including the 200-card library and 2,728-line Console; zero `React.memo`.
- 2.1 MB entry bundle, no `lazy`, no `manualChunks`; mock/demo data shipped in production; service worker precaches all locales.
- No ESLint; 15 inert `eslint-disable` comments; render-time ref write in `downloads.tsx:181`.
- No render tests; 5 E2E specs for 35 screens.
- Controller dead ends: install modal, Game page, Remote play, onboarding modal.
- `/game/:id` opens scrolled 796 px on mobile (`game-nav.tsx:57`).

### UI/UX / Console
- The "AI slop" is the **Soft preset default**, not the whole console: Soft forces cream/coral and disables the app theme; Studio looks coherent. Flip the default to Studio and make Soft opt-in.
- Console typography is ~3× under the project's own 10-foot rule (9.5–13 px metadata vs 28 px minimum); it does not scale with `--bp-u` (identical at 1080p/4K).
- Translucent Soft surfaces drop text contrast to 4.5–5.5:1 against the 7:1 target.
- Controller-only dead ends and modal focus traps; no route-change focus/`document.title`.
- 1,915 arbitrary `text-[Npx]` utilities across 27 sizes; 9 shadow recipes; 207 raw `border border-line` containers; stock Lucide icons reused for different destinations.

### Tests / CI / release
- 859 desktop tests not run (36% of 2,302 total); no Windows check on PRs; required checks exclude PostgreSQL/macOS.
- Edge updater `latest.json` dead since 2026-11-11 (`0.3.2-edge.179` vs `0.4.15-edge.246`); edge jobs race on the same release.
- Release gate is E2E only; public release created before artifacts; no `SHA256SUMS`; `LUDEX_GIT_COMMIT` never set.
- No `cargo audit`/`cargo deny`/SBOM/coverage; Dependabot security updates disabled; `enforce_admins: false`.
- Docs are 2–3 releases behind (`README` says v0.2.0/no PostgreSQL; DEPLOY/GITOPS pin v0.2.0).

## Refactor program

Waves are sequential on `main`, each behind green CI. No wave mixes a move with a behavior change. Effort is focused engineer-days with agent assistance.

### Wave 0 — Stop the bleeding (4–6 d)
- Desktop `cargo test --lib --offline` + `clippy -D warnings` in CI; add Windows `cargo check` on PRs.
- #234: `panic = "unwind"`, worker `catch_unwind`, attempt budget, safe mode, startup circuit breaker.
- #233: one `WindowPolicy` (windowed/borderless/fullscreen), F11/Alt+Enter, tray escape, sanitize restored state.
- #236: raise the Ludusavi cap to 100k/200k, keep the 64 MB guard, regression test.
- Local-mode URL refresh at resume; atomic settings save; typed `InstallError` at the four retry/fallback decision sites.

### Wave 1 — Contract and hygiene (6–9 d)
- Move response DTOs to `ludex-core::models`; delete desktop shadow DTOs.
- Generate `types.gen.ts` from OpenAPI; `--check` job; shrink hand-written types.
- Pure `git mv`: `saves/`, `update/`, `install/transport/files.rs`; `bridge.rs` as the only `ludex_core::` path (grep guard).
- Shared JSON fixtures for controller profiles and queue ordering.

### Wave 2 — Data safety (6–10 d)
- Desktop downgrade guard (`user_version > SCHEMA_VERSION`), pre-migration `VACUUM INTO` backup, numbered schema files.
- `db::with_tx` + transactional scan/delete/collection/bulk sites; PostgreSQL validation.
- Playtime outbox formalized; `ludex-backup` PostgreSQL message test.

### Wave 3 — Install pipeline decomposition (15–25 d)
- `install/job.rs`, `install/plan.rs`, `install/scheduler.rs`, `net::{RetryPolicy, ProgressSink, CancelSet}`, `install/verify.rs`, `install/commit.rs`; `execute()` disappears.
- Keep `pipeline.rs` tests as the oracle; no test rewrites; installed-tree SHA-256 must be identical before/after.

### Wave 4 — Server decomposition (12–20 d)
- `AppState` → subsystem `Arc`s; `transfer/` module; split `routes/games.rs`; per-module OpenAPI docs.
- Gate: `openapi.json` byte-identical.

### Wave 5 — Frontend structure (12–18 d)
- One API adapter; `lib/query-keys.ts`; merge I18n/Preferences; slim Downloads context.
- Split `big-picture.tsx` into `console/`; one `useInstallAction()`; dynamic-import mock/demo data; add ESLint + render tests.

### Wave 6 — Platform & security (8–12 d)
- `ludex-embed` facade + feature-gated server subsystems; `cargo-deny`/advisory/SBOM; contract version header; ADRs and `docs/ARCHITECTURE.md`.

### UI/UX track (parallel to Wave 5, after a reference review)
- Flip Console default to **Studio**; make Soft opt-in and stop forcing its palette. Do not iterate Soft until the user supplies 2–3 references and answers the eight questions in `ui-ux.md`.
- 10-foot type scale: 28 px body/metadata, 48 px+ hero at 1080p, tied to `--bp-u`; 7:1 contrast.
- Close controller dead ends: install sheet, Game page, Remote play, onboarding; focus manager; route-change focus/title.
- Consolidate type/shadow/border/icon primitives; remove duplicate hint strips/rings.

## Decisions required
1. Is the desktop allowed to crash? No (`unwind` + isolation).
2. Two databases in local mode? Keep two, with an ADR: server = catalog, desktop = local truth.
3. Who owns the wire contract? `ludex-core::models` + generated OpenAPI/TS.
4. Keep desktop↔server coupling? Yes, behind `ludex-embed`.
5. Pipeline unit of composition? Transport trait + one `net` core.
6. Typed errors across Tauri? Yes, `code` + retry class.
7. UI state authoritative? Desktop authoritative, events are invalidation hints.
8. Poll or push? Poll; document intervals.
9. Keep dual Rust/TS gamepad/queue logic? Keep, bind with shared fixtures.
10. Desktop in workspace? No, but gate tests/clippy/Windows in CI.

## Gates and metrics
- **G1 chaos gate**: kill the desktop 20× mid-download/extract/verify, once with an injected panic: every job terminal or resumable, zero orphan staging, zero duplicate installs, process alive.
- **Contract gate**: OpenAPI byte-identical for server moves; `types.gen.ts` diff empty.
- **Pipeline gate**: installed-tree SHA-256 identical before/after each split.
- **UI gate**: 1080p/4K type/contrast checks, keyboard/controller E2E, no modal traps.
- **Release gate**: desktop tests + clippy + Windows check before tag; `SHA256SUMS`.
- Metrics: `install.rs` < 1,500 LOC by end of Wave 3; CI runtime; bundle size; crash-free startup rate; time-to-patch.

## Do not refactor
- i18n catalogs, theme tokens, `pipeline::plan_next`, `space.rs`, `ludex-core::archive`, rusqlite on desktop, the Tauri command surface as a whole.
- Do not unify the two local databases; do not drop PostgreSQL; do not rewrite the frontend into a global store; do not add a chunk store/delta pipeline now.

## Issue map
- #233 fullscreen trap
- #234 crash loop
- #235 launcher-style default download
- #236 Ludusavi manifest cap
- #231 server-side extracted file transport
- #232 classic extraction speed
- #229/#230 Console/controller (closed first passes; UI/UX track reopens the visual work)
