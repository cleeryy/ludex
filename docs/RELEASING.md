# Releasing Ludex

Releases are tag-driven, E2E-gated and built entirely on GitHub-hosted runners.
The only secret used is the automatic `GITHUB_TOKEN`; there is no signing key,
deploy key or self-hosted runner in the release path.

## Channels

| Channel | Trigger | Artifact | Use |
|---|---|---|---|
| `edge` | push to `main` after `rust`, `web` and `E2E` pass | prerelease tag `edge`, rolling assets | CT258 homelab, testers |
| stable | annotated tag `vX.Y.Z` | GitHub release `vX.Y.Z` | production |

`edge` is force-moved to the latest `main` commit. It is a prerelease and may
contain unreleased or breaking code. The CT258 updater follows it by default; pin
a stable tag to stop following edge (see [`GITOPS.md`](GITOPS.md)).

## Version bump (stable)

Before tagging, update every place that carries the version:

- `Cargo.toml` `[workspace.package].version`
- `crates/*/Cargo.toml` use `version.workspace = true`; no edit needed unless a
  crate pins its own version
- `apps/desktop/src-tauri/Cargo.toml`
- `apps/desktop/src-tauri/tauri.conf.json` `version`
- `package.json`, `apps/web/package.json`, `apps/desktop/package.json`,
  `e2e/package.json`
- `CHANGELOG.md` (move `Unreleased` notes under the new version and date)
- Refresh `Cargo.lock`, `apps/desktop/src-tauri/Cargo.lock` and `pnpm-lock.yaml`

Confirm the license metadata matches the release: v0.2.0 and later is
AGPL-3.0-or-later. Do not tag until the `LICENSE` file, README badge, Cargo
`license` fields, `package.json` and Tauri metadata agree.

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

Every push to `main` that touches server-affecting paths triggers
`edge-release` after `rust`, `web` and `E2E`:

- Builds `ludex-server --features embed-web`.
- Packages `ludex-server-edge-x86_64-unknown-linux-gnu.tar.gz` + `.sha256`.
- Force-moves the `edge` tag to the current commit and creates/updates the
  **Ludex edge** prerelease with `--clobber`.

Docs-only pushes skip the rebuild via a path gate but still pass CI. The edge
release is public so the CT258 timer can download it without a token.

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
