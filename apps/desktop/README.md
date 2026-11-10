# Ludex desktop

Tauri 2 shell around the shared React UI (`apps/web`). It adds the native parts a
browser cannot do: a resumable installer, archive extraction, a local library
server, game launching and playtime tracking.

- Package: `ludex-desktop` · binary: `ludex` · identifier: `cloud.cleeryy.ludex`
- Frontend: `apps/web/dist` (Vite build) or `http://localhost:5173` in dev
- Contract: `docs/CONTRACT.md` (Desktop commands, Embedded server Rust API)

## Requirements

- Rust stable (edition 2021) and the Tauri 2 system dependencies.
- Linux: `webkit2gtk-4.1`, `libappindicator3`/`libayatana-appindicator3`, `librsvg2`,
  `patchelf` for bundling.
- Node 22 + pnpm 9 (workspace root).
- Windows: WebView2 runtime (installed by the NSIS installer if missing).
- macOS: Xcode command line tools.

## Development

From the repository root:

```sh
pnpm install
pnpm --filter @ludex/desktop dev      # tauri dev, starts the Vite dev server
pnpm --filter @ludex/desktop build    # tauri build (bundles for the host OS)
pnpm --filter @ludex/desktop dist     # builds apps/web first, then tauri build
```

From `apps/desktop` the same scripts are available directly (`pnpm dev`,
`pnpm build`, `pnpm dist`). `beforeDevCommand` / `beforeBuildCommand` already
build `../web`, so the UI is never stale.

Rust-only checks (faster, no bundling):

```sh
cd apps/desktop/src-tauri
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
```

### Icons

The icon set in `src-tauri/icons` is generated from the official mark in
`apps/web/public/app-icon.svg`:

```sh
cd apps/desktop
pnpm tauri icon ../web/public/app-icon.svg
```

Regenerate it whenever the brand mark changes; `tauri.conf.json` references the
standard filenames (`32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`,
`icon.ico`).

## Runtime modes

- **Local library** (default): `local_server_start` boots `ludex-server`
  in-process on `127.0.0.1` (random port, or `serverPort` when non-zero), with
  the SQLite database in `{appDataDir}/server` and the user's games folder as the
  library. The returned URL and auto-provisioned admin JWT are stored in
  settings, so the UI never shows a login screen. The server starts
  automatically at launch when a games folder is configured.
- **Remote server**: `mode: "remote"`, `serverUrl` and `token` point at any
  Ludex server.

`local-server://status` is emitted with `{ running, url }` on every change; the
server is shut down on app exit.

## Settings

`{appDataDir}/settings.json`, written atomically (temp file + fsync + rename,
mode 0600 on Unix because it holds the JWT). Defaults:

| Key | Default |
|---|---|
| `mode` | `local` |
| `installDir` | `{appDataDir}/Games` |
| `downloadDir` | empty → `{installDir}/.ludex-tmp` (same volume) |
| `serverPort` | `0` (random) |
| `gamesDir` | `null` until picked |
| `serverUrl` / `token` | filled by the embedded server |
| `streamInstall` | `false` |
| `keepArchives` | `false` |
| `archivePasswords` | `[]` (tried in order before the empty password) |
| `bandwidthLimitKib` | `0` (unlimited; caps installs, repairs and streams) |

`save_settings` deep-merges partial payloads, so a wizard can save one field at
a time. An empty `downloadDir` is meaningful: the effective directory is
`{installDir}/.ludex-tmp`, keeping the archive on the same volume as the
extracted game. `install_game` can also override it per call with `downloadDir`.

## Local index

`{appDataDir}/ludex-desktop.db` (SQLite, WAL, `user_version` migrations):

- `installed_games` — one row per installed game (`game_id` PK, version, title,
  install path, executable, size, timestamps, internal `install_root`).
- `sessions` — play sessions, with `synced` flag for offline playtime.
- `install_jobs` — persisted install jobs: phase, bytes, speed, `.part` path,
  install path, error, timestamps. Interrupted jobs are marked `error` with a
  resume hint at startup; `paused` rows stay paused so they can be resumed after
  a restart. `list_install_jobs` merges active in-memory jobs (fresh snapshots)
  with every persisted row, newest first.
