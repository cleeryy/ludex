# Application self-update, by install kind (#312)

Ludex updates itself from the channel manifest (`latest.json`). How an update is
installed depends on **how the running binary was installed**, which the updater
now decides at update time instead of trusting the bundle type compiled into the
binary.

| Install kind | How it is detected | Who installs the update |
|---|---|---|
| AppImage | `$APPIMAGE` is set | `tauri-plugin-updater` replaces the running AppImage |
| dpkg / rpm package | `dpkg-query -S` / `rpm -qf` names a package that owns the executable | `tauri-plugin-updater` runs `pkexec dpkg -i` / `rpm -U`; if that fails the error names the package and the command that updates it |
| Portable ELF | none of the above (a hand-placed binary, an extracted rpm/deb payload, `install.sh --deb` where the package record was lost) | Ludex itself, see below |
| Windows NSIS/MSI, macOS bundle | the platform | `tauri-plugin-updater` |

The detection lives in `apps/desktop/src-tauri/src/update/install_kind.rs` and is
logged at the start of every install attempt (`install kind detected`, with the
executable, the owning package and the `install.sh` record).

`install.sh`'s record (`~/.local/share/ludex/install.json`) is a hint, never an
authority: it resolves the real file behind a wrapper or a symlink and is
ignored when it names another prefix or when the package database disagrees.

## The portable path

`apps/desktop/src-tauri/src/update/portable.rs`:

1. **Manifest.** `GET` the channel's `latest.json` (the same endpoint the check
   used) and take the `linux-x86_64-deb` entry. Both `stable` and `edge` publish
   it; a release without it is refused with the missing asset named. The `.deb`
   is used because it is always shipped and its `data.tar.gz` is readable
   without an extra archive dependency — an rpm payload is not extracted.
2. **Version.** The manifest's version becomes the installed version. A manifest
   that still points at the running version is refused.
3. **Download.** The artifact is streamed to memory with `update://progress`
   events (same payload as the plugin path). Query strings and userinfo never
   reach a log line.
4. **Signature.** The `.deb` is verified with the base64 minisign signature the
   manifest carries and the public key in `tauri.conf.json`
   (`plugins.updater.pubkey`) — the exact call `tauri-plugin-updater` makes
   (prehashed minisign, same key). A manifest without a signature is refused
   before anything is written.
5. **SHA-256.** The artifact digest is computed and logged (`sha256=<hex>`) so an
   artifact can be compared with `SHA256SUMS` after the fact. It is not the
   trust anchor: the signature is. A SHA-256 mismatch against a signed list
   means the release page itself is wrong, not the download.
6. **Extract.** `usr/bin/ludex` is read from `data.tar` / `data.tar.gz` /
   `data.tar.zst` inside the `.deb`, then checked: non-empty, `\x7fELF`, 64-bit.
   The embedded `LUDEX_BUILD_COMMIT` marker is logged when present.
7. **Stage.** The verified binary is written to
   `{appDataDir}/updates/ludex-<version>` (0644 → 0755, fsynced) so a refused
   install can print a command that works. Older staged builds are removed.
8. **Replace, atomically.**
   - probe the target folder by creating and deleting a probe file; a failure
     becomes the precise message below;
   - write `<target>.new-<pid>` next to the target, fsync it, `chmod 0755`;
   - rename `<target>` to `<target>.old` (this is the rollback point);
   - rename the temporary file to `<target>`, fsync the directory;
   - if the second rename fails, `<target>.old` is renamed back. A failure
     reports which of the three states it reached: untouched, rolled back, or
     backup kept (with the path).

   The running process keeps its own inode, so replacing the file under it is
   safe; the new binary is used on the next start. `install-state.json` records
   the version before the replace, so the next start's self-check (#186) still
   verifies that the version that runs is the one that was installed.
9. **Restart.** The command returns the usual `UpdateStatus` with
   `update://installed`; the update center offers **Restart Ludex** (the same
   button the AppImage flow uses). Nothing restarts silently.

### When the target is not writable

A root-owned `/opt/ludex/ludex` (the common case on Arch-based distributions) is
not a generic failure. The error names the file, its owner and mode, the folder
that refused the write, the cause, and the exact command:

```
Could not install the update. /opt/ludex/ludex is owned by root:root (mode 0755)
and /opt/ludex is not writable by this account: Permission denied (os error 13).
Nothing was replaced. The verified update is kept at
/home/x/.local/share/cloud.cleeryy.ludex/updates/ludex-0.4.25; install it with:
  sudo install -m755 '/home/x/.local/share/cloud.cleeryy.ludex/updates/ludex-0.4.25' '/opt/ludex/ludex'
then start Ludex again.
```

The same manual path, independent of the app:

```bash
curl -fsSL -o /tmp/ludex.deb \
  https://github.com/cleeryy/ludex/releases/latest/download/Ludex_0.4.26_amd64.deb
# verify SHA256SUMS + SHA256SUMS.sig first (docs/install/linux.md)
bsdtar -xOf /tmp/ludex.deb data.tar.gz | tar -xzO usr/bin/ludex > /tmp/ludex-new
sudo install -m755 /tmp/ludex-new /opt/ludex/ludex
```

On Arch-based systems the AUR package (`ludex-bin`, `packaging/aur/ludex-bin`)
keeps a system-wide install current instead; `install.sh --dir` installs under
a user-writable prefix where the in-app portable update needs no elevation.

### Rollback

- Nothing is renamed until the download verified and the new file was fsynced.
- The previous binary stays at `<target>.old` after a successful update. To go
  back: `sudo install -m755 '<target>.old' '<target>'`, then restart Ludex. The
  next update overwrites the backup.
- A failed second rename restores the backup automatically and the error says
  so; if even that fails the error names the backup path instead of claiming a
  clean state.

## Logs

The update attempt writes to `{appDataDir}/logs/ludex.log`
(`~/.local/share/cloud.cleeryy.ludex/logs/ludex.log`) and stderr:

```
INFO ludex_desktop_lib::update::install_kind: install kind detected kind=portable exe=/opt/ludex/ludex package=none record=none
INFO ludex_desktop_lib::update::app: downloading the application update version=0.4.26 exe=/opt/ludex/ludex install_kind=portable
INFO ludex_desktop_lib::update::portable: application update attempt kind=portable version=0.4.26 current=0.4.24 endpoint=https://github.com/cleeryy/ludex/releases/latest/download/latest.json target=/opt/ludex/ludex
INFO ludex_desktop_lib::update::portable: portable update artifact downloaded bytes=36148840 sha256=…
INFO ludex_desktop_lib::update::portable: portable update binary replaced target=/opt/ludex/ludex backup=/opt/ludex/ludex.old commit=…
INFO ludex_desktop_lib::update::portable: application update installed kind=portable … signature=verified sha256=…
```

A failure logs one line with the stage and the reason, and the same reason is
what the UI shows:

```
WARN ludex_desktop_lib::update::portable: application update failed stage=writability attempt=… failure="Could not install the update. /opt/ludex/ludex is owned by root:root …"
```

Every URL is scrubbed (userinfo, query, fragment) and messages are collapsed to
one line before they are logged; signatures, keys and tokens are never logged.
The plugin path logs the raw updater error before mapping it, so a
`Failed to install package` in the log always comes with the install kind and
the executable it applied to.

## What is intentionally not changed

- The Tauri updater signing flow: same key, same `latest.json`, same `.sig`
  files, same `plugins.updater.pubkey`. The portable path reuses them.
- The release pipeline: no new asset, no new manifest entry. The `.deb` already
  published is the update payload.
- Windows and macOS installs keep the plugin's own installers.
