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
- Silent runs are Windows-only: `run_installer` refuses to execute anywhere else
  but `installer_plan` still reports the plan. An installer that needs elevation
  fails to start; run it interactively instead.

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
