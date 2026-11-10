# Changelog

All notable changes to Ludex are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Multiple library roots (#45): ordered `library_roots` table with label,
  path, enabled flag and availability, `LUDEX_LIBRARY_ROOTS` configuration,
  admin CRUD at `/api/library/roots` and per-root scans at
  `/api/library/roots/{id}/scan`. Versions record their root, identical
  archives mirrored across roots are indexed once (SHA-256), and a missing or
  unmounted root marks its versions `root_unavailable` instead of deleting
  games. Existing installs are backfilled to root #1 by migration
  `0006_library_ops.sql`.
- Optional folder watcher (#44): `notify`-based recursive watch of enabled
  roots with a debounced scan trigger (`LUDEX_WATCHER_ENABLED`,
  `LUDEX_WATCHER_DEBOUNCE_MS`), ignored partial/hidden files, runtime root
  changes and `GET /api/library/watcher` status.
- Integrity checks (#43): SHA-256 baselines for archives
  (`game_versions.content_hash`) and cached media (`media_manifest`), a
  scheduled read-only check (`LUDEX_INTEGRITY_INTERVAL_HOURS`) and
  `/api/integrity/check` + `/api/integrity/reports` endpoints with
  `missing`/`corrupt`/`size_mismatch` findings. Nothing is deleted.
- Media cache garbage collection (#18): `POST /api/media/gc` (dry-run by
  default) removes only unreferenced artwork older than
  `LUDEX_MEDIA_GC_GRACE_HOURS`, with an optional schedule
  (`LUDEX_MEDIA_GC_INTERVAL_HOURS`) and a `media_gc_runs` history.
- `docs/LIBRARY.md` documenting roots, watcher, integrity and GC operations,
  including platform limits and safety rules.

- Advanced library filters: genre, release year range, player count, playtime
  range, installed state, metadata provider, minimum rating and sort by
  playtime. Filters combine, live in the URL, persist in localStorage, and are
  shown as removable chips with a clear-all action.
- Fuzzy search over titles, sort titles and provider aliases, tolerant to typos
  and accents, with ranked results and matched-field metadata. `GET
  /api/games/suggest?q=` returns the top eight type-ahead suggestions.
- Library search field with debounced suggestions, keyboard navigation,
  matched-substring highlighting and an empty state.
- Global keyboard shortcuts (`/`, `g h`, `g l`, `g d`, `g s`, `d`, `f`, `?`,
  `Esc`) and a shortcuts help sheet. The command palette gains fuzzy filtering,
  filter commands and shortcut hints.
- Gamepad navigation in the library and Big Picture: d-pad/stick focus movement
  with repeat, A select, B back, X filters, Y downloads, LB/RB tabs or shelves,
  a visible focus ring and a graceful keyboard-only fallback.
- `maxPlayers` and `altTitles` on the API, populated from IGDB where available
  and editable by editors; indexes for the new filter columns.

- API keys for integrations (#40): create/list/revoke `ldx_` bearer tokens with
  `read`/`write`/`admin` scopes, SHA-256 at rest, one-time secret display and
  `lastUsedAt` tracking. Keys work anywhere a JWT does.
- Audit log (#42): append-only `audit_logs` table covering logins, user and API
  key changes, game ingest/delete, downloads, saves, settings, webhook changes
  and OIDC login/link; `GET /api/audit` for administrators with `limit`/`before`
  pagination and actor/action/target filters.
- Outbound webhooks (#41): admin CRUD plus test delivery, HMAC-SHA256
  `X-Ludex-Signature`, async delivery with 5 attempts and exponential backoff,
  10 s timeout and SSRF protection (`LUDEX_WEBHOOKS_ALLOW_PRIVATE` for tests).
  Events: `game.ingested`, `game.deleted`, `download.completed`,
  `user.created`, `save.uploaded`.
- `docs/INTEGRATIONS.md` with curl, signature-verification and payload examples.
- Desktop: move an installed game to another folder or drive (#36). A move on
  the same filesystem is an atomic rename; a cross-drive move copies, verifies
  the byte count, then deletes the old folder. The index, executable path and
  file manifest follow the game.
- Desktop: verify and repair installed files (#37). Installs now record a
  manifest (relative path, size, SHA-256); Verify reports missing, corrupt and
  extra files, and Repair restores only the damaged files from the version
  archive, with a clear reinstall prompt when no archive is available.
- Desktop: pause and resume installs (#20). Pausing keeps the partial archive
  and resuming continues from the byte offset with an HTTP Range request,
  including after an app restart; paused jobs stay in the queue.
- Desktop: global bandwidth limit in KiB/s (0 = unlimited) for archive
  downloads, repairs and stream installs.

### Changed

- `GET /api/games?q=` now ranks by relevance instead of substring order and
  searches aliases as well as titles. `sort=playtime` is supported.
- Desktop: the local database is schema v3, adding the `install_manifests`
  table. The migration is additive and runs automatically on start.

### Fixed

- Dialog forms no longer lose focus on every keystroke: the open/close effect no
  longer re-runs when callers pass inline `onOpenChange` callbacks (news editor,
  create-user form and every other dialog). Added a Playwright regression test.

## [0.2.0] - 2026-11-10

### Added

- Global downloads manager: queue and history, background installs that survive
  closing the install window, per-job progress/speed/ETA, cancel and resume.
- Custom install and download folders, plus stream-install settings in the
  desktop app.
- Stream install for ZIP and 7z archives: extract while downloading without
  keeping the full archive on disk. RAR still uses the normal download path.
- Big Picture mode with keyboard and gamepad navigation, a full-screen shelf and
  Escape-to-exit.
- SSO OIDC login (Authorization Code + PKCE, discovery/JWKS, account linking and
  optional admin-group mapping) for Authentik/Keycloak/Authelia, with a local
  login fallback.
- Per-user cloud saves: upload/list/download/delete opaque `.zip` snapshots,
  isolated per account, 2 GiB default limit. Ludusavi automation is roadmap.
- Higher-resolution IGDB artwork: 2x covers and 1080p backgrounds/screenshots
  are requested and cached locally.
- Playwright browser E2E suite (library, search/filter, detail, download, setup,
  ingest, navigation, Big Picture). The `E2E` job is a required check and the
  release workflow is gated on it.
- Rolling `edge` prerelease from `main`, with an auth-free server archive and
  SHA-256 for pull-based GitOps updates.

### Changed

- License: Ludex v0.2.0 and later is **AGPL-3.0-or-later**. v0.1.0 was released
  under MIT OR Apache-2.0; that historical grant is not revoked.
- Desktop installs are managed by the shared downloads provider, so progress and
  history are visible outside the install dialog.

### Security

- OIDC state/nonce/PKCE, one-time exchange codes and JWKS signature verification
  are covered by the server integration tests. API keys and share links remain
  on the roadmap.

## [0.1.0] - 2026-11-09

### Added

- Rust server with SQLite, JWT auth, roles (`admin`, `editor`, `user`), library
  scanning, IGDB/Steam metadata, search/filter/sort, bookmarks, progress states,
  playtime, versions and news.
- Range download streaming, download tickets and the URL-based ingest queue with
  resume and archive verification.
- React web UI and Tauri 2 desktop app for Windows/Linux, including local mode
  with an embedded server.
- Docker image, Compose example, GameVault archive-volume migration path and
  deployment documentation.
