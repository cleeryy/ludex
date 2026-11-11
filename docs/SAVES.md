# Save folder detection and cloud save sync (#107, #108)

Ludex stores cloud saves as opaque per-user archives on the server. Before an
archive can be uploaded, the desktop app has to know **which local files are the
game's saves**. This document covers local discovery (#107, #153) and the
automatic sync/restore flow built on top of it (#108). The server never sees a
host path.

## Scope

- Desktop only. No server route and no bundled data. The only network call is
  the save-path manifest download in [Auto-managed manifest](#auto-managed-ludusavi-manifest-153).
- Detection is advisory: it lists existing save files/folders and their size so
  the UI can show a `backup --preview`-style summary before anything is
  archived.
- A game with no match reports an explicit **"no save folder detected"** state
  instead of producing an empty archive.
- Save editing, save migration and anti-cheat/DRM games are out of scope.

## Registry and candidate discovery (#146)

There is no cross-industry save-location standard. Discovery therefore combines
three reliable layers: resolvable OS folders (the root), engine defaults (the
layout), and per-game observations (the manifest or a user override). The
versioned resolver lives in the repository:

| File | Purpose |
|---|---|
| `data/save-roots.yaml` | Per-OS known folders, engine patterns, Steam Cloud roots, exclusions and caps. Embedded into the desktop binary at build time. |
| `data/save-overrides.yaml` | Optional per-game observations, sourced from a real launch. Empty by default. |

Neither file is a copy of Ludusavi or PCGamingWiki data. The registry contains
no per-game paths, and adding a root or an engine is a data change plus a test
fixture in `apps/desktop/src-tauri/src/saves_registry.rs`.

### Known folders

Known folders are resolved through an injectable resolver, not string
concatenation:

- **Windows**: `SHGetKnownFolderPath` supplies `FOLDERID_Profile`,
  `FOLDERID_SavedGames`, `FOLDERID_Documents`, `FOLDERID_RoamingAppData`,
  `FOLDERID_LocalAppData`, `FOLDERID_LocalAppDataLow`, `FOLDERID_ProgramData`
  and `FOLDERID_Public`. Tests inject the same map, so no Windows shell is
  needed. `%USERPROFILE%\Documents` is never built by hand: OneDrive Known
  Folder Move and GPO redirection are honoured. When the shell API is
  unavailable, Documents can still resolve through the `OneDrive`,
  `OneDriveConsumer` or `OneDriveCommercial` environment, and Saved Games uses
  its documented default (Known Folder Move does not redirect it).
- **LocalLow**: Windows has no `%LOCALAPPDATA%Low` variable. The resolver asks
  for `FOLDERID_LocalAppDataLow`; the fallback derives `LocalLow` from the
  resolved LocalAppData parent (`...\AppData\Local` → `...\AppData\LocalLow`).
- **Linux**: `XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` with the
  XDG defaults, plus `~/.var/app` for Flatpak app data.
- **Proton**: `drive_c/users/<user>/{Saved Games,Documents,AppData/Local,
  AppData/LocalLow,AppData/Roaming}` inside the per-game prefix (#121).
- **macOS**: `~/Library/Application Support`, `~/Library/Containers/<bundle-id>/Data`
  and `~/Library/Preferences`. Container bundle ids are only matched when they
  contain the game title; a container far from the title is not guessed.

### Engines, stores and provenance

`data/save-roots.yaml` lists patterns for Unity, Unreal Engine, Godot, Ren'Py
and Minecraft, plus a generic title-folder pattern for the known roots. A
pattern uses a root placeholder first (`{saved_games}`, `{documents}`,
`{xdg_data}`, `{application_support}`, `{proton_drive_c}`, …) and then literal
segments or placeholders:

| Placeholder | Meaning |
|---|---|
| `{product}`, `{project}`, `{game}` | Must match the game title (normalised, edition suffix removed). Never guessed from a partial token. |
| `{company}`, `{publisher}` | Enumerated one level below an engine marker (`unity3d`, `godot`, `app_user_data`, …). |
| `{windows_user}` | A user folder inside a Proton prefix. |
| `{bundle}` | A macOS container whose bundle id contains the title. |

Steam `remotecache.vdf` is the store ground truth when the game has an app id
and a Steam userdata folder. Its `WinSavedGames`, `WinMyDocuments`,
`WinAppDataLocal`, `WinAppDataLocalLow`, `WinAppDataRoaming`,
`WinProgramData` and `SteamCloudDocuments` roots map to the same known folders
(or to the Proton prefix for a Windows build). Reading that small, bounded VDF
file is the only file read during discovery; no save file is opened.

Every candidate carries:

| Field | Meaning |
|---|---|
| `path` | Canonical host path. Local only; never uploaded. |
| `rootId` | Known-folder id, or `proton_prefix`. |
| `subpath` | Path below that root. |
| `source` | `override`, `store`, `known-folder`, `engine` (or `manifest`/`manual` for the older layers). |
| `confidence` | `certain`, `likely` or `guess`. |
| `evidence` | Human-readable reason, e.g. `Unity default layout: …`. |
| `firstSeen`, `lastVerified` | RFC 3339 timestamps, kept across scans. |
| `confirmed` | True once the user confirmed it; it is then a save path. |

A **guess is labelled and requires an explicit confirmation**. Registry
candidates are never written to `save_paths` automatically, so automatic sync
(#108) cannot archive one. Only the per-game manifest result keeps its existing
behaviour, and a confirmed candidate is folded into the next snapshot.

### Safety caps

- Enumeration stops at `limits.max_depth` (4) below a root.
- A directory read stops at `limits.max_entries` (2000); one run stops at
  `limits.max_candidates` (64).
- At or below `limits.whitelist_min_depth` (2), wildcards are only expanded
  under a literal engine marker or for a game-title match — never across a
  whole home directory.
- Symlinks, junctions and Windows reparse points are skipped, including
  `dosdevices/z:`. Paths outside the allowlisted roots are dropped.
- Exclusions (`Cache`, `Logs`, `CrashReportClient`, `Screenshots`, redists,
  `*.log`, …) are filtered by name, and candidates are directories, not file
  lists.

### Adding a root or an engine

1. Edit `data/save-roots.yaml`. Windows folders need a `FOLDERID_*` id and a
   GUID when `api: true`; never add an `env` entry for Saved Games or LocalLow.
2. Keep every pattern at most `max_depth` segments below its root and use only
   the placeholders above.
3. Add a fixture in `saves_registry.rs` that creates the layout and asserts the
   root id, source and confidence. A malformed registry fails the test suite
   before a release build.

Xbox / Game Pass opaque containers and encrypted saves remain unsupported:
their files are not portable save directories and are never listed as
candidates.

## Sources and precedence

Detection uses the first source that produces paths, and always keeps the user's
manual entries. Registry candidates (#146) are listed separately and never
archived until confirmed; the resolver ranks a user override before Steam
ground truth, a known folder and an engine default. The per-game manifest stays
the trusted layer for the paths that can be archived:

1. **Manual / override paths** (SQLite, per game). Always included. A manual path
   that matches a manifest result wins for that location, and is labelled
   `override`; a standalone manual path is labelled `manual`. A confirmed
   registry candidate is a `SavePathRecord` owned by the user, so it survives
   the next detection just like a manual path.
2. **Ludusavi manifest**, in this order (#153):
   1. `ludusaviManifestPath` when the user set it and the file exists;
   2. the copy Ludex downloaded into
      `{appDataDir}/manifest/ludusavi-manifest.yaml`;
   3. a manifest from a user-installed Ludusavi data folder
      (`$XDG_DATA_HOME/ludusavi`, `~/.local/share/ludusavi`,
      `~/.var/app/com.github.mtkennerly.ludusavi/data/ludusavi`,
      `~/.config/ludusavi`, `~/Library/Application Support/ludusavi`,
      `%APPDATA%/ludusavi`, `%LOCALAPPDATA%/ludusavi`).

   A configured path that disappeared is reported as a read error when no other
   copy exists; an existing managed or user copy is preferred over a broken
   override.
3. **Ludusavi CLI** (`ludusavi` binary), as an advanced optional fallback. Used
   when `ludusaviBinary` is set, or when `ludusavi` is found on `PATH`, but only
   when the manifest listed nothing (or no manifest exists). Ludex runs
   `ludusavi --version` and then `ludusavi --api` with a `preview` request and
   reads the reported files.
4. **No source**: an actionable "no save folder detected" message with the next
   steps (launch the game once, download the save database in Settings → Saves,
   set a manifest path, or add a path manually).

The Ludusavi manifest is **not bundled** with Ludex. It is MIT-licensed but
built from CC BY-NC-SA PCGamingWiki data, so redistributing it needs its own
licensing review. Ludex downloads it at runtime instead (see below).

## Auto-managed Ludusavi manifest (#153)

A fresh desktop install has neither a `ludusavi` binary nor a manifest. Ludex
now downloads the canonical file itself, so save detection works out of the box.
The upstream project publishes it on the `data` branch of
`mtkennerly/ludusavi-manifest` (the repo has no GitHub release assets).

| Item | Value |
|---|---|
| Download URL | `https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/data/data/manifest.yaml` |
| Override / mirror | `ludusaviManifestUrl` (must be `http`/`https`; can point at a release asset or a pinned copy) |
| Cached file | `{appDataDir}/manifest/ludusavi-manifest.yaml` |
| Metadata | `{appDataDir}/manifest/ludusavi-manifest.json` |
| Version | `sha256:<12 hex>` of the cached file |
| Metadata fields | `version`, `etag`, `lastCheckedAt`, `lastUpdatedAt`, `sourceUrl`, `sizeBytes`, `sha256`, `error` |
| Size cap | 64 MiB (`MAX_MANIFEST_BYTES`), checked on `Content-Length` and while streaming |
| Timeout | 60 s for the whole request (`DOWNLOAD_TIMEOUT_SECS`) |
| Conditional request | `If-None-Match: <etag>`; `304` only refreshes `lastCheckedAt` |
| Checksum | `x-checksum-sha256` response header, when the source publishes one |

### Update policy

- **Startup**: `saves_manifest::auto_update` runs in the background. It is
  throttled to once per 24 h using `lastCheckedAt`; a missing metadata file
  counts as due.
- **Manual**: Settings → Saves → **Update manifest** calls
  `update_ludusavi_manifest`, which ignores the throttle.
- **Consent**: `ludusaviManifestAutoUpdate` (default `true`) gates the startup
  hook. Turn it off and only the manual button downloads. The default is on so
  a first run with no manifest works without a setup step.
- **Offline / 404 / timeout / oversize / bad checksum / not a YAML manifest**:
  the previous cached file is kept, `lastCheckedAt` and `error` are recorded,
  and the UI says the cached copy is still in use.
- **Remove**: `remove_ludusavi_manifest` deletes both files; detection falls
  back to a user-installed copy, the CLI, or manual paths.

### Safety

- The body is streamed into a capped buffer; nothing above 64 MiB is written.
- Atomic replace: the new bytes go to `.{name}.part-<id>` in the same folder,
  are flushed, then renamed over the target. On Windows, where a rename cannot
  replace an existing file, the old file is removed only after the temp file is
  complete.
- The download is **data only**. `saves_detect` parses it with `serde_yaml`;
  nothing in it is executed, and the optional CLI is never launched from it.
- Manifest contents, the cached path and the metadata stay local. They are
  never included in an API request, telemetry or a save snapshot.

### Licence and provenance

- The manifest is published by the Ludusavi project under the MIT licence.
- Its save-path data is derived from PCGamingWiki, whose content is
  **CC BY-NC-SA** (attribution, non-commercial, share-alike). Ludex therefore
  downloads it at runtime instead of vendoring it in the repo, the installer or
  a Docker image, and Settings shows the attribution.

## Settings

Settings → Saves → **Save locations** reads the local candidate index and can
run the registry on demand. The desktop commands stay local:

| Command | Purpose |
|---|---|
| `list_save_candidates` | Persisted candidates grouped by installed game; no disk scan. |
| `scan_save_candidates` | Run the registry for every installed game and persist the result. |
| `confirm_save_candidate` | Validate one candidate (or an edited path) and add it to `save_paths`. |
| `ignore_save_candidate` | Dismiss a candidate so a later scan does not resurface it. |

The save-detection extras in `LocalSettings`:

| Setting | Default | Meaning |
|---|---|---|
| `ludusaviManifestPath` | `""` | Explicit manifest file. Empty = the managed copy, then a detected Ludusavi install. |
| `ludusaviManifestAutoUpdate` | `true` | Download/refresh the manifest at startup (max once per 24 h). Off = manual only. |
| `ludusaviManifestUrl` | `""` | Mirror or pinned URL. Empty = the canonical upstream manifest. |
| `ludusaviBinary` | `""` | `ludusavi` executable. Empty = look for `ludusavi` on `PATH`. Only used when the manifest lists nothing. |
| `winePrefix` | `""` | Wine/Proton prefix containing `drive_c`. Empty = use the game's per-game launch prefix, then `$WINEPREFIX`, the Steam `compatdata/<appid>/pfx` folder, `{installDir}/.prefix`, then `~/.wine`. |

A per-game prefix saved by the Linux launch options (Proton/UMU/Wine, #121) is
the most specific source and wins over the global setting. A configured binary
is tried even if it no longer exists. A configured manifest path falls back to
the managed copy when the file disappeared, and is only reported as a read error
when no other copy exists.

## Matching a game

- **Steam App ID first.** Imported Steam games carry
  `launchUri: steam://rungameid/<appid>`; the manifest's `steam.id` is matched
  exactly. App ids are never guessed from a title.
- **Then normalised title** (lowercase alphanumerics, `&` → `and`), then the
  edition-stripped title (`Game of the Year Edition`, `GOTY`, `Definitive
  Edition`, …). Two manifest entries matching the same key are reported as
  ambiguous; Ludex does not pick one.
- **Store filters.** `when: [{store: steam}]` (also `gog`, `epic`, …) only
  applies when the store is known: Steam imports, `steam_appid.txt` /
  `steam_api*.dll`, `goggame-*.info` / `galaxy.dll`, or `*.egstore`. Unknown
  required stores are skipped and listed in `warnings`.
- **OS filters.** `when: [{os: linux|windows|macos}]`. A `windows` entry also
  applies on Linux/macOS when a Wine/Proton prefix is available. Unknown `when`
  keys are treated as "does not apply", never guessed.

## Path templates

Both string manifest entries and `{path: …, when: …}` entries are supported.

| Template | Resolution |
|---|---|
| `<base>` | The game's install folder. |
| `<home>` | The current user's home folder. |
| `<osUserName>` | The current OS user name. |
| `<storeUserId>` | Numbered Steam `userdata/<id>` folders (one path per profile). Unresolved when no Steam profile exists. |
| `<xdgData>`, `<xdgConfig>` | `$XDG_DATA_HOME` / `$XDG_CONFIG_HOME`, with `~/.local/share` / `~/.config` defaults. |
| `<winAppData>` | Windows: `%APPDATA%`. Wine/Proton: `drive_c/users/<user>/AppData/Roaming` for **each** prefix user. |
| `<winLocalAppData>` | Windows: `%LOCALAPPDATA%`. Prefix: `…/AppData/Local`. |
| `<winDocuments>` | Windows: `~/Documents`. Prefix: `…/Documents`. |
| `<winPublic>` | `C:\Users\Public` / `drive_c/users/Public`. |
| `<winProgramData>` | `%PROGRAMDATA%` / `drive_c/ProgramData`. |
| `<winDir>` | `%SYSTEMROOT%` / `drive_c`. |

Rules:

- A leading `~` expands to the home folder; `C:\…` becomes
  `<prefix>/drive_c/…` on Linux/macOS. Other drive letters are refused.
- `*` is supported **inside one path segment** (`profile-*/save.dat`), never
  across a separator. At most 256 matches per segment. `?` also matches one
  character.
- Multiple Windows users in a prefix and multiple Steam profiles are aggregated
  into several results for the same entry.
- If a directory and one of its files are both listed, only the directory is
  kept.

## Result shape

Each detected location carries:

```ts
interface SavePath {
  path: string;               // canonical host path, local only
  kind: "file" | "directory";
  source: "manifest" | "manual" | "override";
  primary: boolean;           // one per game
  sizeBytes: number;          // capped scan total
  lastModified: string | null; // RFC 3339, newest file in the folder
  notes: string | null;        // missing path, capped scan, refusal…
}
```

A `directory` size and `lastModified` are the sum/newest file inside it. Scans
stop after 10 000 files or 8 GiB and add a note; this bounds a hostile or huge
save folder.

Distinct states reported in `message`:

- `N save path(s) detected with the Ludusavi manifest/CLI.`
- `N save path(s): X detected … and Y configured manually.`
- `No save files exist yet for this game. Launch it once so it creates them,
  then re-detect.` (missing-save retry)
- `This game is not in the Ludusavi manifest. …`
- `The Ludusavi manifest only lists registry locations for this game. …`
- `No Ludusavi manifest or CLI was found. …`
- `No save folder detected for this game yet. …`

## Manual override

`set_save_paths` replaces the whole mapping for a game with what the UI sends.
Every path must:

- exist and be readable (folders are opened, files are read);
- be a file or a folder; `kind` is checked when supplied;
- stay under an allowlisted root (home, XDG folders, install folder, prefix
  `drive_c`, native Windows folders);
- contain no `..`;
- be unique per game, with **at most one primary** (a second one is an error).

An empty list clears the mapping. A path that matched a previous manifest result
is stored as `override`, so a later re-detection does not replace it.

## Persistence

Desktop SQLite (`{appDataDir}/ludex-desktop.db`), current schema (the save
tables are the #107 migration; the Linux launch/ProtonDB migration landed
before them):

```sql
save_paths (game_id, path, kind, source, "primary", created_at)
  PRIMARY KEY (game_id, path)
  UNIQUE INDEX (game_id) WHERE "primary" = 1

save_detection (game_id, version_id, manifest_version, detected_at, result_json)
```

`result_json` is the full `Detection` payload (sizes, notes, warnings, preview
message), so `get_save_paths` never rescans the disk. The manifest version is a
`sha256:<12 hex>` fingerprint of the manifest text, or the `ludusavi --version`
line. `version_id` records the installed game version that was inspected.

`uninstall_game` deletes both tables for the game. Reinstalling starts from a
clean detection.

## Automatic sync (server)

Server-side snapshots carry sync metadata (#108):

- `source` — `manual` (a user upload) or `auto` (the desktop after `game://exited`).
- `deviceId` — opaque machine identifier; used for display only, never for
  authorisation.
- `versionId` — the build the save belongs to, when reported.
- `sha256` — content hash; automatic uploads with a hash that already exists for
  the user and game return the existing snapshot (`200`) instead of creating a
  duplicate.
- Retention keeps the newest **10 automatic** snapshots per user and game and
  never prunes manual snapshots. Pruning removes rows first, then files, so a
  crash cannot leave a row pointing at a missing archive.
- The existing size limit and per-user isolation are unchanged; uploads are
  multipart `file`, `label`, `source` and `deviceId` fields.

## Desktop automatic sync (#108)

The desktop archives the persisted save paths and uploads one snapshot per
session. Everything below runs locally except the upload/download request.

### Trigger and debounce

`launch::spawn_watcher` emits `game://exited` after the **whole process tree**
exits, then schedules `saves_sync::schedule_after_exit`. The sync sleeps **3
seconds** before archiving so the game's final writes settle, and never blocks
the exit path. A second exit for the same game within the debounce window is
coalesced into one upload. The same code path is reachable manually through
`sync_cloud_save_now`, which works even when automatic sync is off.

The user can also change the global default in Settings → Desktop and the
per-game switch on the game page. Effective state is:

| Per-game `auto_sync` | Global `cloudAutoSync` | Result |
|---|---|---|
| `NULL` (inherit) | `true` | on (`reason: "on"`) |
| `NULL` (inherit) | `false` | off (`reason: "global_off"`) |
| `true` | either | on (`reason: "on"`) |
| `false` | either | off (`reason: "game_off"`) |

### Archive and hash

- Paths are read from `save_paths` in the order `db::list_save_paths` returns
  them (primary first, then path). Missing paths and symlinks are skipped.
- Layout: `saves/00/<relative path>`, `saves/01/…`, one top-level folder per
  configured save path. A save path that is a single file contributes exactly
  one entry named after the file.
- Stored (not recompressed) zip, bounded to 50 000 entries, 64 directory levels
  and 4 GiB; special files (sockets, FIFOs, devices) are skipped.
- The archive is SHA-256-hashed. If the hash equals the last uploaded hash for
  that game, the upload is skipped; if detection found no files, no archive is
  uploaded and no empty snapshot is created.
- Multipart upload fields: `source=auto`, `deviceId`, `label=auto · <device
  label>`. The server returns `200` for an existing hash and `201` for a new
  snapshot, and prunes to the newest 10 automatic snapshots per user and game.
  Manual snapshots are never pruned.

### Device identity

`LocalSettings.cloudDeviceId` is a random UUID generated on first use and stored
in `settings.json`. It is **not** a hardware fingerprint and is used only for
display. `cloudDeviceLabel` is optional; when empty, the host name (or
"Desktop") is used. The label is sent only inside the snapshot label.

### Retry queue

A failed automatic upload is persisted in SQLite (`cloud_save_prefs.pending_*`)
and retried by a background worker every 60 seconds with exponential backoff
(30 s, 60 s, 120 s, 240 s, 480 s; 5 attempts). Network errors, `408`, `429` and
`5xx` are retryable; `4xx`/auth failures are permanent and not retried. The game
page shows `pending`, the attempt count, the next attempt and the last error.
Automatic failures never open a toast: the game page and the `cloud-save://pending`
event are the only signals.

### Conflict detection

A conflict exists when **both** sides changed:

1. local save files exist and their newest mtime is newer than the last upload
   from this machine (2 s tolerance); and
2. the newest server snapshot hash differs from `last_hash`, the hash this
   machine last uploaded.

One-sided changes stay silent: local-only changes are uploaded on the next sync;
a remote-only change is simply a newer snapshot to restore if the user wants.
The status includes both timestamps and both hashes so the choice is explicit.

### Restore

- `restore_cloud_save(gameId, strategy)` requires `strategy`:
  - `remote` — download the newest snapshot, verify its SHA-256, then extract.
    If local save files exist, a timestamped backup archive is written to
    `{appDataDir}/cloud-saves/backups/game-<id>-<stamp>.zip` **before** any file
    is touched, and its path is returned. When the save folder is empty or
    absent, nothing needs a backup and the snapshot is written directly.
  - `keep_local` — keep the local files, record them as the new baseline and
    upload them as an automatic snapshot. The ignored server hash is remembered
    so the same conflict is not reported again; this is **not** a restore, so
    the restore-once slot stays free and the snapshot can still be restored
    later if the user changes their mind.
- Extraction maps `saves/NN` back to the save path at index `NN`, rejects
  absolute paths, `..`, backslashes, control characters, symlink entries,
  entries outside the mapped root and archives expanding beyond the size cap.
- Restore runs at most once per snapshot: `cloud_save_restore.snapshot_id`
  records the last restored snapshot, and a repeat returns "already restored"
  without touching files.
- `resolve_cloud_save_conflict` is the same operation; after a successful
  `keep_local` upload or `remote` restore, `last_hash` matches the new content,
  so the conflict disappears.

### Local state

`SCHEMA_V9` adds:

```sql
cloud_save_prefs (
  game_id PRIMARY KEY, auto_sync, last_hash, last_upload_at, last_restore_at,
  pending_attempts, pending_next_at, pending_error, ignored_remote_hash
)
cloud_save_restore (
  game_id PRIMARY KEY, snapshot_id, restored_at, local_backup_path
)
```

`auto_sync = NULL` means "inherit the global setting". `last_hash` and
`last_upload_at` are the upload baseline used by the hash skip and conflict
rule. Pending retry state survives a restart. Download and backup archives live
under `{appDataDir}/cloud-saves/` and temporary archives are deleted after each
attempt.

### Privacy

- Only the opaque zip leaves the machine. Host paths, the real save layout and
  the device label (inside an optional snapshot label) are not sent as separate
  fields.
- Tokens, save contents and archive bytes are never logged.
- Snapshots are private to the account; the server stores them as-is and never
  opens them.
- The browser build cannot read save files: it lists, downloads and uploads
  manual snapshots and explains that automatic sync needs the desktop app.

## Tauri commands

See the command table in `docs/CONTRACT.md`:

- `ludusavi_manifest_status` — cached manifest version, dates, size and source.
- `update_ludusavi_manifest` — download now; returns the status plus `updated`
  and `notModified`.
- `remove_ludusavi_manifest` — delete the cached copy and its metadata.
- `detect_save_paths { gameId }` — detect + persist, returns `SavePaths`.
- `redetect_save_paths { gameId }` — same action after a first launch.
- `get_save_paths { gameId }` — persisted mapping + last preview, no disk scan.
- `set_save_paths { gameId, paths }` — manual override list.
- `cloud_save_status { gameId }` — paths, latest snapshot, last upload/restore,
  effective toggle + reason, pending retry, conflict.
- `set_cloud_save_sync { gameId?, enabled }` — global default (`gameId` null)
  or one game's override.
- `sync_cloud_save_now { gameId }` — archive + upload immediately.
- `restore_cloud_save { gameId, strategy: "remote" | "keep_local" }`.
- `resolve_cloud_save_conflict { gameId, strategy }`.

The first four are local. No host path is ever included in an API request, log
line sent to the server, telemetry or error report.

## Security

- Results are canonicalised; a path must start with a canonical allowlisted
  root. Symlinks are resolved **before** the check, so a link inside the home
  folder that points to `/etc` is refused.
- `..` components are rejected before touching the filesystem; directories are
  only listed when they are under a root. Literal paths are checked before their
  metadata is read.
- Directory scans use `symlink_metadata` and skip every symlink, so a link
  planted inside a save folder cannot pull outside files into an archive.
- Manifest content is parsed as data (`serde_yaml`), never executed. A manifest
  larger than 64 MiB is refused.
- The Ludusavi CLI is run with `--api`, a 20 s timeout, stdin/stdout pipes only
  and a 1 MiB output cap. Its reported paths go through the same allowlist.
- The manifest download is capped at 64 MiB, times out after 60 s, verifies an
  advertised SHA-256 before replacing the cache and writes atomically.
- Registry-only saves are reported, not read.

## Tests

`cargo test --lib` (desktop crate) covers, with temporary fixture trees only and
no real Ludusavi installation:

- manifest parsing and Steam App ID / title / edition matching;
- `when` OS and store filters;
- Windows templates under a fake prefix, multi-user aggregation and
  `<storeUserId>` expansions;
- missing-save retry after the first launch;
- glob expansion inside a segment and directory/file aggregation;
- traversal and symlink-escape rejection;
- manual override validation, one primary, allowlisted roots and precedence;
- explicit "no save folder detected" state;
- the CLI preview parser, plus a fake `ludusavi` shell script on Unix;
- SQLite roundtrip, delete, per-game uniqueness and idempotent migration;
- the manifest precedence: explicit path → managed copy → user install;
- the auto-managed manifest with a mocked fetcher: success, `404`, timeout,
  oversize, bad checksum, empty or malformed body, `304`, atomic replace,
  cached fallback;
- the 24 h throttle and the automatic-update toggle;
- settings round-trip for the four manifest settings.

`pnpm --filter @ludex/web test` covers the Settings panel state helpers
(`missing`, `ready`, `stale`, `error`, Update/Remove availability and version
shortening) without a Tauri shell.

For automatic sync (#108), still with temporary trees and no network:

- archive layout, file save paths, symlink skipping and the empty-detection
  no-op;
- the unchanged-hash skip, and bounded retry backoff;
- toggle precedence (`on`, `global_off`, `game_off`);
- conflict detection for local-only, remote-only and two-sided changes;
- restore into an empty folder and backup-before-overwrite;
- path traversal and symlink-mode rejection during extraction;
- SQLite pending-retry bookkeeping, restore-once state and migration to v9.

A real server is not exercised by unit tests; the upload/download paths are
covered by type-checking and the server's own route tests.
