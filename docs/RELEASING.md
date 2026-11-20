# Releasing Ludex

Releases are tag-driven, E2E-gated and built entirely on GitHub-hosted runners.
The only mandatory secret is the automatic `GITHUB_TOKEN`; the desktop updater
bundles and the release `SHA256SUMS` are signed with a minisign key (see
*Updater keys and channels*). There is no deploy key or self-hosted runner in
the release path.

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

| Channel | Endpoint | Published from | GitHub release |
|---|---|---|---|
| `stable` | `/releases/latest/download/latest.json` | annotated tag `vX.Y.Z` | normal release |
| `early` (`edge`) | `/releases/download/edge/latest.json` | green `main` push | `edge`, always a **prerelease** |

Two invariants keep stable clients safe, and CI enforces both:

1. **The `edge` release is always a prerelease.** GitHub resolves
   `/releases/latest` to the newest non-prerelease release, so an edge build is
   never served by the stable endpoint. `edge-desktop` passes
   `prerelease: true`, `edge-release` passes `--prerelease`, and
   `Guard the edge updater manifest (#264)` fails the build when
   `gh release view edge --json isPrerelease` returns `false`.
2. **The stable release publishes its own `latest.json`.** The `publish` job
   reads the draft back and fails when `latest.json.version` is not the tag
   version, when a stable release carries a `-edge.` version, when
   `windows-x86_64`/`linux-x86_64` is missing, or when a platform URL does not
   point at `/releases/download/<tag>/` with an updater signature. The
   `edge-latest` job re-checks the merged edge manifest after both matrix legs
   (`-edge.` version, both platforms, edge URLs) and that `/releases/latest`
   does not resolve to `edge`.

A tag that carries a prerelease suffix (`v0.5.0-rc.1`) is published with
`--prerelease` and is invisible to `/releases/latest`; the same rule applies.
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

- `stable` publishes the generated release body as `notes` (see *Release
  notes*), so the in-app update center shows the same highlights, changes and
  contributor credits as the release page.
- `edge` publishes the `## [Unreleased]` changelog section with the stamped
  `<next patch>-edge.<run_number>` version in `notes`; the center shows the real
  changes instead of falling back to the build number. The build number is still
  in the version, so a specific edge build remains identifiable.
- Skipping a version is a local preference (`skippedUpdateVersion` in desktop
  settings). It never hides the update from the center and never blocks an
  install; only the launch prompt, the badge and the toast are silenced until a
  newer version appears.
- Switching between stable and early persists the channel and re-checks
  immediately. Early installs show the edge warning in the center.

### Installing the update, by install kind (#312)

The bundle type compiled into the binary is not the install kind: a build made
for the rpm bundle keeps calling `rpm` even when the file was hand-placed in
`/opt/ludex`. `install_update` therefore detects the kind at update time — an
AppImage (`$APPIMAGE`), a dpkg/rpm-owned executable, or a portable ELF — and a
portable install is updated by Ludex itself: the release `.deb` is verified with
the same minisign key, `usr/bin/ludex` is extracted, and the executable is
replaced atomically (backup `.old`, rollback on failure, precise error when the
folder is root-owned). See [UPDATES.md](UPDATES.md).

### Desktop database schema across channels

The desktop index (SQLite) is migrated forward automatically, with two
safeguards that the release process relies on:

- **Before the first migration statement**, the app writes a snapshot next to
  the database: `index-backup-<UTC stamp>-v<old version>.db` (the three newest
  are kept, older ones are pruned). The snapshot uses `VACUUM INTO`, so it is a
  consistent file even with WAL active, and a failed migration rollback uses the
  database itself (DDL is transactional since v0.4.x). A migration that fails
  leaves the file at its old `user_version` and retries on the next launch; the
  snapshot is the manual escape hatch when retries keep failing.
- **A newer database is refused, never downgraded.** Going from `edge` back to
  `stable` (or installing an older release) opens a file whose `user_version`
  is higher than the build supports. The app shows
  "This library was opened by a newer Ludex (database version N, this build
  supports M)" and names the backup folder. To recover: update back to the
  newer build, or restore/copy a `<name>-backup-*.db` snapshot over the index
  (or move the index aside to start with an empty library).

