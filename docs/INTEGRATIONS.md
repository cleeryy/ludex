# Integrations: API keys, audit log, webhooks and desktop clients

Everything below uses the normal JSON API under `/api`. Authenticate either
with a session JWT (`Authorization: Bearer <jwt>`) or an API key
(`Authorization: Bearer ldx_<token>`). API keys never grant more than their
scopes: `read`, `write` or `admin` (each implies the previous one).

## API keys

Create a read-only key for a script:

```bash
curl -sS -X POST https://ludex.example.com/api/keys \
  -H "Authorization: Bearer $LUDEX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"backup-job","scopes":["read"]}'
```

```json
{
  "id": 3,
  "userId": 1,
  "name": "backup-job",
  "prefix": "ldx_9f3a1c2b",
  "scopes": ["read"],
  "createdAt": "2026-11-10T12:00:00Z",
  "lastUsedAt": null,
  "revokedAt": null,
  "secret": "ldx_9f3a1c2b...64-hex..."
}
```

The `secret` is shown once. Store it in the client's secret store:

```bash
export LUDEX_API_KEY='ldx_9f3a1c2b...'
curl -sS https://ludex.example.com/api/games?limit=5 \
  -H "Authorization: Bearer $LUDEX_API_KEY"
```

Mutating endpoints require the `write` scope (for example starting a scan):

```bash
curl -sS -X POST https://ludex.example.com/api/library/scan \
  -H "Authorization: Bearer $LUDEX_API_KEY"
```

Administrator routes (users, audit, webhooks) require the `admin` scope **and**
an administrator account. List and revoke:

```bash
curl -sS https://ludex.example.com/api/keys -H "Authorization: Bearer $LUDEX_TOKEN"
curl -sS -X DELETE https://ludex.example.com/api/keys/3 \
  -H "Authorization: Bearer $LUDEX_TOKEN"
```

Revocation is immediate: the next request with the key gets `401 unauthorized`.
Every successful use updates `lastUsedAt`.

## Audit log

Administrators can page through the audit trail. Newest entries come first.

```bash
# Last 50 events
curl -sS 'https://ludex.example.com/api/audit' \
  -H "Authorization: Bearer $LUDEX_TOKEN"

# Failed logins, 20 per page
curl -sS 'https://ludex.example.com/api/audit?action=login.failed&limit=20' \
  -H "Authorization: Bearer $LUDEX_TOKEN"

# Next page: pass the previous response's nextBefore
curl -sS 'https://ludex.example.com/api/audit?limit=20&before=912' \
  -H "Authorization: Bearer $LUDEX_TOKEN"
```

```json
{
  "items": [
    {
      "id": 913,
      "actorId": 1,
      "actorKind": "user",
      "action": "api_key.created",
      "targetType": "api_key",
      "targetId": "3",
      "metadata": {"name": "backup-job", "prefix": "ldx_9f3a1c2b", "scopes": ["read"], "userId": 1},
      "ip": "192.0.2.10",
      "createdAt": "2026-11-10T12:00:00Z"
    }
  ],
  "nextBefore": 912
}
```

Filters: `actorId`, `actorKind` (`user`, `api_key`, `system`), `action`,
`targetType`, `targetId`. Secrets, passwords and plaintext API keys are never
written to the audit trail.

## Webhooks

Register an endpoint (admin only). `events` accepts `game.ingested`,
`game.deleted`, `download.completed`, `user.created`, `save.uploaded` and
`achievement.unlocked`.

```bash
curl -sS -X POST https://ludex.example.com/api/webhooks \
  -H "Authorization: Bearer $LUDEX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://hooks.example.com/ludex","events":["game.ingested","user.created"]}'
```

```json
{
  "id": 1,
  "url": "https://hooks.example.com/ludex",
  "events": ["game.ingested", "user.created"],
  "enabled": true,
  "createdAt": "2026-11-10T12:00:00Z",
  "secret": "64-hex-hmac-key"
}
```

`GET /api/webhooks` lists hooks without the secret. `PATCH` updates `url`,
`events` or `enabled`; send `{"rotateSecret": true}` to get a new secret once.
`DELETE /api/webhooks/1` removes the hook and its deliveries. Queue a test
delivery with:

```bash
curl -sS -X POST https://ludex.example.com/api/webhooks/1/test \
  -H "Authorization: Bearer $LUDEX_TOKEN"
# 202 {"deliveryId": 7}
```

