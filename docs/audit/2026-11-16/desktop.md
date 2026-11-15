# Ludex desktop crate — quality & cleanliness audit

**Target** `/root/ludex/apps/desktop/src-tauri` @ `v0.4.14` (commit `7199a2c`)
**Scope** `src/**` (70 `.rs`), `Cargo.toml`, `tauri*.conf.json`, `capabilities/`, `build.rs`, inline tests, platform code.
**Mode** read-only static audit + one local `cargo test --lib` run. No repository file was modified.

---

## 0. Executive summary

| Metric | Value |
|---|---|
| Rust files / LOC | 70 / **90,763** |
| Files > 1,000 LOC | 26 (37 %) |
| Largest file | `install.rs` 7,995 LOC (8.8 % of the crate) |
| Tauri commands declared / registered | **167 / 167** (consistent) |
| `#[tauri::command]` per file | `commands.rs` 129, `runtime/commands.rs` 9, `roms` 7, `overlay` 7, `remote_play` 6, others 9 |
| Unit tests | **857 executed, 0 failed** (2.23 s) — 859 `#[test]` attributes, minus 2 `#[cfg(windows)]`-only (`paths.rs:740`, `process.rs:225`); all inline `#[cfg(test)]` |
| `.unwrap()` total | 1,785 — **≈ 0 in production** (all inside `#[cfg(test)]`) |
| `.expect(` total | 125 — **1 in production** (`lib.rs:433`) |
| `panic!` / `todo!` / `unreachable!` | 29 — **all in tests** |
| `cfg(...)` occurrences | 259 · `cfg(windows)` 49 · `cfg(not(windows))` 22 · `cfg(all(unix, not(macos)))` 21 |
| `unsafe` blocks | 12 (11 Win32 FFI in `overlay.rs`, 1 `SHGetKnownFolderPath` in `saves_registry.rs`) |
| Rust events emitted / listened by web | 27 / 25 — 1 dead emission (`overlay://state`), 0 orphan listeners |
| Desktop CI coverage | **`cargo check --locked` only** (Linux + macOS); no fmt, no clippy, no tests |

**Headline**: this is a disciplined crate — near-zero production `unwrap`, doc-comment-first modules, security reasonings encoded in code, real tests for the hard parts. The problems are structural, not sloppy: one 8k-line install module, a 5.4k-line command file with 2 tests, non-transactional DB migrations, a silent startup-failure path, and a CI job that never runs the 857 passing tests.

**Five findings that matter most**

1. **CI never runs the desktop tests.** `cargo fmt/clippy/test --workspace` target the *root* workspace; `apps/desktop/src-tauri` declares its own `[workspace]` and is therefore excluded. The desktop job is `cargo check --locked` only (`.github/workflows/ci.yml:145-206`). **857 passing tests, 0 enforced** — and no `rustfmt`/`clippy` either.
2. **Non-transactional SQLite migrations can brick startup.** `db::migrate` (`src/db.rs:613-728`) runs 20 `execute_batch` steps with `ALTER TABLE` and no `BEGIN/COMMIT`; `user_version` is written only after all steps succeed (`db.rs:719-723`). A failure mid-migration (disk full, duplicate column after a partial run) leaves a half-migrated schema that fails again on every launch — and `AppState::new` propagates the error (`src/state.rs:348-372`, `src/lib.rs:174-179`), so **the app never opens**.
3. **Startup failures are silent on Windows.** `windows_subsystem = "windows"` (`src/main.rs:2`) means no console; file logging is installed *after* `AppState::new` (`src/lib.rs:185-189`). A failed state build records `diagnostics.json` and returns `Err` from `setup` — no window, no dialog, no log, no retry/reset path. Same loop on every launch.
4. **Windows settings save deletes the destination before renaming** (`src/settings.rs:1452-1470`). A crash/power loss between `remove_file` and `rename` leaves no `settings.json`; `load` then silently falls back to defaults (`settings.rs:1395-1417`) — server list, mode, install dirs, token gone, and the profile DB (`active_server_id`) is orphaned.
5. **`panic = "abort"` + zero `catch_unwind` ⇒ one panic kills the whole app.** Any panic in a worker (install worker, overlay listener thread, process watcher, `spawn_blocking` task) aborts the process mid-install. The panic hook records a report first, but there is no task-level isolation anywhere.

---

## 1. Layer map