- `install_manifests` — one row per installed game: version, install path and
  the JSON manifest (relative path, size, SHA-256 per file). Written at install
  completion and used by `verify_install` / `repair_install`.

## Install pipeline

Layout: `{installDir}/{slug}/{versionLabel}/game files` (`versionLabel` is the
version or `default`; both components are sanitised, no separators, no `..`).

1. **Local library shortcut** — when the target URL is the embedded server,
   `GET /api/games/{id}/versions/{vid}/local-path` returns the archive path; it
   is extracted in place, with no copy and no download.
2. **Stream install (remote ZIP)** — when `stream`/`streamInstall` is enabled,
   the server is not loopback and the version is a ZIP, the desktop calls
   `GET /api/games/{id}/versions/{vid}/stream` and unpacks the uncompressed tar
   **while downloading**, with no archive or tar on disk. A 415/404 (or a
   non-tar content type) falls back transparently to the normal download path.
   Progress uses `Content-Length` when exact, else `X-Ludex-Estimated-Bytes`,
   else indeterminate; an estimate is clamped below 100% until the tar ends.
   Stream installs are not resumable.
3. **Download** — `GET /api/games/{id}/download?versionId=…` with
   `Authorization: Bearer`, streamed to
   `{downloadDir}/.{gameId}-{versionId}.part` in 1 MiB buffered chunks (never in
   RAM), with `Range` resume, redirects, `Accept-Encoding: identity` and
   `install://progress` events at most every 250 ms (a 400 ms heartbeat covers
   quiet periods). A 416 whose `Content-Range` total equals the part size means
   the download is already complete; the file is kept for extraction instead of
   re-downloading it. Free space is checked before writing: less than the
   archive size aborts with an actionable error. The job message warns that
   peak usage is archive + extracted, and the archive is deleted right after a
   successful extraction unless `keepArchives` is set.
4. **Single-file games** — `.exe`, `.sh`, `.AppImage`, `.bat`, `.cmd` and files
   with a PE/ELF/shebang header are not archives: they are copied into the
   install folder (streamed with progress) and used as the executable directly.
5. **Extraction** — `ludex_core::archive::extract_archive` in
   `spawn_blocking`, staging at `{installDir}/.{slug}.staging-{jobId}`, with the
   configured passwords. Progress uses entries and bytes when available.
6. **Executable detection** — `ludex_core::exe::pick_executable`, or the
   `executable` override from the command (must stay inside the game folder).
   When nothing is found the install still succeeds with `executable: null`;
   the UI can open the folder and pick a binary later. The install folder is
   never deleted in that case.
7. **Finalise** — `fs::rename` staging → final; across devices it falls back to
   a recursive copy with progress. An existing install is kept as a hidden
   backup until the move succeeds, then removed.
8. **Manifest** — every installed file is hashed (relative path, size,
   SHA-256) into `install_manifests`. Hashing progress is reported with the
   finalizing phase. A manifest failure is logged and never fails the install;
   such an install reports `noManifest` until it is reinstalled.
9. **Index** — row upsert in `installed_games`. Failures keep the `.part` file
   for resume and only remove the staging folder.

Only one install runs at a time (a semaphore); additional jobs stay `queued`
with a message and start automatically. `install_game` for a version that is
already installing returns the existing job, and a new attempt after a restart
reuses the `.part` file.

Pause stops the HTTP body read at the next chunk boundary, flushes and keeps the
`.part` file, then marks the job `paused` and releases the install slot. Resume
reuses the same job id and sends `Range: bytes=<offset>-`, so a paused download
continues where it stopped (also after an app restart). Stream installs cannot
be paused because nothing is written to the download folder; `pause_install`
returns an actionable error. The optional `bandwidthLimitKib` setting paces the
copy loop of downloads, repairs and stream installs.