### Verifying deliveries

Each request carries the exact body, `X-Ludex-Event`, `X-Ludex-Delivery` and
`X-Ludex-Signature: sha256=<hex>`, where the hex is HMAC-SHA256 of the raw
request body using the webhook secret. Verify before parsing:

```python
import hashlib, hmac

def verify(secret: str, body: bytes, header: str) -> bool:
    expected = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)

# Flask example
# body = request.get_data()
# assert verify(WEBHOOK_SECRET, body, request.headers["X-Ludex-Signature"])
```

```bash
# Inspect a delivery by hand
SECRET='64-hex-hmac-key'
BODY='{"event":"game.ingested","timestamp":"2026-11-10T12:00:00Z","data":{"title":"Celeste"}}'
printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET"
```

Payload shape:

```json
{
  "event": "save.uploaded",
  "timestamp": "2026-11-10T12:00:00Z",
  "data": {
    "snapshotId": 12,
    "gameId": 4,
    "userId": 2,
    "size": 1048576,
    "sha256": "...",
    "label": "before boss"
  }
}
```

### Delivery semantics

- Asynchronous, at-least-once, ordered per webhook by delivery id.
- 10 s timeout, no redirects, `Content-Type: application/json`.
- 5 attempts with exponential backoff (1 s, 2 s, 4 s, 8 s by default; override
  the base with `LUDEX_WEBHOOKS_BACKOFF_MS` for tests). A 2xx marks the delivery
  as done; anything else is retried.
- The stored payload is re-sent byte-for-byte, so the signature stays valid.
- Loopback, private, link-local and unique-local addresses are rejected unless
  `LUDEX_WEBHOOKS_ALLOW_PRIVATE=true`. The host is re-resolved and the
  connection pinned before every attempt to limit DNS-rebinding SSRF.
- A disabled hook receives no deliveries; its queued deliveries resume when it
  is re-enabled.

## Metadata providers

Administrator → **Metadata providers** installs a community metadata source
without rebuilding or restarting the server. A provider is an HTTP sidecar:
Ludex calls it over the network and never executes third-party code in process.

IGDB stays the built-in default and is always tried first when credentials are
configured. Enabled providers then run in ascending `priority`, followed by the
Steam Store fallback and finally the file-name parser. A provider only fills
metadata that is still empty (manual edits and IGDB always win), and a failing
provider never fails an enrichment or a scan.

### Contract

`GET {baseUrl}/search?title=<urlencoded>&year=<optional>`

Headers: `Accept: application/json`, and `Authorization: Bearer <token>` when a
token is configured. The response is a JSON object with a `results` array
(at most 50 entries are read):

```json
{
  "results": [
    {
      "id": "celeste-1",
      "title": "Celeste",
      "description": "Help Madeline survive her inner demons.",
      "releaseDate": "2018-01-25",
      "coverUrl": "https://cdn.example/celeste-cover.jpg",
      "backgroundUrl": "https://cdn.example/celeste-hero.jpg",
      "screenshots": ["https://cdn.example/celeste-1.jpg"],
      "genres": ["Platformer", {"name": "Indie"}],
      "developers": ["Maddy Makes Games"],
      "publishers": ["Extremely OK Games"],
      "tags": ["speedrun"],
      "rating": 92,
      "ageRating": 7,
      "providerUrl": "https://metadata.example.com/games/celeste-1"
    }
  ]
}
```

- `id` and `title` are required; every other field is optional and ignored when
  absent or of the wrong type.
- `releaseDate` accepts `YYYY-MM-DD`, RFC 3339 or a bare year. `rating` is 0-100.
  `ageRating` is a minimum age (0-21). `description` may contain HTML; tags are
  stripped before storage.
- Covers, backgrounds and screenshots are downloaded by the server and cached
  in the media directory, exactly like IGDB artwork. Only local `/api/media/...`
  paths are stored in the database.
- `GET {baseUrl}/games/{id}` is optional and used by future refreshes. It returns
  either one candidate object or a `{"results": [ ... ]}` envelope.

### Field mapping

Some providers wrap their payload differently. The admin form's **Field
mapping** is a JSON object from canonical fields to dot paths:

```json
{
  "title": "data.name",
  "coverUrl": "data.images.cover",
  "releaseDate": "data.released_at"
}
```