```
main.rs (6)                 → ludex_desktop_lib::run()
lib.rs (441)                → tracing + console receipt + WebView2 reset + Builder:
                              plugins (single-instance, dialog, notification, opener,
                              window-state, updater), window-event policy,
                              setup (AppState, remote-play cleanup, tray, presence,
                              overlay listener, 12 startup tasks),
                              167 generate_handler entries, run loop.
   │
   ├─ state.rs (615)        AppState: settings, db (single rusqlite Connection), jobs map,
   │                        processes map, installers map, runtimes map, HTTP client,
   │                        NetworkGate, SlotLimiter×2, SpaceLedger, SecretStore,
   │                        LoginSessions, OverlayRuntime, DiagnosticsStore.
   │
   ├─ commands.rs (5,358)   167-entry IPC surface (129 commands) — thin delegation,
   │                        plus console/Sunshine publishing and diagnostics glue.
   │
   ├─ INSTALL DOMAIN
   │   install.rs (7,995)   queue, scheduler dispatch, persistence, download+retry,
   │                        Range resume, verification, extraction (zip/7z/tar),
   │                        staging lifecycle, stream fallback, local archive.
   │   pipeline.rs (335)    pure admission policy (slot × space ledger).
   │   space.rs (650)       per-volume reservation ledger.
   │   stream_install.rs    per-entry resumable stream install (2,409, 0 pub API).
   │   file_manifest.rs     file-manifest transport engine (2,044, #231).
   │   archive_verify.rs    pre-extraction SHA-256 proof (693, #216).
   │   installer.rs (1,494) silent setup.exe automation (#7).
   │   runtime/** (5,220)   "Make it run" guided runtime installer (#178):
   │                        catalog → detect → plan → verified install.
   │
   ├─ VERIFY / MANIFEST
   │   manifest.rs (497)    per-install file manifest (build/verify/repair).
   │   maintenance.rs        move / verify / repair installs (1,173).
   │
   ├─ DB / SETTINGS / PATHS
   │   db.rs (5,530)        SCHEMA_V1..V20 inline, 124 pub fns, all local tables.
   │   settings.rs (2,450)  LocalSettings, normalize/validate/ensure_dirs, save.
   │   locations.rs (551)   named install/download/temp locations (#99).
   │   profiles.rs (893)    multi-server profiles, per-profile DB adoption (#24).
   │   secrets.rs (446)     keychain (secret-tool/security) + SQLite fallback.
   │   paths.rs (894)       traversal guards, dir stats, native path normalisation.
   │
   ├─ LAUNCH / SESSION
   │   launch.rs (1,940)    spawn, session record, watcher thread, playtime sync.
   │   linux_gaming.rs       UMU/Proton/Wine planning (#89), prefix.rs winetricks.
   │   process_tree.rs       descendant tracking (#26); process.rs spawn helper (#181).
   │   window_focus.rs       focus-on-launch ladder per platform (#219).
   │   resume.rs (251)       live session / resume running game (#191).
   │   roms.rs (656)         emulator registry + ROM launch (#52).
   │
   ├─ UI SHELL
   │   tray.rs, window/chrome.rs, overlay.rs + detector.rs + global_listener.rs (gilrs),
   │   console.rs (--console / Sunshine shell #172/#227), webview_profile.rs.
   │
   ├─ STREAMING / REMOTE
   │   streaming.rs (4,249) Sunshine apps.json merge w/ backup + Moonlight launch (#97/#98).
   │   remote_play.rs (3,750) UPnP port mapping wizard, VPN-first (#179).
   │   remote_install.rs      phone-queued installs + device heartbeat (#167/#184).
   │
   ├─ SAVES
   │   saves_detect.rs (4,106) Ludusavi manifest matching (#107).
   │   saves_registry.rs (2,786) template resolution / path overrides.
   │   saves_sync.rs (2,643) zip snapshot upload/restore with backup (#108).
   │   saves_manifest.rs (1,163) manifest download with ETag throttle (#153).
   │   config_fields.rs / config_adapters.rs / game_config.rs — config editing (#105/#106/#148).
   │
   ├─ IMPORT / ADOPT
   │   adopt.rs (2,606), launchers.rs (3,076), steam.rs (785), game_executables.rs (859),
   │   compat.rs (703), storage_suggest.rs (1,010), protondb.rs (308).
   │
   └─ PLATFORM / DIAG
       elevate.rs (1,700, Windows UAC closed-enum), updater.rs + updates.rs +
       update_notify.rs + install_health.rs, diagnostics.rs, logging.rs, util.rs,
       bridge.rs, server.rs (embedded ludex-server), oidc_login.rs.
```

Startup order (`src/lib.rs:69-431`): tracing → `--console` receipt → WebView2 cache reset → builder/plugins → `setup` → console session state → panic hook → `AppState::new` → file logging → remote-play cleanup → tray/presence/overlay → async task: `install_health` → `server::autostart` → `auto_resume_on_startup` → `rebuild_space_ledger` → `sweep_stale_staging` → `launch::sync_pending` → saves retry worker → remote-install heartbeat → tray refresh → saves manifest auto-update → updater startup + periodic check.

---

## 2. Per-file inventory

Quality score: **5** exemplary · **4** good · **3** acceptable with notable debt · **2** problematic · **1** poor.
Priority: **P1** fix soon · **P2** next refactor cycle · **P3** opportunistic.

### 2.1 Core & heavy modules