Cancel aborts the download promptly, deletes the `.part`, removes staging and
marks the job `cancelled`. Cancel is cooperative during extraction (the core
extractor is not interruptible mid-entry; staging is removed afterwards).

## Move, verify and repair

- `move_install(gameId, targetDir)` keeps the `{slug}/{versionLabel}` layout
  under the new root. Same filesystem: an atomic rename. Different filesystem:
  copy to a hidden sibling, compare the byte count, rename into place, then
  delete the old folder; any failure removes the partial copy and leaves the
  original untouched. It refuses while the game is running or an install for it
  is active, and rewrites `installed_games`, the executable path and the
  manifest path.
- `verify_install(gameId)` compares sizes first and hashes only files whose size
  matches, reporting `missing`, `corrupt` and `extra` entries plus progress on
  `install://verify-progress`.
- `repair_install(gameId)` verifies, then downloads the version archive (Range
  resume, or the local library file) and extracts it to a temporary folder. Only
  the missing/corrupt files are copied back, after their hashes are checked, so
  intact files and extra user files are never touched. The archive download is
  the whole version archive, not per-file ranges. When no archive is available
  the report has `fallback: true` and the UI asks for a full reinstall.

### Events

| Event | Payload |
|---|---|
| `install://progress` | `InstallJob` |
| `install://done` | `InstallJob` |
| `install://error` | `{ jobId, message }` |
| `install://jobs` | `InstallJob[]` (full refreshed list on queue/finish) |
| `install://move-progress` | `{ gameId, bytesDone, bytesTotal, message }` |
| `install://verify-progress` | `{ gameId, filesDone, filesTotal, bytesDone, bytesTotal, message }` |
| `install://repair-progress` | same as verify plus `phase` |
| `game://exited` | `{ sessionId, gameId, minutesPlayed }` |
| `local-server://status` | `{ running, url }` |

## Launching and playtime

- Windows: the executable is spawned with `current_dir` set to the install
  folder.
- Linux: `.sh` through `sh`, `.exe` through `wine` (clear error when Wine is
  missing), `.AppImage`/ELF get the executable bit, then run directly.
- macOS: `.app` bundles through `open`, other files directly.
- `args` is split with quote support (backslashes kept, Windows paths intact).

A watchdog thread polls the child, records the session (rounded to the nearest
minute), updates `last_launched_at`, emits `game://exited` and posts
`{ minutes }` to `/api/games/{id}/playtime`. Failures leave `sessions.synced = 0`
and are retried at the next launch/exit. `stop_game` kills the direct child
(child processes are not tracked) and the watchdog still reports the session.

## Safety

- All commands return `Result<T, String>` with actionable messages; no panics on
  user paths.
- Slugs, version labels and executable overrides are sanitised and checked
  against the install folder; uninstall canonicalises and refuses paths outside
  the recorded install root.
- Tokens are never logged and only stored in the 0600 settings file.
- The CSP allows `http:`/`https:`/`ws:`/`wss:` connections so LAN servers on
  plain HTTP work, plus `data:`/`blob:`/remote images.
- Single-instance plugin: launching Ludex again focuses the existing window.

## Bundle targets

`tauri.conf.json` targets `deb` + `appimage` on Linux, `nsis` on Windows and
`app` + `dmg` on macOS (platform configs `tauri.linux.conf.json`,
`tauri.windows.conf.json`, `tauri.macos.conf.json`). Windows builds are produced
by CI; no code signing is configured.

## Known limitations

- Stream installs are not resumable and cannot be paused (no `.part`); a
  mid-stream failure removes the staging folder and the retry starts from
  scratch.
- Repair re-downloads the whole version archive and restores only the damaged
  files; the server has no per-file endpoint yet.
- Cross-drive moves need free space for a second copy and are untested on
  Windows in CI.
- Reinstalling into a different `installDir` leaves the previous folder on disk
  (the row is replaced).
- Extraction progress cannot be cancelled mid-archive; cancellation is applied
  between phases.
- `stop_game` kills the direct child only; child processes are not tracked.