When a release adds a desktop migration, state the new `SCHEMA_VERSION` in the
changelog entry and keep the snapshot/prune behaviour covered by
`db::tests::an_old_database_is_backed_up_then_migrated` and
`db::tests::opening_a_newer_database_is_refused_with_a_recovery_path`.

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

| Channel | Trigger | Version | Updater endpoint | GitHub release |
|---|---|---|---|---|
| `stable` | annotated tag `vX.Y.Z` | `X.Y.Z` | `/releases/latest/download/latest.json` | normal release, published by `publish` |
| `edge` | green `main` push (`rust`, `web`, `E2E`) | `<next patch>-edge.<run_number>` | `/releases/download/edge/latest.json` | rolling `edge` prerelease |

Rules:

- A tag with a prerelease suffix (`v0.5.0-rc.1`) is published with
  `--prerelease`; `/releases/latest` skips it, exactly like `edge`.
- `edge` is force-moved to the latest `main` commit. It may contain unreleased
  or breaking code. The CT258 updater follows it by default; pin a stable tag to
  stop following edge (see [`GITOPS.md`](GITOPS.md)).
- One run owns a tag at a time: `concurrency` groups `release-<tag>` (stable),
  `edge-desktop-<platform>` (per-platform uploads) and `edge-release` (tag
  force-push) never interleave.
- `edge` never becomes the updater's latest: the release is always a
  prerelease, the `edge-latest` job rejects a manifest that lost a platform or
  points outside `/releases/download/edge/`, and the stable `publish` job
  rejects a `-edge.` version in a stable `latest.json`.

## Release notes

`CHANGELOG.md` stays the source of truth; the release body is generated from it
by `scripts/release-notes.sh` after every artifact is attached. Do not hand-edit
a release body: the next release overwrites it.

The generated body contains:

- **Highlights** — the lead entry of up to four sections (`Added`, `Changed`,
  `Fixed`, `Performance`, `Security`), cut at the first clause. A
  `### Highlights` section in the changelog overrides the derivation.
- **Upgrade notes** — promoted out of a `### Upgrade`, `### Migration` or
  `### Breaking changes` section when the changelog has one.
- **What's changed** — the exact `## [X.Y.Z]` section for a stable release, or
  `## [Unreleased]` for edge, with repeated `### Added`/`### Changed` headings
  merged in first-seen order.
- **Contributors** — authors and `Co-authored-by:` trailers from the GitHub
  compare API between the previous tag and this one, deduplicated, bots excluded
  (`*[bot]`, dependabot, renovate, github-actions, …). Without an authenticated
  `gh`, the script falls back to `git log` authors and only turns an email into
  an `@handle` when it is a `users.noreply.github.com` address.
- **Assets and checksums** — one SHA-256 line per uploaded artifact, identical to
  the attached `SHA256SUMS`.
- **How to verify** — version, tag, commit link, signature expectations and the
  `gh release download` / `sha256sum -c` recipe.

Run it locally before tagging (it needs no network when `gh` is unavailable):

```bash
scripts/release-notes.sh --tag v0.4.14 --previous v0.4.13 --repo cleeryy/ludex
scripts/release-notes.sh --tag v0.4.14 --assets dist --sums-out dist/SHA256SUMS \
  --sums-signature SHA256SUMS.sig --out release-body.md
scripts/release-notes.sh --tag edge --channel edge --version 0.4.15-edge.247
```

`--assets DIR` checksums every file in the directory (recursive, `*.sig`,
`*.sha256` and `SHA256SUMS*` excluded); `--checksums FILE` reuses an existing
`sha256sum` output. The script exits non-zero when the changelog section is
missing, so a tag without notes fails the draft instead of publishing an empty
page.

## Checksums and signatures

Every stable release carries:

| Asset | Content |
|---|---|
| `SHA256SUMS` | `sha256sum` output for every other artifact, `latest.json` included |
| `SHA256SUMS.sig` | minisign signature of `SHA256SUMS`, made with the updater key |
| `*.sig` (next to each installer) | updater signature consumed through `latest.json` |
| `*.sha256` (edge, server archive) | per-file digest used by the GitOps updater and scripts |

