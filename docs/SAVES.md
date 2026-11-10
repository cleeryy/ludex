# Save folder detection (#107)

Ludex v0.2 stores cloud saves as opaque per-user archives on the server. Before
an archive can be uploaded, the desktop app has to know **which local files are
the game's saves**. This document covers that local discovery step. Upload,
restore and automatic sync are a separate issue (#108); the server never sees a
host path.

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

## Tauri commands

See the command table in `docs/CONTRACT.md`:

- `detect_save_paths { gameId }` — detect + persist, returns `SavePaths`.
- `redetect_save_paths { gameId }` — same action after a first launch.
- `get_save_paths { gameId }` — persisted mapping + last preview, no disk scan.
- `set_save_paths { gameId, paths }` — manual override list.

All four are local. No host path is ever included in an API request, log line
sent to the server, telemetry or error report.

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
