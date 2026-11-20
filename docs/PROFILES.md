# Per-game profiles: export, import and diff

A profile is one TOML document that describes how a game is meant to run:
launch plan, display wrapper, declared prerequisites, notes and the build stamps
that wrote it. It exists so a working configuration can leave the database it
was trapped in: send the file, paste it back, see exactly what would change.

The document is built from the per-game launch options the desktop app already
stores (#89, #287) and from the prerequisites recorded for "Make it run"
(#178). There is no second settings model, no hidden side table and no
migration: export reads what is stored, import writes it through the same
validation path as the Launch options panel.

The panel lives at the bottom of **Game page → Launch options → Game profile**.

## Why the file has no paths

A profile is meant to be shared between machines. Anything that only means
something on the machine that wrote it is left out, with a warning:

| Value | In the file | Why |
|---|---|---|
| Wine prefix (`prefixPath`) | absent, never imported | a folder on this device; the importing game keeps its own |
| Audio device (`audioSink`) | absent, never imported | a PulseAudio/PipeWire sink name only exists on this host; the importing game keeps its own (#289) |
| Compat tool | `launch.tool = "GE-Proton9-20"` (a **name**) | the stored id (`kind:path`) contains an absolute path; a name resolves on the other machine |
| Working directory | relative to the install folder, or absent | an absolute folder outside the install root cannot exist elsewhere |
| `steam_app_id`, `env`, winetricks verbs, display profile | copied as-is | portable |
| Environment value holding a path (or the home directory) | dropped, warning per key | `DXVK_CONFIG_FILE=/home/alice/…` is noise on another machine |

The export never writes the home directory, the user name or the install path.
The categorical guarantee is tested: `the_exported_profile_has_no_absolute_paths_or_usernames`.

## The document

```toml
schema_version = 1

[meta]
title = "Hades"
app_version = "0.4.25"
rules_version = "unversioned"
exported_at = "2026-11-19T10:00:00Z"

[launch]
mode = "umu"
tool = "GE-Proton9-20"
steam_app_id = 1145360
working_dir = "bin"
winetricks = ["vcrun2022"]

[launch.env]
DXVK_HUD = "fps"
MANGOHUD = "1"

[display]
mode = "gamescope"
width = 1920
height = 1080
refresh = 60
filter = "fsr"
adaptive_sync = true
hdr = false

prerequisites = ["dotnet", "vcrun"]
notes = ""
```

| Key | Meaning |
|---|---|
| `schema_version` | Document format. This build writes `1` and reads a higher number best effort with a warning |
| `meta.title` | Game title at export time; shown in the import proposal, never used to match games |
| `meta.app_version` | Ludex build that wrote the profile |
| `meta.rules_version` | Compatibility rules stamp. `unversioned` until the rules database (#301) ships its own |
| `meta.exported_at` | RFC 3339 timestamp |
| `launch.mode` | `auto`, `native`, `proton` or `umu` (the same enum as Launch options) |
| `launch.tool` | Compat tool **name**; empty or absent means "best available" |
| `launch.env` | Explicit environment overrides (a table, `KEY = "value"`) |
| `launch.winetricks` | Winetricks verbs run on demand in the game prefix |
| `launch.steam_app_id` | Steam app id used by ProtonDB and `GAMEID`; omitted when unset |
| `launch.working_dir` | Folder the game starts in, **relative** to the install folder; absent means "keep the local choice" |
| `display.*` | The Gamescope profile (#287): `desktop` is a no-op, `gamescope` carries width, height, refresh, filter and the `adaptive_sync`/`hdr` switches |
| `prerequisites` | Declared ids (`vcrun`, `dotnet`, `directx`, `physx`) |
| `notes` | Free-form text carried by the document; see [limits](#limits) |

The wrapper chain is described by two existing fields rather than a third copy:
`launch.mode` selects the compatibility wrapper (UMU, Proton, Wine or native),
`display.mode` selects the Gamescope display wrapper.

### Forward compatibility

Parsing keeps every key this build does not know and writes it back.
A future `[audio]` section, a new flag in `[meta]` or a top-level key from a
newer release survives `parse → export` on an older build. The unknown keys are
also visible in the import proposal as `future.<key>` changes, so they are never
silently accepted. This includes keys inside a known table (`[launch]`,
`[meta]`, `[display]`): they round-trip in the matching section.

A newer `schema_version` is not a hard failure: the known fields are read, the
rest is preserved, and the proposal shows a warning.

## Import is a proposal

`preview_game_profile_import` parses the pasted TOML, builds the current profile
from the database and returns:

- `diff`: one row per changed field (`launch.mode`, `launch.env.DXVK_HUD`,
  `display.width`, `prerequisites`, `future.audio`, …), with the current and
  incoming values;
- `warnings`: non-blocking facts (missing build, unknown prerequisite, missing
  working folder);
- `errors`: blocking problems (invalid env name, invalid verb, display profile
  outside the documented bounds, absolute working directory);
- `canApply`: true when `errors` is empty.

Nothing is written by the preview. The UI only calls
`apply_game_profile_import` after the user confirms, and that command re-parses
and re-validates the same text before writing.

`diff` is a pure function with its own tests: `what changed since it worked` is
a short list of dotted paths, not a wall of TOML.

### Warnings, not failures

A profile that references a Proton build this device does not have is a normal
case when sharing:

- `missing-build` — the named build is absent; the import still applies and
  `launch.tool` resolves to the best available build;
- `missing-working-dir` — the relative folder is not in this install folder;
  the local working directory is kept;
- `unknown-prerequisite` — not one of the ids this build knows; ignored;
- `local-path-omitted` / `prefix-omitted` — export-side notices about values
  that had to stay on the machine;
- `missing-schema` / `unknown-schema` — document version notices.

The only blocking conditions are invalid values that Launch options would also
refuse.

### What apply touches

| Stored field | Apply behaviour |
|---|---|
| mode, env, winetricks, Steam app id, display | replaced by the profile values |
| `tool_id` | resolved from the profile name against the host scan; best available when missing |
| `prefix_path` | **never touched** |
| `audio_sink` | **never touched**: a sink name is host-local, so the profile keeps the per-game device (#289) |
| `gamemode` | in-memory only (#297); the launch plan decides it at launch time, apply leaves it off |
| `working_dir` | relative folder resolved against the install folder when it exists; otherwise the local value stays |
| prerequisites | replaced with the known ids the profile declares |
| notes | shown in the proposal, not stored (see below) |

## Limits

- **Notes are carried, not stored.** The local library has no per-game notes
  column yet; `notes` round-trips through the document and is visible in the
  proposal, but apply does not persist it.
- **No CLI yet.** The desktop binary has no command-line entry point of its own
  (it is a Tauri app), so profiles are exported and imported from the UI only.
  The logic lives in `game_profile.rs` and is a pure library: a CLI would be a
  thin wrapper.
- **Settings tiers (#299) are out of scope.** `meta.rules_version` is a document
  format stamp (`unversioned` for now), not the active compatibility-rules
  version — `compat_rules_status` reports that one (#301).
- **One audio setting stays local.** A profile carries no audio device: the
  per-game `audioSink` (#289) is a PulseAudio/PipeWire sink name that only
  exists on the machine that wrote it, so import keeps the local device. An
  unknown `[audio]` table written by a future release still survives a
  round-trip here thanks to unknown-key preservation.

## Tests

`apps/desktop/src-tauri/src/game_profile.rs` covers TOML round-trip, unknown-key
preservation, missing-build warning, import-as-proposal (no write before
confirm), diff output, prefix preservation and the no-absolute-path guarantee.
`apps/web/src/lib/game-profile.test.ts` covers the diff labels.
