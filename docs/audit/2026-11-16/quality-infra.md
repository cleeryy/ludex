# Ludex v0.4.14 — Tests, CI/CD, Docs & Infra Audit

- **Repository**: `/root/ludex`, commit `7199a2c` (`chore(release): v0.4.14`), clean tree.
- **Date**: 2026-11-16.
- **Method**: static inventory of every `tests/`, `#[cfg(test)]`, `*.test.ts`, `e2e/*.spec.ts`, workflow, script and doc; local execution of the web and desktop suites; GitHub Actions API for job/step timings and branch protection; live release metadata from GitHub Releases. No repository file was modified.
- **Evidence base**: CI run `43767346190` (main, 2026-11-15 22:05Z), Release run `43767349512` (v0.4.14), `gh api` branch protection / repo security settings, `gh release view edge|v0.4.14`.

---

## 0. The five findings that matter

1. **859 desktop tests are written but never run — and never even compiled by CI.** `apps/desktop/src-tauri` is excluded from the root workspace (`Cargo.toml:1-4`, `apps/desktop/src-tauri/Cargo.toml:12`) and CI only runs `cargo check` (`ci.yml:145-180`, `:182-205`), which does not build `#[cfg(test)]` targets. Locally the same suite is **839 passing tests in 15.8 s**. That is ~36 % of the project's 2 302 tests invisible to CI, including the entire install engine, updater, overlay, saves and DB-migration logic.
2. **The `edge` updater channel is effectively dead since 2026-11-11.** The `edge` release has 524 assets (6.4 GB) up to `0.4.15-edge.246`, but its `latest.json` asset was last updated **2026-11-11T18:04Z** and still advertises **`0.3.2-edge.179`**. Early-channel clients therefore never see the ~65 builds published since. `docs/RELEASING.md` claims tauri-action "merges the Windows/Linux entries into a single `latest.json`" — empirically false in this layout; `edge-desktop` and `edge-release` also race on the same tag/release with no `concurrency`.
3. **The public docs are 2–3 release lines behind the code.** `README.md:20` still says "Status: v0.2.0 — current release line"; `README.md:41` says "no PostgreSQL" although PostgreSQL has shipped since 0.3.0; `README.md:83-87` marks share links, collections, community, Ludusavi, Discord/Steam, gamepad, tray and auto-updater as "roadmap" although all are shipped; `FEATURE-PARITY.md:35-37,60,101-107` repeats that. Worse, `docs/DEPLOY.md:45` and `docs/GITOPS.md:146,155` tell operators to pin **`v0.2.0`** images/updater tags.
4. **There is no dependency-security or license gate at all.** Dependabot *alerts* and security updates are disabled at the repo level (`gh api repos/cleeryy/ludex` → `dependabot_security_updates: disabled`), there is no `cargo audit`/`cargo deny`/SBOM/CodeQL job, and no coverage measurement. Branch protection requires 5 checks but **not** `Rust (PostgreSQL backend)`, **not** `Desktop (macOS cargo check)` and **not** desktop smoke; `enforce_admins: false`.
5. **Release safety depends on one E2E job and a pre-created public release.** The tag workflow's `verify` job runs only `scripts/e2e.sh` (`release.yml:17-78`); no fmt/clippy/unit tests. `create-release` publishes the release *before* any artifact is built (`release.yml:80-97`), so a late `desktop`/`docker` failure leaves a public release with missing platforms. No `concurrency` on tag pushes. Stable releases also never publish the `SHA256SUMS` asset the download page expects (`docs/CONTRACT.md:258-260`; asset list of v0.4.14 confirms only per-server `.sha256`).

---

## 1. Test inventory

### 1.1 Totals (executed or counted at `7199a2c`)

| Area | Unit tests | Integration / E2E | Test files | Source LOC | Runs in CI? | Observed time |
|---|---|---|---|---|---|---|
| `crates/ludex-core` | 116 | 0 (empty `tests/` dir, RAR fixtures only) | 9 | 8 633 | Yes (`--workspace`) | included below |
| `crates/ludex-server` | 294 inline | 197 across 40 files | 40 + 50 src modules | 65 941 | Yes | 3 m 41 s (`Test` step) |
| `apps/desktop/src-tauri` | 859 attributes, **839 executed** | 0 | 63 | 90 763 | **No** | 15.8 s locally, 4 threads |
| `apps/web` | 851 (`vitest`) | 0 | 72 | 95 672 (11 028 test LOC) | Yes | 9.5 s |
| `e2e` (Playwright) | — | 5 specs | 5 + helper | 279 | Yes | ~30 s inside 2 m 45 s job |
| **Total** | **2 120** | **202** | **~190** | **261 000** | **64 % executed** | — |

Rust workspace total is exactly **607** test functions (116 core + 294 server unit + 197 server integration), matching the count reported by `cargo test --workspace --all-targets`.

Local runs performed for this audit:

```
apps/web:  Test Files 72 passed, Tests 851 passed, 9.46 s
desktop :  test result: ok. 839 passed; 0 failed; 15.76 s
```

Test-density (tests per 1 000 source LOC): core 13.4, server 7.4, desktop 9.2, web 10.0. Volume is healthy; the problem is *where* it runs, not how much exists.

### 1.2 What is genuinely well covered

