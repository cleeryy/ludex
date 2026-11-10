# Feature parity: GameVault vs Ludex

This table compares the public feature set of GameVault (1.x and 2.x) with Ludex
v0.1. It is deliberately conservative: a row says **Yes** only when the feature
is in the v0.1 scope and covered by the API/desktop contract, **Partial** when a
useful subset exists, and **No** when it is not shipped.

GameVault is a moving target; this was compiled from its public website and
documentation in November 2026. GameVault 2 is a rewrite and some 1.x features
were not yet at parity at that time. If a row is wrong, open a PR with a source.

Legend:

- **Yes** — available in the current public release.
- **Partial** — available with limits.
- **No** — not available.
- **GV+** — paid GameVault+ tier.
- **Roadmap** — planned in Ludex, not in v0.1.

## Library and organization

| Feature | GameVault | Ludex v0.1 | Status / notes |
|---|---|---|---|
| Recursive library scan of archives | Yes | Yes | Ludex scans `.zip`, `.rar`, `.7z`, portable `.exe`, depth 3, skips support folders and files under 1 MiB. |
| Filename parsing (title, year, version, edition) | Yes | Yes | Conservative parser; manual edits override. |
| Search, filters, sorting, pagination | Yes | Yes | Query params in `docs/CONTRACT.md`. |
| Bookmarks / favorites | Yes | Yes | Per user. |
| Progress states | Yes | Yes | unplayed, playing, completed, abandoned, on hold, plan to play. |
| Playtime tracking | Yes | Yes | Desktop reports minutes; server aggregates per user. |
| Last played | Yes | Yes | Exposed in `GameSummary`. |
| Multiple versions per game | Yes | Yes | One archive = one version. |
| Random game picker | Yes | Yes | `GET /api/games/random` with filters. |
| Stats overview | Yes | Yes | `/api/stats`. |
| News posts | Yes | Yes | Admin-managed. |
| Collections / smart lists | Partial (2.x) | No | Roadmap (v0.2). |
| Share links | Yes (2.x) | No | Roadmap (v0.2). |
| Community view | Yes (2.x) | No | Roadmap (v0.4). |
| Server-side game streaming | No | No | Explicit non-goal; games run locally. |

## Metadata

| Feature | GameVault | Ludex v0.1 | Status / notes |
|---|---|---|---|
| IGDB metadata | Yes | Yes | Client ID + secret in server env. |
| Steam Store fallback | No | Yes | No API key required; used when IGDB is unset or misses. |
| Manual metadata editing | Yes | Yes | Admin/editor. |
| Covers, backgrounds, screenshots | Yes | Yes | Downloaded and cached in `/media`; no third-party hotlinking. |
| Genres, tags, developers, publishers, rating, release date, age rating | Yes | Yes | Provider-dependent. |
| Trailers | Yes | Partial | Trailer links are stored and exposed; v0.1 has no embedded media player. |
| Metadata refresh | Yes | Yes | `POST /api/games/{id}/refresh-metadata`. |
| Metadata provider plugins | No | No | Fixed chain: IGDB → Steam → filename. |

## Downloads and installs

| Feature | GameVault | Ludex v0.1 | Status / notes |
|---|---|---|---|
| Download queue | Yes | Yes | Server-side ingest queue, newest first. |
| Resumable downloads (HTTP Range) | Yes | Yes | `.part` files; range resume; magic-byte format detection. |
| URL ingest downloader | External `gv-downloader` | Yes | Built into the server; jobs survive restarts. |
| Upload archives through the web UI | Yes | No | URL-based ingest only in v0.1; multipart web upload is roadmap (v0.2). |
| Archive verification | Partial | Yes | Full archive test via ingest API. |
| Extract zip / 7z / rar | Yes | Yes | Path-traversal guarded; password list support. |
| Main executable detection | Yes | Yes | Scoring + blocklist for uninstallers/redists. |
| Launch game and report playtime | Yes | Yes | Desktop app. |
| Choose install directory | Yes | Yes | Desktop setting. |
| Multiple install roots / libraries | Partial | No | Roadmap (v0.4). |
| Server-side extraction | No | No | Extraction happens on the desktop. |

## Users and operations