| File | LOC | Role | Public surface | Coupling | Q | Concrete issues | Prio |
|---|---:|---|---|---|---|---|---|
| `src/install.rs` | 7995 | Queue, scheduler, persist, download, verify, extract, staging, fallback | 22 pub items, 40 `pub(crate)` | db, space, pipeline, paths, archive_verify, stream_install, file_manifest, tray, settings | 3 | God module; `execute` = 828 LOC (`install.rs:2513`); `download_into` = 528 LOC (`:4970`); sync `persist()` (SQLite + fs) called from async workers every 250–500 ms (`:2253`); module owns 6 responsibilities; 54 tests but none for `dispatch` scheduling races | P1 |
| `src/db.rs` | 5530 | SQLite index, SCHEMA_V1..20, 124 query fns | 124 pub fn/struct | Everything | 3 | Inline SQL blobs; **non-transactional migrations** (`:613-728`); no backup/integrity_check before migrating; `open_in_memory_for_tests` uses `unwrap` (test-only); 45 tests, all happy-path | P1 |
| `src/commands.rs` | 5358 | 129 Tauri commands | 145 pub items | Every module | 2 | 5.4 k LOC for a supposedly thin IPC layer; **2 tests**; `run_installer` 223 LOC (`:869`), `protondb_rating` 217 (`:2801`), `launcher_import` 185 (`:1918`); domain logic (Sunshine publishing `:4443-4560`, save-candidate promotion `:3098-3414`) lives here, not in its module | P1 |
| `src/streaming.rs` | 4249 | Sunshine apps.json merge + backups, Moonlight launch, console art | 83 pub items | elevate, paths, settings, db | 4 | 5 tests for 4.2 k LOC; JSON merge logic reimplemented in-house; `#[allow(dead_code)]` ×3 (`:893,1060,1287`) | P2 |
| `src/saves_detect.rs` | 4106 | Ludusavi manifest → save paths | 38 pub items | saves_registry, db, settings | 3 | 4.1 k LOC in one module; heavy fs scanning with `std::fs` in blocking contexts; `detect_at` 267 LOC (`:382`) | P2 |
| `src/remote_play.rs` | 3750 | UPnP port mapping wizard | 101 pub items | streaming, settings, http | 4 | Own HTTP/UPnP stack over `std::net`; correctly wrapped in `spawn_blocking` (`:2365,2412,2611,2707,2798`); 33 tests | P3 |
| `src/launchers.rs` | 3076 | Launcher discovery/import (#116) | 49 pub items | paths, db, settings | 3 | Platform heuristics; 32 tests; overlaps `steam.rs`/`adopt.rs` import machinery | P3 |
| `src/saves_registry.rs` | 2786 | Template resolution, overrides | 36 pub items | paths, settings | 4 | 21 tests; one `unsafe` Win32 known-folder block (`:536`) | P3 |
| `src/saves_sync.rs` | 2643 | Cloud save zip snapshot/restore | 40 pub items | db, saves_detect, http | 4 | Restore = backup → extract; **no rollback if extract fails mid-way** (`:1544-1566`) | P2 |
| `src/adopt.rs` | 2606 | Adopt existing folders (#129) | 30 pub items | paths, db | 4 | Deletion guards are genuinely good (`validate_deletable:276`, `plan_delete:618`, `ensure_no_symlink_escape:311`) | P3 |
| `src/settings.rs` | 2450 | LocalSettings + persistence | 40 pub items | profiles, linux_gaming | 4 | **Windows save deletes then renames** (`:1452-1470`); invalid JSON ⇒ rename + defaults (`:1395-1417`); container-level `#[serde(default)]` saves compat (`:184`) | P1 |
| `src/stream_install.rs` | 2409 | Per-entry resumable zip stream | 4 `pub(crate)` | install only | 3 | 2.4 k LOC behind one entry point; 13 tests; 18 `expect` (all test) | P2 |
| `src/overlay.rs` | 2298 | Overlay runtime, Win32 focus/WndProc, window mgmt | 50 pub items | settings, console, tray, db | 4 | 11 `unsafe` Win32 blocks (`:1347-1675`); one giant `open_with` (`:1050`); emits dead event `overlay://state` (`:950`) | P3 |
| `src/game_config.rs` | 2071 | Safe text config editing | 19 pub items | db, paths | 4 | Hand-written INI/TOML/JSON span parsers (justified: no format-preserving dep); encodings UTF-16/1252 handled | P3 |
| `src/file_manifest.rs` | 2044 | File-manifest transport (#231) | 0 pub (crate) | install | 4 | Prototype gated on measurement; `fetch_file` 254 LOC (`:838`) | P3 |
| `src/config_fields.rs` | 1955 | Common config fields (#148) | 15 pub items | db, game_config | 3 | `match_from` 408 LOC (`:549`); sync writes game files on launch | P3 |
| `src/launch.rs` | 1940 | Launch, session, watcher, playtime | 7 pub items | db, process_tree, window_focus, presence, tray | 4 | Thread-per-game watcher (`:1152`); sync `stop()` does `kill_tree` on the IPC thread (`:1258`) | P3 |
| `src/elevate.rs` | 1700 | Windows UAC one-shot | 25 pub items | paths, process, streaming | 5 | Closed `ElevationOperation` enum, path validation, encoded commands — model for the rest of the IPC surface | P3 |
| `src/runtime/plan.rs` | 1561 | Diagnosis → numbered plan | 20 pub items | runtime/* | 4 | 18 tests; pure state machine | P3 |
| `src/installer.rs` | 1494 | Silent setup.exe automation | 27 pub items | process, paths | 3 | Runs downloaded installers silently; timeout only; no signature check (by design, documented) | P3 |
| `src/runtime/install.rs` | 1439 | Verified runtime install | 20 pub items | catalog, detect | 4 | 15 tests; allow-listed hosts, size+SHA256 from catalog | P3 |
| `src/runtime/commands.rs` | 1228 | 9 "Make it run" commands | 14 pub items | plan/install/detect | 2 | **0 tests**; `make_it_run_apply` 292 LOC (`:275`), `apply_step` 256 (`:567`) | P2 |
| `src/maintenance.rs` | 1173 | Move/verify/repair | 10 pub items | manifest, db, paths | 3 | `repair_install` 349 LOC (`:518`); move updates DB *after* moving files (crash ⇒ orphan row, files intact) | P2 |
| `src/saves_manifest.rs` | 1163 | Ludusavi manifest download | 31 pub items | settings, http | 4 | ETag/24 h throttle; 14 tests | P3 |
| `src/config_adapters.rs` | 1074 | Engine adapters (#106) | 6 pub items | game_config | 3 | 9 tests only | P3 |
| `src/storage_suggest.rs` | 1010 | Drive-based install suggestions | 10 pub items | settings, space | 4 | 11 tests | P3 |
| `src/install_health.rs` | 960 | Post-update binary self-check (#186) | 23 pub items | updater, diagnostics | 5 | Scans exe for build stamp; bounded reads; 14 tests | P3 |
| `src/paths.rs` | 894 | Traversal guards, dir stats | 19 pub items | everywhere | 5 | `remove_installed_dir` canonicalises and refuses outside base (`:193-221`); 13 tests | — |
| `src/profiles.rs` | 893 | Multi-server profiles (#24) | 16 pub items | db, secrets, settings | 4 | `adopt_legacy_database` copies (not moves) the legacy DB + WAL sidecars (`:448-482`) — safe, but duplicates disk | P3 |
| `src/game_executables.rs` | 859 | Executable picker cache (#143) | 9 pub items | paths | 4 | 8 tests | P3 |
| `src/runtime/detect.rs` | 827 | Probe-based detection | 18 pub items | catalog | 4 | `Probe` trait for injectable fixtures | P3 |
| `src/runtime/catalog.rs` | 795 | Shipped runtime manifest | 3 pub items | — | 5 | Allow-listed HTTPS hosts + pinned SHA-256 | — |
| `src/steam.rs` | 785 | Steam library import | 17 pub items | paths, db | 4 | 10 tests | P3 |
| `src/diagnostics.rs` | 760 | Crash reports, opt-in send | 22 pub items | logging, http | 4 | Panic hook relies on `try_state::<AppState>()` (`:533`) — panics before `manage` are not captured there (only `record_startup_failure`) | P2 |
| `src/linux_gaming.rs` | 745 | UMU/Proton/Wine planning | 15 pub items | compat, prefix | 4 | 11 tests | P3 |
| `src/updater.rs` | 722 | App self-update | 18 pub items | install_health, settings | 4 | Double responsibility with `updates.rs` (game updates) — names are inverted | P3 |
| `src/presence.rs` | 721 | Discord rich presence | 27 pub items | launch | 4 | Global `OnceLock<Mutex<…>>` (`:325`); worker thread; 12 tests | P3 |
| `src/compat.rs` | 703 | Proton/UMU/Wine detection | 9 pub items | settings, paths | 4 | 5 tests | P3 |

### 2.2 Support, UI shell and platform modules

| File | LOC | Role | Public surface | Coupling | Q | Concrete issues | Prio |
|---|---:|---|---|---|---|---|---|
| `src/archive_verify.rs` | 693 | Pre-extraction SHA-256 proof | 0 pub | install | 4 | 12 tests; digest cache keyed by path | P3 |
| `src/oidc_login.rs` | 669 | Desktop SSO handoff (#157) | 16 pub items | profiles | 5 | 32-byte state, poll timeout, no logging of state/code | — |
| `src/roms.rs` | 656 | Emulator registry + ROM launch | 21 pub items | process, db | 3 | 5 tests; command tokenisation (`validate_command`) is the security boundary | P2 |
| `src/space.rs` | 650 | Per-volume reservation ledger | 22 pub items | paths, pipeline | 5 | Doc says "all methods are cheap and lock a single map" (`:296`) but `try_reserve` calls `fs2::available_space` **under the mutex**; 11 tests | P2 |
| `src/window_focus.rs` | 620 | Focus-on-launch per platform | 13 pub items | launch | 4 | 3 platform implementations + `focus_with` injection; 16 tests | P3 |
| `src/state.rs` | 615 | `AppState` + concurrency primitives | 36 pub items | everything | 4 | `persist_settings` holds the settings mutex across file write + keychain write (`:474-519`); poisoned-lock fallbacks are inconsistent (`snapshot` recovers, `persist_settings` errors) | P2 |
| `src/console.rs` | 606 | `--console` / Sunshine shell | 23 pub items | tray, streaming, overlay | 4 | Launch receipt rotation (16 lines); 9 tests | P3 |
| `src/locations.rs` | 551 | Named locations (#99) | 10 pub items | settings, paths, space | 4 | 9 tests | P3 |
| `src/remote_install.rs` | 550 | Phone-queued installs + heartbeat | 6 pub items | install, http | 3 | 4 tests; heartbeat every 25 s from the startup task | P3 |
| `src/manifest.rs` | 497 | Installed-file manifest | 17 pub items | paths, db | 4 | Name collides conceptually with `file_manifest`/`saves_manifest` | P3 |
| `src/overlay/detector.rs` | 462 | Chord/double-Home gesture FSM | 12 pub items | — | 5 | Pure, 11 tests — best-in-crate example of testable design | — |
| `src/secrets.rs` | 446 | Keychain + fallback store | 14 pub items | profiles | 3 | Fallback SQLite 0600 (Unix only); keys still duplicated in `settings.json` | P2 |
| `src/lib.rs` | 441 | Bootstrap | 1 pub item (`run`) | all | 2 | `run()` = **372 LOC**; 167-entry handler list inline; `.expect("failed to build the Ludex application")` (`:433`) is the only production `expect`; startup failure path is silent (`:174-179`) | P1 |
| `src/test_support.rs` | 418 | Test HTTP server with Range | 6 pub items | — | 4 | Only usable from inline tests; no integration-test crate | P3 |
| `src/overlay/global_listener.rs` | 402 | gilrs supervisor thread | 5 pub items | overlay | 4 | Own thread, capped backoff, hot-plug reset; 5 tests | P3 |
| `src/pipeline.rs` | 335 | Admission policy | 4 pub items | space, install | 5 | Pure `plan_next`, 9 tests, documented fairness rule | — |
| `src/prefix.rs` | 320 | Wine prefix ops | 7 pub items | linux_gaming | 4 | Removes prefixes with path guards | P3 |
| `src/process_tree.rs` | 312 | Descendant tracking | 9 pub items | sysinfo | 4 | 7 tests | P3 |
| `src/protondb.rs` | 308 | ProtonDB ratings | 12 pub items | http, db | 4 | 7 tests, cache TTL | P3 |
| `src/updates.rs` | 307 | **Game** update detection (#30) | 5 pub items | install | 4 | Name inverted vs `updater.rs` (app updates) — genuine reader trap | P2 |
| `src/update_notify.rs` | 300 | OS notification for new build | 9 pub items | tray, updater | 4 | 4 tests; click handler registered in setup | P3 |
| `src/runtime/mod.rs` | 299 | Module root + shared rows | 19 pub items | runtime/* | 4 | 0 tests but mostly types | P3 |
| `src/resume.rs` | 251 | Resume running game | 8 pub items | launch | 4 | 2 tests | P3 |
| `src/tray.rs` | 250 | Tray menu | 7 pub items | db, install | 3 | 1 test; menu rebuilt on every job event | P3 |
| `src/process.rs` | 244 | Spawn helper + `CREATE_NO_WINDOW` | 3 pub items | — | 5 | Source-scan test enforces no bypass of the helper | — |
| `src/webview_profile.rs` | 217 | WebView2 cache reset (#177) | 3 pub items | — | 5 | Best-effort, marker-based retry, 4 tests | — |
| `src/server.rs` | 204 | Embedded server lifecycle | 8 pub items | settings | 3 | **0 tests**; `stop_on_exit` uses `try_lock` and may silently skip shutdown | P3 |
| `src/logging.rs` | 118 | tracing + file mirror | 3 pub items | — | 4 | No rotation in this module (the startup block in `install_health` truncates that other log; `logs/ludex.log` grows unbounded) | P2 |
| `src/util.rs` | 85 | Time/id/format helpers | 5 pub items | — | 4 | 2 tests | P3 |
| `src/bridge.rs` | 80 | Re-export facade over `ludex-core` | 4 pub items | ludex-core | 4 | 0 tests but trivial | P3 |
| `src/chrome.rs` | 79 | Window decorations | 1 pub item | settings | 4 | 1 test; good trade-off documentation | P3 |
| `src/main.rs` | 6 | Entry point | — | — | 5 | — | — |

---

## 3. Cross-cutting analysis

### 3.1 Concurrency & slot model

Well-designed and documented:

- **Two independent resources**: `MAX_CONCURRENT_DOWNLOADS = 2` (setting `downloadConcurrency`, clamped 1–2) and `MAX_CONCURRENT_EXTRACTS = 1` (`state.rs:22-37`). `SlotLimiter` is a CAS counter + `Notify` because Tokio's semaphore cannot shrink (`state.rs:37-117`) — correct, including the "register before check" ordering (`:99-113`).
- **Admission is pure**: `pipeline::plan_next` decides from a candidate list + ledger + capacity, tests with an injected `Capacity` (`pipeline.rs:75-140`).
- **Disk reservations are per volume**, not per path (`space.rs:1-17`), rebuilt at startup (`install.rs:1027-1049`) so a restart never double-counts.
- **Per-job control**: `cancel` `CancellationToken`, `pause` token replaced on resume, `running: AtomicBool` + `RunGuard` to guarantee release (`install.rs:1934-1946`), `retry_now` notify with a permit.
- **Shutdown**: workers release both slots and the ledger reservation before finalize (`install.rs:2024-2030`).

Weak points:

- `SpaceLedger::try_reserve` calls the capacity provider (a real `statvfs`/`GetDiskFreeSpace` syscall) **while holding the mutex** (`space.rs:329+`, `SystemCapacity` at `:96-101`) — blocking I/O under lock, from an async context (`install::dispatch`).
- **Single SQLite connection behind `std::sync::Mutex`** (`state.rs:316`) used from async commands: 8 `db.lock()` sites inside `async fn` bodies (`commands.rs:2554`, `install.rs:2096,3255,3282,3414`) plus `persist()` every 250–500 ms per active job. Every DB touch serialises against the whole app; a slow query blocks an executor thread.
- **Queue can park forever on `WaitingForSpace`**: re-dispatch only happens when a job finishes or the user presses "Check again" (`commands.rs:303-307`, web `downloads.tsx:325`). If the user frees disk space externally (empties trash, ejects another volume), nothing re-runs `dispatch`.
- `dispatch`'s `loop { … continue }` on an already-claimed job (`install.rs:827-886`) relies on `candidates()` filtering `running` to guarantee progress; a future change to that filter turns it into a spin loop. Add an explicit visited set or debug assertion.
- Lock-ordering is mostly "settings then db", but `persist_settings` holds the settings `Mutex` across `settings::save` (fs + fsync) and `secrets.set` (a keychain subprocess on Linux/macOS) (`state.rs:474-519`). A hung `secret-tool` freezes every settings read.

### 3.2 Panic / crash isolation

- **Zero production `unwrap`** (1,785 total, all in `#[cfg(test)]`; the only pre-module test helpers are `saves_registry.rs:2065-2119`).
- **One production `expect`**: `lib.rs:433` `.expect("failed to build the Ludex application")`.
- **`panic = "abort"`** in `[profile.release]` (`Cargo.toml`, last block). Consequence: `tokio::spawn`/`spawn_blocking`/`std::thread` panics are *not* joinable errors, they kill the process. No `catch_unwind` anywhere in the crate.
- The panic hook (`diagnostics.rs:533-565`) writes a redacted report, then the process aborts. Panics before `app.manage(state)` are invisible to it (`try_state` returns `None`).
- Practical risk: a panic in `spawn_watcher` (`launch.rs:1152`), `global_listener` (`overlay/global_listener.rs`), or `spawn_focus_watch` (`launch.rs:1098`) takes down a running game session's tracker and the whole app; in-flight installs survive on disk (`.part` + checkpoints) but the user sees a crash.

Recommendation: `panic = "unwind"` in release (or per-target), plus a supervisor around thread bodies that records and restarts instead of dying.

### 3.3 DB schema / migration discipline

- `SCHEMA_VERSION = 20` (`db.rs:133`), append-only string constants, `user_version` gating (`db.rs:613-728`).
- **No transaction around a migration step.** `execute_batch(SCHEMA_Vn)` is atomic per statement only; V14/V16/V17/V18/V19 mix `ALTER TABLE` + `CREATE TABLE`. A partial failure leaves the schema ahead of `user_version` and the next run re-executes `ALTER TABLE … ADD COLUMN`, which fails with "duplicate column name" — permanently.
- **No pre-migration backup**, no `PRAGMA integrity_check`, no `VACUUM`/checkpoint, no size guard.
- **No down-migration guard**: a downgraded build (version > 20) silently skips every block and runs against a newer schema.
- **No recovery path**: `db::open` failure aborts `AppState::new`, which aborts `setup`, which aborts the process. There is no "reset index" or "open in safe mode" command in the 167-command surface.
- Per-profile databases (`profiles.rs:448-482`) copy the legacy DB once; a failed copy leaves the source intact (good) but the copy is not atomic either.

### 3.4 Command surface

- 167 `#[tauri::command]` declarations, 167 registrations — no drift today (verified by count and by prefix).
- Argument naming: snake_case in Rust, camelCase from JS (Tauri v2 convention). No single place defines/validates the surface.
- **No automated check** that `apps/web/src/lib/native.ts` (168 `invoke` sites) stays in sync with `generate_handler!`. A typo becomes a runtime "command not found" only when the user clicks.
- `open_path` (`commands.rs:557-565`) opens **any existing path** through the OS handler: on Windows `ShellExecute` will launch an executable. Every other privileged operation is gated (elevate.rs closed enum, ROM command tokenisation), so this is the weakest link if the webview ever renders attacker-influenced content.
- 55 `async fn` commands; 29 `spawn_blocking` sites — mostly disciplined. The exceptions are the 8 `db.lock()` sites listed above and sync `kill_tree` in `stop_game`.

### 3.5 Event protocol

Emitted (27 app events): `install://progress|jobs|done|verify-progress|manifest-progress|move-progress|repair-progress|error`, `installer://done`, `cloud-save://synced|detected|pending`, `update://available|progress|installed`, `local-server://status`, `overlay://state|open|close|navigate|settings`, `game://focus|exited`, `ui://navigate|launch`, `runtime://progress`, `save-manifest://updated` (+ framework `tauri://window-created`).

- Only **one dead emission**: `overlay://state` (`overlay.rs:950`) is never listened to by the web bundle (which uses `overlay://open`/`close`/`settings`).
- All 25 listened events have a Rust emitter (checked one by one).
- No schema/version on any payload; `install://progress` and `install://jobs` both carry `InstallJob` shapes that must stay aligned with `apps/web/src/lib/types.ts`. The 27-name set is defined nowhere centrally — it is a string convention across 9 modules.

### 3.6 Settings compatibility

Strong: `LocalSettings` is `#[serde(default)]` at the container level (`settings.rs:184`), so adding a field never invalidates an old file; unknown enum values have bespoke `Deserialize` impls (`DownloadStaging`, `OverlayTrigger` migration). `save_settings` merges partial payloads. Defaults/`normalize` migrate the single install/download folder into named locations.

Caveats:
- Invalid JSON ⇒ the file is renamed `settings.json.invalid` and **all** settings silently reset (`settings.rs:1395-1405`). One truncated write (the Windows path below) is enough.
- `AppState::new` rewrites `settings.json` on **every** startup (`state.rs:367`), turning any transient write failure into a startup failure.

### 3.7 Test harness gaps

- 859 `#[test]`/`#[tokio::test]` attributes; **857 run, 857 pass, 0 failed in 2.23 s** (`cargo test --offline --lib`, verified locally on 2026-11-16). Coverage is genuinely good on pure logic (gesture FSM, space ledger, admission, archive verification, save detection, UPnP planning, resize/focus plans).
- **The Tauri layer has no tests at all**: no `tauri::test::mock_builder`, no integration test crate, no command-level test. `commands.rs` has 2 tests for 129 commands; `runtime/commands.rs` has 0 for 9; `server.rs` 0.
- Consequence: the invoke wiring, setup sequence, window lifecycle, event emission and single-instance promotion are only exercised by hand.
- **CI does not run any of it** (see finding 1). Locally `cargo test --offline --lib` builds and runs, but nothing enforces it on push.

### 3.8 Windows-only paths

- 49 `cfg(windows)` blocks across 15 files: `launchers` (9), `streaming` (5), `process` (5), `paths` (5), `overlay` (5), `window_focus` (4), `saves_registry` (3), `runtime/detect` (3), `launch` (3), `steam` (2), `webview_profile`, `storage_suggest`, `space`, `install_health`, `adopt` (1 each).
- CI compiles Windows **only in `release.yml`** (matrix `windows-latest`, plus `verify-windows-install.ps1`). No Windows `cargo check` on PRs, no Windows tests, ever.
- Windows code is therefore the least-verified part of the crate: job objects/process trees, `SHGetKnownFolderPath`, `GetForegroundWindow`/`SetForegroundWindow` ladders, WebView2 profile reset, NSIS hand-off, UAC elevation.
- `process.rs` documents a manual Windows checklist (`process.rs:7-22`) — that checklist is the only coverage for console-flash behaviour.

### 3.9 Security posture

Good:
- `elevate.rs`: closed operation enum, absolute-path validation inside expected roots, array arguments / `-EncodedCommand`, one prompt, never automatic (`elevate.rs:1-40,534-620`).
- `runtime/*`: HTTPS allow-list + pinned size/SHA-256 from the shipped catalog, redirects re-checked.
- `remote_play.rs`: never maps 47990/5353, refuses CGNAT/private WAN, short leases, cleanup on startup.
- `design`: `open_external_url` restricted to http(s) (`commands.rs:5189-5197`); Tauri capabilities are minimal (`core:default` + window ops, no `shell`/`fs` plugin) (`capabilities/default.json`); CSP `script-src 'self'`, `object-src 'none'`, `frame-ancestors 'none'`.
- Secrets: keychain preferred, SQLite fallback at 0600 (Unix), tokens never in logs.

Weak:
- CSP `connect-src http: https: ws: wss:` and `img-src http: https:` allow any origin (needed for self-hosted servers, but broad).
- Active token stored in cleartext `settings.json`; 0600 is applied only on Unix (`settings.rs:1436-1443`). On Windows the file inherits the `%APPDATA%` ACL (per-user, acceptable, undocumented).
- `open_path` as described in 3.4.
- `installer.rs` runs setup binaries from downloaded archives with the user's full rights — inherent to the product, but there is no signature/allow-list check beyond the archive hash.
- `remote_install` claims requests using the saved token (good) but the heartbeat runs every 25 s forever, including in offline/console sessions.

---

## 4. Quantification

### 4.1 Top 20 largest files

| # | File | LOC | Share |
|---:|---|---:|---:|
| 1 | `src/install.rs` | 7995 | 8.8 % |
| 2 | `src/db.rs` | 5530 | 6.1 % |
| 3 | `src/commands.rs` | 5358 | 5.9 % |
| 4 | `src/streaming.rs` | 4249 | 4.7 % |
| 5 | `src/saves_detect.rs` | 4106 | 4.5 % |
| 6 | `src/remote_play.rs` | 3750 | 4.1 % |
| 7 | `src/launchers.rs` | 3076 | 3.4 % |
| 8 | `src/saves_registry.rs` | 2786 | 3.1 % |
| 9 | `src/saves_sync.rs` | 2643 | 2.9 % |
| 10 | `src/adopt.rs` | 2606 | 2.9 % |
| 11 | `src/settings.rs` | 2450 | 2.7 % |
| 12 | `src/stream_install.rs` | 2409 | 2.7 % |
| 13 | `src/overlay.rs` | 2298 | 2.5 % |
| 14 | `src/game_config.rs` | 2071 | 2.3 % |
| 15 | `src/file_manifest.rs` | 2044 | 2.3 % |
| 16 | `src/config_fields.rs` | 1955 | 2.2 % |
| 17 | `src/launch.rs` | 1940 | 2.1 % |
| 18 | `src/elevate.rs` | 1700 | 1.9 % |
| 19 | `src/runtime/plan.rs` | 1561 | 1.7 % |
| 20 | `src/installer.rs` | 1494 | 1.6 % |

Top 20 = 57,621 LOC = **63.5 %** of the crate.

### 4.2 Grep counts (production vs tests split by first `#[cfg(test)]`)

| Pattern | Total | Production |
|---|---:|---:|
| `.unwrap()` | 1785 | **0** (+6 in test-only helpers) |
| `.expect(` | 125 | **1** (`lib.rs:433`) |
| `panic!` / `todo!` / `unimplemented!` / `unreachable!` | 29 | **0** |
| `catch_unwind` | 0 | 0 |
| `#[allow(dead_code)]` | 8 | 8 |
| `TODO`/`FIXME`/`HACK`/`XXX` | 0 | 0 |
| `spawn_blocking` call sites | 90 | 90 |
| `std::thread::sleep` | 9 | 9 (all inside blocking contexts) |
| `std::process::Command::new` | 30 | 30 (routed through `process::command/capture`) |
| `unsafe` blocks | 12 | 12 (Win32 FFI only) |

Largest functions:

| Function | LOC | File:line |
|---|---:|---|
| `execute` | 828 | `install.rs:2513` |
| `download_into` | 528 | `install.rs:4970` |
| `run` (bootstrap) | 372 | `lib.rs:69` |
| `stream_install::install` | 360 | `stream_install.rs:337` |
| `repair_install` | 349 | `maintenance.rs:518` |
| `make_it_run_apply` | 292 | `runtime/commands.rs:275` |
| `detect_at` | 267 | `saves_detect.rs:382` |
| `begin` (install) | 263 | `install.rs:564` |
| `apply_step` | 256 | `runtime/commands.rs:567` |
| `fetch_file` | 254 | `file_manifest.rs:838` |
| `launch` | 252 | `launch.rs:130` |
| `manifest_install` | 252 | `install.rs:3699` |

### 4.3 Test density (tests per 1,000 LOC, files with ≥ 600 LOC)

| File | Tests | per 1k LOC |
|---|---:|---:|
| `overlay/detector.rs` | 11 | 23.8 |
| `pipeline.rs` | 9 | 26.9 |
| `space.rs` | 11 | 16.9 |
| `window_focus.rs` | 16 | 25.8 |
| `remote_play.rs` | 33 | 8.8 |
| `saves_detect.rs` | 33 | 8.0 |
| `install.rs` | 54 | 6.8 |
| `db.rs` | 45 | 8.1 |
| `streaming.rs` | 65 | 15.3 |
| `commands.rs` | **2** | **0.4** |
| `runtime/commands.rs` | **0** | **0.0** |
| `lib.rs` | **0** | **0.0** |
| `server.rs` | **0** | **0.0** |

### 4.4 Command count by file

`commands.rs` 129 · `runtime/commands.rs` 9 · `roms.rs` 7 · `overlay.rs` 7 · `remote_play.rs` 6 · `updater.rs` 3 · `resume.rs` 2 · `install_health.rs` 2 · `update_notify.rs` 1 · `remote_install.rs` 1 = **167**.

---

## 5. Top 20 refactors (impact / effort / risk)

| # | Refactor | Impact | Effort | Risk | Notes |
|---:|---|---|---|---|---|
| 1 | **Run desktop fmt + clippy + tests in CI** (new job: `cargo fmt --check`, `clippy --all-targets -D warnings`, `test --lib` in `apps/desktop/src-tauri`) | High | S | Low | Nothing else in this list is enforced until this exists. `ci.yml:145-206` currently runs `cargo check` only. |
| 2 | **Transactional migrations + pre-migration backup + downgrade guard** (`db.rs:613-728`): wrap each version in `BEGIN…COMMIT`, copy `ludex-desktop.db` before migrating, refuse `version > SCHEMA_VERSION` | High | M | Med | Removes the only known permanent-startup-failure path. Test: inject a failing statement at V20 and assert the DB still opens. |
| 3 | **Startup recovery path**: on `AppState::new` failure show a native error window/dialog with "Open log", "Reset index (backup first)", "Quit" instead of returning `Err` from `setup` (`lib.rs:174-179`) | High | M | Low | Windows release currently fails invisibly. Reuse `install_health`'s log path. |
| 4 | **Split `lib.rs::run`** into `bootstrap/{plugins,window_events,startup,handlers}.rs` (372 LOC → 4 × ~90) | High | S | Low | Handler list moves to `ipc.rs::handlers!` macro or a generated list. |
| 5 | **Fix Windows settings save** (`settings.rs:1452-1470`): never delete the destination; retry `rename` with backoff, fall back to `ReplaceFileW`/`MoveFileEx(MOVEFILE_REPLACE_EXISTING)`, keep `settings.json.bak` | High | S | Low | Also makes `AppState::new`'s unconditional save safe. |
| 6 | **Split `install.rs`** into `install/{queue,scheduler,persist,download,verify,staging,execute}.rs`; extract `execute` (828) and `download_into` (528) into named stages | High | L | Med | Keep public API (`begin/cancel/pause/resume/list`) as a facade. |
| 7 | **Thin `commands.rs`**: move domain logic (`sunshine_*` publishing, save-candidate promotion, console plan) into their modules; keep 1–5-line delegations only | High | M | Low | Mechanical but wide; do per-domain with tests. |
| 8 | **Add a Tauri test harness** with `tauri::test::mock_builder`, a temp `AppState`, and tests for the top 20 commands (install lifecycle, launch, settings round-trip, overlay settings) | High | L | Med | Would have caught the Windows save and the queue-stall class of bugs. |
| 9 | **Split `db.rs`** into `db/{schema,installed,jobs,saves,config,runtime,cloud}.rs`; keep `SCHEMA_Vn` in one `schema.rs` with a compile-time count assertion | Med-High | M | Low | 124 pub fns is the largest API surface in the crate. |
| 10 | **`AppState` facade for the DB**: `state.with_db(|conn| …)` / `spawn_blocking` wrapper, replacing 32 raw `state.db.lock()` sites | Med-High | M | Med | Enables moving SQLite to a dedicated thread later; fixes blocking-in-async. |
| 11 | **Auto-recheck `WaitingForSpace` jobs** (timer or on `install_space_ledger` read) | Med | S | Low | Today: `install.rs:955-981` parks forever until another job ends or the user clicks. |
| 12 | **`panic = "unwind"` + task supervision** for release; wrap `spawn_watcher`, `global_listener`, `focus_watch` bodies in a supervisor that records and restarts | Med-High | S | Med | Binary grows ~5–10 %; current abort loses sessions and overlay silently. |
| 13 | **Settings lock scope**: read a snapshot, drop the guard, then write/fsync/keychain (`state.rs:474-519`); serialise writes with a dedicated write mutex | Med | S | Med | Also fixes a hung `secret-tool` freezing settings. |
| 14 | **Contract test for the IPC surface**: snapshot the 167 names + arg names, compare with `native.ts` invokes | Med | S | Low | Catches drift at PR time; also documents the surface. |
| 15 | **Rename for clarity**: `updates.rs` → `game_updates.rs`, `updater.rs` → `app_updater.rs`, `manifest.rs` → `install_manifest.rs`, `file_manifest.rs` → `file_transport.rs`, `saves_manifest.rs` → `ludusavi_manifest.rs` | Med | S | Low | Three different "manifest" concepts today. |
| 16 | **Split `runtime/commands.rs`** and add tests for `make_it_run_apply` consent/rollback paths | Med | M | Low | 0 tests on a privileged installer path. |
| 17 | **Log rotation** for `logs/ludex.log` (size-capped, one rotation) and for `diagnostics.json` (already capped at 20) | Med | S | Low | `logging.rs` appends forever; `install_health` truncates only the *startup* log. |
| 18 | **`#[deny(clippy::unwrap_used, clippy::expect_used)]`** at crate root with `#[cfg_attr(test, allow(...))]` | Med | S | Low | Locks in the existing zero-unwrap discipline; today it is convention only. |
| 19 | **Consolidate duplicated helpers**: `retry_delay` ×3 (`install.rs:4867`, `overlay/global_listener.rs:98`, `saves_sync.rs:817`), `human_*`/`format_*` ×2 | Low-Med | S | Low | Names collide semantically; unify into `util::retry`, `util::fmt`. |
| 20 | **Split `streaming.rs`** into `streaming/{sunshine,moonlight,publish}.rs` and test the merge invariants | Low-Med | M | Low | 4.2 k LOC, 5 tests; edits a user-owned `apps.json` (backups exist). |

**Quick wins (≤ 1 h each)**

- Delete the dead `overlay://state` emission (`overlay.rs:950`) or add the listener.
- Fix the `space.rs:296` doc claim or move `Capacity` calls outside the lock.
- Add a debug assertion in `dispatch` that each `loop` iteration claims a different job.
- Add `cargo fmt --check` for the desktop crate to the existing `rust` job (one extra step).
- Add `RUSTFLAGS`/`--locked` consistency: desktop uses `cargo check --locked` but tests would need the same (`Cargo.lock` is committed).
- `server.rs:197-204`: log when `try_lock` fails on exit instead of silently skipping `shutdown`.

---

## 6. Data-loss & startup crash-loop watchlist

| Risk | Where | Trigger | Impact | Mitigation today |
|---|---|---|---|---|
| **Startup brick** | `db.rs:613-728` + `state.rs:348-372` + `lib.rs:174-179` | Disk full or power loss during an `ALTER TABLE` migration step | App never opens; silent on Windows release | None (no transaction, no backup, no reset command) |
| **Startup brick** | `state.rs:367` (`settings::save(...)?`) | Read-only app data, quota, AV lock on Windows | App never opens | None |
| **Settings wipe** | `settings.rs:1452-1470` | Crash between `remove_file` and `rename` | `settings.json` gone → defaults: server list, mode, install dirs, token; per-profile DB orphaned (`profiles.rs:448-482` keys the DB path on `active_server_id`) | `.tmp` file survives but is never recovered |
| **Half-restored saves** | `saves_sync.rs:1544-1566` | Failure during `extract_archive` after the zip was validated | Save folder partially overwritten | A local backup zip exists and its path is returned in the error — manual restore only |
| **Orphaned install row** | `maintenance.rs:80-190` | Crash after files moved, before the DB row update | Library points at a missing folder; files intact | None; user must re-adopt |
| **Orphaned install row** | `commands.rs:515-527` (`uninstall_game`) | Folder deleted, DB delete fails | Row points at nothing | Reinstall overwrites the row |
| **Lost install progress** | `install.rs:2039-2075` | Any panic (abort) during download | `.part` + checkpoints persist; job resumes on next start | `auto_resume_on_startup` (`install.rs:2314`) |
| **Stalled queue** | `install.rs:955-981` | User frees disk space externally | Job stays `waitingForSpace` indefinitely | Manual "Check again" only |

---

## 7. What is good — do not regress

- **Zero production `unwrap`/`panic!`** across 90 k LOC, and one `expect` (app build). Enforce it with lints (item 18).
- **Pure cores with injected side effects**: `pipeline::plan_next` + `Capacity` trait, `space::SpaceLedger`, `overlay/detector` FSM, `runtime::detect::Probe`, `window_focus::focus_with`, `elevate::ElevationRequest::plan`. This is why 857 tests run without a Tauri runtime.
- **Reasoning is in the code**: every module header states the failure modes and the security rules it enforces (`elevate.rs`, `remote_play.rs`, `runtime/mod.rs`, `file_manifest.rs`, `process.rs`, `chrome.rs`).
- **Path safety**: `paths::remove_installed_dir` canonicalise + prefix check; `adopt::validate_deletable`/`plan_delete`/`ensure_no_symlink_escape`; `stream_install` unsafe-entry rejection.
- **Migration discipline (shape)**: append-only `SCHEMA_Vn`, `user_version`, per-profile DBs. Only the transaction/backup is missing.
- **Settings evolution**: container-level `#[serde(default)]`, tolerant enum deserialisers, explicit migrations in `normalize`.
- **CI builds all three platforms**, and Windows release runs a real install verification script.

---

*Method: `grep`/`awk` inventory over `src/**`; per-file test counts and unwrap split by first `#[cfg(test)]`; `cargo test --offline --lib` executed locally to confirm the suite builds and runs.*

**Local run (2026-11-16, Linux, debug):**

```
cargo test --offline --lib
test result: ok. 857 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.23s
```

The suite is healthy and fast (2.2 s once compiled) — it is simply never executed by CI. No repository file was modified.
