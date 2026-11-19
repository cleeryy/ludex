# Installing Ludex on Linux

`install.sh` fetches the right release asset for your machine, verifies it with
the minisign key that ships in the app, and drops it in your home directory. It
is a convenience path over artifacts that already exist — never the only path.
The audited path (download, verify, run) is documented below and printed by the
script itself whenever it cannot verify something.

## One line

```sh
curl -fsSL https://ludex.cleeryy.cloud/install.sh | sh
```

The same script is attached to every release, so the version-independent URL
`https://github.com/cleeryy/ludex/releases/latest/download/install.sh` serves
the same bytes if you would rather read it first:

```sh
curl -fsSL https://github.com/cleeryy/ludex/releases/latest/download/install.sh | less
```

Nothing is downloaded before `SHA256SUMS` itself has been verified against the
pinned key (see [Verification](#verification)). There is no telemetry, no
banner, and `sudo` is used by one code path only: `--deb`.

## Channels

| Channel | Tag | Manifest | Notes |
|---|---|---|---|
| `stable` (default) | newest `v*` tag | `latest.json` of the stable release | Signed `SHA256SUMS` covers every asset |
| `edge` | rolling `edge` prerelease | `/releases/download/edge/latest.json` | Built from `main`; each artifact carries its own minisign signature |

```sh
sh install.sh --channel edge     # early build, always a prerelease
sh install.sh --version v0.4.22  # pin an exact release
```

Every stable release publishes the same Linux bundles — AppImage, `.deb` and
`.rpm`; `edge` builds only the AppImage and the `.deb`. They differ in how they
are verified, not in what they install.

`--version` accepts `v0.4.22` or `0.4.22` and skips the manifest entirely.
GitHub resolves `/releases/latest` to the newest non-prerelease, so an edge
build is never installed by the default command.

## What it does

1. Refuses any architecture other than `x86_64`/`amd64` (exit `2`) — no arm64
   artifact exists today, and installing one that cannot run is worse than
   saying so.
2. Checks that `libwebkit2gtk-4.1` is present (see
   [Dependencies](#dependencies)).
3. Downloads `SHA256SUMS` and `SHA256SUMS.sig`, verifies the signature with the
   pinned key, then downloads the artifact and checks its SHA-256 against the
   now-trusted list. For `edge`, which publishes no `SHA256SUMS`, the artifact's
   own `.sig` is verified instead.
4. Installs the AppImage to `~/.local/bin/ludex`, the desktop entry to
   `~/.local/share/applications/ludex.desktop`, the icon to
   `~/.local/share/icons/hicolor/256x256/apps/ludex.png` and writes
   `~/.local/share/ludex/install.json` (the record `--uninstall` uses).

Re-running is an upgrade: the recorded prefix wins over the default, the binary
is replaced atomically, and the record is rewritten.

## Options

| Option | Effect |
|---|---|
| `--version <tag>` | Install an exact release; no manifest lookup |
| `--channel stable\|edge` | Pick the channel (default `stable`) |
| `--deb` | Download `Ludex_<version>_amd64.deb` and install it with `sudo dpkg -i` (needs apt/dpkg) |
| `--dir <path>` | Absolute install prefix (default `~/.local`, or the recorded one) |
| `--uninstall` | Remove exactly the files listed in the record |
| `--purge` | With `--uninstall`: also delete `~/.local/share/ludex` (save data, settings) |
| `--force` | Continue when `libwebkit2gtk-4.1` is missing |
| `--help` | Usage, exit codes, audited commands |

`--version` and `--channel` are mutually exclusive; `--dir`, `--deb`,
`--version` and `--channel` cannot be combined with `--uninstall`.

The record lives in `${XDG_DATA_HOME:-~/.local/share}/ludex/install.json`, so a
custom `--dir` is still uninstallable later.

## Verification

The trust anchor is `plugins.updater.pubkey` in
`apps/desktop/src-tauri/tauri.conf.json` — the same key that signs the updater
bundles. It is pinned in `install.sh`; `scripts/test-install.sh` fails if the
two ever drift.

```
untrusted comment: minisign public key: 67DF1F7D6F8425D8
RWTYJYRvfR/fZ7gvqJswoVBRjOGygHbRCgCkyUEF5pQFLYpiEnwDthsC
```

Release assets ship `SHA256SUMS.sig` as base64 of the minisign signature (that
is what the Tauri signer writes), hence the `base64 -d` step. Verify it
yourself:

```sh
version=0.4.22
base=https://github.com/cleeryy/ludex/releases/download/v$version

curl -fsSLO "$base/Ludex_${version}_amd64.AppImage"
curl -fsSLO "$base/SHA256SUMS"
curl -fsSLO "$base/SHA256SUMS.sig"

base64 -d SHA256SUMS.sig > SHA256SUMS.minisig
minisign -Vm SHA256SUMS -x SHA256SUMS.minisig \
  -P 'RWTYJYRvfR/fZ7gvqJswoVBRjOGygHbRCgCkyUEF5pQFLYpiEnwDthsC'
sha256sum -c --ignore-missing SHA256SUMS

chmod +x Ludex_${version}_amd64.AppImage
./Ludex_${version}_amd64.AppImage
```

The edge prerelease has no `SHA256SUMS` asset. Its artifacts are signed
individually, so verify the bundle directly:

```sh
base=https://github.com/cleeryy/ludex/releases/download/edge
version=0.4.23-edge.1

curl -fsSLO "$base/Ludex_${version}_amd64.AppImage"
curl -fsSLO "$base/Ludex_${version}_amd64.AppImage.sig"
base64 -d "Ludex_${version}_amd64.AppImage.sig" > Ludex.minisig
minisign -Vm "Ludex_${version}_amd64.AppImage" -x Ludex.minisig \
  -P 'RWTYJYRvfR/fZ7gvqJswoVBRjOGygHbRCgCkyUEF5pQFLYpiEnwDthsC'
```

If `minisign` is missing, the installer does not fall back to a checksum-only
install: it prints the commands above with the resolved tag and exits `1`. A
checksum file you have not authenticated proves nothing.

## Dependencies

| Runtime | Ubuntu/Debian | Arch | Fedora |
|---|---|---|---|
| WebKitGTK 4.1 + GTK 3 | `sudo apt-get install libwebkit2gtk-4.1-0 libgtk-3-0` | `sudo pacman -S webkit2gtk-4.1 gtk3` | `sudo dnf install webkit2gtk4.1 gtk3` |

The preflight looks for `libwebkit2gtk-4.1.so*` through `ldconfig -p` and a
filesystem search. When it is missing, the installer prints the command for
your family and stops — `--force` installs anyway, which is useful when the
library lives outside the usual paths, not when you want a broken window.

AppImages need FUSE 2 to self-mount. Ubuntu 24.04 ships `libfuse2t64`; on Arch
and CachyOS install `fuse2`, or launch with the extraction fallback:

```sh
APPIMAGE_EXTRACT_AND_RUN=1 ~/.local/bin/ludex
```

## .deb instead of the AppImage

`--deb` downloads the package Tauri builds next to the AppImage and runs
`sudo dpkg -i`. It is the right choice on Debian/Xubuntu images where FUSE is
unavailable, and it is the only path that needs sudo. When dpkg reports unmet
dependencies, finish with `sudo apt-get install -f`. `--dir` does not apply.

## .rpm instead of the AppImage

Tauri builds `Ludex-<version>-1.x86_64.rpm` next to the AppImage and the `.deb`.
`install.sh` does not handle it: verify it like any other asset (see
[Verification](#verification)), then install the local file so dnf resolves the
dependencies.

```sh
version=0.4.22   # the release you verified
sudo dnf install "./Ludex-${version}-1.x86_64.rpm"
# or, with the dependencies already present
sudo rpm -i "Ludex-${version}-1.x86_64.rpm"
```

It is the right choice on Fedora/RHEL, and on any host where games run through
Python-based compatibility tools: the AppImage exports `PYTHONHOME`,
`PYTHONPATH`, `LD_LIBRARY_PATH` and `LD_PRELOAD` pointing into `/.mount_*`, and
those values leak into every process it spawns (#278). A native rpm installs
real files under `/usr` and exports none of them.

## Uninstall

```sh
sh install.sh --uninstall           # binary, desktop entry, icon, record
sh install.sh --uninstall --purge   # also ~/.local/share/ludex (saves, settings)
```

`--uninstall` removes exactly the files listed in the record and refuses paths
outside the recorded prefix. Your save data in `~/.local/share/ludex` is kept
unless `--purge` is given. For a `--deb` install, the record triggers
`sudo dpkg -r ludex` instead.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Installed, upgraded, or uninstalled |
| `1` | Verification, dependency, download or install failure |
| `2` | Unsupported architecture (not `x86_64`/`amd64`) |
| `3` | Usage error (unknown option, missing value, conflicting flags) |

## Testing the installer

`scripts/test-install.sh` runs the real script against a local
`python3 -m http.server` fixture: a throwaway minisign key signs fake assets,
and every failure path is asserted — tampered artifact, tampered `SHA256SUMS`,
tampered signature (before any artifact download), non-x86_64 refusal, missing
`minisign` (exit `1` with the audited commands), missing
`libwebkit2gtk-4.1` (blocked unless `--force`), `--uninstall` keeping user data,
`--purge`, usage errors and the edge channel. It needs `minisign`, `python3`,
`curl`, `sha256sum` and `base64`; without them it skips with a message instead
of downloading a real release asset.

```sh
./scripts/test-install.sh
```

The `Install script` workflow (`.github/workflows/install.yml`) runs shellcheck
on `install.sh` and that test on `ubuntu-24.04` for every change to either
script.

## Design notes

- POSIX `sh`, `set -eu`, every expansion quoted, no `eval`, no nested
  `curl | sh`, no `sudo` outside `--deb`.
- Verification happens before installation and, on the stable channel, before
  the artifact is downloaded.
- One line per step, the installed version and the next command at the end.
- The script never overwrites a custom prefix silently: a recorded install is
  upgraded in place.
