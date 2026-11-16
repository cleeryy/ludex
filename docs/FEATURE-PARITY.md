# Feature parity: GameVault vs Ludex

This table compares the public feature set of GameVault (1.x and 2.x) with **Ludex v0.4**. It
is deliberately conservative: a row says **Yes** only when the feature ships and is covered by
the API/desktop contract, **Partial** when a useful subset exists, and **No** when it does not.

GameVault is a moving target. The GameVault column was compiled from its public website and
documentation in November 2026 and is not re-verified on every Ludex release; LV numbers in the
right-hand column are the Ludex release that shipped the feature. If a row is wrong, open a PR
with a source.

Legend:

- **Yes** — available in the current public release.
- **Partial** — available with limits, stated in the notes.
- **No** — not available.
- **GV+** — GameVault+ paid tier.
- **Non-goal** — deliberately out of scope for Ludex.

## Library and organization

| Feature | GameVault | Ludex v0.4 | Status / notes |
|---|---|---|---|
| Recursive library scan of archives | Yes | Yes (v0.1) | `.zip`, `.rar`, `.7z`, portable `.exe`, depth 3; support folders and files under 1 MiB are skipped. |
| Filename parsing (title, year, version, edition) | Yes | Yes (v0.1) | Conservative parser; manual edits win. |
| Search, filters, sorting, pagination | Yes | Yes (v0.1) | Query params in [`CONTRACT.md`](CONTRACT.md). |
| Bookmarks / favorites | Yes | Yes (v0.1) | Per user. |
| Progress states | Yes | Yes (v0.1) | unplayed, playing, completed, abandoned, on hold, plan to play. |
| Playtime tracking | Yes | Yes (v0.1) | Reported by the desktop; aggregated per account. |
| Last played | Yes | Yes (v0.1) | Exposed in `GameSummary`. |
| Multiple versions per game with rollback | Yes | Yes (v0.4) | Version list, default version, delete and re-pin (#168). |
| Random game picker | Yes | Yes (v0.1) | `GET /api/games/random` with filters. |
| Stats overview | Yes | Yes (v0.1) | `/api/stats`. |
| News posts | Yes | Yes (v0.1) | Admin-managed. |
| Collections / smart lists | Partial (2.x) | Yes (v0.4) | Collections with manual membership, per-collection visibility, import (#173). No rule-based smart lists. |
| Share links | Yes (2.x) | Yes (v0.4) | Public game, version and collection pages via expiring tokens, optional password, no account needed. |
| Community view | Yes (2.x) | Yes (v0.4) | Friends, presence, activity feed and achievements, gated by per-profile privacy (#161). |
| Server-side game streaming | No | Non-goal | Games run on your machine; the console shell is for a TV, not a remote GPU. |

## Metadata

| Feature | GameVault | Ludex v0.4 | Status / notes |
|---|---|---|---|
| IGDB metadata | Yes | Yes (v0.1) | Client ID + secret in the server environment. |
| Steam Store fallback | No | Yes (v0.1) | No API key required; used when IGDB is unset or misses. |
| Manual metadata editing | Yes | Yes (v0.1) | Admin/editor. |
| Manual metadata matching (pick the right provider hit) | Partial | Yes (v0.4) | Editor search across IGDB and configured providers (#135). |
| Covers, backgrounds, screenshots | Yes | Yes (v0.1) | Cached in `/media`; no third-party hotlinking at render time. |
| Higher-resolution artwork (2× covers, 1080p backgrounds) | Partial | Yes (v0.2) | Requested from IGDB and cached locally. |
| Genres, tags, developers, publishers, rating, release date, age rating | Yes | Yes (v0.1) | Provider-dependent. |
| Trailers | Yes | Yes (v0.4) | Click-to-play facade over the stored YouTube link, with an "open on YouTube" fallback (#152). |
| Metadata refresh | Yes | Yes (v0.1) | `POST /api/games/{id}/refresh-metadata`. |
| Metadata provider plugins | No | Partial (v0.4) | Ordered, configurable provider chain (IGDB, Steam, custom HTTP) from the admin UI; no third-party plugin ABI. |

## Downloads and installs

| Feature | GameVault | Ludex v0.4 | Status / notes |
|---|---|---|---|
| Download queue | Yes | Yes (v0.1) | Server ingest queue, newest first. |
| Client install queue (persisted, reorderable) | Yes | Yes (v0.4) | One active install, the rest queued with positions; survives window close (#202). |
| Resumable downloads (HTTP Range) | Yes | Yes (v0.1) | `.part` staging, range resume, stall detection, bounded reconnect (#195). |
| URL ingest downloader | External `gv-downloader` | Yes (v0.1) | Built into the server; jobs survive restarts. |
| Upload archives through the web UI | Yes | Yes (v0.4) | Admin upload with background jobs and chunked resume (#185). |
| Archive verification | Partial | Yes (v0.4) | Full-archive SHA-256 before install, plus a visible `verifying` phase (#216, #223). |
| Extract zip / 7z / rar | Yes | Yes (v0.1) | Path-traversal guarded, password list, parallel ZIP extraction (v0.4.14). |
| Stream install (extract while downloading) | No | Yes (v0.2) | ZIP and 7z with per-entry resume; RAR stays classic (#203–#208). |
| Main executable detection | Yes | Yes (v0.1) | Scoring plus a blocklist for uninstallers and redistributables. |
| Launch game and report playtime | Yes | Yes (v0.1) | Desktop app; foreground hand-off (#219). |
| Choose install directory | Yes | Yes (v0.2) | Custom download and install folders, per-game overrides. |
| Multiple install roots / libraries | Partial | Partial | One server-side library root; the desktop manages its own install folders and reads platform library roots for adoption. |
| Server-side extracted cache | No | Partial (v0.4) | Opt-in, content-addressed cache with per-file manifests (`LUDEX_EXTRACTED_CACHE=1`, #231); off by default until the benchmark gate passes. |
| Disk-space reservation before download | No | Yes (v0.4) | Per-volume ledger with projections (#222). |

## Users and operations

| Feature | GameVault | Ludex v0.4 | Status / notes |
|---|---|---|---|
| Multi-user accounts | Yes | Yes (v0.1) | Argon2id hashes, HS256 JWT sessions. |
| Roles / permissions | Yes | Yes (v0.1) | `admin`, `editor`, `user`, enforced in the API. |
| First-run admin setup | Yes | Yes (v0.1) | Only while zero users exist. |
| Invitations instead of open registration | Yes | Yes (v0.4) | Expiring single-use links (#166). |
| Two-factor authentication | No | Yes (v0.4) | TOTP with recovery codes; no session before the second factor (#166). Passkeys are roadmap. |
| Session / device list with revoke | Partial | Yes (v0.4) | Per-device sessions, revoke one or all; admin reset evicts devices (#166). |
| Docker deployment | Yes | Yes (v0.1) | Single image, three volumes. |
| Single binary, no external database | No | Yes (v0.1) | Rust + SQLite. Differentiator, not a parity gap. |
| PostgreSQL backend | Yes | Yes (v0.3) | `LUDEX_DATABASE_URL=postgres://…`, SQLite stays the default ([`POSTGRES.md`](POSTGRES.md)). |
| S3-compatible archive storage | Partial | Yes (v0.2) | `LUDEX_STORAGE_BACKEND=s3` ([`STORAGE.md`](STORAGE.md)). |
| Backup and restore | Yes | Yes (v0.4) | Admin UI: create, verify, download, restore with a mandatory safety copy (#169), plus `ludex-backup`. |
| Observability | Partial | Yes (v0.4) | Prometheus metrics, `/api/health/details`, bounded request/error dashboard (#170). |
| Audit log, API keys, signed webhooks | Partial | Yes (v0.2–v0.4) | `ldx_` API keys, HMAC-SHA256 webhooks, audit events ([`INTEGRATIONS.md`](INTEGRATIONS.md)). |
| Library health report | No | Yes (v0.4) | Missing archives, orphaned media, duplicates, metadata gaps, explicit repairs (#165). |
| Anonymous diagnostics | Unknown | Yes (v0.4) | Opt-in, default **off**, redacted, previewed before sending (#171). |
| GameVault database import (metadata, progress, users) | n/a | No | Archive scan works today ([`MIGRATION.md`](MIGRATION.md)); a PostgreSQL importer is roadmap. |
| JSON API | Yes | Yes (v0.1) | [`CONTRACT.md`](CONTRACT.md) is the source of truth; `docs/OPENAPI.md` describes the served schema. |
| Generated API documentation | Yes | Partial | Hand-written contract plus an OpenAPI description; no Swagger UI served by default. |

## Clients

| Feature | GameVault | Ludex v0.4 | Status / notes |
|---|---|---|---|
| Web UI (browse/download) | Yes | Yes (v0.1) | Served by the server, embedded in the binary. |
| Windows desktop | Yes | Yes (v0.1) | Tauri 2, NSIS installer. Not code-signed yet. |
| Linux desktop | Yes | Yes (v0.1) | AppImage and `.deb`. |
| macOS desktop | Beta | Partial | Universal DMG from CI; Developer ID signing and notarization activate with the release secrets, unsigned builds otherwise. |
| Android | Yes | No | Installable PWA with a remote-install queue instead (#167). |
| iOS | Beta | No | Responsive web only. |
| Local mode without a server | No | Yes (v0.1) | Desktop embeds `ludex-server` on loopback. |
| Offline / PWA shell | No | Yes (v0.3) | Installable, precached shell, update prompt; the library itself needs the server. |
| Desktop auto-update | Yes | Yes (v0.4) | Tauri updater with signed artifacts and an in-app update notifier (#213). |

## Integrations and UX

| Feature | GameVault | Ludex v0.4 | Status / notes |
|---|---|---|---|
| Cloud saves | GV+ | Yes (v0.2) | Per-account opaque `.zip` snapshots, isolated and quota-limited. |
| Automatic save discovery (Ludusavi) | Yes | Yes (v0.3) | Auto-managed Ludusavi manifest, registry of save roots, confirmed folders (#146, #153). |
| Discord Rich Presence | Yes | Yes (v0.2) | Desktop worker, opt-in; shows what you are playing. |
| Steam library integration | Yes | Partial | Installed Steam titles can be adopted into the library; importing a full library is roadmap. |
| Playnite plugin | Yes | No | Not planned for v0.4. |
| Gamepad navigation | Yes (2.x) | Yes (v0.4) | One input→command router for keyboard and pads, rememberable per-device profiles (#138, #230). |
| Tray icon / minimize to tray | Yes | Yes (v0.4) | Tray menu, tooltip with active downloads. |
| In-game overlay | No | Yes (v0.4) | Native global controller listener, configurable chord, self-diagnosing panel (#144, #228). |
| Console mode (10-foot UI on a TV) | No | Yes (v0.4) | Sunshine publishes a single managed app; gamepad-first shell with Soft/Studio appearances (#172, #229). |
| Achievements | Yes | Yes (v0.2) | Per-game and global definitions, unlock state, points, webhooks and an audit trail (#117). |
| ROM and emulator libraries | Yes | Yes (v0.3) | `kind=rom` roots, emulator registry with validated `%ROM%` command templates (#52). |
| VirusTotal scanning | No | Yes (v0.4) | Hashes only by default, opt-in upload, cached reports, quota-aware re-check policy (#224, #239). |
| Themes | GV+ | Yes (v0.3) | Six built-in palettes plus the console's Soft/Studio appearance, light/dark/system with no flash. |
| Multiple profiles | Yes (2.x) | No | Server profiles and controller profiles exist; several user profiles per account do not. |
| Streamer mode (hide titles) | Yes | No | Not planned for v0.4. |
| Translations | Yes | Yes (v0.3) | English and French official, 11 further locales shipped; RTL infrastructure in place (#139, #188). |
| Keyboard shortcuts and command palette | Partial | Yes (v0.3) | Global palette, per-screen shortcuts, console keyboard mapping. |
| Responsive mobile web | Partial | Yes (v0.4) | Safe-area aware layout, PWA install and update prompts, remote-install queue. |

## Roadmap

What is explicitly **not** in v0.4 and is being worked on, in rough order:

1. GameVault PostgreSQL importer (users by username, metadata overrides, progress, playtime —
   password hashes cannot be reused).
2. Passkeys as a second factor alongside TOTP.
3. Storefront library import beyond adoption of installed titles.
4. File-manifest transport enabled by default once the 40 GB benchmark clears its gate
   ([`DOWNLOADS.md`](DOWNLOADS.md)).

Non-goals: server-side game streaming, DRM circumvention, bundled games, storefront or piracy
features, telemetry.
