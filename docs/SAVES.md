# Save folder detection and cloud save sync (#107, #108)

Ludex stores cloud saves as opaque per-user archives on the server. Before an
archive can be uploaded, the desktop app has to know **which local files are the
game's saves**. This document covers local discovery (#107) and the automatic
sync/restore flow built on top of it (#108). The server never sees a host path.

## Scope

- Desktop only. No server route, no bundled data, no new network calls.
- Detection is advisory: it lists existing save files/folders and their size so
  the UI can show a `backup --preview`-style summary before anything is
  archived.
- A game with no match reports an explicit **"no save folder detected"** state
  instead of producing an empty archive.
- Save editing, save migration and anti-cheat/DRM games are out of scope.

## Sources and precedence

Detection uses the first source that produces paths, and always keeps the user's
manual entries:

1. **Manual / override paths** (SQLite, per game). Always included. A manual path
   that matches a manifest result wins for that location, and is labelled
   `override`; a standalone manual path is labelled `manual`.
2. **Ludusavi CLI** (`ludusavi` binary). Used when `ludusaviBinary` is set, or
   when `ludusavi` is found on `PATH`. Ludex runs `ludusavi --version` and then
   `ludusavi --api` with a `preview` request, reads the reported files, and
   falls back to the manifest file if the preview fails or lists nothing.
3. **Ludusavi manifest file** (`manifest.yaml`). Read from `ludusaviManifestPath`
   when set, otherwise auto-discovered in the usual Ludusavi data folders
   (`$XDG_DATA_HOME/ludusavi`, `~/.local/share/ludusavi`,
   `~/.var/app/com.github.mtkennerly.ludusavi/data/ludusavi`,
   `~/.config/ludusavi`, `~/Library/Application Support/ludusavi`,
   `%APPDATA%/ludusavi`, `%LOCALAPPDATA%/ludusavi`).
4. **No source**: an actionable "no save folder detected" message with the next
   steps (launch the game once, set a manifest path, or add a path manually).

The Ludusavi manifest is **not bundled** with Ludex. It is MIT-licensed but
derived from CC BY-NC-SA PCGamingWiki data, so shipping it requires its own
licensing review. Users install Ludusavi (or point Ludex at their own manifest
copy); Ludex only reads it.

## Settings

All three are desktop-only extras in `LocalSettings`:

| Setting | Default | Meaning |
|---|---|---|
| `ludusaviManifestPath` | `""` | Manifest file. Empty = auto-discover the standard Ludusavi locations. |
| `ludusaviBinary` | `""` | `ludusavi` executable. Empty = look for `ludusavi` on `PATH`. |
| `winePrefix` | `""` | Wine/Proton prefix containing `drive_c`. Empty = use the game's per-game launch prefix, then `$WINEPREFIX`, the Steam `compatdata/<appid>/pfx` folder, `{installDir}/.prefix`, then `~/.wine`. |

A per-game prefix saved by the Linux launch options (Proton/UMU/Wine, #121) is
the most specific source and wins over the global setting. A configured manifest
path or binary is tried even if it no longer exists; the command reports a
warning and falls back to the next source.

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
- SQLite roundtrip, delete, per-game uniqueness and idempotent migration.

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