Verify a download:

```bash
TAG=v0.4.14
gh release download "$TAG" --dir "ludex-$TAG"
cd "ludex-$TAG"
sha256sum -c SHA256SUMS

# SHA256SUMS.sig is signed with the same key as the bundles: the public key is
# the base64 `plugins.updater.pubkey` in apps/desktop/src-tauri/tauri.conf.json.
# Both the key and the .sig are base64-wrapped minisign documents.
python3 -c 'import base64,json;print(base64.b64decode(json.load(open("apps/desktop/src-tauri/tauri.conf.json"))["plugins"]["updater"]["pubkey"]).decode(),end="")' > ludex.pub
base64 -d SHA256SUMS.sig > SHA256SUMS.minisig
minisign -Vm SHA256SUMS -p ludex.pub -x SHA256SUMS.minisig
```

`minisign` is packaged (`apt install minisign`, `brew install minisign`). The
`publish` job fails instead of publishing when the signing secrets are missing,
so a release never ships an unsigned `SHA256SUMS`. Signing keys rotate as
described in *Updater keys and channels*: the checksum signature follows the
update key automatically.

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
TAG=v0.4.14
gh release view "$TAG" --json isDraft,isPrerelease,assets \
  --jq '{draft:.isDraft, prerelease:.isPrerelease, assets:[.assets[].name]}'
gh release download "$TAG" --pattern SHA256SUMS --pattern SHA256SUMS.sig
curl -fsSL "https://github.com/cleeryy/ludex/releases/latest/download/latest.json" | python3 -m json.tool
```

- The release is published (`draft: false`), carries `SHA256SUMS` and
  `SHA256SUMS.sig`, and its body contains `## Highlights` and the changelog
  sections instead of GitHub's generated commit list.
- `latest.json` has `version`, `notes`, `pub_date`, and
  `platforms.windows-x86_64` / `platforms.linux-x86_64`, each with a `signature`
  and a release-download `url` that points at `vX.Y.Z`.
- `sha256sum -c SHA256SUMS` passes and `SHA256SUMS.sig` verifies with the
  updater public key (see *Checksums and signatures*).
- Install the previous stable build, check for updates, download and install.
- On Windows, confirm `%APPDATA%\cloud.cleeryy.ludex\logs\ludex.log` names the
  new version and the release commit (see *Windows install layout*) and that
  `Get-Process ludex | Select-Object Path` points at
  `%LOCALAPPDATA%\Ludex\Ludex.exe`.
- Truncate a copy of an artifact and confirm the updater rejects it.
- Confirm `latest` resolves to the stable release (prereleases are skipped) and
  that `/releases/download/edge/latest.json` still reports an `-edge.` version.

## Desktop crash containment (#234)

The desktop release profile sets `panic = "unwind"` on purpose. Download,
extraction and verification workers are polled inside
`crash_guard::guard_worker`, so a panic fails one job instead of taking the app
down. Never change it back to `panic = "abort"` without replacing that
containment first: with `abort` a malformed `.part` or archive can crash the
process, and the startup auto-resume turns that into a crash loop on every
launch. Reviewing a PR that touches `[profile.release]` in
`apps/desktop/src-tauri/Cargo.toml` is part of the release checklist.

Support can start the app with `LUDEX_SAFE_MODE=1` (or `ludex --safe-mode`) to
skip every automatic resume and open Downloads with the suspicious job visible.
The two-strike circuit breaker (`resume_markers` / `resume_failures` tables)
parks such a job after two consecutive unclean launches; a manual Resume clears
the streak.

## Pre-release checklist

- [ ] `main` is green, including the required `E2E` check.
- [ ] `CHANGELOG.md` is complete and does not claim roadmap items (OIDC, cloud
      saves, share links) as shipped.
- [ ] `scripts/release-notes.sh --tag vX.Y.Z --previous vX.Y.Z-1` renders the
      expected body locally (highlights, contributors, no empty section).
