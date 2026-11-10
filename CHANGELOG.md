# Changelog

All notable changes to Ludex are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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

### Changed

- `GET /api/games?q=` now ranks by relevance instead of substring order and
  searches aliases as well as titles. `sort=playtime` is supported.

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