| Feature | GameVault | Ludex v0.1 | Status / notes |
|---|---|---|---|
| Multi-user accounts | Yes | Yes | Argon2id hashes, JWT sessions. |
| Roles / permissions | Yes | Yes | `admin`, `editor`, `user`. |
| First-run admin setup | Yes | Yes | Only while zero users exist. |
| Docker deployment | Yes | Yes | Single image, three volumes. |
| Single binary, no external database | No | Yes | Rust + SQLite. Differentiator, not a parity gap. |
| Backup/restore documentation | Yes | Yes | `docs/DEPLOY.md`. |
| Existing archive folder import | n/a | Yes | Point `LUDEX_GAMES_DIR` at it. |
| GameVault database import (metadata, progress, users) | n/a | No | Roadmap (v0.2); archive scan works today. See `docs/MIGRATION.md`. |
| JSON API | Yes | Yes | `docs/CONTRACT.md`. |
| API documentation | Yes | Yes | Markdown contract in `docs/CONTRACT.md`; no generated schema in v0.1. |

## Clients

| Feature | GameVault | Ludex v0.1 | Status / notes |
|---|---|---|---|
| Web UI (browse/download) | Yes | Yes | Served by the server. |
| Windows desktop | Yes | Yes | Tauri 2, NSIS; unsigned in v0.1. |
| Linux desktop | Yes | Yes | AppImage and `.deb`; unsigned. |
| macOS desktop | Beta | Partial | Universal DMG from CI; Developer ID signing + notarization activate with the release secrets (#27), unsigned builds are published otherwise. |
| Android | Yes | No | Responsive web only. |
| iOS | Beta | No | Not planned for v0.1. |
| Local mode without a server | No | Yes | Desktop embeds `ludex-server` on loopback. |
| Offline/PWA mode | No | No | Roadmap, unscheduled. |

## Integrations and UX

| Feature | GameVault | Ludex v0.1 | Status / notes |
|---|---|---|---|
| Cloud saves via Ludusavi | GV+ | No | Roadmap (v0.3). |
| Discord Rich Presence | Yes | No | Roadmap (v0.3). |
| Steam library integration | Yes | No | Roadmap (v0.3). |
| Playnite plugin | Yes | No | Roadmap (v0.3). |
| Gamepad navigation | Yes (2.x) | No | Roadmap (v0.3). |
| Tray icon / minimize to tray | Yes | No | Roadmap (v0.3). |
| Auto-updater | Yes | No | Roadmap (v0.3); installers are manual in v0.1. |
| Themes | GV+ | Yes | Light / dark / system in v0.3; tokens only, same layout. |
| Multiple profiles | Yes (2.x) | No | Roadmap (v0.4). |
| Streamer mode (hide titles) | Yes | No | Roadmap (v0.4). |
| Translations | Yes | Yes (partial) | English + French official, German/Spanish/Arabic community seeds, RTL in v0.3. |
| Keyboard shortcuts | Yes | Partial | Browser defaults; command palette planned. |
| Responsive mobile web | Partial | Partial | Usable, PWA-installable with offline shell in v0.3; not a dedicated mobile client. |

## Roadmap

### v0.1 — Foundations (current)

Core library, scanner, metadata, search/filter/sort, bookmarks, progress,
playtime, versions, Range downloads, ingest queue, web UI, users/roles, news,
Docker, Windows/Linux desktop, local mode, GameVault archive scan.

### v0.2 — Migration and sharing

- GameVault PostgreSQL importer (users mapped by username, metadata overrides,
  progress states, playtime; password hashes cannot be reused).
- Share links for games and versions.
- Collections / smart lists.
- Parser improvements (multi-disc, scene tags, ambiguous years).
- Upload and ingest polish: multipart web upload, pause/resume all, bandwidth limits.

### v0.3 — Ecosystem

- Ludusavi cloud-save integration.
- Discord Rich Presence.
- Playnite plugin and Steam library import.
- Gamepad navigation, tray icon, desktop auto-updater.
- Signed Windows builds (code signing certificate required).

### v0.4 — Scale and community

- Multiple libraries / install roots.
- Community view and public profiles.
- Native review of community locales.
- macOS signing/notarization.

### Explicit non-goals

- DRM circumvention, storefront scraping or piracy features.
- Game streaming/remote play (games run locally).
- A paid tier, marketplace, or feature gate.
- Copying GameVault code or assets.