Allowed canonical fields: `id`, `title`, `description`, `releaseDate`,
`coverUrl`, `backgroundUrl`, `screenshots`, `genres`, `developers`,
`publishers`, `tags`, `rating`, `ageRating`, `providerUrl`. Paths are
dot-separated object keys (`data.images.cover`), never array indexes. A path
that does not resolve is ignored and the canonical key is tried instead, so a
mapping never hides fields a provider already returns in the standard shape.

### Security

- Server-side fetch only: the webview and the browser never call a provider.
- One request per provider and per game, 10 s timeout, at most 2 MiB of body.
  Response parsing is defensive; malformed JSON marks the attempt as failed.
- Redirects are disabled (a redirect could downgrade HTTPS to HTTP).
- Only `http` and `https` are accepted; URLs with credentials, queries or
  fragments are rejected. The host is resolved once and the connection is
  pinned to the validated address, so a DNS answer cannot move the request into
  a private network.
- Loopback, private, link-local and unique-local addresses are refused. Tests
  may set `LUDEX_PROVIDER_ALLOW_PRIVATE=true`; never enable it in production.
- The optional bearer token is encrypted at rest with AES-256-GCM under a key
  derived from the server secret (HKDF-SHA256, purpose-bound AAD). It is
  write-only: `secretConfigured` tells the UI whether one is stored, and no
  endpoint ever returns the plaintext or the ciphertext. Rotating
  `LUDEX_JWT_SECRET` invalidates stored tokens; re-enter them in the admin UI.
- Every attempt is logged in `provider_fetches` (provider, game, ok, HTTP
  status, short error). Rows older than 30 days are pruned on write.

### Admin API

All routes require an administrator account (API keys need the `admin` scope).

```bash
# List (ordered by priority)
curl -sS https://ludex.example.com/api/admin/metadata-providers \
  -H "Authorization: Bearer $LUDEX_TOKEN"

# Create; the token is write-only and the provider starts disabled
curl -sS -X POST https://ludex.example.com/api/admin/metadata-providers \
  -H "Authorization: Bearer $LUDEX_TOKEN" -H "Content-Type: application/json" \
  -d '{
    "name": "Celeste DB",
    "baseUrl": "https://metadata.example.com/v1",
    "token": "provider-token",
    "priority": 100,
    "enabled": true,
    "mapping": {}
  }'

# Update (a blank/absent token keeps the stored one; "clearToken": true removes it)
curl -sS -X PUT https://ludex.example.com/api/admin/metadata-providers/1 \
  -H "Authorization: Bearer $LUDEX_TOKEN" -H "Content-Type: application/json" \
  -d '{"priority": 10, "enabled": true}'

# Test a title without saving anything
curl -sS -X POST https://ludex.example.com/api/admin/metadata-providers/1/test \
  -H "Authorization: Bearer $LUDEX_TOKEN" -H "Content-Type: application/json" \
  -d '{"title": "Celeste", "year": 2018}'

# Reorder (priorities are rewritten 10, 20, ...)
curl -sS -X POST https://ludex.example.com/api/admin/metadata-providers/reorder \
  -H "Authorization: Bearer $LUDEX_TOKEN" -H "Content-Type: application/json" \
  -d '{"ids": [2, 1]}'

# Delete
curl -sS -X DELETE https://ludex.example.com/api/admin/metadata-providers/1 \
  -H "Authorization: Bearer $LUDEX_TOKEN"
```

Every create, update, reorder and delete emits
`settings.metadata_provider_updated` with the provider id, name, enabled flag,
priority and the changed field names. The token is never part of the audit
metadata.

### Example provider

`examples/metadata-provider/` is a dependency-free Node server serving
`/search` from a static fixture. Run it, then point a provider at its URL:

```bash
node examples/metadata-provider/server.mjs --port 8787
# Base URL: http://localhost:8787 (with LUDEX_PROVIDER_ALLOW_PRIVATE=true for a
# local test only)
```

See `examples/metadata-provider/README.md` for the fixture format and a mapping
example.

## Desktop integrations

Everything in this section runs on the machine that installed the games, inside
the Ludex desktop app. None of it needs a bot token, a paid service or a server
change.

### Silent installers (`setup.exe`)

Some DRM-free releases are an archive containing a `setup.exe` instead of the
game itself. After installing the version, the game page gains an **Installer**
panel: it lists installer-looking programs in the install folder and can run one
with silent switches.

- The command is `run_installer`; nothing runs without a click.
- Attempts are tried in order. Defaults:
  `/S` (NSIS), `/silent`, `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART` (Inno
  Setup) and `--silent` (Squirrel/electron-builder).
