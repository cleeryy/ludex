# Changelog

All notable changes to Ludex are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Web i18n, themes and PWA (#23, #29, #110): a gettext-style message catalog
  (`src/lib/i18n.tsx`) with English as the source language, full English and
  French official locales, community seeds for German, Spanish and Arabic
  (RTL, unreviewed), lazy locale chunks discovered from `locales/*.json`, a
  JSON locale manifest that needs no TypeScript change for a new language,
  `Intl` dates/numbers/plurals, and a CI gate (`scripts/i18n.mjs`) that fails on
  missing official keys, unknown ids, empty values, placeholder mismatches and
  stale coverage. Settings gains a grouped language picker with per-locale
  coverage; the theme bootstrap now runs before first paint and a WCAG AA
  contrast test covers dark, light and the `force-dark` game stage. The web app
  is installable (manifest, maskable icons), ships a content-versioned service
  worker that precaches the shell and public media only, an offline fallback
  page, dismissible install and update prompts, logical-property RTL support,
  mirrored directional icons and mobile safe-area/16 px input polish. Docs in
  `docs/I18N.md`.
- macOS signing, notarization and Apple Silicon (#27): the release matrix builds
  a universal DMG (`aarch64` + `x86_64`) on `macos-latest`; Tauri signs with the
  hardened runtime and notarizes/staples when the `APPLE_*` secrets exist, and
  publishes an explicitly unsigned DMG otherwise. New `macos-check` CI job runs
  `cargo check --locked` on Apple Silicon; minimum macOS is 11.0 and
  `docs/RELEASING.md` documents certificate export, secrets and verification.
- Parental controls (#39): `users.age_limit` (0–21, nullable) is
  enforced in game lists, the random picker, title suggestions, game detail and
  download tickets. Restricted games answer exactly like missing ones (404
  `game_not_found`) so the library cannot be enumerated, and the refusal is
  audited as `game.access_denied` with `reason=age_restricted`. Unrated games
  stay visible; administrators can raise or clear the limit through
  `PATCH /api/users/{id}` (`ageLimit`, `null` clears). Migration
  `0016_parental.sql`. The admin user editor exposes the age limit (empty =
  unrestricted). Integration tests cover list/detail/download/suggest and limit
  validation.
- Custom artwork upload (#38): editors can `POST /api/games/{id}/artwork`
  (multipart `kind=cover|background`, `file`) to replace a cover or background.
  Images are sniffed from their magic bytes (PNG/JPEG/WebP/GIF, SVG rejected),
  capped at 8 MiB and stored in the media cache under a cache-busting name; the
  game row is updated and the change audited (`game.artwork_updated`).
  `DELETE /api/games/{id}/artwork?kind=...` resets to provider art and removes
  the custom file. Routes are in the OpenAPI table and covered by integration
  tests. The game page gains an editor "Artwork" dialog with local preview,
  upload and reset-to-provider confirmation. Migration numbering fixed after the
  achievements merge (OIDC settings is `0014_oidc_settings.sql`, save sync
  `0015_save_sync.sql`).
- Automatic cloud save sync (#108): the desktop app archives the save paths
  detected in #107 after `game://exited` (3 s debounce), hashes the archive and
  uploads it as an `auto` snapshot. An unchanged SHA-256 is a no-op, empty
  detection never creates an empty snapshot, symlinks and huge trees are
  skipped/bounded, and failed uploads are persisted in SQLite and retried with
  bounded exponential backoff. New `cloud_save_status`,
  `set_cloud_save_sync` (global or per-game override with `on`/`global_off`/
  `game_off` reasons), `sync_cloud_save_now`, `restore_cloud_save` and
  `resolve_cloud_save_conflict` Tauri commands. Restore verifies the SHA-256,
  writes a timestamped local backup before overwriting newer saves, refuses
  traversal/symlink entries, and runs once per snapshot; `keep_local` records
  and uploads the local state instead. Snapshot uploads now carry `source` and
  `deviceId`, duplicate automatic hashes return the existing snapshot, and
  automatic snapshots are pruned to the newest 10 per user and game (manual
  snapshots are never pruned). Desktop migration `SCHEMA_V9`; the game page
  gains automatic-backup status, Sync now, Restore and a conflict dialog, and
  Settings gains the global default, device name and a privacy note. The
  browser build explains that automatic sync needs the desktop app.
(docs: document i18n, themes and PWA)
- Achievements framework (#117): per-game and global definitions, per-account
  unlock state with a unique `(user, achievement)` constraint, points, hidden
  definitions, global unlock percentages, and completion summaries. Routes:
  `GET /api/games/{id}/achievements`, `GET /api/me/achievements`,
  `GET /api/users/{id}/achievements` (follows the community profile opt-in),
  admin `POST/PATCH/DELETE /api/games/{id}/achievements[/{key}]`, idempotent
  `POST /api/games/{id}/achievements/{key}/unlock`, and
  `POST /api/games/{id}/achievements/scan`. New unlocks emit
  `achievement.unlocked` (audit + webhooks). The game page gains an
  achievements section (progress, points, locked/unlocked states, hidden
  masking, per-achievement unlock percentage, icon fallback) and public
  profiles show an achievement showcase. Desktop and web demo data included.
  Migration `0013_achievements.sql`; documented in the generated OpenAPI
  document (routes in `openapi::ROUTES`, `ToSchema` DTOs); contract and
  `docs/ACHIEVEMENTS.md`.
- Local achievement providers (#118): `game_files` (Steam emulation
  `UserStats/Stats.ini`, `steam_settings/achievements.ini` and Goldberg
  `steam_settings/achievements.json`), `gog_galaxy_api` (loopback JSON with
  configurable `{appId}` URL patterns) and `gog_galaxy_sqlite` (read-only
  `galaxy-2.0.db` introspection). Scans are allowlisted
  (`LUDEX_ACHIEVEMENT_SCAN_ROOTS`), idempotent, tolerant of malformed files,
  record per-provider outcomes and raw payloads, and can rescan on a schedule
  (`LUDEX_ACHIEVEMENT_SCAN_INTERVAL_HOURS`, 0 = off) from the remembered source
  per game and account.

## [0.2.2] - 2026-11-10

### Added

- Save folder detection (#107): the desktop app resolves a game's save files
  and folders from a user-installed Ludusavi CLI (preview API), a user-provided
  Ludusavi manifest, or manual per-game paths. Steam App ID and normalized
  title matching, store/OS `when` filters, Windows/Linux/Wine path templates
  (`<base>`, `<home>`, `<winAppData>`, `<winLocalAppData>`, `<winDocuments>`,
  prefix `drive_c/users/<user>`, multi-profile aggregation) and a per-game
  primary path. Paths stay on the machine; the manifest is never bundled.
  Detection results are persisted with sizes, timestamps, source and notes,
  manual overrides survive re-detection, and traversal/symlink escapes are
  rejected. Covered by 17 unit tests with fixture manifest and prefix trees.
- Stream installs for RAR4/RAR5 (#8): `archive_tar_size` and
  `stream_archive_as_tar` now plan and stream RAR archives with the same
  bounded-memory, path-sanitised tar writer as ZIP/7z; encrypted archives are
  rejected with a clear error and the HTTP route accepts RAR (only genuinely
  unsupported files keep the 415). Real RAR4/RAR5/encrypted/symlink fixtures and
  end-to-end tar tests cover the new path.
- GameVault import (#6): `ludex-server import-gamevault --dump <file>
  [--dry-run] [--json]` reads the versioned JSON exported by
  `scripts/gamevault-export.sql` (never the live PostgreSQL database), matches
  games by relative path then title+size (ambiguity is reported, never
  guessed), imports progress/playtime with the documented state mapping,
  bookmarks, news and metadata edits, and creates local accounts with one-time
  temporary passwords (GameVault hashes are never reused). The dry run writes
  nothing, the import is idempotent, and `docs/MIGRATION.md` documents the
  export, dry-run, apply and verification steps.
- Share link web UI (#9): a public, account-free `#/share/<token>` page renders
  only the shared game's metadata, offers the password prompt when needed and
  downloads through the short-lived ticket. Editors get a Share dialog on the
  game page to create (version, expiry, password, download cap, label), copy the
  public URL once and revoke existing links.
- User account editing (#101): the public `User` now exposes `source`
  (`local`/`oidc`) and `oidcIssuer`; `PATCH /api/users/{id}` accepts `username`
  and write-only `password` for local accounts (unique usernames, Argon2,
  `user.password_reset` audit event) and rejects identity edits on SSO accounts
  with `409 managed_by_idp` while role/activation stay editable. The admin Users
  screen shows Local/SSO badges and an edit dialog with read-only SSO identity
  fields.
- Smooth download ETA and transfer graph (#102): per-job 1 Hz sampling with a
  rolling-median smoothed speed, a material-change gate on the displayed ETA,
  honest "Estimating…"/phase labels, an aggregate ETA only when every running
  download has a known total, and a two-minute SVG speed graph with
  current/peak/average labels and an accessible summary. Helpers are unit-tested
  with Vitest and the web test step runs in CI.

## [0.2.1] - 2026-11-10

### Added

- Share links (#9): editor-created, expiring, revocable and optionally
  password-protected public links to a game or one version. Public routes
  `/api/share/{token}` (metadata only) and `/api/share/{token}/access`
  (short-lived download ticket) need no account; management lives at
  `POST /api/games/{id}/shares`, `GET /api/shares` and
  `DELETE /api/shares/{id}`. Tokens are stored as SHA-256 only and returned
  once, passwords as Argon2id. Downloads reuse the scoped download-ticket path
  (Range and object storage unchanged); access is rate-limited per token,
  `maxDownloads` is reserved atomically, and the public payload never carries
  progress, bookmarks, playtime, paths or other games. Migration
  `0011_share_links.sql`, new `docs/CONTRACT.md` "Share links" section.
- Downloads history (#100): `delete_install_job` removes one finished entry and
  `clear_finished_install_jobs` removes every finished entry from the desktop
  SQLite index; active and paused jobs are refused. The Downloads screen gains a
  per-row "Remove from history" action (with a confirmation for failed or
  interrupted jobs) and "Clear finished" now persists instead of hiding rows
  client-side. Install folders, archives and `.part` downloads are kept.
- Desktop Linux compatibility detection (#90): `compat_scan` reports Steam
  roots, Proton builds in `steamapps/common` and `compatibilitytools.d`,
  `umu-run`, `wine`/`wine64`, `winetricks` and `protonfixes`, with per-path
  overrides (`compatUmuPath`, `compatProtonPath`, `compatWinePath`) and a
  Settings → Compatibility panel. Detection is filesystem-only and unit tested
  with fixture trees.
- Desktop Proton/UMU launches (#89): per-game `launchOptions` (`auto`, `native`,
  `proton`, `umu`), tool selection (UMU → GE-Proton/Proton → Wine) with
  fallbacks, and the environment each mode needs (`PROTONPATH`, `GAMEID`,
  `UMU_LOG`, `STEAM_COMPAT_DATA_PATH`, `STEAM_COMPAT_CLIENT_INSTALL_PATH`,
  `WINEPREFIX`). `auto` is the Linux default for `.exe`; no tool means an
  explicit error. Playtime tracking is unchanged.
- ProtonDB ratings (#91): `protondb_rating` fetches the public summary, caches
  it in SQLite for seven days, degrades to stale/offline instead of failing, and
  supports a manual Steam app id. The game page shows the tier badge with a
  ProtonDB link.
- Per-game Wine prefixes (#92): `{installPath}/.prefix` by default or a custom
  path, `wine_prefix_info`, `reset_wine_prefix` with a confirmation and safety
  checks, validated environment overrides, and on-demand
  `run_winetricks` (`-q`, Proton Wine binary, 30-minute timeout, bounded log).
- Repack installers under Proton/Wine (#93): `run_installer` now runs on Linux
  through the game's UMU/Proton/Wine tool in the game prefix, captures the log
  (`logTail`) and detects the installed binary under `drive_c`. Windows
  behaviour is unchanged; macOS asks for a manual run.
- Settings: `defaultLaunchMode` and the three compatibility path overrides;
  schema v6 adds `game_launch_options` and `protondb_reports`.
- OpenAPI 3.1 discovery (#17): public `GET /api/openapi.json` generated from the
  server route table and `utoipa` DTO schemas, optional `/api/docs` explorer
  (`LUDEX_OPENAPI_DOCS`), and a contract test that fails when a registered
  route is missing from the document, a `$ref` is unresolved, or a documented
  path is not mounted. See `docs/OPENAPI.md`.
- Auth hardening (#19): per-IP + username login throttle with exponential
  backoff and lockout, global per-IP API rate limit (`LUDEX_RATE_LIMIT_*`),
  `jti`-backed sessions with server-side revocation (`token_revocations`,
  `auth_sessions`, `users.tokens_valid_after`), session listing/revocation,
  logout revocation, password change that signs out every other session,
  admin revoke-all, constant-time unknown-user verification and `Retry-After`
  on 429s. See `docs/SECURITY.md`.
- Resumable archive uploads (#21): `POST /api/uploads`,
  `PUT /api/uploads/{id}/parts/{n}`, `GET /api/uploads/{id}`,
  `POST /api/uploads/{id}/complete`, `DELETE /api/uploads/{id}`; idempotent
  chunks, SHA-256/size verification, stale cleanup and indexing through the
  normal scanner. New "Upload game" admin screen with drag-and-drop and
  resume-after-reload.
- Docs: `docs/OPENAPI.md`, `docs/SECURITY.md`, contract and changelog updates.
- Desktop silent installers (#7): `installer_plan`, `run_installer`,
  `cancel_installer`, `set_executable` and `choose_executable`. Silent switch
  presets (`/S`, `/silent`, `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`,
  `--silent`) are tried in order with per-attempt timeouts, exit-code handling
  (`0` and `3010` count as installed), cancellation and an interactive fallback;
  the game executable is recorded afterwards. Configurable with
  `installerSilentArgs` and `installerTimeoutSecs`. Windows execution plus a
  Linux compatibility launch; cross-platform tests.
- Desktop Discord Rich Presence (#12): optional `discordPresence` setting and
  `discordClientId` application id, game title/cover/elapsed time, idle state,
  reconnect with backoff, `presence_status` and clean no-op without Discord.
- Desktop Steam library import (#13): VDF/ACF parsing of
  `libraryfolders.vdf` and `appmanifest_*.acf`, platform path discovery,
  conservative title matching, `steam_scan`/`steam_import`, entries launched via
  `steam://rungameid/<appid>`, `source`/`launchUri` columns (schema v5) and a
  Settings → Integrations preview with checkboxes.
- Playnite library plugin (#14) under `integrations/playnite/`: read-only
  `GET /api/games` sync, one Playnite entry per game, Play action to a Ludex URL,
  file-based configuration and manual `dotnet` build instructions.
- `docs/INTEGRATIONS.md` gains a "Desktop integrations" section; the desktop
  README documents the new settings, commands and events.
- S3-compatible object storage for game archives (#47):
  `LUDEX_STORAGE_BACKEND=local|s3` with `LUDEX_S3_ENDPOINT`, `LUDEX_S3_REGION`,
  `LUDEX_S3_BUCKET`, `LUDEX_S3_ACCESS_KEY`, `LUDEX_S3_SECRET_KEY`,
  `LUDEX_S3_PATH_STYLE` and `LUDEX_S3_PREFIX`. Archives are uploaded by the
  scanner (new, replaced or still-local rows); `game_versions` stores
  `storage_backend`, `object_key` and `etag` (migration `0010_storage_ops.sql`).
  Downloads and stream installs proxy S3 with HTTP Range support, never
  redirecting to signed URLs; the local file stays as the fallback. Media and
  saves stay local. New `docs/STORAGE.md`.
- Backup and restore CLI (#49): `ludex-backup backup|restore|verify` creates a
  `.tar.gz` with a `VACUUM INTO` SQLite snapshot, `media/`, `saves/`, a
  non-secret config manifest and a JSON manifest holding sizes and SHA-256
  checksums. Restore verifies every checksum, refuses non-empty destinations
  without `--force`, supports `--dry-run` and `--upload-s3`, and is documented
  in `docs/BACKUP.md`.
- Metrics and health details (#48): `GET /metrics` in Prometheus text format
  (HTTP counts/latency histogram, library/root/ingest/scan/webhook/media/disk/DB
  gauges, uptime, build and storage backend), protected by
  `LUDEX_METRICS_TOKEN` when set and rate-limited otherwise; and
  `GET /api/health/details` with database, disk, library, ingest, watcher,
  integrity and version/commit details. No paths or secrets are exposed.
- Docker image now ships `/usr/local/bin/ludex-backup`.
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
- Trailer and media playback (#31): IGDB `videos` are stored with their
  YouTube id and provider, exposed as `videos[]` on `GameDetail`, and played in
  the web UI through a privacy-enhanced `youtube-nocookie.com` embed that only
  loads after a click. Game pages gain a trailer section and a combined
  screenshots/videos gallery with arrow-key lightbox navigation and fullscreen.
- Collections and smart lists (#25): per-user manual shelves and rule-based
  smart collections (`genre`, `year`, `players`, `playtime`, `rating`,
  `installed`, `provider`, `title`, AND/OR) with CRUD, add/remove, public
  visibility, a collection page, a create/edit dialog with rule builder and a
  picker, and an add-to-collection flow from game detail.
- Community (#10): opt-in public profiles (`publicProfile`, `displayName`,
  `bio`), a member directory, public profile pages with library/collections/
  activity, and a paginated activity feed recorded for library additions,
  installs, play sessions, completions and collection creation. Private
  profiles return 404 to other accounts.
- Migration `0008_collections_community.sql` with `collections`,
  `collection_games` and `activities`, plus public-profile columns on `users`.
- `POST /api/games/{id}/installed` for the desktop to report an install without
  flooding the feed (deduped hourly).
- Demo data and mock coverage for trailers, collections (manual and smart),
  public profiles and the activity feed.

- Desktop: tray icon (#15) with Show/Hide, Downloads, a Recent games submenu,
  Check for updates and Quit; left click toggles the window and the tooltip
  reports active downloads. `closeToTray` (default true) hides the window
  instead of quitting, and Home lists the games installed on this device with
  one-click launch.
- Desktop: in-app updates (#16) with `stable` and `early` channels, a startup
  check throttled to once per 24 h, manual checks in Settings, download progress
  and a restart prompt. Builds without a signing key report the missing key and
  refuse to install instead of failing silently.
- Desktop: game update detection (#30). The server exposes
  `GET /api/games/{id}/versions/latest` with the version label, archive size,
  sampled archive fingerprint and last update; installed games show an
  "Update" badge and button on cards and on the game page, and `update_game`
  reuses the install pipeline, keeps the previous install until the new files
  are in place, and never touches save files.
- Desktop: process-tree playtime tracking (#26). Sessions stay open while any
  descendant of the launched game runs, `stop_game` kills the whole tree, and
  the desktop DB records exact seconds. The game page shows "Played Xh" and the
  recent sessions on this device.
- Server: `game_versions.archive_hash`, `file_modified` and `updated_at`
  (migration 0009), computed by the scanner; `archiveHash` and `updatedAt` on
  `GameVersion`.
- Core: `ludex_core::fingerprint` — a cheap sampled archive fingerprint
  (`sha256-sample:` over size + first/last MiB) shared by the server scanner and
  the desktop installer.
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

- Desktop: the local database is schema v4, adding `installed_games.archive_hash`
  and `sessions.seconds` (exact durations; `minutes` is still synced to the
  server). `game://exited` gains `secondsPlayed`.
- `GET /api/games?q=` now ranks by relevance instead of substring order and
  searches aliases as well as titles. `sort=playtime` is supported.
- Desktop: the local database is schema v3, adding the `install_manifests`
  table. The migration is additive and runs automatically on start.

### Fixed

- Stream install failures are actionable (#114): install jobs persist a
  `stream` flag, a dropped stream shows "Retry as download" (the resumable
  Range path) as the primary action plus a warned "Retry stream", and
  "Resume" is never offered for a stream job. Pausing is refused for stream
  jobs in the UI, the error no longer depends on parsing English text, and the
  Settings/install-dialog copy explains that streams cannot resume. The failed
  history row and its message are kept for diagnostics.
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
