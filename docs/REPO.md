# Repository settings

The GitHub repository page is part of the product: a newcomer decides in ten seconds whether
the project is alive and trustworthy. Most of that page is *not* stored in git, so this file
records the current settings and the commands that reproduce them. Update it in the same PR
that changes any of them.

## Description and topics

| Field | Value |
|---|---|
| Description | `Self-hosted, DRM-free game library. Rust + axum server, Tauri 2 desktop, React web UI. Your archives, your server, no telemetry.` |
| Website | unset — the README is the landing page (a marketing site is out of scope, see #262) |
| Topics | `self-hosted`, `game-library`, `drm-free`, `rust`, `tauri`, `react`, `axum`, `sqlite`, `postgresql`, `gamevault-alternative`, `docker`, `home-server` |

```bash
gh repo edit cleeryy/ludex \
  --description "Self-hosted, DRM-free game library. Rust + axum server, Tauri 2 desktop, React web UI. Your archives, your server, no telemetry." \
  --add-topic self-hosted,game-library,drm-free,rust,tauri,react,axum,sqlite,postgresql,gamevault-alternative,docker,home-server
```

## Social preview

`docs/assets/social-preview.png` (1280×640), generated from the committed vector:

```bash
scripts/screenshots.sh --only social
```

Set it in **Settings → General → Social preview → Edit → Upload**. GitHub has no API for this
field, so it is the one setting that stays manual.

## Discussions, issues and templates

- **Discussions**: enabled for *Announcements*, *General* and *Show and tell*; *Q&A* stays off
  while the issue tracker is small enough to search.
- **Pinned issues**: the roadmap issue and the security policy pointer (`.github/ISSUE_TEMPLATE`
  and `SECURITY.md` are linked from both).
- **Issue templates**: bug report and feature request, both asking for the server version from
  `GET /api/health`, the client (browser / desktop / console) and the archive format.
- **Releases**: `latest` is the newest stable tag; the rolling `edge` prerelease is built from
  `main` and is what the homelab GitOps timer consumes ([`GITOPS.md`](GITOPS.md)).

## Branch protection

`main` requires the CI workflow's checks — including the `E2E` job — before merge, and the
release workflow refuses to publish when they are not green. `cleeryy` may push directly to
`main` (rules bypassed); other contributors go through pull requests.

## Badges

The README badges are all live and sourced from this repository:

| Badge | Source |
|---|---|
| CI | `actions/workflows/ci.yml/badge.svg?branch=main` |
| Release | `img.shields.io/github/v/release/cleeryy/ludex` |
| License | static `AGPL-3.0-or-later`, links `LICENSE` |
| Image | static `ghcr.io/cleeryy/ludex`, links the package page |
| Security | static, links `SECURITY.md` |
| Stars | `img.shields.io/github/stars/cleeryy/ludex` |

No coverage badge until the coverage job exists (#243); no contributor-count badge while the
maintainer is the only author. Both would be decoration, not information.

## Things that are intentionally not here

- A marketing site or a landing page — the README and `docs/` carry the same content.
- Analytics or third-party counters in the README.
- A demo deployment. The screenshots come from the demo adapter
  ([`assets/screenshots/README.md`](assets/screenshots/README.md)), not from a hosted instance.
