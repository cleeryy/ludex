# Achievements

Ludex stores achievement definitions and per-account unlock state on the
server. The framework is provider-agnostic: administrators can add definitions
by hand, and three local providers read what the installed game already wrote
on disk or exposes on loopback. Nothing is uploaded anywhere.

## Schema

Migration `0013_achievements.sql`.

| Table | Purpose |
|---|---|
| `achievements` | Definitions. `game_id` is `NULL` for global achievements; `key` is unique per game (partial unique index), `provider` records the origin (`manual`, `game_files`, `gog_galaxy_api`, `gog_galaxy_sqlite`) and `source_json` keeps the raw provider payload. |
| `user_achievements` | One row per `(user_id, achievement_id)`, enforced by a unique constraint. `unlocked_at` is the first unlock and never moves; `progress` is the completion fraction; `source` and `raw_json` record where the unlock came from. |
| `achievement_sources` | `(game_id, user_id)` → install path, app id and provider list, so scheduled rescans do not need a client call. |
| `achievement_scans` | Audit trail: one row per provider per run (status, found, unlocked, error, timestamp). |

Unlock writes use `INSERT … ON CONFLICT DO NOTHING`: a rescan can never
duplicate a row, and re-reading the same files returns `unlocked: 0`. Deleting
a definition cascades to its unlocks; deleting a game or account does the same.

## API

All routes require a session or API key (except nothing here is public).
Errors follow the standard envelope.

| Method | Route | Auth | Notes |
|---|---|---|---|
| GET | `/api/games/{id}/achievements` | read | Definitions with the caller's unlock state, points, completion and last scan |
| GET | `/api/me/achievements` | read | Every unlock of the caller, newest first |
| GET | `/api/users/{id}/achievements` | read | Public showcase; 404 `profile_not_found` unless the target enabled the community profile (or is the caller) |
| POST | `/api/games/{id}/achievements` | admin | `{ key, name, description?, iconPath?, points?, hidden? }` → 201; 409 `achievement_exists`; 400 `invalid_achievement` |
| PATCH | `/api/games/{id}/achievements/{key}` | admin | Partial update; 404 `achievement_not_found` |
| DELETE | `/api/games/{id}/achievements/{key}` | admin | 204; deletes the unlocks too |
| POST | `/api/games/{id}/achievements/{key}/unlock` | editor/write | `{ userId?, source? }`; idempotent, `unlocked` reports whether this call created the row; targeting another account requires admin |
| POST | `/api/games/{id}/achievements/scan` | editor/write | `{ installPath?, appId?, providers? }` → `ScanReport` |

Hidden definitions are masked for any account that has not unlocked them:
`name` becomes `"Hidden achievement"`, `description` and `iconUrl` are `null`.
`unlockPercentage` is the share of activated accounts holding the unlock,
computed on read.

Every new unlock emits `achievement.unlocked` on the event bus: it is written to
`audit_logs` and delivered to webhooks subscribed to that event. Public
definitions also append an `achievement_unlocked` row to the community activity
feed (hidden definitions are never named there). Definition CRUD and scans emit
`achievement.definition_created|updated|deleted` and `achievement.scanned`.

## Providers

A scan runs the enabled providers in order and merges their results by
normalised key (`lowercase`, non-alphanumerics folded to `_`). Unknown keys are
still accepted: the definition is created from the provider data so a game that
only reports unlocks still shows something.

### `game_files` — Steam emulation and Goldberg

Read from the reported install directory:

- `steam_settings/achievements.json` (Goldberg) — definitions:
  `[{ "name", "displayName", "description", "hidden", "icon", "points" }]`.
  A `{ "achievements": [...] }` wrapper is accepted too.
- `UserStats/Stats.ini` — `[Achievements]` section, `KEY=1` lines.
- `steam_settings/achievements.ini`, `steam_settings/stats/achievements.ini`,
  `steam_settings/user_stats/achievements.ini` — same format, plus per-key
  sections with `Achieved=1` and `UnlockTime=<unix|RFC 3339>`.
- `steam_emu.ini` — optional, same parser.

Malformed JSON or unreadable files are skipped; they never fail the scan.

### `gog_galaxy_api` — loopback JSON