- Exit code `0` and `3010` (success, reboot required) count as installed. Any
  other code, a timeout or a cancellation moves to the next attempt.
- `installerSilentArgs` (Settings → Desktop) replaces the list: one argument set
  per line. `installerTimeoutSecs` caps one attempt (default 600 s, min 30).
- After success Ludex picks the game binary the installer left behind and
  records it, so **Play** works without another prompt. If nothing sensible is
  found, use **Choose executable**.
- When every silent attempt fails the panel offers an interactive run (no
  arguments, no timeout). Cancelling keeps the files the installer already wrote.
- Windows runs the setup program directly. On Linux it runs through the game's
  UMU/Proton/Wine tool inside the game prefix and captures the log (the last
  8 KiB is shown in the panel). macOS refuses and asks for a manual run.
  Installers that need elevation fail to start; run them interactively instead.

### Linux gaming: Proton, UMU, ProtonDB and Wine prefixes

Linux plays Windows games through a compatibility layer. Ludex does not ship
one; it detects what the machine already has and runs the game through it.
Nothing is required at build time and every path is optional at runtime.

**Detection (`compat_scan`, Settings → Compatibility).** One scan reports:

- Steam roots (`~/.steam/steam`, `~/.local/share/Steam`, Flatpak, extra
  libraries in `libraryfolders.vdf`) and the Proton builds inside
  `steamapps/common` (`Proton - Experimental`, `Proton 9.0`, GE-Proton…) and
  `compatibilitytools.d`.
- `umu-run` from `PATH`, `~/.local/bin`, `~/.local/share/umu`, `/usr/bin` and
  `/usr/local/bin`.
- System `wine` / `wine64`, `winetricks` and an optional `protonfixes` checkout.
- Every path can be overridden (`compatUmuPath`, `compatProtonPath`,
  `compatWinePath`); a bad override is a warning, not a failure.

The scan is pure filesystem inspection (plus `wine --version`), so it is unit
tested against fixture directory trees without Steam installed.

**Launch modes (`launchOptions.mode`).**

| Mode | Behaviour |
|---|---|
| `auto` (default) | Native files run directly; a Windows `.exe` goes through UMU, then the best GE-Proton/Proton build, then Wine |
| `native` | Run the file as-is (`.sh` through `sh`, `.exe` through Wine when that was the explicit choice) |
| `proton` | `<proton>/proton run game.exe` |
| `umu` | `umu-run game.exe` with a local Proton build when one is detected, else UMU's own `GE-Proton` |

Tool order is UMU → GE-Proton → Proton Experimental → numbered Proton → Wine.
A stored tool id that no longer exists falls back to the best available tool
instead of breaking the launch. With nothing installed the launch fails with an
actionable message naming UMU, Proton and Wine.

The environment built for each mode:

- **UMU**: `PROTONPATH` (Proton folder or `GE-Proton`), `GAMEID` (the Steam app
  id, else `0`), `UMU_LOG=warn`, `STEAM_COMPAT_DATA_PATH`,
  `STEAM_COMPAT_CLIENT_INSTALL_PATH`, `WINEPREFIX`.
- **Proton**: `STEAM_COMPAT_DATA_PATH=<prefix>`,
  `STEAM_COMPAT_CLIENT_INSTALL_PATH=<Steam root>`; Proton creates its Wine
  prefix in `<prefix>/pfx` and `WINEPREFIX` points there.
- **Wine**: `WINEPREFIX=<prefix>`.

Per-game environment overrides (`DXVK_HUD`, `PROTON_NO_ESYNC`,
`WINEDLLOVERRIDES`…) are applied last. Names must match
`[A-Za-z_][A-Za-z0-9_]*`; `PATH`, `HOME`, `LD_PRELOAD`, `LD_LIBRARY_PATH` and
`PYTHONPATH` are refused. Playtime, the process tree, Discord presence and the
tray behave exactly as for native games.

