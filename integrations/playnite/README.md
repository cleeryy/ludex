# Ludex for Playnite

A small, read-only Playnite library plugin that lists a Ludex server's library
next to your other stores and opens each game through its Ludex URL.

- One REST call family (`GET /api/games`), no SDK of ours, no database access.
- Nothing is downloaded and nothing on the server is modified.
- Works with an API key (`ldx_…`, `read` scope) or a session JWT.
- Play action: an URL action (default: the Ludex game page).

## Build

The plugin targets Playnite's SDK, which ships with the Playnite installation.
No .NET SDK means no automated build here: build it manually.

Requirements: .NET SDK (or the `dotnet` bundled with Visual Studio), Playnite
installed for the current user, PowerShell.

```powershell
cd integrations/playnite
dotnet restore
dotnet build -c Release -p:PlayniteSdkPath="$env:LOCALAPPDATA\Playnite"
# or, without parameters, the csproj defaults to %LOCALAPPDATA%\Playnite
```

The output lands in `bin/x64/Release` (or `bin/Release`). A Playnite library
extension is a folder, so copy the files:

```powershell
$target = "$env:APPDATA\Playnite\Extensions\Ludex_Playnite"
New-Item -ItemType Directory -Force $target | Out-Null
Copy-Item bin/x64/Release/* $target -Recurse -Force
Copy-Item extension.yaml $target -Force
```

Restart Playnite → **Libraries → Configure integrations** → enable **Ludex** →
**Update library**.

## Configuration

`ludex.config.json` is created next to `LudexIntegration.dll` on first load:

```json
{
  "ServerUrl": "https://ludex.example.com",
  "ApiKey": "ldx_your-read-key",
  "LaunchUrlTemplate": "{serverUrl}/game/{gameId}",
  "IncludeUnavailable": false
}
```

| Key | Meaning |
|---|---|
| `ServerUrl` | Ludex base URL, no trailing slash. |
| `ApiKey` | API key with the `read` scope, or a session JWT. Empty only works against a server that allows anonymous reads (it does not). |
| `LaunchUrlTemplate` | Play action. `{serverUrl}` and `{gameId}` are substituted. Set it to `ludex://play/{gameId}` once your desktop build registers the `ludex` scheme. |
| `IncludeUnavailable` | Also list games with no downloadable version (metadata only). |

Create the key in Ludex → Settings → API keys (scope `read`). See
`docs/INTEGRATIONS.md`.

## Behaviour and limits

- **No two-way sync.** Playtime and state stay in Ludex; Playnite shows what the
  API reports at sync time. Editing a game in Playnite does not change Ludex.
- **Cover/background** are taken from the server metadata URLs. A private
  (LAN-only) URL only loads on a machine that can reach it.
- **Launching** opens the URL in the default browser. The desktop app handles
  `ludex://` only when a deep-link handler is registered (see
  `docs/INTEGRATIONS.md`, "Playnite").
- A failed sync logs an error and shows a Playnite notification instead of
  breaking the other libraries.
- The build is not exercised in CI (no .NET SDK); compile errors around SDK
  version changes are possible. Pin `PlayniteSDK` by using the DLL from your
  Playnite install, as above.

## Repository check

`dotnet` is not installed in the Ludex CI image, so this extension is shipped as
source. `dotnet build` (or Visual Studio) is the only build step; there is no
generated artifact to update in the repository.