`LUDEX_ACHIEVEMENT_LOCAL_API_URLS` is a comma-separated list of patterns with
`{appId}` / `{gameId}` placeholders (default
`http://localhost:29662/api/achievements?appId={appId}`). Targets must be
loopback (`localhost`, `127.0.0.0/8`, `::1`); anything else is refused unless
`LUDEX_ACHIEVEMENT_ALLOW_REMOTE_APIS=true` (tests and unusual setups).

Accepted payloads: a bare array, or `{ "achievements" | "items" | "data" |
"results": [...] }`. Per item, keys are matched tolerantly:

| Field | Accepted names |
|---|---|
| key | `key`, `id`, `achievementId`, `achievement_id`, `apiName`, `name` |
| name | `name`, `title`, `displayName`, `display_name` |
| description | `description`, `desc`, `unlockedDescription` |
| unlocked | `unlocked`, `achieved`, `isUnlocked`, `is_unlocked`, or any unlock date |
| date | `unlockedAt`, `unlockDate`, `unlock_date`, `dateUnlocked`, `date` |
| points | `points`, `xp`, `score` |
| hidden | `hidden`, `isHidden`, `secret` |
| icon | `icon`, `iconUrl`, `icon_url`, `image` |

### `gog_galaxy_sqlite` — Galaxy database

Opens `galaxy-2.0.db` read-only. Candidates: `LUDEX_ACHIEVEMENT_GALAXY_DB`, then
`<install>/galaxy-2.0.db`, `<install>/Galaxy/storage/galaxy-2.0.db`, and the
install's parent directory. The provider looks for a table whose name contains
`achievement`, introspects its columns and maps `achievementId`/`key`/`name`,
`dateUnlocked`/`unlocked`/`achieved`, `description`, `points`, and
`hidden`/`visible` (inverted when the column is `visible`). Unknown column sets
are skipped rather than guessed.

## Path allowlist

Scans only read directories inside `LUDEX_ACHIEVEMENT_SCAN_ROOTS`
(comma-separated, default: the library roots). A reported path is canonicalised
and must be a child of an allowed root; otherwise the API answers
`403 path_not_allowed`. A missing path is `400 invalid_install_path`. This keeps
`POST …/scan` from becoming an arbitrary filesystem reader.

## Configuration

| Variable | Default | Effect |
|---|---|---|
| `LUDEX_ACHIEVEMENT_SCAN_INTERVAL_HOURS` | `0` | Hours between scheduled rescans of every recorded source; `0` disables the job |
| `LUDEX_ACHIEVEMENT_PROVIDERS` | `game_files,gog_galaxy_api,gog_galaxy_sqlite` | Enabled providers; `none` disables scanning |
| `LUDEX_ACHIEVEMENT_SCAN_ROOTS` | library roots | Directories a scan may read |
| `LUDEX_ACHIEVEMENT_LOCAL_API_URLS` | GOG Galaxy pattern | Loopback JSON patterns |
| `LUDEX_ACHIEVEMENT_GALAXY_DB` | unset | Explicit `galaxy-2.0.db` |
| `LUDEX_ACHIEVEMENT_ALLOW_REMOTE_APIS` | `false` | Allow non-loopback API targets |

Scheduled scans reuse the last `installPath`/`appId` per `(game, user)` from
`achievement_sources`, so the desktop only reports a path once.

## Privacy

- Unlocks are per account; the game page and `/api/me/achievements` only ever
  show the caller's own state.
- The cross-game showcase follows the community profile opt-in
  (`public_profile`). A private profile answers 404 to everyone else, exactly
  like the rest of the community API.
- Hidden definitions stay masked until unlocked, in every response.
- Provider icons are limited to URLs, `/api/media/…` paths or plain file names;
  raw filesystem paths from a local install are never returned to another
  account.
- Scan paths, raw provider payloads and install locations are server-side only;
  audit metadata records the game, account, key and provider name, never a path.

## Limitations

- The server scans its own filesystem. A desktop whose game runs on another
  machine must mount the install directory on the server (or scan from a client
  in a future release).
- RAR/7z archives that have not been extracted cannot be scanned.
- Provider schemas vary between emulator forks; the parsers are intentionally
  tolerant but only the documented keys are mapped.