- [ ] Versions are bumped consistently and lockfiles are committed.
- [ ] License files/metadata are consistent (AGPL-3.0-or-later from v0.2.0).
- [ ] `docs/CONTRACT.md` matches the API if routes or payloads changed.
- [ ] `docs/DEPLOY.md` / `docs/GITOPS.md` are current.
- [ ] `scripts/e2e.sh` passes locally or on a PR.
- [ ] No secrets, dumps or game files are in the diff.
- [ ] `apps/desktop/src-tauri/Cargo.toml` still sets `panic = "unwind"`
      (desktop crash containment, #234).

## Stable tag process

```bash
git switch main
git pull --ff-only
git tag -a v0.4.15 -m "Ludex v0.4.15"
git push origin v0.4.15
```

The `Release` workflow then runs:

1. **`verify`** — installs Rust, pnpm and Playwright Chromium, runs
   `scripts/e2e.sh`. Nothing else starts if this fails.
2. **`create-release`** — `needs: verify`; creates a **draft** release
   (`gh release create --draft --verify-tag`). Nothing is public yet, and a
   prerelease tag (`vX.Y.Z-rc.N`) is recorded as `prerelease: true`.
3. **`server`**, **`desktop`**, **`docker`** — all `needs: create-release`:
   - Linux x86_64 server archive:
     `ludex-server-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` + `.sha256`
   - Windows x64 NSIS installer and Linux AppImage/deb/rpm (universal macOS DMG
     when the matrix includes it), each with `.sig` and a merged `latest.json`
   - `ghcr.io/cleeryy/ludex` with tags `latest`, `vX.Y.Z` and `X.Y`
   `desktop` uploads to the draft (`releaseDraft: true`) instead of publishing.
4. **`publish`** — `needs: [create-release, server, desktop, docker]`:
   downloads every draft asset, writes `SHA256SUMS`, signs it with the updater
   minisign key into `SHA256SUMS.sig`, runs the `latest.json` guard, generates
   the release body with `scripts/release-notes.sh`, uploads the sums and the
   signature, and only then runs `gh release edit --draft=false`.

No release asset is published if the E2E gate or any platform build fails: the
draft stays invisible. Delete it before re-tagging
(`gh release delete <tag> --yes`). The desktop job remains unsigned; Windows
SmartScreen and macOS Gatekeeper warnings are expected.

## Edge process

Every push to `main` that passes `rust`, `web` and `E2E` updates two jobs:

- `edge-release` builds `ludex-server --features embed-web`, packages
  `ludex-server-edge-x86_64-unknown-linux-gnu.tar.gz` + `.sha256`, generates the
  edge notes from the `[Unreleased]` changelog section with
  `scripts/release-notes.sh --channel edge` (stamped
  `<next>-edge.<run_number>` version), force-moves the `edge` tag and
  creates/updates the **Ludex edge** prerelease.
- `edge-desktop` (Linux + Windows matrix) stamps a monotonic
  `<next>-edge.<run_number>` version (the committed files are untouched), builds
  the bundles, signs them, uploads `Ludex_<version>_amd64.AppImage`,
  `_amd64.deb`, `_x64-setup.exe`, their `.sha256`, `.sig` and a merged
  `latest.json` with `--clobber`, and posts the same files as a 14-day
  workflow artifact. Each leg then re-reads the manifest and fails if it is not
  an `-edge.` version pointing at the `edge` tag.

`edge-latest` runs after the matrix and guards the rolling release: the manifest
must still carry an `-edge.` version, both `windows-x86_64` and `linux-x86_64`,
edge URLs, and `/releases/latest` must not resolve to `edge`.

E2E is the gate for both; the desktop matrix does not run on pull requests. The
edge release is public so the CT258 timer and early-channel clients can consume
it without a token. Stable releases and `/releases/latest` are unaffected.

## Post-release verification

```bash
gh release view v0.4.15
gh release download v0.4.15 --pattern '*linux-x86_64*'
sha256sum -c ludex-server-v0.4.15-x86_64-unknown-linux-gnu.tar.gz.sha256
docker pull ghcr.io/cleeryy/ludex:v0.4.15
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