**Wine prefixes (#92).** Each game gets `{installPath}/.prefix` unless a custom
absolute path is set. The Prefix section on the game page shows its size,
offers a folder picker, and deletes it after an inline confirmation. Reset
refuses while the game is running and never deletes the install folder or one
of its ancestors. The prefix is recreated empty on the next launch.

**Winetricks.** The stored verb list (plus common suggestions such as
`corefonts`, `vcrun2019`, `dotnet48`, `dxvk`) is run on demand as
`winetricks -q <verbs>` with `WINEPREFIX` set to the game prefix, the Proton
Wine binary when a Proton build is selected, and a 30-minute timeout. Output is
bounded and shown in the panel. Verbs are validated: lowercase identifiers,
no leading flag, at most 32.

**ProtonDB (#91).** The game page shows a ProtonDB badge (Platinum/Gold/
Silver/Bronze/Borked) linking to `protondb.com/app/<appid>`. The rating comes
from `https://www.protondb.com/api/v1/reports/summaries/<appid>.json` with an
8 s timeout and is cached in the desktop SQLite index for seven days. Offline,
the last cached tier is shown as `stale`; with no cache the badge degrades to
"ProtonDB offline". A game without a Steam match uses the manual Steam app id
in its launch options. The page never waits on the network to render.

**What still needs a real gaming box.** Detection, command building, prefix
validation and ProtonDB parsing are unit tested on Linux CI, but the following
were not executed in CI and need a machine with Steam/Proton/UMU and a display:
starting a real Proton/UMU game, a real `setup.exe` under Proton, winetricks
against a real prefix, and Steam's own `steam://` handler under UMU.

### Discord Rich Presence

Settings → Desktop → **Discord Rich Presence** (off by default), plus a Discord
*application id* (`discordClientId`). An application id only labels the
activity; it is not a secret and no bot is involved.

- While a game runs, Discord shows its title, the cover when the URL is public,
  and the elapsed session time. With nothing running it shows the idle activity.
- Covers are only sent over `https` and only for public hosts: loopback and
  private ranges are dropped instead of rendering a broken image.
- Ludex talks to the local Discord IPC socket. If Discord is closed, the
  connection is retried with a 4 s → 120 s exponential backoff; `presence_status`
  reports `connected`, `lastError` and `retryInSeconds`.
- Disabling the setting clears the activity. On a machine without Discord (most
  Linux servers) it simply stays disconnected; no socket, no error spam.
- Discord only accepts the activity when the application id is a real Discord
  application. Create one at <https://discord.com/developers/applications>
  (any name works, no bot user needed).

### Launcher import (#13, #116)

Settings → **Integrations → Launchers on this machine** reads the metadata each
launcher already keeps on disk, lists the games it installed and imports the
selected ones into Ludex. No files are downloaded, copied or moved, and the
owning client keeps ownership, updates and DRM.

| Launcher | Source read | Launch URI |
|---|---|---|
| Steam | `steamapps/libraryfolders.vdf`, `appmanifest_*.acf` (roots: `~/.steam/steam`, `~/.local/share/Steam`, Flatpak, `~/Library/Application Support/Steam`, `%ProgramFiles(x86)%\Steam` and the `SteamPath` registry value) | `steam://rungameid/<appid>` |
| Epic Games | `%ProgramData%\Epic\UnrealEngineLauncher\LauncherInstalled.dat` and `…\EpicGamesLauncher\Data\Manifests\*.item` (namespace + catalog item id) | `com.epicgames.launcher://apps/<ns>%3A<item>%3A<app>?action=launch&silent=true` |
| GOG Galaxy | `%ProgramData%\GOG.com\Galaxy\storage\galaxy-2.0.db` (read-only SQLite) and the `GOG.com\Games` registry keys | `goggalaxy://openGameView/<productId>` |
| EA app / Origin | `Electronic Arts\EA Games` registry keys; the offer id is used when the key carries one | `origin://launchgame/<offerId>` |
| Ubisoft Connect | `Ubisoft\Launcher\Installs` registry keys (`InstallDir` per numeric install id) | `uplay://launch/<installId>/0` |
| Xbox / Microsoft Store | `Get-AppxPackage` + `AppxManifest.xml` (application id), with `Get-StartApps` for names and as a fallback when `WindowsApps` is not readable; OS package families are filtered out | `shell:AppsFolder\<PackageFamilyName>!<AppId>` |
| itch.io | `butler.db` (`caves` joined to `games`) in `%APPDATA%\itch`, `~/.config/itch` or `~/Library/Application Support/itch` | `itch://caves/<caveId>/launch` |
| Heroic | `store_cache/legendary_library.json`, `store_cache/gog_library.json`, `sideload_apps/library.json` | `heroic://launch/<runner>/<app>` (sideload: `heroic://launch/<app>`) |
| Lutris | `~/.local/share/lutris/pga.db` (read-only SQLite, installed rows) | `lutris:rungameid/<id>` |

- Discovery is best effort and never fails a scan: a missing launcher, an
  unreadable database or a malformed manifest becomes a warning next to the
  list. Windows-only collectors are skipped on Linux and marked "Windows only";
  the Xbox list is the Start menu and Store packages, so it can contain
  non-games — untick them.
- A game is listed only when the launcher records it as installed and, when an
  install folder is recorded, that folder still exists.
- Titles are matched against the server library by normalised title, then once
  more with edition suffixes stripped (`GOTY`, `Definitive Edition`, …). An
  ambiguous title is never guessed: the row stays **Not in the library** and can
  be mapped by hand with the row's picker.
- Imported entries keep their server id when matched (playtime sync and the game
  page keep working) and a stable negative id otherwise. Steam keeps its
  historical `-appId`; other launchers derive the id from `source` + launcher
  entry id, so rescanning is idempotent.
- Duplicates are refused twice: the partial unique index on `launchUri` rejects a
  second import of the same launch target, and the import skips any entry whose
  launch URI is already in the local index. The scan also drops a game reported
  twice for the same install folder (for example Epic and Heroic).
- Entries whose launcher exposes no documented launch id (some EA app installs)
  are listed with an explicit reason and a **launch URI** field: paste one
  (`origin://launchgame/Origin.OFR.50.0000557`) to import them. `file:`,
  `javascript:`, `data:` and similar schemes are refused.
- **Uninstall** on an imported entry only removes the Ludex row: the launcher's
  folder is never touched, and there are no uninstall files in the install
  folder to clean up.
- `steamRoot` (Settings → Desktop → Steam folder) overrides Steam detection.
  `LUDEX_STEAM_ROOT` does the same for automation.
- Playtime for an imported game is tracked by Ludex only while the launcher's
  URI handler process tree is alive; the launcher's own counter stays
  authoritative.

### Game settings and engine adapters

The game page's Settings tab reads config files from the install folder only
(ini, cfg, Godot `project.godot`, Unity `prefs`, TOML, JSON). Every write is
type-checked, backed up under `{appDataDir}/config-backups/game-<id>/` and
recorded with a line diff; the Backups tab restores a previous version. Built-in
adapters detect Unreal Engine, GOG, Godot and Unity markers and expose
resolution, window mode, V-Sync, frame limit, quality and language as typed
controls. Settings stored in the Windows registry, Unreal's `%LOCALAPPDATA%` or
Godot's `user://` are read-only: Ludex never edits outside the install folder.
See `docs/CONTRACT.md` for the commands and `apps/desktop/README.md` for the
format details.

### Playnite

`integrations/playnite/` is a read-only Playnite library plugin: `GET
/api/games`, one Playnite entry per game, Play action = a Ludex URL. It needs an
API key with the `read` scope and is built manually (no .NET SDK in CI); see
`integrations/playnite/README.md`.

Configuration lives in `ludex.config.json` next to the plugin DLL:

```json
{
  "ServerUrl": "https://ludex.example.com",
  "ApiKey": "ldx_read_only_key",
  "LaunchUrlTemplate": "{serverUrl}/game/{gameId}",
  "IncludeUnavailable": false
}
```

Set `LaunchUrlTemplate` to `ludex://play/{gameId}` once a desktop build registers
the `ludex` scheme; until then the game page is opened in the browser. The plugin
never writes to the server.

### Streaming hosts (Sunshine)

Sunshine turns the machine that owns a game into a streaming host. Ludex
publishes its installed library to Sunshine so the game shows up in Moonlight
without adding each app by hand. Sunshine is external: Ludex detects it, edits
its `apps.json` after an explicit click and never bundles or installs it.

- **Detection.** Settings → Streaming reports the Sunshine binary, `apps.json`,
  `sunshine.conf` and how many entries Ludex already manages. The default
  folders are `%ProgramFiles%\Sunshine\config`, `%ProgramFiles(x86)%\Sunshine\config`,
  `%LOCALAPPDATA%\Sunshine`, `~/.config/sunshine`, the Sunshine Flatpak config
  (`~/.var/app/dev.lizardbyte.app.Sunshine/config/sunshine`) and
  `/etc/sunshine`. `apps.json`/`sunshine.conf` can be pointed at manually
  (`sunshineAppsPath`, `sunshineConfPath`); an override whose folder does not
  exist is a warning, not a crash.
- **What is published.** One entry per game installed through Ludex or imported
  from Steam: `name`, `cmd`, `working-dir`, `image-path` (the local cover cache)
  and `detached: "true"`. The command is the Steam launch URI when the entry is
  a Steam import, else the recorded executable, else `ludex://play/<id>`; the
  preview marks the last case because it needs a registered `ludex://` handler.
- **Marker and idempotency.** Every entry carries
  `"ludex": { "managed": true, "version": 1, "gameId": …, "serverId": … }`.
  Re-syncing updates the fields Ludex owns and never touches another app;
  entries belonging to a different saved server are left alone; removing the
  Ludex apps deletes only marked entries. A second sync with the same library
  reports `written: false` and changes nothing.
- **Preview and backup.** **Preview changes** lists what would be added, updated
  or removed without writing. Every real write first copies the current file to
  `apps.json.ludex-backup-<UTC>-<id>.bak`; when the backup cannot be created the
  write is aborted. A malformed `apps.json` is reported and left untouched.
- **Covers.** Covers are downloaded into
  `{appDataDir}/streaming/art/<gameId>.<ext>` (5 s timeout, 8 MiB cap). A cover
  failure only means the entry has no image.

### Moonlight client

Moonlight plays the stream. Ludex reads Moonlight's own host list and starts the
client; it never proxies video or stores credentials.

- **Detection and hosts.** The Moonlight binary is looked up on `PATH`, in
  `/usr/bin`, `/usr/local/bin`, the Flatpak exports and
  `%ProgramFiles%\Moonlight Game Streaming`; `moonlightBinary` overrides it.
  Hosts come from
  `~/.config/Moonlight Game Streaming Project/Moonlight.conf` or
  `%LOCALAPPDATA%\Moonlight Game Streaming Project\Moonlight.conf`
  (`moonlightConfigPath` overrides). Keys written as `host\address`,
  `host/localaddress`, a per-host section or a manual address are all accepted.
  A host counts as paired when Moonlight stored its certificate (`srvcert` /
  `servercert`) or a positive `pairstatus`.
- **Streaming.** A game published to Sunshine gains a **Stream** action on its
  page. Ludex resolves the app name from its own marker, refuses an unpaired
  host, and runs `moonlight stream <host> <app>`. The desktop stays out of the
  stream path: closing it does not stop the session.
- **Honest failures.** Missing Moonlight, a missing host list, an unpaired host
  and a game that was never published each produce a specific message instead
  of a silent no-op. macOS host parsing is reported as unsupported because
  Moonlight stores a plist there; the Stream action still needs a real host.

### Multiple servers and accounts

The desktop app and the web client can keep more than one Ludex server or
account and switch between them from the shell.

- **Profiles.** A profile has a label, a normalised URL, an optional username (a
  local note, not a credential) and last-use time. The embedded local server is
  the stable `local` profile even though its port changes across restarts.
- **Tokens.** Saved profile tokens go to the OS keychain: libsecret
  (`secret-tool`) on Linux, Keychain (`security`) on macOS. Without a keychain
  they go to `{appDataDir}/secrets.db` (mode 0600) and Settings → Servers shows
  a warning. The active connection token also stays in `settings.json` (0600),
  as in previous releases. In a browser, the session token is scoped per server
  in `localStorage` and never shared between servers.
- **Isolation.** Each profile has its own desktop index at
  `{appDataDir}/servers/<id>/index.db`: installed games, play sessions, install
  jobs and stream checkpoints never mix. The existing single-server index is
  copied into the first profile (the old file is kept). Switching is refused
  while a game runs or an install is active. Removing a profile deletes its
  saved token and metadata; its index and installed game folders stay on disk.
- **Migration.** On the first launch after upgrading, the current `serverUrl`
  and `token` become the first profile; nothing changes for the user. In the
  browser the existing `ludex.token` is copied into the first profile's token
  slot on load.
- **Connection test.** The add/edit dialog tests `GET {url}/api/health` with an
  8 s timeout and reports the version and latency; testing never saves.

**What still needs a real Sunshine/Moonlight machine.** Detection, merging,
backup, idempotency, config parsing and command building are unit tested on
Linux CI. Not executed in CI: a real Sunshine host serving a stream, a real
Moonlight pairing and playback, Windows and Flatpak path layouts, covers coming
from a live server, and macOS Moonlight host parsing.