- **HTTP contract**: `crates/ludex-server/tests/openapi.rs` parses every literal `.route("…")` in `src/routes/*.rs` + `src/ingest/mod.rs`, compares it with `openapi::ROUTES` (231 operations: 29 public, 101 session, 35 editor, 66 admin), checks every `$ref`, then boots a server and PATCHes every documented path to prove it is mounted. This is the strongest test in the repo.
- **Auth hardening**: lockout/backoff/`Retry-After` (`api_security.rs:23-70`), session revocation (`api_security.rs:72-140`), TOTP replay + recovery codes (`account_security.rs:260`), invitations (`account_security.rs:32,152,225`), admin reset invalidating devices (`account_security.rs:539`), OIDC (7 tests incl. a real RSA JWKS fixture).
- **Downloads / resume**: the desktop install engine has simulation-based tests for pause→resume byte identity (`install.rs:6680`), rehash from byte zero (`:6848`), corrupted body never reaching the gate (`:6902`), stall resuming at the exact flushed offset (`:7071`), retry backoff caps (`:7328`), retry-window policy (`:7350`), `Retry-After` (`:7400`), cancel during retry (`:7709`), persisted-job resume (`:7792`). Server side: `download_limits.rs` (5), `desktop_download.rs` (6), `remote_installs.rs` (5), `extracted_cache.rs` (6 + 8 unit), `stream_install.rs` (13).
- **VirusTotal (#224)**: 11 unit + 9 integration tests with a mock API covering clean/malicious/suspicious/404/401/429/5xx/timeout, quota persistence and upload consent.
- **Archives**: 45 core tests incl. encrypted RAR5 headers and symlink archives (`crates/ludex-core/tests/fixtures/`).
- **Config/data guards**: CSP and trailer origins (`apps/web/src/lib/csp.test.ts`), PWA manifest/icons (`pwa.test.ts`), no hardcoded colours (`styles/no-hardcoded-colors.test.ts`), theme token completeness + WCAG (`styles/tokens.test.ts`), i18n catalog parity (CI `i18n:check`).

### 1.3 What is untested or thin

| Gap | Evidence | Consequence |
|---|---|---|
| Desktop suite not in CI | `ci.yml:145-180` runs `cargo check` only; no `cargo test` anywhere for `apps/desktop` | 839 tests (install, updater, overlay, saves, DB) can break silently |
| Desktop test code not even compiled | `cargo check` without `--all-targets` | A PR can break `#[cfg(test)]` imports/helpers and merge green |
| No Windows compile on PRs | jobs: ubuntu `desktop-check`, `macos-check`, Windows only in `edge-desktop` (`ci.yml:330-504`, `if: push && main`) | 49 `cfg(windows)` sites are first compiled **after** merge |
| Authz matrix absent | 125 `UNAUTHORIZED/FORBIDDEN` assertions across 25 files, but no exhaustive check of the 101 `Auth::Admin`+`Editor` routes declared in `openapi.rs` | A missing `AuthUser`/role check on one route is not caught unless someone wrote that specific test |
| Server migration upgrades | all tests start from an empty DB; `_sqlx_migrations` only queried in `postgres.rs:189-196` | 32 forward-only migrations, zero proof that a v0.3/v0.4.13 database upgrades |
| Desktop DB upgrades | `memory_database()` applies `SCHEMA_V1..V19` directly (`db.rs:3627-3648`); no fixture at `user_version < 20` | 20 schema versions, no data-preserving upgrade test |
| Desktop DB downgrade | `migrate()` only advances when `version < SCHEMA_VERSION` (`db.rs:719-726`); no `version > SCHEMA_VERSION` guard | Edge→stable rollback after a future schema bump opens a newer DB silently |
| PostgreSQL parity | `tests/postgres.rs` = **2 tests**, one 390-line mega-test (`:179`), 23 distinct API paths vs 313 in SQLite tests | Most features are SQLite-only in CI; Postgres regressions ship |
| SQLite↔Postgres schema equality | `postgres.rs:186-196` only counts migration files | Column/index drift between `migrations/` and `migrations_postgres/` undetected |
| `ludex-backup` CLI | `src/bin/ludex-backup.rs` (198 lines) has zero tests; no test invokes the binary | Shipped in every release archive and used by operators, including `--upload-s3` |
| Restore compatibility | `backup.rs:129` records `schema_version`; restore path never compares it to the running build (`backup.rs:449` only checks `format_version`) | Restoring a backup from a newer server into an older binary is unguarded |
| S3 backend (#47) | `storage.rs` has 7 tests, only key sanitisation; tests mention S3 4 times, never exercise `object_store` | A whole shipped backend + `ludex-backup --upload-s3` untested |
| Updater end-to-end | `updater.rs` 7 unit tests (channels, skip, error copy); no signed `latest.json` install, no rollback, no "installer ran but version unchanged" integration test | Only release-time marker check (`verify-windows-install.ps1`) exists |
| Commit stamping | `metrics::commit()` reads `LUDEX_GIT_COMMIT` at runtime (`metrics.rs:854`), never set by `release.yml`, `ci.yml`, `Dockerfile`; tests only assert `is_some()` (`observability.rs:121`, `admin_observability.rs:53`) | `/api/health/details.commit` and `/metrics` always say `"unknown"` in shipped builds |
| Release checksums | no workflow writes `SHA256SUMS` (`grep -r SHA256SUMS .github scripts` → only docs) | `checksumsUrl`/`sha256` in the download page are `null` in production |
| Web screens | 22 screens, 0 component tests; only 5 E2E specs | Every screen bug not on the 5 E2E paths is caught only by manual testing |

### 1.4 Flaky / slow tests

- Fixed sleeps: 55 occurrences in server integration tests, ~6.8 s aggregate. Dominant: `virustotal.rs:124` (`sleep 3 s` for the timeout mock), `uploads.rs:282` (`sleep 1.1 s` because TTL timestamps have second precision), `desktop_handoff.rs:160` (400 ms), `integrations.rs:611` (300 ms), 32×25 ms.
- Wall-clock coupling: login lockout/backoff (`api_security.rs:47` sets `login_backoff = 1 ms`), upload TTL, VirusTotal budget day boundary. These are the first candidates to flake on a loaded runner.
- E2E port race: `scripts/e2e.sh:97-110` binds three sockets, closes them, then starts servers; another process can steal a port between close and bind. Use `--port 0` + read the bound port, or keep the socket FD and pass it.
- E2E console assertion: `e2e/tests/helpers.ts:10-30` fails on **any** unexpected `console.error`/`pageerror`; the ignore list is explicit and narrow — good signal, but Chromium/webkit noise becomes red CI.
- `retries: 1` only in CI (`e2e/playwright.config.ts:19`), `workers: 1`, `fullyParallel: false`: masking of genuinely order-dependent tests is possible but currently the suite is small.
- No `#[ignore]` tests anywhere; Postgres tests self-skip when `LUDEX_TEST_POSTGRES_URL` is unset (`postgres.rs:180-184`) — a skip is indistinguishable from a pass in CI logs unless the job is inspected. The CI job sets it, but a local `cargo test` silently runs 0 PG assertions.
- Slowest structural cost: 40 separate integration binaries, each booting a full server + 32 migrations. Parallelism is per-binary/threads only; no nextest, no sharding, no per-binary timing report.

### 1.5 Duplicated fixtures / helpers

The shared harness is good: `crates/ludex-server/tests/common/mod.rs` centralises server start/config, token setup, `add_game`, byte uploads. Remaining duplication is small:

- `write_archive` defined twice (`admin_games.rs:19`, `library_ops.rs:107`), `sha256_hex` twice, `write_game` twice, 7 local `async fn setup` wrappers.
- Each of the 6 E2E specs re-imports the same 5 helpers; fine.
- `scripts/e2e.sh` and `scripts/smoke.sh` duplicate health-wait/port-pick/JSON-path logic (~80 lines each). One `scripts/lib.sh` would remove it, but these are stable.
- Web tests have no shared fetch/localStorage harness (only 4 files stub globals), so not a problem yet.

### 1.6 Tests that assert implementation details

These are deliberate guardrails, but they will produce churn on refactors and should be labelled as such:

- Source-scanning CSS/CSP tests: `apps/web/src/styles/console-material.test.ts` (parses `index.css`, pins radius bands 16–24 px and forbids animated `backdrop-filter`), `page-scroll.test.ts` (looks for literal `body {` and `overflow: hidden`), `no-hardcoded-colors.test.ts` (regex over components), `apps/web/src/lib/csp.test.ts` (reads `tauri.conf.json` and reconstructs directives), `styles/tokens.test.ts` (token key sets).
- The OpenAPI router test parses Rust with a hand-written tokenizer (`tests/openapi.rs:22-35`). It only recognises a **literal** `.route("…")`; a route registered through a constant, a helper function or a new subdirectory would be silently invisible, and the test would still pass. It also does not scan `lib.rs` for non-`/api` routes. Add an assertion that the number of parsed registrations is > 0 and equals the router's expected count, or use `Router::routes()`/`axum::Router` introspection.
- Desktop: 243 `contains("…")` assertions on error copy (e.g. `adopt.rs:1741-1865`). Reasonable, but upgrades to typed error codes would make them robust.
- Exact JSON error-message assertions are rare (4 occurrences) — good discipline overall.

### 1.7 Missing failure-path tests (specific)

1. **Install resume**: well covered (see 1.2). Remaining gap: resume across a *process restart* with a server that changed the archive (`ETag`/size change mid-resume) — there is a hash gate but no test for "server replaced bytes, client must restart from zero".
2. **Crash loop**: `launch.rs:113` maps an immediate exit to an error and tests 1 ms/2 s/5 s boundaries; there is no test (or design note) for repeated crash loops (N launches in M minutes → disable auto-launch/notify). `install_health` handles a stale updater binary, not a crashing game.
3. **Authz**: no table-driven test over `openapi::ROUTES`. A new `Auth::Admin` route without a role guard passes CI unless individually tested. Cheap exhaustive test: every `Session/Editor/Admin` operation returns 401 without a token (231 requests, < 2 s).
4. **Migrations**: no "old DB + data → run `db::connect` → assert data and columns" test for SQLite or Postgres. No test that editing an applied migration is rejected/flagged.
5. **Postgres parity**: one mega-test hides which feature fails; the schema assertion is a file count, not a structure diff. No test runs the same suite on both backends.
6. **Updater**: signature failure is only asserted as a message (`updater.rs:694-700`); no test that a tampered `latest.json`/artifact is refused by the plugin, no install/relaunch cycle, no rollback when the new binary fails its self-check.
7. **Backup/restore**: no CLI round-trip, no restore-from-newer-schema guard, no test for `--force`, `--dry-run`, `--no-media`, S3 upload.
8. **S3 storage**: no MinIO/`object_store` integration test; the feature is compiled into every server (see §4).
9. **Health commit**: tests assert `commit` is a string, not that it equals the built SHA — which is why the always-`"unknown"` bug survives.
10. **E2E**: no download pause/resume/cancel, no settings persistence, no admin game edit/bulk action, no collections/share/community journey, no mobile viewport, no auth failure/rate-limit, no PWA update prompt.

---

## 2. CI/CD

### 2.1 Jobs and observed timings (CI run `43767346190`, main @ v0.4.14)

| Job | Trigger | Wall time | What it really gates | Required? |
|---|---|---|---|---|
| `Rust (fmt, clippy, test)` | all pushes/PRs | 4 m 26 s (fmt 1 s, clippy 16 s, test 3 m 41 s) | fmt, clippy `-D warnings`, 607 workspace tests | Yes |
| `Rust (PostgreSQL backend)` | all | 1 m 58 s (test step 58 s) | 2 PG tests against `postgres:16-alpine` | **No** |
| `Web (install, build, typecheck)` | all | 1 m 08 s | i18n gate (strict advisory), build, tsc, 851 vitest | Yes |
| `Docker image (build only)` | needs rust+web | 8 m 10 s | image **builds**, never runs | Yes |
| `Desktop (cargo check)` | all | 1 m 32 s | `cargo check --locked`, **no tests** | Yes |
| `Desktop (macOS cargo check)` | all | 1 m 28 s | macOS compile (arm64 host) | **No** |
| `E2E` | all | 2 m 45 s | 5 Playwright specs via `scripts/e2e.sh` | Yes |
| `Desktop smoke (xvfb)` | all, `continue-on-error: true` | 3 m 25 s | window opens + startup log line | **No** (advisory) |
| `Edge desktop (Linux/Windows)` | push main only, needs rust/web/e2e | Linux 8 m 56 s, Windows 14 m 00 s | bundles + `latest.json` + installer marker | No |
| `Edge release (main)` | push main only | 2 m 55 s | server tarball, force-moves `edge` tag | No |
| **CI wall clock** | | **18 m 32 s** | critical path = edge Windows | |

Release run `43767349512` (tag `v0.4.14`): **30 m 33 s** total — verify 3 m 18 s, create-release 8 s, server 5 m 06 s, docker 7 m 38 s, Linux desktop 13 m 36 s, Windows 20 m 27 s, **macOS universal 26 m 46 s** (critical path), GHCR push included.

### 2.2 Redundancies (green CI, wasted minutes)

- `web`: `pnpm --filter @ludex/web build` already runs `tsc --noEmit` and `i18n.mjs` (`apps/web/package.json:9`), then CI runs `i18n:check` (`ci.yml:105`) and a second `tsc --noEmit` (`ci.yml:118`). ~30–40 s wasted per run; delete one of each, or make `build` script-only.
- `docker` rebuilds the whole Rust graph inside the image with no shared cache with the `rust` job (`ci.yml:123-143`). Unavoidable without a registry-based layer cache, but note the 8-minute cost.
- `edge-desktop` and `edge-release` both rebuild the web UI and Rust on every main push (two full builds). A single reusable "build-web" artifact job would remove one.
- `desktop-smoke` is advisory yet still costs 3 m 25 s on every push; it has been green since the startup-log fix (`c5a37ab`). Either make it required or move it to a nightly.
- All jobs run on docs-only/Markdown-only changes: no `paths-ignore` on `ci.yml`. A README edit triggers 10 jobs and 18 minutes.

### 2.3 Missing gates

| Missing gate | Risk |
|---|---|
| Desktop `cargo test` (see §0) | 36 % of tests unrun; test compilation can break |
| Desktop `clippy`/`fmt` | 90 k LOC outside any lint; formatting currently clean (verified `cargo fmt --check`) but unenforced |
| `--locked` in fmt/clippy/test | CI resolves the lockfile afresh; a release build (`--locked`) can fail or differ from what CI tested (`ci.yml:34,37,40`; compare `release.yml:139`) |
| `--features embed-web` build/test | the feature is only compiled in `edge-release`/`release` (`ci.yml:577`, `release.yml:139`); a broken `web.rs` embedded branch reaches a tag |
| Doctests | `cargo test --all-targets` skips `--doc`; doc examples never run |
| Windows PR compile | first Windows compilation happens post-merge (`ci.yml:330`) |
| Windows runtime smoke | no install/launch test; only the NSIS payload marker at release (`verify-windows-install.ps1`) |
| Security audit / license | no `cargo audit`, `cargo deny`, `pnpm audit`, SBOM, CodeQL; Dependabot *security updates* disabled |
| Coverage | no `cargo-llvm-cov`/`vitest --coverage`, no threshold or trend |
| `ludex-backup` CLI test | binary shipped untested |
| Container runtime | `docker` job uses `push:false, load:false`; never `docker run` + `/api/health`, never checks non-root uid or volume permissions |
| Shell lint | no `shellcheck`/`actionlint`; three bench scripts deliberately use `set -uo pipefail` without `-e` |
| Release provenance | no build attestations, no SLSA; checksums per server tarball only |

### 2.4 Cache and build time

- Caches are warm: `Test` 3 m 41 s, `Clippy` 16 s. `Swatinem/rust-cache` keys include the job, so `rust`, `e2e`, `desktop-check`, `desktop-smoke`, `edge-*`, and release desktop each keep separate caches — a cold cache in `edge-desktop` costs ~8–14 min, and release macOS ~27 min.
- Playwright browser cache is keyed on `pnpm-lock.yaml`; good.
- Docker uses `type=gha,scope=ludex-docker`; 8 m 10 s warm. **No `.dockerignore`**: `target/` (2.6 GB) and `apps/desktop/src-tauri/target/` (11 GB) are excluded from git so CI's context stays ~30 MB, but every local `docker compose build` uploads ~14 GB. Add `.dockerignore` (`target`, `node_modules`, `apps/*/node_modules`, `e2e/test-results`, `*.png`, `.git`, `.dev`).
- The repo root carries 20+ tracked marketing PNGs (~7 MB) that are not used by the docs site (`bp-*.png`, `dock-*.png`, `settings-*.png`, …) and belong in `docs/assets/`. They also inflate the Docker context and checkout.

### 2.5 Release workflow risks

1. **Stale `edge/latest.json` (confirmed).** `gh release view edge` shows `latest.json` updated **2026-11-11T18:04Z**, version `0.3.2-edge.179`, while bundles `0.4.15-edge.246` were uploaded 2026-11-15T22:24Z. The early channel is not updatable in practice.
2. **Two writers on the same release/tag.** `edge-desktop` (tauri-action, `tagName: edge`, `includeUpdaterJson`) and `edge-release` (`git tag -f edge` + `gh release edit` + asset upload) run concurrently (`ci.yml:330-504` vs `:506-617`). No `concurrency` group, no ordering, `--clobber` only in the hand-written checksum upload. This is the most likely root cause of #1 and can also flip release notes/tag.
3. **Public release before artifacts.** `create-release` (`release.yml:80-97`) creates a non-draft release, then `server`, `desktop` (3 platforms), `docker` build in parallel. A failure leaves a public version with missing Windows/Linux/macOS assets. Use a draft release and publish only after all matrix legs succeed.
4. **Release gate is E2E only.** `verify` (`release.yml:17-78`) runs no fmt/clippy/unit tests. A tag can be pushed on a commit that passed E2E but broke unit tests, or was never CI-tested (branch protection does not cover tags).
5. **No checksum manifest for desktop assets.** Stable publishes `.sig` but no `.sha256`/`SHA256SUMS`; edge publishes per-platform `edge-checksums-*.txt`, which `desktop_download.rs:492-500` explicitly ignores ("Only the release-wide manifest counts"). The documented download-page SHA-256 feature (`docs/CONTRACT.md:258-260`, `README.md:154`) is therefore dead on both channels.
6. **524 assets / 6.4 GB on `edge`.** Every edge build since 0.3.1 is retained; no cleanup. This slows release API calls, bloats the release page and counts against asset limits.
7. **Version reporting splits on edge.** `tauri.conf.json` is stamped `0.4.15-edge.N` by CI (`ci.yml:391-404`), while `env!("CARGO_PKG_VERSION")` stays `0.4.14` (Cargo.toml not stamped). Diagnostics, remote-install user agent and logs show 0.4.14 (`diagnostics.rs:254`, `remote_install.rs:153`), while the updater shows 0.4.15-edge.N (`updater.rs:295`).
8. **`LUDEX_GIT_COMMIT` never set.** Health/metrics commit is always `unknown` (`metrics.rs:854`); fix by passing `LUDEX_GIT_COMMIT=${{ github.sha }}` in both release builds and a Docker `ARG`.
9. **`permissions` are workflow-wide** (`release.yml:8-10`): the E2E `verify` job gets `contents: write` + `packages: write` although it only needs read. Scope per job.
10. **Docs contradiction:** `docs/RELEASING.md:3-5` says "The only mandatory secret is `GITHUB_TOKEN`", but `release.yml:246-256` hard-fails when `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)` are missing. Pick one policy.

### 2.6 Branch protection reality

`gh api repos/cleeryy/ludex/branches/main/protection`:

- required contexts: `Rust (fmt, clippy, test)`, `Web (install, build, typecheck)`, `Desktop (cargo check)`, `Docker image (build only)`, `E2E` — **PostgreSQL and macOS checks are not required**; strict = true.
- `enforce_admins: false` → the sole maintainer can push straight to `main` bypassing all checks (documented at `docs/RELEASING.md:360-366`). This is a policy choice, but it means CI is advisory for the person most likely to push.
- repo security: secret scanning + push protection enabled; **Dependabot alerts and security updates disabled**; no code scanning.

---

## 3. Docs and contracts

### 3.1 Drift, with evidence

| Claim | Reality | Ref |
|---|---|---|
| "Status: v0.2.0 — current release line" | project is 0.4.14 | `README.md:20` |
| "One binary… no PostgreSQL" | PostgreSQL backend since 0.3.0, documented in `docs/POSTGRES.md`, tested in CI | `README.md:41` |
| "metadata and progress import is on the roadmap" | `gamevault.rs` importer + `tests/migration.rs` + `docs/MIGRATION.md` exist | `README.md:52`; `FEATURE-PARITY.md` |
| "Share links, collections, community view — roadmap" | `routes/share.rs`, `routes/collections.rs`, `routes/community.rs`, `tests/collections_share.rs`, `tests/social.rs`, CONTRACT #9/#25/#173/#10 | `README.md:83`; `FEATURE-PARITY.md:35-37` |
| "Ludusavi automatic save discovery — roadmap" | `saves_detect.rs`, `saves_registry.rs`, `docs/SAVES.md`, #107/#146/#153 | `README.md:85`; `FEATURE-PARITY.md:101` |
| "Discord Rich Presence, Steam, Playnite — roadmap" | `presence.rs` (Discord IPC), `steam.rs`, tests, docs/INTEGRATIONS.md | `README.md:86`; `FEATURE-PARITY.md:102` |
| "Gamepad navigation, tray icon, auto-updater — roadmap" | `gamepad.rs`, `tray.rs`, `updater.rs`, `tests/overlay*`, tauri updater configured | `README.md:87`; `FEATURE-PARITY.md:107` |
| "pin v0.2.0" | old, security-relevant line to copy | `README.md:148`; `docs/DEPLOY.md:45`; `docs/GITOPS.md:146,155` |
| "FEATURE-PARITY compares v0.1 scope" | frozen at v0.1, contradicts shipped rows | `docs/FEATURE-PARITY.md:1-6` |
| `overlayTrigger` "default doubleHome" | code default is `chord` on Windows since #228 | `docs/CONTRACT.md:2022` vs `settings.rs:472-483` |
| Contract title "(v0.1)" | 4 minor/minor releases later | `docs/CONTRACT.md:1` |
| All Tauri commands documented in CONTRACT | 4 shipped commands only in `docs/SAVES.md`: `list_save_candidates`, `scan_save_candidates`, `confirm_save_candidate`, `ignore_save_candidate` | `docs/SAVES.md:245-250` vs `lib.rs:365-366` |
| `quit_app` / `system_sleep` pending | correctly marked pending (no drift) | `docs/CONTRACT.md:3010-3011` |
| `.env.example` is the config template | only 35 of 83 `LUDEX_*` variables; 58 not forwarded by `compose.yaml`, including `LUDEX_ADMIN_USERNAME/PASSWORD`, login/rate limits, OIDC env, MFA limits, extracted-cache tuning | `.env.example:1-4` (self-aware caveat), `compose.yaml:16-60` |
| Release publishes SHA-256 for installers | no `SHA256SUMS`; per-asset `.sha256` explicitly ignored by the parser | `docs/CONTRACT.md:258-260`; `desktop_download.rs:492-500` |
| `LUDEX_GIT_COMMIT` feeds health `commit` | never set by pipelines; always `"unknown"` | `docs/CONTRACT.md:173`; `metrics.rs:854` |

### 3.2 What is strong

- `docs/CONTRACT.md` is unusually complete: 4 963 lines, 231 operations, and its paths match the router almost exactly (only 3 regex false positives when diffed). The OpenAPI test enforces route/schema/mount consistency, which is the best possible guard.
- `docs/POSTGRES.md` is honest about behavioural differences (NULL ordering, LIKE case sensitivity, `SUM` casts) and documents that `ludex-backup` refuses on PG.
- `docs/BACKUP.md`, `docs/DOWNLOADS.md`, `docs/SAVES.md`, `docs/SAVES`, `docs/INTEGRATIONS.md` and `docs/RELEASING.md` (apart from the stale examples and the latest.json claim) are detailed and match the code closely.
- `SECURITY.md`, secret-scanning push protection, and `.env.example` warnings are solid. Test PEMs (`tests/oidc.rs:28`) are fixtures, not secrets; no live credentials in the tracked tree.

### 3.3 Missing docs

- No testing/coverage document (what runs where, how to run desktop tests, why Postgres is a separate job, what "advisory" means).
- No desktop DB downgrade/compatibility policy (edge→stable rollback is the realistic scenario).
- No backup restore compatibility policy (older binary + newer `schema_version`).
- No release-artifact/checksum manifest specification (and no implementation).
- `docs/DEPLOY.md` glibc requirement (2.39) is correct; `docs/DEPLOY.md`/`GITOPS.md` examples need `vX.Y.Z` placeholders, not `v0.2.0`.

---

## 4. Dependencies and build

### 4.1 Workspace layout

- Root workspace: `crates/ludex-core`, `crates/ludex-server`, `exclude = ["apps/desktop/src-tauri"]` (`Cargo.toml:1-4`).
- Desktop is its own workspace (`apps/desktop/src-tauri/Cargo.toml:12`) with its own `Cargo.lock` and target dir. This is a deliberate Tauri trade-off, but it means: two dependency graphs, two lockfiles to bump, no shared `rust-cache` between jobs, 11 GB of local target data, and desktop tests fundamentally outside `cargo test --workspace`.
- No `rust-version` (MSRV) anywhere; `rust-toolchain.toml:2` pins channel `stable` (floating) while `docker/Dockerfile:32` pins `rust:1.99-bookworm` and CI uses `dtolnay/rust-toolchain@stable`. Three different notions of "the toolchain".

### 4.2 Duplicate versions and feature bloat

`Cargo.lock` has **29 crates at more than one major/minor version**. The costly clusters:

- `reqwest 0.12.28` (workspace) + `reqwest 0.13.5` (via `object_store 0.14.2`), plus `rand 0.8/0.9/0.10`, `sha2 0.10/0.11`, `base64 0.22/0.23`, `syn 2/3`, `windows-sys 0.48/0.52/0.61`. `object_store` is the sole reason for the second HTTP/crypto stack.
- `object_store` is a mandatory dependency of `ludex-server` (`Cargo.toml` workspace `object_store` + `crates/ludex-server/Cargo.toml`), even though S3 is an opt-in runtime backend (`LUDEX_STORAGE_BACKEND=s3`). Making it a `s3` cargo feature would cut build time and binary size for the default server and remove the reqwest 0.13/rand 0.10/sha2 0.11 duplication.
- `tokio = { features = ["full"] }` at the workspace level is broader than the server needs (`process`, `signal` are used; `full` also pulls `net`, `io-std`, etc.). Minor.
- No automated unused-dependency check (`cargo machete`/`cargo udeps`); a manual spot check found the server's direct deps referenced, so no smoking gun.
- `zip` is pulled twice with different feature sets (`ludex-core`: `zstd`; desktop: `deflate` only) — expected given the workspace split.

### 4.3 Binary / artifact size

| Artifact | Size | Note |
|---|---|---|
| Server archive `ludex-server-v0.4.14-…tar.gz` | 24.2 MB | two stripped binaries (`ludex-server`, `ludex-backup`) |
| Linux AppImage | 92.4 MB | bundles WebKitGTK-dependent shell + assets |
| macOS universal `.app.tar.gz` | 26.2 MB | universal binary |
| Windows NSIS `setup.exe` | 10.2 MB | |
| Web `dist/` | 6.6 MB | 20 locale chunks + fonts; PWA precaches all of it |

The server archive size is dominated by `object_store`/AWS, `utoipa`, `rust-embed` and sqlx drivers; S3-as-a-feature and `--no-default-features` measurement would give a defensible number.

### 4.4 Reproducibility / offline / Windows

- Release builds use `--locked` (`release.yml:139`) and desktop checks use `--locked` (`ci.yml:177`), but the test/clippy jobs do not (§2.3). `Cargo.lock` and `apps/desktop/src-tauri/Cargo.lock` are committed.
- `pnpm install --frozen-lockfile` everywhere in CI; `pnpm-lock.yaml` v9.0 with `packageManager: pnpm@9.15.0`; CI pins pnpm 9.15.0. Good.
- Windows reproducibility: no `rust-toolchain` target pin for Windows, no MSRV, no vendored deps; `windows` crate 0.62 pinned in the desktop manifest. The only Windows verification is the NSIS payload marker, not a launch.
- Docker builds `rust:1.99` but `rust-toolchain.toml` says stable; if stable ≠ 1.99 the image and the released binary are built by different compilers. Pin one exact version in all three places and add `rust-version` to `[workspace.package]`.
- No `.dockerignore` (see §2.4).
- Secrets: `TAURI_SIGNING_PRIVATE_KEY`/password and Apple cert secrets are passed only where needed; no secret is echoed. `permissions` are broader than necessary in `release.yml`. `tauri.conf.json:70` correctly contains only the public minisign key. No tracked secrets found.

### 4.5 License

The project is AGPL-3.0-or-later, but no license audit exists. `unrar 0.5.8` declares `MIT OR Apache-2.0` while bundling UnRAR C++ sources with their own restrictions; `sevenz-rust2`, `zip`, `object_store` and the Tauri stack all need a compatibility check. A `cargo deny` license allowlist is the cheapest way to make this explicit before distribution.

---

## 5. Scripts

| Script | Purpose | Strengths | Failure modes / risks |
|---|---|---|---|
| `scripts/release.sh` (143 l.) | bump, changelog, lockfiles, commit, tag, push | refuses dirty tree (`:79`), refuses non-main (`:84`), refuses existing tag (`:90`), dry-run, `--no-push` | Does not fetch/compare `origin/main`; releases a local `main` that may be behind/ahead. Does not check that the target commit passed CI. `cargo update --workspace --offline` (`:128`) fails without a warm local index; `pnpm install --lockfile-only … || true` (`:130`) silently swallows lockfile failures. Pushes `main` then the tag (`:140-141`): if the tag push fails, a release commit is stranded on main and the tag must be recreated manually. |
| `scripts/bump-version.sh` (72 l.) | edits 7 version locations | validates semver, validates JSON, never touches dependency versions | Does not update `Cargo.lock`/`pnpm-lock` itself (prints steps); relies on the operator. |
| `scripts/e2e.sh` (241 l.) | two servers + ingest HTTP + Playwright | isolated temp dirs, health polling, log dump on failure, kills PIDs with SIGKILL fallback | Port allocation race (`:97-110`); `LUDEX_E2E_BIN` reused without checking it matches the checkout; builds with `--locked` only when the lock exists; fixed 60 s scan timeout; no artifact retention itself (CI uploads). |
| `scripts/smoke.sh` (199 l.) | API smoke: setup, scan, Range 206 | random port, temp dirs, JSON assertions, log dump | Duplicates e2e helpers; not run in CI. |
| `scripts/desktop-smoke.sh` (83 l.) | Xvfb window + startup log | waits for `ludex version=` content, checks `exe="`, kills app | Depends on `xdotool`/window title; advisory in CI (`ci.yml:275`). |
| `scripts/ludex-update.sh` (198 l.) | pull-based server updater | checksum verified before install, `flock`, backup binary, health check + rollback | **Updates only `ludex-server` (`:138-145`)**; `ludex-backup` from the same archive is ignored, so the CLI silently stays at the old version. Rollback does not health-check the restored binary. `tar -xzf` without path hardening (release artifacts are trusted, but defense-in-depth is cheap). |
| `scripts/verify-windows-install.ps1` (86 l.) | proves the NSIS payload carries the stamped exe | marker + hash warning, 7-Zip discovery, temp extraction | Runs only at edge/release time; does not install or launch. |
| `scripts/bench-*.sh` (730/402/626 l.) | network/extract/manifest benchmarks | redact `?token=`, never print bearer, temp-only writes, documented thresholds | `set -uo pipefail` without `-e` (intentional to continue the matrix, but failures can be missed); long, untested, duplicated helpers. |
| `scripts/dev.sh` (68 l.) | server + Vite | simple, `set -euo pipefail` | No port-conflict handling. |

**Worktrees**: `git worktree list` reports **81 worktrees** (65 under `/root/.local/share/opencode/worktree/a4a9d8`, 16 under `/root/ludex-wt`), totalling ~3.7 GB. Nothing in `docs/`, `CONTRIBUTING.md` or `scripts/` documents the lane workflow or cleans them up. This is operational debt that makes `git worktree list`, editor indexing and disk usage confusing; add a documented `scripts/worktree-prune.sh` (only removes clean/merged worktrees) and a note in `CONTRIBUTING.md`.

---

## 6. Top 20 quality/infra improvements

Impact/effort/risk are scored 1 (low) – 5 (high); rank is impact÷effort, then impact.

| # | Improvement | Impact | Effort | Risk | Notes |
|---|---|---|---|---|---|
| 1 | Run `cargo test` (desktop) in CI on Linux + Windows, `--all-targets` | 5 | 1 | 1 | 839 tests already green in 15.8 s; add to `desktop-check` and add a `windows-latest` job |
| 2 | Documentation truth pass: README status/feature table, FEATURE-PARITY, DEPLOY/GITOPS pins, CONTRACT header + overlay default | 4 | 1 | 1 | Pure text; highest user-facing trust win |
| 3 | Add `--locked` to fmt/clippy/test, compile `--features embed-web`, add a `windows-latest` PR `cargo check`/test job | 4 | 1 | 1 | Closes reproducible-build and feature-rot gaps |
| 4 | Fix edge publishing: one publish job, draft release, `concurrency`, explicit `latest.json` merge + `--clobber`; add `SHA256SUMS` for stable+edge | 5 | 2 | 2 | Directly fixes the stale 0.3.2-edge.179 channel and dead checksum feature |
| 5 | Enable Dependabot alerts/security updates; add `cargo deny` (advisories + license allowlist) and `pnpm audit --prod` jobs | 5 | 2 | 2 | Currently zero vulnerability/license signal |
| 6 | Table-driven authz test from `openapi::ROUTES`: 401 for all 202 protected ops, 403 for admin/editor ops | 5 | 3 | 2 | 231 requests, < 2 s; catches the classic "forgot the extractor/role" bug |
| 7 | Migration upgrade tests: SQLite from v0.4.13 fixture with data; desktop DB from `user_version` 12/19; downgrade guard for `version > SCHEMA_VERSION` | 4 | 3 | 2 | 32+20 schemas currently only tested from empty |
| 8 | Postgres parity: split the mega-test, run the core integration suites on both backends, schema-structure diff test, make the job required | 4 | 3 | 2 | 23 API paths vs 313; 66 declared-admin routes mostly PG-untested |
| 9 | Release gate: add fmt/clippy/unit tests to `verify`, scope `permissions`, publish as draft then flip | 4 | 1 | 1 | Tags can currently ship untested code |
| 10 | Windows runtime smoke (install NSIS silently → launch → window/log → uninstall) on `windows-latest`, nightly + release | 4 | 3 | 3 | Only post-merge marker check exists today |
| 11 | Coverage: `cargo-llvm-cov` + `vitest --coverage`, publish per-PR comment, then enforce floors | 3 | 2 | 1 | Needed to make the target below measurable |
| 12 | Wire `LUDEX_GIT_COMMIT` into server/Docker builds and assert the exact SHA in health tests | 2 | 1 | 1 | Always `"unknown"` today; tests are too weak |
| 13 | `ludex-backup` integration tests (backup → verify → restore → row counts, `--force`, `--dry-run`, PG refusal) + restore `schema_version` guard | 3 | 2 | 2 | Shipped operator tool with zero tests |
| 14 | S3 backend integration test against MinIO; make `object_store` an optional `s3` feature | 3 | 3 | 2 | Cuts duplicate reqwest/rand/sha2 stack for everyone else |
| 15 | E2E expansion: download pause/resume/cancel, settings persistence, admin game edit/bulk, collections/share, mobile viewport, auth failure | 3 | 4 | 2 | 5 specs is a smoke gate, not coverage |
| 16 | Pin one exact toolchain (`rust-toolchain.toml`, Docker, CI) + `rust-version`; add `LUDEX_GIT_COMMIT` Docker `ARG` | 2 | 1 | 1 | Three toolchain notions today |
| 17 | `.dockerignore` + `docker run` health/runtime smoke in the `docker` job; move root PNGs to `docs/assets` | 2 | 1 | 1 | Local builds upload ~14 GB; image never executed |
| 18 | Split `tests/api.rs` (3 596 lines, 24 tests) and `postgres.rs` mega-test; add per-binary timing (`cargo nextest`) | 2 | 2 | 1 | Failure localization only |
| 19 | Reduce fixed sleeps: inject a clock for TTL/rate-limit tests (`uploads.rs:282`, `virustotal.rs:124`) | 2 | 2 | 2 | Removes ~4 s and the main flake source |
| 20 | Worktree hygiene: `scripts/worktree-prune.sh` + document lanes; drop stale `ludex-wt` trees | 1 | 1 | 1 | 81 worktrees / 3.7 GB, undocumented |

### 6.1 Five quick wins (one day or less)

1. **Desktop tests in CI** (row 1): `cargo test --offline` under `apps/desktop/src-tauri` on ubuntu + windows; keep `--all-targets` for check. 839 tests for ~1–2 min of cold compile.
2. **Docs truth pass** (row 2): README status/features, FEATURE-PARITY banner, `vX.Y.Z` placeholders in DEPLOY/GITOPS, CONTRACT overlay default.
3. **`--locked` + `--all-targets` + `embed-web`** (row 3): three YAML edits; also run `cargo test --doc`.
4. **Edge/stable publish fixes** (row 4): replace tauri-action's release upload with build-only, then one serialized publish job that rebuilds `latest.json` from the matrix and writes `SHA256SUMS`. This is the difference between a working early channel and a dead one.
5. **Dependency scanning** (row 5): enable Dependabot alerts/security updates in repo settings, add a `deny.toml` + `cargo deny check` job, and `pnpm audit --prod --audit-level=high`.

### 6.2 Proposed coverage / CI target

**Every PR runs four suites, all required:** workspace Rust (607), desktop Rust (839), web (851), Playwright (≥5 now, target ≥12). Current total 2 302 tests; the target is 100 % of them executed, not more tests for their own sake.

**Coverage floors after a 2-week baseline** (start advisory, then required):

- `ludex-core`: ≥ 85 % lines (pure logic, cheap to reach).
- `ludex-server`: ≥ 70 % lines / ≥ 60 % branches (integration-heavy).
- `ludex-desktop`: ≥ 60 % lines, with the install engine and `db.rs` ≥ 75 %.
- `apps/web`: ≥ 65 % lines on `src/lib` + `src/state`; screens measured via E2E only.
- Structural (hard gates, no percentage): 100 % of `Auth::Session/Editor/Admin` routes covered by the 401/403 matrix; SQLite↔Postgres schema diff = 0; every migration has a mirror; every release artifact has a `SHA256SUMS` entry; every shipped binary is exercised at least once (server health, `ludex-backup --help`, desktop launch).

**Pipeline shape target:**

- PR wall clock ≤ 12 min p50 (today 18.5 min, critical path edge Windows which should not run on PRs).
- `paths-ignore` for `docs/**`, `**/*.md`, `apps/web/screenshots/**`, `*.png` at root.
- Required checks: `Rust`, `Web`, `Desktop test (linux)`, `Desktop test (windows)`, `PostgreSQL`, `Docker smoke`, `E2E`.
- Nightly: Windows runtime smoke, coverage report, full PG matrix, `cargo deny`.
- Release ≤ 25 min p50: draft release, all artifacts first, single publish step, `latest.json` + `SHA256SUMS` generated once.

---

## Appendix A — How the numbers were obtained

- Counts: `grep -c '#\[\(tokio::\)\?test\]'` per file (Rust), `it(`/`test(` over `apps/web/src/**/*.test.ts`, `playwright.config.ts` for E2E.
- Web: `pnpm --filter @ludex/web test` → `Test Files 72 passed, Tests 851 passed, 9.46 s`.
- Desktop: prebuilt test binary `apps/desktop/src-tauri/target/debug/deps/ludex_desktop_lib-7783e02a8ec8130d --test-threads=4` → `839 passed; 0 failed; 15.76 s`.
- CI/release timings: `gh run view 43767346190 --json jobs`, `gh api repos/cleeryy/ludex/actions/runs/43767346190/jobs`.
- Branch protection/security: `gh api repos/cleeryy/ludex/branches/main/protection`, `gh api repos/cleeryy/ludex/actions/permissions/workflow`, `gh api repos/cleeryy/ludex`.
- Release metadata: `gh release view v0.4.14 --json assets`, `gh release view edge --json assets` (524 assets, 6.4 GB), downloaded `latest.json` for both channels.
- Contract/router diff: parsed `openapi::ROUTES` (231 operations) and compared normalised `/api/...` literals in `docs/CONTRACT.md`; 3 false positives only.
- No test suite was run for the server workspace during this audit beyond CI evidence (the release/tag commit is green); desktop and web suites were run read-only against existing build artifacts.

## Appendix B — Raw CI job durations

```
CI 43767346190 (main, 2026-11-15T22:05:57Z → 22:24:29Z, 18m32s)
  Web (install, build, typecheck)      1m08s
  Desktop (macOS cargo check)          1m28s
  Desktop (cargo check)                1m32s
  Rust (PostgreSQL backend)            1m58s  (test step 58s)
  Rust (fmt, clippy, test)             4m26s  (fmt 1s, clippy 16s, test 3m41s)
  E2E                                  2m45s
  Desktop smoke (xvfb)                 3m25s
  Docker image (build only)            8m10s
  Edge desktop (Linux x64)             8m56s
  Edge desktop (Windows x64)          14m00s
  Edge release (main)                  2m55s

Release 43767349512 (tag v0.4.14, 30m33s)
  Verify (E2E)                         3m18s
  Create GitHub release                   8s
  Server binary (linux x86_64)         5m06s
  Docker image (GHCR)                  7m38s
  Desktop (Linux x64)                 13m36s
  Desktop (Windows x64 NSIS)          20m27s
  Desktop (macOS universal)           26m46s  ← critical path
```
