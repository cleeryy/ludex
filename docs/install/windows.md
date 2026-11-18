# Installing Ludex on Windows

Ludex ships a per-user NSIS installer: no administrator prompt, no service, no
machine-wide changes. It installs under `%LOCALAPPDATA%\Ludex`, adds a Start
Menu shortcut and one uninstall entry under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall`.

`install.ps1` wraps that installer: it resolves the release, verifies the
download and runs the setup silently. It never elevates, never changes the
execution policy and never unblocks files.

## One command

In a normal PowerShell window (Windows 10 1809+ or Windows 11, x64):

```powershell
irm https://raw.githubusercontent.com/cleeryy/ludex/main/install.ps1 | iex
```

The script is not written to disk, so a default `RemoteSigned` session runs it
as-is. It needs internet access to GitHub, and to Microsoft once if WebView2 is
missing (the NSIS bundle uses the WebView2 bootstrapper).

What happens, in order:

1. Resolve the latest **stable** tag from `latest.json`
   (`https://github.com/cleeryy/ludex/releases/latest/download/latest.json`,
   GitHub API as fallback).
2. Download `SHA256SUMS` and `Ludex_<version>_x64-setup.exe` into a temporary
   folder.
3. Verify the installer (see [Verification](#verification)).
4. Print the SmartScreen warning that the unsigned setup will trigger, then run
   it with `Start-Process -Wait -FilePath <setup> -ArgumentList '/S'`. Per-user,
   no UAC prompt.
5. Read the version back from the installed app (`DisplayVersion` in HKCU plus
   `Ludex.exe`'s file version) and print the install location. The tag string is
   never used as the reported version.

## Verification

Every release publishes `SHA256SUMS` and `SHA256SUMS.sig`, signed with the same
minisign key as the Tauri updater bundles. The script pins the public key from
[`tauri.conf.json`](../../apps/desktop/src-tauri/tauri.conf.json)
(`plugins.updater.pubkey`, key id `67DF1F7D6F8425D8`).

| `minisign.exe` on `PATH` | What the script checks |
|---|---|
| yes | minisign signature over `SHA256SUMS`, minisign signature over the installer itself, then the installer's SHA-256 against `SHA256SUMS` |
| no | the installer's SHA-256 against `SHA256SUMS` only |

The line `Verification: ...` in the output always says which method actually
ran. When minisign is absent it also says the publisher signature was **not**
checked. To get the stronger path, put `minisign.exe` on `PATH` (a
[jedisct1/minisign](https://github.com/jedisct1/minisign/releases) release, or
`scoop install minisign`).

A signature failure or a hash mismatch aborts before anything is installed and
exits non-zero. Nothing is "repaired" or retried silently.

## SmartScreen: expected on first run

Ludex has no Authenticode certificate yet, so the first run of the setup may
trigger SmartScreen with "Windows protected your PC" and an unknown publisher,
regardless of whether it came from this script, a browser or `winget`.

- Choose **More info**, then **Run anyway**.
- That prompt is a publisher reputation gap, not a failed signature. The
  SHA-256 (and minisign signature, when available) was already verified.
- `Unblock-File` is deliberately not used: it only removes the mark-of-the-web
  from a downloaded file, it does not vouch for a publisher.
- The real fix is a code-signing certificate (OV/EV), tracked separately
  because it has a recurring cost.

## Options

Download the script first when you need options; `iex` cannot pass arguments.

```powershell
$installer = Join-Path $env:TEMP 'install-ludex.ps1'
irm https://raw.githubusercontent.com/cleeryy/ludex/main/install.ps1 -OutFile $installer
& $installer -WhatIf
```

| Option | Effect |
|---|---|
| `-WhatIf` | Print the resolved tag, installer, expected SHA-256 and the exact install command; download and install nothing. |
| `-Version <tag>` | Install a specific tag, e.g. `-Version v0.4.22` (`0.4.22` is accepted too). |
| `-Channel edge` | Install the rolling `edge` prerelease instead of stable. |
| `-Dir <path>` | Install somewhere else; mapped to the NSIS `/D=<path>` switch (must stay per-user; do not point it at `Program Files`). |
| `-Force` | Reinstall even when the same version is already installed. |
| `-Uninstall` | Remove Ludex instead of installing it. |
| `-BaseUri <dir or url>` | Use another asset root (air-gapped mirror, tests). |

Examples:

```powershell
# Preview, touching nothing
& $installer -WhatIf

# Pin a version from two releases ago
& $installer -Version v0.4.20

# Install the edge build into a custom per-user folder
& $installer -Channel edge -Dir "$env:LOCALAPPDATA\LudexEdge"
```

Running the script over an existing install upgrades it in place: the old
`Ludex.exe` is replaced under `%LOCALAPPDATA%\Ludex`, not copied next to it.
The same version is a no-op unless `-Force` is passed. Close Ludex first; the
script refuses to install or uninstall while `Ludex.exe` is running.

## Uninstall

```powershell
& $installer -Uninstall
```

`-Uninstall` reads the entry under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall` whose `DisplayName`
is `Ludex`, runs its uninstaller with `/S`, and waits for the registry entry to
disappear before reporting success. It is idempotent: with nothing installed it
prints "nothing to uninstall" and exits 0.

Settings, library cache and crash reports under `%APPDATA%\cloud.cleeryy.ludex`
are left in place, so a later reinstall keeps your server profile.

Settings -> Apps -> **Ludex** -> Uninstall does the same thing.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Installed, upgraded, already current, or nothing to uninstall. |
| `1` | Verification failed, the installer returned non-zero, Ludex was running, or no install entry appeared. |

## Troubleshooting

- **"Windows protected your PC"** -- expected, see [SmartScreen](#smartscreen-expected-on-first-run).
- **"Ludex is running (PID ...)"** -- quit the app from its tray/menu and re-run.
- **"SHA-256 mismatch"** -- the download was corrupted or the asset was replaced
  after `SHA256SUMS` was published; nothing was installed. Re-run, and compare
  the printed SHA-256 with the release page if it persists.
- **Install finishes but Windows cannot start the app** -- WebView2 is missing
  and the bootstrapper could not reach Microsoft. Install the WebView2 Evergreen
  Runtime, then re-run the script.
- **Corporate proxy / offline** -- mirror the release assets and point
  `-BaseUri` at the directory (it must contain `SHA256SUMS` and the setup).

## How the script itself is tested

- `scripts/test-install.ps1` -- Pester-free: AST parse, a local `-WhatIf`
  fixture, a tampered `SHA256SUMS` that must fail without installing, and
  assertions that the script text never calls `Set-ExecutionPolicy` or
  `Unblock-File`.
- `.github/workflows/install-windows.yml` -- parses the script with Windows
  PowerShell 5.1 and `pwsh -NoProfile`, runs the `-WhatIf` fixture, then the test
  script. No network, no installer execution.

[`install.ps1`](../../install.ps1) is the source of truth for the behaviour
described here; when in doubt, `-WhatIf` prints the exact plan.
