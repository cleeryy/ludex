# Packaging

Native packages for the desktop client. One directory today, `aur/ludex-bin`,
plus the check that keeps its digest honest.

```
packaging/
  aur/ludex-bin/
    PKGBUILD    pinned release, verified digest, installs the AppImage
    .SRCINFO    generated: makepkg --printsrcinfo, committed for the AUR web UI
```

The package is `-bin` on purpose: the client is a Tauri shell over WebKitGTK, and
a source build would drag a full Rust + Node toolchain onto every user's machine
for a 95 MB download that is already published.

## What `ludex-bin` installs

| Path | Content |
|---|---|
| `/opt/ludex/ludex.AppImage` | the published `Ludex_<version>_amd64.AppImage` |
| `/usr/bin/ludex` | wrapper: `exec /opt/ludex/ludex.AppImage --appimage-extract-and-run "$@"` |
| `/usr/share/applications/ludex.desktop` | launcher entry (`Exec=ludex`, `Icon=ludex`) |
| `/usr/share/icons/hicolor/512x512/apps/ludex.png` | icon, from the repository at the pinned tag |

`--appimage-extract-and-run` is not a preference: Arch ships no FUSE 2, so the
AppImage runtime has to unpack itself instead of mounting. No scriptlet exists,
so nothing runs `sudo`, and `pacman -Rns ludex-bin` removes exactly the four
paths above — the user data in `~/.local/share/cloud.cleeryy.ludex` and the
installed games are untouched.

## Checking a change

```sh
node scripts/check-packaging.mjs --offline   # PKGBUILD ↔ .SRCINFO, no network
node scripts/check-packaging.mjs             # digest against the release SHA256SUMS
```

The online pass reads `SHA256SUMS` for the tag the PKGBUILD pins (`v<pkgver>`)
with `gh release download`, falling back to the GitHub API with
`GITHUB_TOKEN`/`GH_TOKEN`, then the plain release URL. CI runs both passes plus a
real `makepkg` build in `archlinux:latest` — [`.github/workflows/packaging.yml`](../.github/workflows/packaging.yml).

To build the package by hand (no Arch host required):

```sh
docker run --rm -it -v "$PWD/packaging/aur/ludex-bin:/pkg:ro" archlinux:latest bash
# in the container
pacman -Syu --noconfirm base-devel
useradd -m builder && cp -a /pkg /home/builder/pkg && chown -R builder /home/builder/pkg
su builder -c 'cd ~/pkg && makepkg -si --noconfirm'
```

`pacman -Q ludex-bin` reports the packaged version; the binary is the AppImage, so
there is no separate `ludex --version` output to compare.

### Accepted namcap findings

| Finding | Why |
|---|---|
| `E: ELF files outside of a valid path ('opt/')` | the AppImage lives in `/opt/ludex`, the layout #277 asks for; namcap cannot introspect it |
| `W: Dependency included, but may not be needed (webkit2gtk-4.1, gtk3, libayatana-appindicator)` | the AppImage shell loads WebKitGTK, GTK and the tray library from the host stack |
| `W: Referenced library 'sh' is an uninstalled dependency` | the `/usr/bin/ludex` wrapper is a POSIX shell script |

The CI job fails on any other `E:` line.

## Publishing to the AUR

One-time setup: an AUR account with your SSH public key registered, then

```sh
git clone ssh://aur@aur.archlinux.org/ludex-bin.git
cd ludex-bin
cp <ludex>/packaging/aur/ludex-bin/PKGBUILD .
cp <ludex>/packaging/aur/ludex-bin/.SRCINFO .
git add PKGBUILD .SRCINFO
git commit -m "0.4.22-1"
git push
```

The AUR serves the `.SRCINFO` verbatim; a push without it is invisible. Generate
it, never edit it:

```sh
cd packaging/aur/ludex-bin
makepkg --printsrcinfo > .SRCINFO
```

## Bumping for a release

Do this after the GitHub release is published (the digests do not exist before):

1. `pkgver=<new version>` in the PKGBUILD, and `pkgrel=1` unless only the
   packaging changed.
2. Read the digest from the release, never from memory:

   ```sh
   gh release download "v$VERSION" --repo cleeryy/ludex --pattern SHA256SUMS --dir /tmp
   grep "Ludex_${VERSION}_amd64.AppImage" /tmp/SHA256SUMS
   ```

3. Replace the first `sha256sums` entry with that digest.
4. Refresh the icon digest: the second source is
   `raw.githubusercontent.com/.../v<pkgver>/...icon.png`, so the URL moves with
   the tag.

   ```sh
   curl -fsSL "https://raw.githubusercontent.com/cleeryy/ludex/v$VERSION/apps/desktop/src-tauri/icons/icon.png" \
     | sha256sum
   ```

5. Regenerate and check:

   ```sh
   makepkg --printsrcinfo > .SRCINFO
   node scripts/check-packaging.mjs
   ```

6. Commit both files in the repo (`chore(packaging): ludex-bin $VERSION`), bump
   the AUR clone the same way (`git commit -m "$VERSION-1" && git push`).

`scripts/check-packaging.mjs` is the gate: it refuses a digest that does not
match the release `SHA256SUMS`, a `.SRCINFO` that no longer matches the PKGBUILD,
and an AppImage URL that does not point at the pinned tag — so a forgotten icon
or a stale `.SRCINFO` fails before the AUR sees it.

## Not shipped

- **`ludex-edge-bin`**: optional variant of the `edge` prerelease. Not published
  until `ludex-bin` has proven itself.
- **Source package**: explicitly out of scope (#277) — hours of Tauri build per
  user.
- **winget / Homebrew cask**: tracked as follow-ups in
  [`docs/INSTALL.md`](../docs/INSTALL.md); both wait on code signing.
