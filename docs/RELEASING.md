# Releasing Ludex

Releases are tag-driven, E2E-gated and built entirely on GitHub-hosted runners.
The only mandatory secret is the automatic `GITHUB_TOKEN`; the desktop updater
optionally uses a minisign signing key (see *Updater keys and channels*) and
there is no deploy key or self-hosted runner in the release path.

## Updater keys and channels

The desktop app updates itself through `tauri-plugin-updater`. Releases must be
signed with a minisign key pair:

```sh
# once per maintainer machine; keep the private key out of the repository
pnpm --filter @ludex/desktop exec tauri signer generate -w ~/.tauri/ludex.key
```

- The **public** key goes into `apps/desktop/src-tauri/tauri.conf.json`
  (`plugins.updater.pubkey`). It is safe to commit.
- The **private** key and its password are repository secrets:
  `TAURI_SIGNING_PRIVATE_KEY` (file contents or path) and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. They are provisioned: the committed
  `plugins.updater.pubkey` matches them, and
  `bundle.createUpdaterArtifacts` is **true**.
- The private key and password were generated once into `~/.tauri/ludex.key` /
  `~/.tauri/ludex.key.password` (mode 0600). Back both up **offline** (password
  manager or encrypted backup); they are the only way to sign future updates.
  Never commit them or paste them into an issue, log or chat.
- The desktop job fails fast when artifacts are enabled but the secrets are
  missing. `tauri-action` signs every bundle, uploads the `.sig` files and
  merges the Windows/Linux entries into a single stable `latest.json`.
- Rotation: publish one release signed with the **old** key that contains the
  **new** public key, then swap the secret. Existing installs verify the old
  signature and accept the new key from that update. Revocation without a
  transitional release requires a manual reinstall.

Local dev builds should keep `createUpdaterArtifacts` off (or use a throwaway
key); unsigned dev builds report updates as *not configured*.

### Channels

| Channel | Endpoint | Published from |
|---|---|---|
| `stable` | `/releases/latest/download/latest.json` | the newest non-prerelease tag (`vX.Y.Z`) |
| `early` | `/releases/download/edge/latest.json` | the moving `edge` prerelease — **not published yet**: the edge workflow builds the server only, so the app reports the channel as unavailable instead of offering a broken update |

GitHub's `/releases/latest` skips prereleases, so stable never picks up `edge`.
The `edge-desktop` CI job publishes the early channel on every `main` push that
passes `rust`, `web` and `E2E`: it builds Linux (AppImage/deb) and Windows
(NSIS) bundles with a monotonic version (`<next patch>-edge.<run_number>`, for
example `0.3.1-edge.42`), signs them, merges the updater entries into
`latest.json` and uploads everything to the rolling `edge` prerelease together
with `.sha256` checksums. The same bundles are kept as a 14-day workflow
artifact named `ludex-edge-<platform>-<sha>` so a specific commit can be tested
without keeping a release per commit.

Signing keys rotate by publishing a release signed with the new key **and**
shipping the new public key in the previous release, otherwise existing installs
cannot verify the update.

### What the update center shows

