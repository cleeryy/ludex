# Ludex architecture

Ludex is a clean-room, open-source (MIT OR Apache-2.0) self-hosted game library for
DRM-free games, written in Rust. It provides:

- `ludex-server`: a single static binary that indexes a folder of archives, enriches
  metadata, serves a JSON API and the web UI, and streams game files.
- `ludex` desktop: a Tauri 2 app (Windows, Linux, macOS) sharing the same React UI,
  with native download/extract/launch/playtime support.
- The same React UI is used by the server (browser, browse + download) and by Tauri
  (install + play). Native features are detected at runtime.

No GameVault code is copied. GameVault is source-available, not open source, and its
server is TypeScript. Ludex is a from-scratch Rust implementation of the same product
idea. The GameVault files volume and database can be imported so users can migrate.

## Workspace

```
crates/ludex-core      domain types, filename parser, archive reader/extractor,
                       executable discovery, slug/sort helpers. No network, no DB.
crates/ludex-server    axum + SQLite server. Also exposes an embeddable `start()` API
                       used by the desktop app for local mode. Optional `embed-web`
                       feature serves the compiled React app.
apps/web               React 19 + Vite + Tailwind UI, API client, native bridge.
apps/desktop           Tauri 2 shell (`src-tauri`) around apps/web/dist.
docker/                server Dockerfile and compose example.
docs/                  architecture, API contract, deployment, migration, roadmap.
```

## Runtime modes

### Server (Docker or binary)
- Serves `/api/*` and the web UI.
- `LUDEX_GAMES_DIR` is the library volume: drop `.zip`, `.rar`, `.7z` or portable
  `.exe` files there. The scanner indexes them periodically and on demand.
- `LUDEX_MEDIA_DIR` caches covers, backgrounds and screenshots.
- `LUDEX_DATA_DIR` holds `ludex.db` (SQLite) and the generated JWT secret.
- Multi-user: roles `admin`, `editor`, `user`; JWT auth; per-user progress,
  bookmarks and playtime.

### Desktop app
- Default first-run wizard offers **Local library** or **Connect to a server**.
- Local library starts `ludex-server` in-process on `127.0.0.1` with a random port,
  a SQLite DB in the app data directory, and a user-selected games folder. It
  auto-provisions a local admin token, so the user never sees a login screen.
- Remote mode points at any Ludex server.
- Native commands download (resumable HTTP Range), extract, place files in the
  install folder, launch the game and report playtime. When the embedded server
  owns the archive locally, the desktop asks for the absolute path and extracts
  directly, avoiding a huge copy.

## Library model

A `game` is a logical title. A `game_version` is one archive or executable on
disk. Versions are grouped by normalized title + release year. Naming follows
the GameVault-compatible grammar so existing libraries migrate unchanged:

```
Title (v1.2.3) (EarlyAccess) (Windows) (2016).zip
```

- Trailing `(...)` groups are reserved flags, removed from the title:
  `(v1.2.3)` version (lowercase `v`), `(2016)` release year, `(EarlyAccess)`,
  `(NoCache)`, platform/type flags.
- Version may also appear unparenthesized at the end (`Title v1.2.3.rar`).
- Square brackets stay **in the title** and force separate entries, e.g.
  `Title [GOTY].zip` and `Title.zip` are two games.
- No year and no version → title from the filename, one version.

The parser is deliberately conservative: it never invents metadata, it only
removes well-known suffixes. Metadata comes from providers, then manual edits.

## Scanning

- Recursively walks the games dir up to depth 3.
- Indexes archive files and standalone executables.
- Skips hidden entries, extracted game trees, `Redist`, `_CommonRedist`, `Support`,
  `Tools`, `unins*`, and files smaller than 1 MiB (configurable).
- Removed files are marked `missing`, not deleted from the DB, so progress and
  metadata survive a move.
- A scan is cheap (stat + extension); a manual `POST /api/library/scan` triggers
  metadata enrichment for new games.

## Metadata

Provider chain, first match wins:

1. **IGDB** if `LUDEX_IGDB_CLIENT_ID` + `LUDEX_IGDB_CLIENT_SECRET` are set.
   Covers, backgrounds, screenshots, trailers, genres, developers, publishers,
   rating, release date, summary, age rating.
2. **Steam Store** (no API key): search + appdetails; covers, screenshots,
   trailers, genres, release date, description.
3. Filename only.

Images are downloaded into the media dir and served by the server, so clients and
browsers never hotlink third-party CDNs. Metadata can be refreshed or overridden
by an editor.

## Downloads and installs

Server:
- Auth required. `GET /api/games/{id}/download?versionId=` streams the file with
  HTTP Range support. Browsers use a short-lived ticket URL; native clients send
  the JWT.
- Ingest manager (adapted from the user's `gv-downloader`): paste a URL, the
  server downloads to a `.part` file, resumes with Range, detects the real format
  from magic bytes, renames to `Title.ext`, verifies it, then triggers a scan.
  Jobs survive restarts.

Desktop:
1. Resolve the version file (remote download or local path).
2. Download to a temp file with resume and progress events.
3. Extract to a staging dir with zip/7z/rar support and path-traversal guards.
4. Detect the main executable (scoring, blocklists for uninstallers/redists).
5. Move to `{installDir}/{slug}/{version}` (same volume when possible).
6. Launch, wait for exit, report minutes; playtime is buffered offline and synced.

## Security

- Argon2id password hashes, HS256 JWT (secret generated on first run if not set).
- Path traversal guards on every file operation; ingest filenames sanitized.
- Ingest URLs restricted to http/https; private IPs blocked unless explicitly
  allowed (`LUDEX_INGEST_ALLOW_PRIVATE=true`).
- Media endpoints are public (non-sensitive cached artwork); everything else is
  authenticated. First-run setup is only available while there are zero users.
- Server container runs as a non-root user; volumes are explicit.

## Explicit non-goals

- No storefront integration, no DRM circumvention, no piracy features.
- No game streaming; games run locally.
- No proprietary GameVault+ features (themes marketplace, paid cloud saves).
  Cloud saves via Ludusavi are on the roadmap as an open feature.
