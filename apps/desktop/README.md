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
| `installDir` | `{appDataDir}/Games` (mirrors the default install location) |
| `downloadDir` | empty → `{installDir}/.ludex-tmp` (same volume; mirrors the default download location) |
| `locations` | `[]` → migrated to one install location (plus a download location when `downloadDir` was set) (#99) |
| `serverPort` | `0` (random) |
| `gamesDir` | `null` until picked |
| `serverUrl` / `token` | filled by the embedded server |
| `streamInstall` | `false` |
| `keepArchives` | `false` |
| `archivePasswords` | `[]` (tried in order before the empty password) |
| `bandwidthLimitKib` | `0` (unlimited; caps installs, repairs and streams) |
| `closeToTray` | `true` (closing the window hides to the tray) |
| `updateChannel` | `stable` (`early` follows the `edge` prerelease) |
| `checkUpdatesOnStartup` | `true` (at most once per 24 h) |
| `lastUpdateCheckAt` | `null` until the first check |
| `discordPresence` | `false` (Discord Rich Presence, #12) |
| `discordClientId` | empty (Discord application id, not a bot token) |
| `installerSilentArgs` | `[]` → built-in silent switch presets (#7) |
| `installerTimeoutSecs` | `600` (clamped to 30–7200) |
| `steamRoot` | empty → Steam auto-detection (#13) |
| `compatUmuPath` | empty → UMU auto-detection (#90) |
| `compatProtonPath` | empty → Proton auto-detection (#90) |
| `compatWinePath` | empty → Wine auto-detection (#90) |
| `defaultLaunchMode` | `auto` (new Linux games, #89) |

`save_settings` deep-merges partial payloads, so a wizard can save one field at
a time. An empty `downloadDir` is meaningful: the effective directory is
`{installDir}/.ludex-tmp`, keeping the archive on the same volume as the
extracted game. `install_game` can also override it per call with `downloadDir`.

`locations` is the source of truth for #99. Each entry has a label, an absolute
path, a kind (`install`, `download` or `temp`) and one default per kind. The two
legacy folder fields mirror the defaults, so older web bundles keep working;
`save_settings` maps a legacy `installDir`/`downloadDir` patch onto the matching
default location instead of ignoring it. Missing folders are tolerated when
saving (an unplugged USB drive must not block Settings): they are probed when a
job starts, and only that job fails with an actionable message.

## Local index

`{appDataDir}/ludex-desktop.db` (SQLite, WAL, `user_version` migrations):

- `installed_games` — one row per installed game (`game_id` PK, version, title,
  install path, executable, size, timestamps, archive fingerprint, internal
  `install_root`).
- `sessions` — play sessions with `started_at`, `ended_at`, rounded `minutes`
  (sent to the server), exact `seconds` (local reports) and a `synced` flag.
- `install_jobs` — persisted install jobs: phase, bytes, speed, `.part` path,
  install path, error, timestamps and `stream_completed` (hex bitset of the
  streamed entry indexes, #115). Interrupted jobs are marked `error` with a
  resume hint at startup; `paused` rows stay paused so they can be resumed after
  a restart. `list_install_jobs` merges active in-memory jobs (fresh snapshots)
  with every persisted row, newest first.
- `install_stream_manifests` — one row per per-entry stream attempt: staging
  folder, the manifest JSON and its SHA-256 fingerprint, entry count and total
  bytes. Written once per attempt; the checkpoint bitset lives on the job row so
  completing an entry does not rewrite a multi-megabyte manifest.
- `install_manifests` — one row per installed game: version, install path and
  the JSON manifest (relative path, size, SHA-256 per file). Written at install
  completion and used by `verify_install` / `repair_install`.
- `game_launch_options` — per-game Linux launch mode, tool id, Wine prefix,
  environment overrides (JSON), winetricks verbs (JSON) and manual Steam app id
  (#89, #91, #92). Deleted with the installed row.
- `protondb_reports` — ProtonDB summaries cached by app id with their fetch
  time; stale rows are kept so an offline machine still shows the last tier.
- `cloud_save_prefs` / `cloud_save_restore` — automatic save sync state and the
  last restored snapshot (#108).
- `config_edits` (#105) — one row per game-settings write: relative file path,
  absolute backup path under `{appDataDir}/config-backups/game-<id>/`, line
  diff, size, origin (`manual`, `adapter`, `restore`) and timestamp. Restoring
  a row copies the current file to a new `restore` row first. Schema v11,
  after the stream-checkpoint v10 from #115.

## Install pipeline

Layout: `{installDir}/{slug}/{versionLabel}/game files` (`versionLabel` is the
version or `default`; both components are sanitised, no separators, no `..`).
The install root comes from the per-install location (#99), the download folder
from the per-job download location, with the old single folders as fallbacks.
Before queueing, `install_game` probes the chosen folders for writability and
free space (256 MiB reserve; archive + extracted copy when they share a volume)
and pins the resolved paths on the job. An unplugged location therefore fails
one click immediately, and a paused job keeps its own folders even if Settings
changes later.

1. **Local library shortcut** — when the target URL is the embedded server,
   `GET /api/games/{id}/versions/{vid}/local-path` returns the archive path; it
   is extracted in place, with no copy and no download.
2. **Stream install (remote only)** — when `stream`/`streamInstall` is enabled
   and the server is not loopback, the desktop first tries the resumable
   per-entry protocol (#115): `GET …/stream/manifest` lists the ZIP central
   directory and `GET …/stream/entries/{index}` serves one entry with Range
   support. Files are written directly into staging (no archive on disk), two
   at a time, then CRC-32 and size are verified before an entry is marked
   complete. Completed indexes are persisted after every file; the manifest is
   persisted once and its fingerprint is compared on the next attempt. A retry,
   pause/resume or app restart keeps completed files, resumes a partial entry
   with `Range: bytes=<offset>-` and downloads only missing entries; the UI
   shows `entriesDone/entriesTotal`, the current entry and `resumedEntries`.
   Cancel stops between entries and keeps completed files when there is at
   least one. A 404/415 (old server, non-ZIP) falls back to the sequential tar
   stream (`GET …/stream`, unpacked while downloading), then to the normal
   download path. `7z`/RAR stay on the sequential stream and are not resumable.
   Progress uses `Content-Length` when exact, else `X-Ludex-Estimated-Bytes`,
   else indeterminate; an estimate is clamped below 100% until the stream ends.
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
continues where it stopped (also after an app restart). Per-entry stream
installs pause too: they stop between entries, keep staging and the checkpoint,
and resume from the first missing file. Sequential tar installs cannot be
paused; `pause_install` returns an actionable error. The optional
`bandwidthLimitKib` setting paces downloads, repairs and stream installs (the
shared throttle caps the two concurrent entry downloads together).

Cancel aborts the download promptly, deletes the `.part`, removes staging and
marks the job `cancelled`. Cancel is cooperative during extraction (the core
extractor is not interruptible mid-entry; staging is removed afterwards).
Cancelling a per-entry stream install stops between entries and keeps the files
already downloaded plus the checkpoint, so the next install of the same version
resumes; when no entry completed, staging and the checkpoint are discarded.

## Tray, updates and process-tree playtime

### Tray icon (#15)

A tray icon is created at startup with Show/Hide, Downloads, a Recent games
submenu (the five most recent installs by launch, else import/install date),
Check for updates and Quit. Left click
toggles the window, the tooltip reports running/paused downloads, and the menu
is rebuilt after a launch or a finished install. `closeToTray` (default true)
makes the window close button hide to the tray; quit explicitly from the menu.
On macOS hiding switches the activation policy to accessory and showing restores
it. Linux needs `libayatana-appindicator3` at build time (already in the CI
dependencies).

### Application updates (#16)

`tauri-plugin-updater` with two channels:

| Channel | Endpoint |
|---|---|
| `stable` | `https://github.com/cleeryy/ludex/releases/latest/download/latest.json` |
| `early` | `https://github.com/cleeryy/ludex/releases/download/edge/latest.json` |

Checks run at startup (throttled to once per 24 h) and on demand from Settings or
the tray. `check_for_updates` returns `UpdateStatus`; `install_update` downloads
with `update://progress` events and installs. On Windows the installer restarts
the app; elsewhere the UI offers `restart_app`. The public key lives in
`tauri.conf.json` (`plugins.updater.pubkey`); when it is empty the status is
`configured: false` and installs are refused with an explicit message instead of
failing at verification time. Keys and channel publishing are documented in
[`docs/RELEASING.md`](../../docs/RELEASING.md).

### Game updates (#30)

`check_game_updates` asks the server for `GET /api/games/{id}/versions/latest`
and compares it with the local row: a different version id is a new version; an
identical id with a different archive fingerprint means the archive was replaced
in place. `update_game` reuses the normal install pipeline (stream or archive)
into staging and only removes the previous folder after the new install is in
place and indexed, so a failed update leaves a playable install. Save files live
outside the install folder and are never touched. Install fingerprints are
recorded at install time from the downloaded archive (`sha256-sample:` over the
size plus the first and last MiB, see `ludex_core::fingerprint`); stream installs
have no archive to fingerprint and fall back to version ids.

### Playtime process tree (#26)

The launch watchdog samples the process list once per second and keeps the
session open while **any descendant** of the launched PID is alive, attributing
the whole session to the root game. `stop_game` kills the whole tree
(children first). Sessions close when the tree is empty, so crashes and forced
kills still record the time played. Limits: a child spawned and orphaned between
two samples is lost (launchers that exit in under a second), and PID reuse can
extend a session in pathological cases. On macOS `open` is tracked, so the game
itself is not a descendant; sessions may end when `open` exits.

## Desktop integrations

### Silent installers (#7)

Some releases ship a `setup.exe` instead of the game. After installing, the game
page's **Installer** panel lists installer-looking programs in the folder and can
run one:

- `installer_plan` previews the argument sets, the timeout and the platform; it
  never executes anything.
- `run_installer` tries `installerSilentArgs` in order, then the built-in presets
  `/S`, `/silent`, `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART` and `--silent`.
  Exit code `0` or `3010` (reboot required) is success; another code, a timeout
  or a cancel moves to the next set.
- One attempt is killed after `installerTimeoutSecs`; `cancel_installer` kills
  the current one and stops the plan.
- After success the app records the game binary (never the installer) and emits
  `installer://done`. On Linux the binary is searched inside the game prefix
  (`drive_c`) too. `choose_executable` / `set_executable` fix the choice.
- Windows runs `setup.exe` directly. Linux runs it through the game's
  UMU/Proton/Wine tool in the game prefix and captures stdout/stderr to
  `{appDataDir}/installer-logs/<game>-<uuid>.log`; `InstallerReport.logTail`
  carries the last 8 KiB. macOS refuses with a manual-run message.
- The command builder, attempt selection, exit-code policy, timeout and
  cancellation are unit tested with an injected runner; the Linux invocation
  (`umu-run setup.exe …` / `proton run setup.exe …`) is a pure builder test.
- Installers that need elevation fail to start; run them interactively.
  `Executable` overrides always have to resolve inside the install folder or the
  game's Wine prefix.

### Linux compatibility (#89, #90, #91, #92, #93)

Linux runs Windows games through Proton or UMU; Ludex detects what is installed
and builds the launch. Nothing is a build dependency.

- `compat_scan` inspects Steam roots (`steamapps/libraryfolders.vdf` and
  `steamapps/common`), `compatibilitytools.d`, `umu-run`, `wine`/`wine64`,
  `winetricks` and `protonfixes`. Settings → **Compatibility** lists the tools
  and accepts overrides (`compatUmuPath`, `compatProtonPath`, `compatWinePath`).
- `get_launch_options` / `set_launch_options` store, per game: mode
  (`auto|native|proton|umu`), preferred tool id, Wine prefix, environment
  overrides and winetricks verbs. `auto` is the default; a Windows `.exe` uses
  UMU, then GE-Proton/Proton, then Wine. A missing tool produces an explicit
  error naming what to install.
- Environment: UMU gets `PROTONPATH`, `GAMEID`, `UMU_LOG`,
  `STEAM_COMPAT_DATA_PATH`, `STEAM_COMPAT_CLIENT_INSTALL_PATH` and
  `WINEPREFIX`; Proton gets the Steam compatibility variables and its Wine
  prefix at `<prefix>/pfx`; Wine gets `WINEPREFIX=<prefix>`. Playtime/process
  tree tracking is unchanged.
- `wine_prefix_info` reports path/size, `reset_wine_prefix` deletes it after a
  UI confirmation (never the install folder or an ancestor, refused while
  running), `run_winetricks` runs `winetricks -q <verbs>` in the prefix with a
  30-minute timeout and bounded output.
- `protondb_rating` fetches
  `protondb.com/api/v1/reports/summaries/<appid>.json`, caches it in SQLite for
  seven days and returns stale data offline. The game page shows the badge next
  to the title; the Steam app id can be set manually in the launch options.
- Helpers are pure and unit tested: tool selection/fallbacks, launch env,
  prefix derivation, env and verb validation, winetricks builder, ProtonDB
  parsing and cache TTL, installer invocation. A real Steam/Proton/UMU launch,
  a real `setup.exe` under Proton and winetricks on a real prefix still need a
  gaming machine.

### Discord Rich Presence (#12)

`discordPresence` (default off) plus `discordClientId`, a Discord *application*
id created at <https://discord.com/developers/applications>. No bot token and no
network traffic to Discord's API: the app talks to the local IPC socket.

- The activity shows the game title, a public cover URL and the session start;
  idle otherwise.
- Covers are filtered: `https` only, loopback and private ranges dropped.
- Missing Discord is not an error: reconnects back off 4 s → 120 s and
  `presence_status` reports the reason. Disabling clears the activity.
- `presence.rs` keeps the payload builder and the reconnect state machine free
  of IPC so they are unit tested with a mock sink.

### Launcher import (#13, #116)

`launcher_scan` reads the metadata each launcher already keeps on this machine
and lists the games it installed; `launcher_import` writes the selected ones as
external rows. Nothing is downloaded, copied or moved.

| Source | Files read | Launch URI |
|---|---|---|
| Steam | `steamapps/libraryfolders.vdf`, `appmanifest_*.acf` | `steam://rungameid/<appid>` |
| Epic | `%ProgramData%\Epic\UnrealEngineLauncher\LauncherInstalled.dat`, `…\EpicGamesLauncher\Data\Manifests\*.item` | `com.epicgames.launcher://apps/<ns>%3A<item>%3A<app>?action=launch&silent=true` |
| GOG | `galaxy-2.0.db` (read-only SQLite) and `GOG.com\Games` registry | `goggalaxy://openGameView/<productId>` |
| EA app | `Electronic Arts\EA Games` registry (offer id when present) | `origin://launchgame/<offerId>` |
| Ubisoft | `Ubisoft\Launcher\Installs` registry | `uplay://launch/<installId>/0` |
| Xbox | `Get-AppxPackage` + `AppxManifest.xml`, `Get-StartApps` for names | `shell:AppsFolder\<PackageFamilyName>!<AppId>` |
| itch | `butler.db` (`caves` + `games`) | `itch://caves/<caveId>/launch` |
| Heroic | `store_cache/legendary_library.json`, `store_cache/gog_library.json`, `sideload_apps/library.json` | `heroic://launch/<runner>/<app>` |
| Lutris | `~/.local/share/lutris/pga.db` (installed rows) | `lutris:rungameid/<id>` |

- A game is listed only when the launcher reports it installed and, when a
  folder is recorded, the folder still exists. A missing client, unreadable
  database or malformed manifest is a warning, never a failed scan.
- Parsers are pure and unit tested with fixtures for every format; the
  collectors that need a real client (Windows registry, PowerShell, a live
  SQLite file) only read what the client wrote.
- Titles are matched against the server library by normalised title, then once
  more with edition suffixes stripped; ambiguous titles are skipped and can be
  mapped by hand in the preview (`gameId` in the import entry).
- Imported rows have `versionId: 0`, `source` = the launcher id and
  `launchUri` = the table above. Matched games keep their server id; unmatched
  ones use `-appId` for Steam and a deterministic negative id for the rest.
- Duplicates are refused by the partial unique index on `launchUri` and by a
  scan-time check; the scan also drops a game reported twice for the same
  folder (Epic and Heroic, for example).
- Entries without a documented launch id (some EA installs) are flagged and
  accept a manual URI. `file:`, `javascript:` and `data:` schemes are refused.
- Launching opens the URI handler (`cmd /C start`, `xdg-open`, `open`). For
  external entries the install folder is not required to exist locally: the
  launcher owns the files (Xbox Start menu entries have no folder at all).
- Uninstall of an import removes only the index row: the launcher's folder is
  never touched. `steamRoot` overrides Steam discovery (`LUDEX_STEAM_ROOT` also
  works).
- Playtime while a launcher URI handler is alive is recorded like any other
  session; for games the client launches out of process the session can be
  short.

### Adopted installs (#129)

An adopted install links a game folder that already exists on this machine —
typically a GameVault download — to a library game. Ludex launches the detected
executable in place, tracks playtime and can remove the entry or, after an
explicit confirmation, the folder. Nothing is downloaded, copied or moved.

| Action | Command | Files |
|---|---|---|
| Inspect a folder | `adopt_installation { gameId, folder, versionId? }` | read only, returns executable/size/file count/warnings |
| Link it | `confirm_adopt_installation { gameId, folder, versionId?, executable?, replace? }` | writes the index row only |
| Hash it | `build_install_manifest { gameId }` | writes a manifest row in SQLite, never inside the folder |
| Remove from Ludex | `unlink_installation { gameId }` | kept |
| Uninstall and delete | `delete_adopted_installation { gameId, confirmPath }` | deleted, only after an exact confirmation |
| Scan a GameVault root | `scan_gamevault_folders { root }` + `adopt_gamevault_installations` | read only / index rows |

- Adopted rows have `source: "adopted"`, `installPath` = the canonical chosen
  folder, `installRoot` = its parent, `versionId: 0` unless the user picked a
  server version (the version is never guessed from the title) and
  `archiveHash: null`. One row per game: replacing an existing `ludex` or
  launcher row requires the explicit `replace: true` the UI only sends after
  showing the conflict. A folder already owned by a launcher import (#116) is
  refused, and so is the same folder adopted for two games.
- Paths are canonicalized and stored canonical. Relative paths, `..`, links in
  the chosen path, drive/home roots and the Ludex install root itself are
  refused; a tree with a symlink that points outside the folder is refused too.
- Launch uses the normal pipeline: the executable (detected with
  `ludex_core::pick_executable` or chosen later) runs with the folder as working
  directory and the process tree drives playtime. `launch_game` and
  `game://exited` carry `source: "adopted"`.
- An adopted folder has no Ludex manifest. `verify_install` reports
  `noManifest` with a "build a manifest" message instead of a clean pass;
  `build_install_manifest` hashes it in the background (progress on
  `install://manifest-progress`) so Verify/Repair work. Repair still needs a
  server archive for the chosen version.
- `unlink_installation` removes only the index row, manifest and detected save
  paths: the folder stays. `delete_adopted_installation` without `confirmPath`
  is a read-only preview (canonical folder, size, file count); with a
  `confirmPath` that is exactly the canonical stored path it deletes the folder.
  It refuses non-`adopted` rows, a folder that became a link, a folder directly
  inside a drive/home/install root and any symlink escape. `uninstall_game`
  refuses adopted rows so files can never be deleted by the ordinary uninstall
  path.
- `scan_gamevault_folders` lists the immediate subfolders of a root that contain
  a plausible executable, reads an optional `gamevault.json`/`.gamevault.json`
  title (read-only, never written) and matches folder names to library titles by
  normalised exact title, then word-boundary containment. Several candidates are
  reported as ambiguous and left for the game page, where the user picks the game
  and version. Bulk adoption only accepts immediate children of the scanned root.
- Game updates are not offered for adopted rows: an update would replace files
  Ludex does not own. Remove or delete the adopted folder, then install the Ludex
  version.

### Playnite (#14)

`integrations/playnite/` is a read-only Playnite library plugin (C#, Playnite
SDK). It calls `GET /api/games`, creates one Playnite entry per game and sets the
Play action to a Ludex URL (`LaunchUrlTemplate`, default the game page). It is
built manually with `dotnet`; see the README there. No server endpoint was added
for it.

## Locations (#99)

Settings → Locations manages named folders, each with a label, absolute path,
kind (`install`, `download`, `temp`) and one default per kind. Rows can be
added, renamed, removed, checked (existence, writability through a probe file,
free/total bytes, missing-drive note) and marked default; the panel edits the
draft and `Save desktop settings` persists it. The install dialog preselects
the default install location, offers the download/temp locations for the
archive, shows free space and blocks a download that cannot fit.

Locations are tolerant: a missing folder never blocks saving Settings, and it
only fails the job that selects it, with an actionable error. `install_game`
probes writability and free space (256 MiB reserve; archive + extracted copy
when both live on the same volume) before queueing, then pins the resolved
paths on the job. Extraction stages in the install root, so a cross-volume
archive is read from its own volume and unpacked directly into the install
volume; the archive is removed afterwards unless `keepArchives`. An old
`settings.json` with only `installDir`/`downloadDir` is migrated to locations
on load, and an old UI patch still updates the default location.

## Game settings editor (#105) and adapters (#106)

Installed games gain a Settings tab with three tabs:

- **Game settings** — engine adapters with typed controls.
- **Config files** — a searchable list of ini/cfg (Godot `project.godot`,
  Unity `prefs`), TOML and JSON files found up to five folders deep, skipping
  hidden folders and Wine `drive_c`, with a key/value editor and line numbers.
- **Backups** — timestamped history with the line diff and a restore action.

Every write is type-checked against the existing value, copies the original to
`{appDataDir}/config-backups/game-<id>/<timestamp>-<uuid>-<name>`, records the
diff, then replaces the file atomically (temp file + rename, Unix mode kept).
Binary files, non-UTF-8, files over 1 MiB, paths outside the install folder
(symlinks resolved) and line breaks in values are refused, and JSON must stay
valid. Parsers keep the exact byte span of each value, so comments, ordering
and quoting survive a round trip. Restoring a backup takes a safety copy first.

Adapters are detection-only and point each typed field at a file + key:

| Adapter | Markers | Fields |
|---|---|---|
| Unreal Engine | `*.uproject`, `*.pak`, `Engine/`, `Binaries/` | resolution, window mode, V-Sync, frame limit, resolution quality, language (`Engine.ini`) |
| GOG | `goggame-*.info` | language; other metadata read-only |
| Godot | `project.godot`, `*.pck` | resolution, window mode, V-Sync, MSAA, language |
| Unity | `*_Data/globalgamemanagers`, `UnityPlayer.dll`, `prefs` | resolution, window mode, V-Sync, graphics quality |

Several can apply at once. Values stored in the Windows registry, Unreal's
`%LOCALAPPDATA%` or Godot's `user://` are returned read-only with a note rather
than edited outside the install folder.

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
| `game://exited` | `{ sessionId, gameId, minutesPlayed, secondsPlayed }` |
| `local-server://status` | `{ running, url }` |
| `ui://navigate` | route string requested by the tray |
| `ui://launch` | `gameId` from the tray's Recent games |
| `update://available` | `UpdateStatus` |
| `update://progress` | `{ downloaded, total, percent }` |
| `update://installed` | `{ version, restarted }` |
| `installer://done` | `{ gameId, success, exitCode, tool }` (`tool` = Linux compatibility tool or `null`) |

## Launching and playtime

- Windows: the executable is spawned with `current_dir` set to the install
  folder.
- Linux: `auto` runs native files directly and sends a Windows `.exe` through
  the best compatibility tool (UMU → GE-Proton/Proton → Wine, see below);
  `.sh` through `sh`, `.AppImage`/ELF get the executable bit and run directly.
- macOS: `.app` bundles through `open`, other files directly.
- `args` is split with quote support (backslashes kept, Windows paths intact).

A watchdog thread polls the process tree, records the session (exact seconds,
plus minutes rounded to the nearest minute for the server), updates
`last_launched_at`, emits `game://exited` and posts `{ minutes }` to
`/api/games/{id}/playtime`. Failures leave `sessions.synced = 0` and are retried
at the next launch/exit. `stop_game` kills the whole process tree and the
watchdog still reports the session.

## Safety

- All commands return `Result<T, String>` with actionable messages; no panics on
  user paths.
- Slugs, version labels and executable overrides are sanitised and checked
  against the install folder; uninstall canonicalises and refuses paths outside
  the recorded install root.
- Adopted folders (#129) are only deleted through `delete_adopted_installation`
  after an exact path confirmation; `uninstall_game` refuses them so the
  ordinary uninstall path can never remove files Ludex did not install. Launcher
  imports keep their files on uninstall.
- Tokens are never logged and only stored in the 0600 settings file.
- Game configs are only written inside the recorded install folder, after a
  timestamped backup and a type check; a failed backup aborts the write.
- The CSP allows `http:`/`https:`/`ws:`/`wss:` connections so LAN servers on
  plain HTTP work, plus `data:`/`blob:`/remote images.
- Single-instance plugin: launching Ludex again focuses the existing window.

## Bundle targets

`tauri.conf.json` targets `deb` + `appimage` on Linux, `nsis` on Windows and
`app` + `dmg` on macOS (platform configs `tauri.linux.conf.json`,
`tauri.windows.conf.json`, `tauri.macos.conf.json`). Windows builds are produced
by CI; no code signing is configured.

## Known limitations

- Per-entry stream installs resume only when the server exposes
  `/stream/manifest` and `/stream/entries/{index}` and the archive is a ZIP.
  7z/RAR (and servers older than #115) use the sequential tar stream or the
  normal download and restart from zero after a drop; they cannot be paused.
- Per-entry resume crosses an app restart by re-fetching the manifest and
  comparing its fingerprint; a replaced archive under the same version id
  discards the old checkpoint and starts from zero.
- Repair re-downloads the whole version archive and restores only the damaged
  files; the server has no per-file endpoint yet.
- Cross-drive moves need free space for a second copy and are untested on
  Windows in CI.
- Reinstalling into a different location leaves the previous folder on disk
  (the row is replaced).
- Config editing covers scalar keys in ini/cfg/TOML/JSON. Arrays, inline TOML
  tables, comments inside JSON (invalid anyway) and multi-line values are not
  editable; engine settings stored in the registry or outside the install folder
  are read-only.
- Engine detection is marker based: a repacked build without its markers falls
  back to the generic Config files tab.
- Extraction progress cannot be cancelled mid-archive; cancellation is applied
  between phases.
- Process-tree tracking samples once per second: a launcher that exits in under
  a second before its child is observed can end the session early, and a child
  reparented to PID 1 between two samples is lost. macOS tracks `open`, not the
  game, for the same reason.
- The updater needs a release signing key; source builds and unreleased tags
  report `configured: false` and cannot install updates.
- The `early` channel requires the moving `edge` prerelease to publish
  `latest.json` (see `docs/RELEASING.md`).
- Silent installer execution (Windows and Linux-through-Proton/UMU) is not
  tested against real installers in CI; the command builder, Linux invocation
  and the exit-code/timeout policy are unit tested.
- A real Proton/UMU launch, winetricks against a real prefix and the installer
  under Proton need a gaming machine with Steam/Proton/UMU and a display; CI
  covers detection fixtures, environment builders and path/verb validation.
- Discord presence needs a Discord application id and a running Discord client;
  neither is available in CI, so only the payload and state machine are tested.
- Steam import paths are tested with fixtures, not against a real Steam install;
  playtime for Steam-launched games is only as good as the handler process tree.
- The Playnite plugin is not built in CI (no .NET SDK); compile it against your
  Playnite installation before installing.