The desktop update center (`/updates`, #163) renders the channel's `latest.json`
directly: version, date, release notes and the release page. It also prints the
endpoint that was queried, so a wrong channel or a private mirror is visible
without reading logs.

- `stable` publishes the release body as `notes`; write release notes in the
  GitHub release, not only in the tag.
- `edge` currently publishes `"notes": ""` and a generic `Ludex edge` title.
  The update center then falls back to the stamped build number
  (`<next patch>-edge.<run_number>`) and links to the rolling `edge` release, so
  testers can still identify the build. Filling `notes` (commit subject list) in
  the edge workflow would make the center show the full story.
- Skipping a version is a local preference (`skippedUpdateVersion` in desktop
  settings). It never hides the update from the center and never blocks an
  install; only the launch prompt, the badge and the toast are silenced until a
  newer version appears.
- Switching between stable and early persists the channel and re-checks
  immediately. Early installs show the edge warning in the center.

### Windows install layout (updates, #186)

A Windows update only works when the installer replaces the executable the app
actually runs. Ludex pins one layout:

| Item | Value | Where |
|---|---|---|
| Install mode | `currentUser` (per-user) | `bundle.windows.nsis.installMode` in `tauri.windows.conf.json`, mirrored in `tauri.conf.json` |
| Install folder | `%LOCALAPPDATA%\Ludex` | — |
| Executable | `%LOCALAPPDATA%\Ludex\Ludex.exe` (stored as `ludex.exe`; Windows is case-insensitive) | — |
| Shortcut | one Start Menu entry, per-user | — |
| Update UI | `passive` (progress bar, no prompts) | `plugins.updater.windows.installMode` |
| Startup log | `%APPDATA%\cloud.cleeryy.ludex\logs\ludex.log` | — |
| Update record | `%APPDATA%\cloud.cleeryy.ludex\install-state.json` | — |

The updater downloads the NSIS installer and launches it with `/P /R`; the Tauri
plugin exits, NSIS replaces the binary and relaunches it. Every build carries a
`LUDEX_BUILD_COMMIT=<sha>` marker (stamped by `apps/desktop/src-tauri/build.rs`
from the CI variable of the same name). `install_update` records the target
version in `install-state.json` before the installer runs. On the next start
the app writes the running path, version and commit to the log, compares the
running version with the recorded one, and stores an `InstallMismatch` when
they differ. The web UI shows a toast naming both versions and
`update_install_health` returns the typed mismatch, so a locked or partial
update can never report "updated".

#### Migrating a per-machine install

An install made with `perMachine` (or a hand-made copy under `Program Files`)
cannot be updated by the per-user installer: NSIS writes a second copy to
`%LOCALAPPDATA%\Ludex` while the old executable keeps running from
`C:\Program Files\Ludex`.

1. Quit Ludex and check the process path:
   `Get-Process ludex -ErrorAction SilentlyContinue | Select-Object Id, Path`
2. Uninstall the per-machine copy (Settings → Apps → Ludex).
3. Delete leftovers `C:\Program Files\Ludex` and `C:\Program Files (x86)\Ludex`
   (only when they still contain `ludex.exe`).
4. Install the current NSIS release; it lands in `%LOCALAPPDATA%\Ludex`.
5. Point the shortcut at `%LOCALAPPDATA%\Ludex\Ludex.exe` and remove other
   Start Menu / desktop shortcuts for Ludex.

#### Recovering from a stale install

Symptoms: Settings shows the new version but a fix is missing, or the startup
toast says "The update did not replace this Ludex install".

1. Read `%APPDATA%\cloud.cleeryy.ludex\logs\ludex.log`. The
   `ludex version=… commit=… exe="…"` line is the authority; every
   `other install:` line names a duplicate and its own commit.
2. Confirm what is running:
   `Get-CimInstance Win32_Process -Filter "Name='ludex.exe'" | Select-Object ProcessId, ExecutablePath, CommandLine`
3. Normalise the layout with the migration procedure above.
4. Reinstall the release, launch it once, and check that the log's `commit=`
   matches the tag (`git rev-parse vX.Y.Z`).
5. `dismiss_update_mismatch` (update center) clears the stored warning after a
   manual repair; a later successful update clears it automatically.

#### What still needs a real Windows machine

The Rust helpers (install-root scanning, marker reading, version comparison,
mismatch decision) are unit-tested on every platform, and
`scripts/verify-windows-install.ps1` fails the release when the NSIS installer
does not embed the stamped binary byte for byte. Not covered by Linux CI, and
to check by hand on Windows:

- [ ] Updating from the previous stable NSIS build leaves exactly one
      `%LOCALAPPDATA%\Ludex\Ludex.exe`, and its `commit=` in the log matches the
      release tag.
- [ ] The Start Menu shortcut, tray autostart and Sunshine console command all
      resolve to that same path.
- [ ] A per-machine install migrated as above no longer appears in the
      duplicate scan.
- [ ] With the install folder read-only (or the executable locked), the update
      fails loudly and the next start reports the mismatch instead of
      "updated".


## macOS signing, notarization and Apple Silicon (#27)

The release matrix builds a **universal** macOS DMG
(`--target universal-apple-darwin`) on `macos-latest`. Signing and notarization
are driven by the standard Tauri environment variables, so provisioning the
secrets enables them without a workflow change:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | base64 of the Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | password of that `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID` | Apple ID e-mail used for notarization |
| `APPLE_PASSWORD` | app-specific password for that Apple ID |
| `APPLE_TEAM_ID` | 10-character team identifier |

```bash
# Export the certificate from Keychain as .p12, then:
base64 -i DeveloperID.p12 | tr -d '\n' | gh secret set APPLE_CERTIFICATE
gh secret set APPLE_CERTIFICATE_PASSWORD
gh secret set APPLE_SIGNING_IDENTITY
gh secret set APPLE_ID
gh secret set APPLE_PASSWORD
gh secret set APPLE_TEAM_ID
```

Without the secrets the DMG is still built and published **unsigned**: Tauri
logs that no signing identity was provided, Gatekeeper warns on first launch,
and the release notes must say so. With the secrets, Tauri signs with the
hardened runtime, submits to `notarytool` and staples the ticket.

Verify a release artifact on a Mac:

```bash
spctl --assess --type execute --verbose=4 "Ludex.app"
codesign --verify --deep --strict --verbose=2 "Ludex.app"
xcrun stapler validate "Ludex.dmg"
```

Apple Silicon notes:

- The universal binary is tested by the `macos-check` CI job on `macos-latest`
  (arm64) with `cargo check --locked`; the release job builds both slices.
- Minimum system version is macOS 11.0 (Apple Silicon baseline).
- WebKit behaviour, native file dialogs and app bundles are exercised manually
  on macOS; Wine/CrossOver is **not** required for native `.app`/`.sh` games and
  is unsupported for `.exe` games on macOS (Linux handles those, #89/#92).
- The updater artifacts follow the same signing keys; a rotated certificate
  requires shipping the new public key in the previous release.

## Channels

| Channel | Trigger | Artifact | Use |
|---|---|---|---|
| `edge` | push to `main` after `rust`, `web` and `E2E` pass | prerelease tag `edge`, rolling assets | CT258 homelab, testers |
| stable | annotated tag `vX.Y.Z` | GitHub release `vX.Y.Z` | production |

`edge` is force-moved to the latest `main` commit. It is a prerelease and may
contain unreleased or breaking code. The CT258 updater follows it by default; pin
a stable tag to stop following edge (see [`GITOPS.md`](GITOPS.md)).

## Version bump (stable)

Before tagging, run the bump script:

```bash
scripts/bump-version.sh 0.3.0
```

It updates every place that carries the version (and only the package version,
never a dependency version):

- `Cargo.toml` `[workspace.package].version`
- `crates/*/Cargo.toml` use `version.workspace = true`; no edit needed unless a
  crate pins its own version
- `apps/desktop/src-tauri/Cargo.toml`
- `apps/desktop/src-tauri/tauri.conf.json` `version`
- `package.json`, `apps/web/package.json`, `apps/desktop/package.json`,
  `e2e/package.json`

Then update `CHANGELOG.md` (move `Unreleased` notes under the new version and
date) and refresh `Cargo.lock`, `apps/desktop/src-tauri/Cargo.lock` and
`pnpm-lock.yaml`. The script prints the exact commands.

### Milestone releases

The roadmap is cut along the issue milestones, so a release only ships the
issues closed for that milestone:

| Release | Milestone | Scope |
|---|---|---|
| `v0.3.0` | `v0.3` | GameVault import, RAR/7z streaming, share links, OpenAPI, artwork, parental controls, PostgreSQL option, user editing, OIDC admin UI, download ETA/graph, save discovery and auto-sync, download history |
| `v0.4.0` | `v0.4` | Auth hardening, multipart upload, i18n FR/EN, profiles, controller profiles, emulators, Linux runtimes (UMU/Proton, Steam discovery, ProtonDB, Wine profiles, installers), storage locations, config editor and adapters, custom title bar |
| `v0.5.0` | `v0.5` | macOS signing, metadata plugins, mobile/PWA, Sunshine/Moonlight, community locales/RTL |

Do not tag a milestone release while an issue carrying that milestone is still
open; ship it as a patch of the previous line and move the remaining issue, or
close it with evidence first.

Confirm the license metadata matches the release: v0.2.0 and later is
AGPL-3.0-or-later. Do not tag until the `LICENSE` file, README badge, Cargo
`license` fields, `package.json` and Tauri metadata agree.

## Post-release verification (updater)

```bash
TAG=v0.2.4
gh release view "$TAG" --json assets --jq '.assets[].name' \
  | grep -E 'latest.json|setup.exe.sig|AppImage.sig'
curl -fsSL "https://github.com/cleeryy/ludex/releases/latest/download/latest.json" | python3 -m json.tool
```

- `latest.json` has `version`, `notes`, `pub_date`, and
  `platforms.windows-x86_64` / `platforms.linux-x86_64`, each with a `signature`
  and a release-download `url`.
- Install the previous stable build, check for updates, download and install.
- On Windows, confirm `%APPDATA%\cloud.cleeryy.ludex\logs\ludex.log` names the
  new version and the release commit (see *Windows install layout*) and that
  `Get-Process ludex | Select-Object Path` points at
  `%LOCALAPPDATA%\Ludex\Ludex.exe`.
- Truncate a copy of an artifact and confirm the updater rejects it.
- Confirm `latest` resolves to the stable release (prereleases are skipped).

## Pre-release checklist

- [ ] `main` is green, including the required `E2E` check.
- [ ] `CHANGELOG.md` is complete and does not claim roadmap items (OIDC, cloud
      saves, share links) as shipped.
- [ ] Versions are bumped consistently and lockfiles are committed.
- [ ] License files/metadata are consistent (AGPL-3.0-or-later from v0.2.0).
- [ ] `docs/CONTRACT.md` matches the API if routes or payloads changed.
- [ ] `docs/DEPLOY.md` / `docs/GITOPS.md` are current.
- [ ] `scripts/e2e.sh` passes locally or on a PR.
- [ ] No secrets, dumps or game files are in the diff.

## Stable tag process

```bash
git switch main
git pull --ff-only
git tag -a v0.2.0 -m "Ludex v0.2.0"
git push origin v0.2.0
```

The `Release` workflow then runs:

1. **`verify`** — installs Rust, pnpm and Playwright Chromium, runs
   `scripts/e2e.sh`. Nothing else starts if this fails.
2. **`create-release`** — `needs: verify`; creates the GitHub release with
   generated notes if it does not exist.
3. **`server`**, **`desktop`**, **`docker`** — all `needs: create-release`:
   - Linux x86_64 server archive:
     `ludex-server-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` + `.sha256`
   - Windows x64 NSIS installer and Linux AppImage/deb
   - `ghcr.io/cleeryy/ludex` with tags `latest`, `vX.Y.Z` and `X.Y`
4. Assets are uploaded to the release. The Docker image is pushed to GHCR.

No release asset is published if the E2E gate fails. The desktop job remains
unsigned; Windows SmartScreen and macOS Gatekeeper warnings are expected.

## Edge process

Every push to `main` that passes `rust`, `web` and `E2E` updates two jobs:

- `edge-release` builds `ludex-server --features embed-web`, packages
  `ludex-server-edge-x86_64-unknown-linux-gnu.tar.gz` + `.sha256`, force-moves
  the `edge` tag and creates/updates the **Ludex edge** release.
- `edge-desktop` (Linux + Windows matrix) stamps a monotonic
  `<next>-edge.<run_number>` version (the committed files are untouched), builds
  the bundles, signs them, uploads `Ludex_<version>_amd64.AppImage`,
  `_amd64.deb`, `_x64-setup.exe`, their `.sha256`, `.sig` and a merged
  `latest.json` with `--clobber`, and posts the same files as a 14-day
  workflow artifact.

E2E is the gate for both; the desktop matrix does not run on pull requests. The
edge release is public so the CT258 timer and early-channel clients can consume
it without a token. Stable releases and `/releases/latest` are unaffected.

## Post-release verification

```bash
gh release view v0.2.0
gh release download v0.2.0 --pattern '*linux-x86_64*'
sha256sum -c ludex-server-v0.2.0-x86_64-unknown-linux-gnu.tar.gz.sha256
docker pull ghcr.io/cleeryy/ludex:v0.2.0
```

Install the server archive on a scratch host, create the admin, scan a small
archive and download it with a Range request. For the desktop, install the NSIS
or AppImage build and open a game detail page.

## Rollback

Published tags are immutable: never move or delete a released `vX.Y.Z` tag.
Ship a patch release (`vX.Y.Z+1`) with the fix. If an asset is broken before
users depend on it, edit the release notes to mark it as withdrawn and publish a
replacement tag.

For CT258, the GitOps updater rolls back automatically on a failed health check;
manual rollback steps are in [`GITOPS.md`](GITOPS.md). Pin the updater to the
previous stable tag if edge is the problem.

## Branch protection

`main` requires a pull request, the checks listed in `CONTRIBUTING.md`, resolved
conversations, linear history and no force pushes. Administrators are not
excluded (`enforce_admins: false`) so an emergency push remains possible.
