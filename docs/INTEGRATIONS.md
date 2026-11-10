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
`game.deleted`, `download.completed`, `user.created` and `save.uploaded`.

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

### Steam library import

Settings → **Integrations → Steam library** reads Steam's own metadata, lists the
installed Steam games and imports the selected ones into Ludex. No files are
downloaded and Steam keeps ownership, updates and DRM.

- Discovery reads `steamapps/libraryfolders.vdf` in the default roots
  (`~/.steam/steam`, `~/.local/share/Steam`, Flatpak, `~/Library/Application
  Support/Steam` on macOS, `%ProgramFiles(x86)%\Steam` and the `SteamPath`
  registry value on Windows) and every extra library it points at, then
  `appmanifest_*.acf`.
- Only manifests with `StateFlags` bit 4 (fully installed) and an existing
  `steamapps/common/<installdir>` folder are listed.
- Matching to the Ludex library is by normalised title, then once more after
  stripping edition suffixes (`GOTY`, `Definitive Edition`, …). An ambiguous
  title is never guessed.
- Imported games launch through `steam://rungameid/<appid>`. Matched games keep
  their server id (so playtime sync and the game page keep working); unmatched
  ones get a negative local id and an **Import from Steam** entry appears in
  "On this device".
- Re-importing is refused, and a game already installed through Ludex is left
  alone. **Uninstall** on a Steam import only removes the Ludex entry: the Steam
  folder is never touched.
- `steamRoot` (Settings → Desktop → Steam folder) overrides auto-detection.
  `LUDEX_STEAM_ROOT` does the same for automation.
- Playtime for a Steam game is tracked by Ludex only while the `steam://` handler
  process tree is alive; Steam's own counter stays authoritative.

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
