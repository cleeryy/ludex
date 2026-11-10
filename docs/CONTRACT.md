# Ludex API + desktop contract (v0.1)

Single source of truth for the server, web UI and Tauri shell. All JSON is
`camelCase`. All routes are under `/api` unless stated. Auth is
`Authorization: Bearer <jwt>` or, for integrations, `Authorization: Bearer
ldx_<token>` (an API key) except public routes.

## Errors

```json
{ "error": { "code": "invalid_credentials", "message": "Invalid username or password" } }
```

HTTP status is meaningful: 400 validation, 401 unauthenticated, 403 forbidden,
404 not found, 409 conflict, 422 semantic, 429 rate limited, 500 internal.

## Types

```ts
type Role = "admin" | "editor" | "user";
type ProgressState =
  | "unplayed" | "playing" | "completed" | "abandoned" | "onHold" | "planToPlay";

interface User {
  id: number; username: string; email: string | null; role: Role;
  activated: boolean; avatarUrl: string | null; createdAt: string;
}

interface GameSummary {
  id: number; title: string; sortTitle: string; releaseDate: string | null;
  coverUrl: string | null; backgroundUrl: string | null; rating: number | null;
  genres: string[]; earlyAccess: boolean; versionCount: number; totalSize: number;
  state: ProgressState; minutesPlayed: number; bookmarked: boolean;
  lastPlayedAt: string | null; maxPlayers: number | null;
  /** Present only when the row came from a search (`q`). */
  matchedField?: "title" | "sortTitle" | "altTitle";
  matchedTitle?: string; matchScore?: number;
}

interface GameVersion {
  id: number; fileName: string; size: number; version: string | null;
  releaseDate: string | null; earlyAccess: boolean;
  /** Cheap archive fingerprint (`sha256-sample:<hex>`), null until computed. */
  archiveHash: string | null;
  /** Last time the version row changed (indexed, renamed or re-hashed). */
  updatedAt: string | null;
  downloadUrl: string; // /api/games/{id}/download?versionId={id}
}

/** Newest available version, used by desktop clients to detect updates. */
interface LatestVersion {
  gameId: number; versionId: number;
  versionLabel: string | null;   // parsed from the file name
  fileName: string;
  archiveHash: string | null;    // `sha256-sample:<hex>` sampled fingerprint
  archiveSize: number;           // bytes
  updatedAt: string | null;      // RFC 3339
  earlyAccess: boolean;
  downloadUrl: string;
}

interface GameVideo {
  title: string;
  url: string;
  thumbnailUrl: string | null;
  /** YouTube id for IGDB trailers; null for direct file URLs. */
  videoId?: string | null;
  /** "youtube" or "file". */
  provider?: string | null;
}

interface GameDetail extends GameSummary {
  description: string | null;
  screenshots: string[]; // media URLs
  /** Deprecated alias of `videos`, kept for v0.2 clients. */
  trailers: GameVideo[];
  /** Trailers and other videos, with provider metadata. */
  videos: GameVideo[];
  developers: string[]; publishers: string[]; tags: string[]; ageRating: number | null;
  provider: { slug: string; id: string; url: string | null } | null;
  altTitles: string[];
  versions: GameVersion[];
  launch: {
    executable: string | null; parameters: string | null;
    installerExecutable: string | null; installerParameters: string | null;
    uninstallerExecutable: string | null; uninstallerParameters: string | null;
  };
}

interface NewsPost {
  id: number; title: string; body: string; author: string;
  createdAt: string; updatedAt: string;
}

interface IngestJob {
  id: string; title: string; url: string | null;
  status: "queued" | "downloading" | "verifying" | "done" | "error" | "cancelled";
  fileName: string | null; downloadedBytes: number; totalBytes: number | null;
  speedBps: number; attempt: number; error: string | null;
  createdAt: string; updatedAt: string;
}

interface ScanStatus {
  running: boolean; startedAt: string | null; finishedAt: string | null;
  scanned: number; added: number; updated: number; removed: number;
  metadataPending: number; errors: string[];
}
```

## Public

| Method | Route | Response |
|---|---|---|
| GET | `/api/health` | `{ status: "ok", version, uptimeSeconds }` |
| GET | `/api/setup/status` | `{ needsSetup: boolean, serverName: string, version: string }` |
| POST | `/api/setup` | body `{ username, password, serverName? }` → `{ token, user }`; 409 if users exist |
| GET | `/api/media/{name}` | cached image/file (public, immutable cache headers); `302` to the object URL when `LUDEX_S3_PUBLIC_BASE_URL` is set |
| GET | `/api/config` | `{ serverName, version, features: { igdb, steam, ingest, localMode }, maxUploadBytes }` (public, used by setup/login) |

## Auth

| Method | Route | Notes |
|---|---|---|
| POST | `/api/auth/login` | `{ username, password }` → `{ token, user }` |
| GET | `/api/auth/me` | `{ user }` |
| POST | `/api/auth/logout` | 204 (client discards token) |
| POST | `/api/auth/change-password` | `{ currentPassword, newPassword }` |

## Games (authenticated)

| Method | Route | Notes |
|---|---|---|
| GET | `/api/games` | query: `q`, `genre`, `state`, `bookmarked`, `earlyAccess`, `yearFrom`, `yearTo`, `players`, `playtimeMin`, `playtimeMax`, `minRating`, `provider`, `sort` (`title\|releaseDate\|rating\|addedAt\|lastPlayed\|playtime`), `order` (`asc\|desc`), `page` (1), `limit` (48, max 200) → `{ items, total, page, limit }` |
| GET | `/api/games/suggest` | query: `q`, `limit` (8, max 20) → `{ query, suggestions: [{ id, title, year, coverUrl, matchedField, matchedTitle, score }] }`; top matches, best first |
| GET | `/api/games/{id}` | `GameDetail` |
| GET | `/api/games/random` | same filters as list → `GameSummary`; 404 when empty |
| PATCH | `/api/games/{id}` | editor; editable metadata fields (`altTitles`, `players`) + `launch` object; returns `GameDetail` |
| POST | `/api/games/{id}/refresh-metadata` | editor; 202 `{ queued: true }` |
| POST | `/api/games/{id}/download-ticket` | `{ versionId? }` → `{ url, expiresAt }` (15 min, no auth needed on the URL) |
| GET | `/api/games/{id}/download?versionId=&token=` | file stream, Range supported, `Content-Disposition` |
| GET | `/api/games/{id}/versions/{versionId}/stream?token=` | Stream install: an uncompressed `application/x-tar` of the archive entries, no archive stored on the client. ZIP and 7z are supported; RAR/unknown return 415 and the client falls back. Exact `Content-Length`. Same auth rules as download. |
| GET | `/api/games/{id}/versions/latest` | newest non-missing version → `LatestVersion`; 404 `no_versions` when the game has none |
| GET | `/api/games/{id}/versions/{versionId}/local-path` | embedded/local mode + loopback only → `{ path }`, else 404 |

### Version fingerprints

`archiveHash` is a **sampled** fingerprint, not a full content hash: SHA-256 over
the file size, the first MiB and the last MiB (`sha256-sample:<hex>`, see
`ludex_core::fingerprint`). It changes when an archive is replaced, costs two
read windows per file, and is computed by the scanner (new files immediately,
older rows over the next scans). `updatedAt` moves whenever the row is
re-indexed. Clients compare `versionId` first and fall back to the fingerprint
when a version was re-uploaded in place.

### Library filters and search

`yearFrom`/`yearTo` are inclusive release years. `players` is the minimum
supported player count (`maxPlayers`). `playtimeMin`/`playtimeMax` are inclusive
minutes played by the requesting user. `minRating` is 0-100. `provider` is a
metadata slug (`igdb`, `steam`). All filters combine with `AND` and are safe to
share in a URL.

When `q` is present, titles, sort titles and provider aliases are matched
server-side with diacritic-insensitive fuzzy scoring (trigram + Levenshtein
fallback). Results are ranked by relevance and carry `matchedField`,
`matchedTitle` and `matchScore`; `sort` is ignored in that mode. `GET
/api/games/suggest` exposes the same scorer for type-ahead suggestions.

### Progress / bookmarks

| Method | Route | Body | Response |
|---|---|---|---|
| PUT | `/api/games/{id}/progress` | `{ state }` | `{ state, minutesPlayed, lastPlayedAt }` |
| POST | `/api/games/{id}/playtime` | `{ minutes }` | updated progress |
| PUT | `/api/games/{id}/bookmark` | `{ bookmarked }` | `{ bookmarked }` |
| POST | `/api/games/{id}/installed` | desktop install report → `{ recorded }` |
| GET | `/api/stats` | — | `{ games, versions, totalBytes, users, byState: Record<ProgressState, number> }` |

## Collections (#25)

Collections are per-user shelves. `kind=manual` stores an explicit list;
`kind=smart` stores a rule tree and evaluates it at read time. Every account can
read public collections; private collections return 404 to anyone but the owner.

```ts
type CollectionKind = "manual" | "smart";
type SmartField =
  | "genre" | "year" | "players" | "playtime"
  | "rating" | "installed" | "provider" | "title";

interface SmartRule {
  field: SmartField;
  /** Defaults: "is" except title → "contains", players/playtime/rating → "gte". */
  op?: "is" | "contains" | "gte" | "lte" | "between";
  value: string | number | boolean;
  value2?: string | number; // upper bound for "between"
}
interface SmartRules { match: "all" | "any"; rules: SmartRule[] }

interface Collection {
  id: number; name: string; description: string | null;
  isPublic: boolean; kind: CollectionKind; rules: SmartRules;
  gameCount: number; owner: { id: number; username: string; displayName: string | null };
  owned: boolean; createdAt: string; updatedAt: string;
}
```

Rules combine with `all` (AND) or `any` (OR). An empty rule tree matches
nothing, so a fresh smart collection reads as empty. `installed` means "a
downloadable version exists" because the server cannot see the desktop's local
installs. Playtime and installed state are evaluated for the requesting user.
Unknown fields, operators or value types return `400 invalid_rules` on write.

| Method | Route | Notes |
|---|---|---|
| GET | `/api/collections` | own collections plus public ones, own first |
| POST | `/api/collections` | `{ name, description?, isPublic?, kind?, rules?, gameIds? }` → 201 `Collection` |
| GET | `/api/collections/{id}` | owner or public, else 404 |
| PATCH | `/api/collections/{id}` | owner; partial fields; switching manual→smart clears the explicit list |
| DELETE | `/api/collections/{id}` | owner; 204 |
| GET | `/api/collections/{id}/games` | `?page=&limit=` → `Paginated<GameSummary>`; smart rules evaluated live |
| POST | `/api/collections/{id}/games` | owner, manual only; `{ gameIds: number[] }` → `{ added, total }`; smart → 409 |
| DELETE | `/api/collections/{id}/games/{gameId}` | owner, manual only; 204 |

Names are unique per owner (case-insensitive) → `409 collection_exists`.

## Community (#10)

Profiles are opt-in. `users.public_profile` defaults to false; only opted-in
accounts appear in the directory, the feed and profile pages. Private profiles
return 404 to everyone else, including their activity.

```ts
interface PublicProfile {
  id: number; username: string; displayName: string | null; bio: string | null;
  avatarUrl: string | null; publicProfile: boolean; createdAt: string;
}
interface PublicStats {
  games: number; completed: number; minutesPlayed: number;
  bookmarked: number; collections: number;
}
interface PublicProfileResponse {
  user: PublicProfile; stats: PublicStats;
  library: GameSummary[];   // progress or bookmark rows, most recent first
  collections: Collection[]; // public collections (all of them for the owner)
}
type ActivityKind =
  | "game_added" | "game_installed" | "game_played"
  | "game_completed" | "collection_created";
interface Activity {
  id: number; kind: ActivityKind; createdAt: string;
  metadata: Record<string, unknown>;
  user: { id: number; username: string; displayName: string | null; avatarUrl: string | null };
  game: { id: number; title: string; coverUrl: string | null; releaseYear: number | null } | null;
  collection: { id: number; name: string } | null;
}
interface ActivityPage { items: Activity[]; nextBefore: number | null }
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/me/profile` | own profile, including `publicProfile` |
| PATCH | `/api/me/profile` | `{ publicProfile?, displayName?, bio? }`; `null` clears text |
| GET | `/api/community/users` | public profiles, alphabetical |
| GET | `/api/community/users/{id}` | `PublicProfileResponse`; private → 404 for others |
| GET | `/api/community/users/{id}/activity` | `?limit=&before=` → `ActivityPage` |
| GET | `/api/community/feed` | public activity, newest first; `?limit=&before=` |

Activity is recorded server-side: `game_added` when a game first enters the
account's library (first progress row or playtime report), `game_played` on a
playing state or playtime report (deduped 30 min), `game_completed` on the
completed state (deduped 60 min), `collection_created` on creation, and
`game_installed` from `POST /api/games/{id}/installed` (deduped 60 min; the
desktop calls it). Feed entries survive deletion of the referenced game or
collection as `null` references.

## Cloud saves (per user)

Manual per-user save snapshots (v1). The desktop can later automate this with
Ludusavi; the server stores opaque archives and never inspects them.

```ts
interface SaveSnapshot {
  id: number; gameId: number; label: string; fileName: string;
  size: number; sha256: string; createdAt: string;
}
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/games/{id}/saves` | current user's snapshots, newest first |
| POST | `/api/games/{id}/saves` | multipart `file` + optional `label`; max 2 GiB; returns 201 `SaveSnapshot` |
| GET | `/api/saves/{id}/download` | owner (or admin) download, `Content-Disposition` |
| DELETE | `/api/saves/{id}` | owner (or admin), 204 |

Storage: `{data_dir}/saves/{user_id}/{game_id}/{snapshot_id}-{sanitized_name}`.
A user can never see another user's snapshots. The desktop UI offers "Upload
save archive (.zip)" and "Download/restore"; automatic save-file discovery is a
follow-up (Ludusavi).

## Users (admin)

| Method | Route | Notes |
|---|---|---|
| GET | `/api/users` | `User[]` |
| POST | `/api/users` | `{ username, password, email?, role }` → `User` |
| PATCH | `/api/users/{id}` | `{ email?, role?, activated? }` → `User` |
| DELETE | `/api/users/{id}` | 204; cannot delete the last admin or self |

## API keys (#40)

API keys are random `ldx_<64 hex>` bearer tokens. The plaintext token is
returned exactly once by `POST /api/keys`; only its SHA-256 and a non-secret
`prefix` are stored. Keys inherit their owner's role but are limited by their
scopes: `read` (GET), `write` (mutations: progress, saves, library scan,
ingest, key management) and `admin` (administrator routes). `admin` implies
`write`, which implies `read`.

```ts
interface ApiKey {
  id: number; userId: number; name: string; prefix: string;
  scopes: ("read" | "write" | "admin")[];
  createdAt: string; lastUsedAt: string | null; revokedAt: string | null;
}
// POST response adds `secret: string` (never returned again).
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/keys` | own keys, newest first; `?all=true` (admin) lists every account |
| POST | `/api/keys` | `{ name, scopes?, userId? }` → 201 `ApiKey & { secret }`; `userId` requires admin |
| DELETE | `/api/keys/{id}` | revoke (idempotent, 204); owner or admin |

`Authorization: Bearer ldx_...` is accepted everywhere a JWT is. Every use
updates `lastUsedAt`. Revoked or unknown keys get the same 401 as a bad JWT.

## Audit log (#42)

Every security-relevant event is written to `audit_logs` with an actor
(`user`, `api_key` or `system`), action, optional target, secret-free JSON
metadata, IP when known, and timestamp. Recorded actions: `login.succeeded`,
`login.failed`, `user.created|updated|deleted|password_changed`, `api_key.created|revoked`,
`game.ingested|deleted`, `download.completed`, `settings.updated`,
`webhook.created|updated|deleted`, `save.uploaded|deleted`, `oidc.login`,
`oidc.linked`.

```ts
interface AuditLog {
  id: number; actorId: number | null; actorKind: "user" | "api_key" | "system";
  action: string; targetType: string | null; targetId: string | null;
  metadata: Record<string, unknown>; ip: string | null; createdAt: string;
}
interface AuditPage { items: AuditLog[]; nextBefore: number | null }
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/audit` | admin + `admin` scope. Query: `limit` (50, max 200), `before` (id cursor), `actorId`, `actorKind`, `action`, `targetType`, `targetId` |

Newest first. Pass `nextBefore` as `before` to fetch the next page; `null`
means the last page.

## Webhooks (#41)

Administrators register outbound HTTP endpoints. Deliveries are asynchronous:
each event is stored in `webhook_deliveries`, signed with HMAC-SHA256 over the
exact JSON body, retried up to 5 times with exponential backoff (1 s, 2 s, 4 s,
8 s; base configurable with `LUDEX_WEBHOOKS_BACKOFF_MS`) and a 10 s request
timeout. Non-2xx responses and network errors are retried; redirects are not
followed. Loopback/private/link-local targets are rejected unless
`LUDEX_WEBHOOKS_ALLOW_PRIVATE=true` (tests/LAN only).

Events: `game.ingested`, `game.deleted`, `download.completed`, `user.created`,
`save.uploaded`. The test endpoint queues a `webhook.test` delivery.

Request headers:

```
Content-Type: application/json
X-Ludex-Signature: sha256=<hex HMAC-SHA256 of the raw body with the webhook secret>
X-Ludex-Event: game.ingested
X-Ludex-Delivery: 42
```

Payload:

```json
{
  "event": "game.ingested",
  "timestamp": "2026-11-10T12:00:00Z",
  "data": { "title": "Celeste", "source": "scan" }
}
```

```ts
interface Webhook {
  id: number; url: string; events: string[]; enabled: boolean; createdAt: string;
}
// POST response adds `secret: string`; PATCH adds it only when rotateSecret=true.
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/webhooks` | admin + `admin` scope; secrets never returned |
| POST | `/api/webhooks` | `{ url, events, enabled? }` → 201 `Webhook & { secret }` |
| PATCH | `/api/webhooks/{id}` | `{ url?, events?, enabled?, rotateSecret? }` → `Webhook` (+ new `secret` if rotated) |
| DELETE | `/api/webhooks/{id}` | 204 |
| POST | `/api/webhooks/{id}/test` | 202 `{ deliveryId }`; webhook must be enabled |

## Library

| Method | Route | Notes |
|---|---|---|
| POST | `/api/library/scan` | admin/editor; 202 `{ started: boolean }`; scans every enabled root |
| GET | `/api/library/scan` | `ScanStatus` |
| GET | `/api/library/folders` | `{ gamesDir, mediaDir, dataDir }` (admin) |
| GET | `/api/library/roots` | admin + `admin` scope; `LibraryRoot[]`, ordered |
| POST | `/api/library/roots` | admin; `{ path, label?, enabled? }` → 201 `LibraryRoot`; 409 `root_exists` |
| PATCH | `/api/library/roots/{id}` | admin; `{ label?, path?, enabled?, position? }` → `LibraryRoot` |
| DELETE | `/api/library/roots/{id}?force=` | admin; 204; 409 `root_not_empty` unless `force=true` |
| POST | `/api/library/roots/{id}/scan` | admin/editor; 202 `{ started, rootId }`; scans that root only |
| GET | `/api/library/watcher` | admin; `LibraryWatcherStatus` |

```ts
interface LibraryRoot {
  id: number; label: string; path: string;
  enabled: boolean; available: boolean; position: number;
  createdAt: string; lastScanAt: string | null; lastError: string | null;
  versionCount: number;
}

interface LibraryWatcherStatus {
  enabled: boolean; debounceMs: number;
  roots: { id: number; label: string; path: string }[];
  lastEventAt: string | null; lastScanAt: string | null;
  pending: boolean; lastError: string | null;
}
```

A root whose path is missing or unmounted is reported with `available: false`
and a `lastError`; its versions keep `missing = 0` and downloads answer
`404 root_unavailable`. Removing a root keeps games, progress and cloud saves.
See [`LIBRARY.md`](./LIBRARY.md) for scan ordering, mirror deduplication and
watcher platform limits.

## Integrity checks

| Method | Route | Notes |
|---|---|---|
| POST | `/api/integrity/check` | admin + `admin` scope; `{ scope?: "all" \| "archives" \| "media" }` → 202 `{ reportId, status }`; 409 `integrity_running` |
| GET | `/api/integrity/reports?limit=` | admin; newest first, `limit` 1–200 (default 20) |
| GET | `/api/integrity/reports/{id}` | admin; `{ report, findings }`; 404 `report_not_found` |

```ts
type IntegrityScope = "all" | "archives" | "media";

interface IntegrityReport {
  id: number; scope: IntegrityScope;
  status: "running" | "completed" | "failed";
  checked: number; findingsCount: number;
  error: string | null; startedAt: string; finishedAt: string | null;
}

interface IntegrityFinding {
  id: number; kind: "missing" | "corrupt" | "size_mismatch";
  targetType: "archive" | "media"; targetId: string | null;
  path: string | null; detail: string | null; createdAt: string;
}
```

Checks never delete anything: findings are informational. The first successful
hash of an archive or media file becomes its baseline. Scheduled by
`LUDEX_INTEGRITY_INTERVAL_HOURS` (default 168, `0` disables).

## Media cache garbage collection

| Method | Route | Notes |
|---|---|---|
| POST | `/api/media/gc?dryRun=true\|false` | admin + `admin` scope; `dryRun` defaults to `true` |

```ts
interface MediaGcResult {
  dryRun: boolean; graceHours: number;
  scanned: number; referenced: number;
  orphanFiles: number; orphanBytes: number;
  deletedFiles: number; deletedBytes: number;
  candidates: string[]; // up to 100 orphan file names
  startedAt: string; finishedAt: string;
}
```

Only unreferenced files older than `LUDEX_MEDIA_GC_GRACE_HOURS` (default 24)
are removed; files referenced by a game or user row are never deleted. A
scheduled run is enabled by `LUDEX_MEDIA_GC_INTERVAL_HOURS` (default `0`).

## Metrics (#48)

`GET /metrics` is **not** under `/api` and is never public.

| Method | Route | Notes |
|---|---|---|
| GET | `/metrics` | Prometheus text exposition (`text/plain; version=0.0.4`) |

Auth: `Authorization: Bearer <LUDEX_METRICS_TOKEN>` when the token is
configured; otherwise an administrator session JWT or an API key with the
`admin` scope. Anything else is `401` (missing/invalid credential) or `403`
(authenticated but not an administrator).

Series (counters are monotonic for the process lifetime; gauges are sampled at
scrape time): `ludex_build_info`, `ludex_uptime_seconds`,
`ludex_http_requests_total{method,route,status}`,
`ludex_http_request_duration_seconds{route}` (histogram),
`ludex_active_downloads`, `ludex_games_total`, `ludex_game_versions_total`,
`ludex_users_total`, `ludex_ingest_jobs{state}`, `ludex_media_cache_files`,
`ludex_media_cache_bytes` (local backend only),
`ludex_media_backend_info{backend}`, `ludex_scans_total`,
`ludex_scan_duration_seconds`, `ludex_webhook_deliveries_total{outcome}`,
`ludex_db_pool_connections{state}`. See [`OPERATIONS.md`](OPERATIONS.md).

## Ingest (admin/editor) — adapted from gv-downloader

| Method | Route | Notes |
|---|---|---|
| GET | `/api/ingest/downloads` | `IngestJob[]`, newest first |
| POST | `/api/ingest/downloads` | `{ url, title? }` → 201 `IngestJob` |
| GET | `/api/ingest/downloads/{id}` | `IngestJob` |
| DELETE | `/api/ingest/downloads/{id}` | cancel; deletes `.part`; 204 |
| POST | `/api/ingest/downloads/{id}/retry` | resume error/cancelled job |
| PUT | `/api/ingest/downloads/{id}` | `{ url }` replace URL, requeue if stopped |
| GET | `/api/ingest/files` | `{ name, size, partial, modifiedAt }[]` |
| DELETE | `/api/ingest/files/{name}` | delete a library file; 204 |
| POST | `/api/ingest/files/{name}/verify` | full archive test → `{ ok, log }` |
| GET | `/api/ingest/stats` | `{ gamesDir, diskTotal, diskFree, libraryBytes, gameCount, activeJobs }` |

A successful ingest triggers `library/scan` automatically.

## News

| Method | Route | Notes |
|---|---|---|
| GET | `/api/news` | `NewsPost[]` |
| POST | `/api/news` | admin; `{ title, body }` |
| PATCH | `/api/news/{id}` | admin |
| DELETE | `/api/news/{id}` | admin |

## Server config

| Method | Route | Notes |
|---|---|---|
| GET | `/api/config` | public; `{ serverName, version, features: { igdb, steam, ingest, localMode }, maxUploadBytes }` |

## Desktop (Tauri) commands

The web bundle runs in both browsers and Tauri. `apps/web/src/lib/native.ts`
detects Tauri (`window.__TAURI_INTERNALS__`) and calls these commands; in a
browser every function reports `available: false`.

```ts
interface LocalSettings {
  mode: "local" | "remote";
  serverUrl: string;            // remote URL, or the embedded server URL
  token: string | null;         // JWT
  gamesDir: string | null;      // local library folder
  installDir: string;           // where games are installed
  downloadDir: string;          // temp archives; empty = {installDir}/.ludex-tmp
  serverPort: number;           // preferred embedded port, 0 = random
  streamInstall: boolean;       // remote ZIP/7z installs without storing the archive
  keepArchives: boolean;        // keep the downloaded archive after install
  archivePasswords: string[];   // tried in order
  bandwidthLimitKib: number;    // global download cap in KiB/s, 0 = unlimited
  closeToTray: boolean;         // closing the window hides to the tray (default true)
  updateChannel: "stable" | "early"; // app updater channel (default stable)
  checkUpdatesOnStartup: boolean;    // throttled to once per 24 h
  lastUpdateCheckAt: string | null;  // RFC 3339, drives the throttle
  discordPresence: boolean;          // Discord Rich Presence (#12), default false
  discordClientId: string;           // Discord application id, not a bot token
  installerSilentArgs: string[][];   // silent install attempts (#7), empty = presets
  installerTimeoutSecs: number;      // per attempt, default 600, clamped 30-7200
  steamRoot: string;                 // Steam folder override (#13), empty = detect
}

interface InstalledGame {
  gameId: number; versionId: number; title: string; version: string | null;
  installPath: string; executable: string | null; sizeBytes: number;
  installedAt: string; lastLaunchedAt: string | null;
  /** Fingerprint of the archive this install came from, when known. */
  archiveHash: string | null;
  /** `steam` entries were imported from a Steam library (#13). */
  source: "ludex" | "steam";
  /** External launch URI (`steam://rungameid/<appid>`) for imported entries. */
  launchUri: string | null;
}

/** Preview of a silent installer run (#7). */
interface InstallerPlan {
  executable: string | null;
  detected: string[];       // installer-looking files in the install folder
  attempts: string[][];     // argument sets, in order
  timeoutSecs: number;
  interactive: boolean;
  platform: string;         // "windows", "linux", "macos"
}

interface InstallerAttempt {
  args: string[]; exitCode: number | null;
  timedOut: boolean; cancelled: boolean;
  error: string | null; durationMs: number;
}

interface InstallerReport {
  gameId: number; executable: string;
  success: boolean; cancelled: boolean; timedOut: boolean; interactive: boolean;
  exitCode: number | null; attempts: InstallerAttempt[];
  recordedExecutable: string | null; // set after a successful install
  candidates: string[];              // exe picker suggestions
  message: string;
}

/** Discord Rich Presence state (#12). */
interface PresenceStatus {
  enabled: boolean; configured: boolean; connected: boolean;
  lastError: string | null; details: string | null; retryInSeconds: number;
}

/** One installed Steam game discovered on disk (#13). */
interface SteamGame {
  appId: number; name: string; installDir: string; libraryPath: string;
  sizeBytes: number; lastPlayed: number | null;
  matchedGameId: number | null; matchedTitle: string | null;
  imported: boolean; localGameId: number; // negative app id when unmatched
}

interface SteamPreview {
  roots: string[]; games: SteamGame[]; warnings: string[];
  matched: boolean;  // false: the server could not be listed
}

interface SteamImportReport {
  imported: InstalledGame[];
  skipped: { appId: number; title: string; reason: string }[];
}

/** A game with a newer version on the server (#30). */
interface GameUpdate {
  gameId: number; title: string;
  installedVersionId: number; installedVersion: string | null;
  latestVersionId: number; latestVersionLabel: string | null;
  archiveSize: number; archiveHash: string | null; updatedAt: string | null;
  reason: "newVersion" | "replacedArchive";
}

interface PlaySession {
  id: string; gameId: number; startedAt: string; endedAt: string | null;
  minutes: number; seconds: number;
}

interface GamePlaytime {
  gameId: number; totalSeconds: number; sessions: number;
  firstPlayedAt: string | null; lastPlayedAt: string | null;
  byDay: { day: string; seconds: number }[];   // UTC days, newest first, 30 days
  recentSessions: PlaySession[];               // newest first
}

interface UpdateStatus {
  configured: boolean;              // false: build has no signing key
  channel: "stable" | "early";
  currentVersion: string;
  lastCheckedAt: string | null;
  available: { version: string; currentVersion: string; notes: string | null;
               publishedAt: string | null } | null;
  message: string | null;
}

interface InstallJob {
  jobId: string; gameId: number; versionId: number;
  phase: "queued" | "downloading" | "extracting" | "finalizing" | "paused"
       | "done" | "cancelled" | "error";
  bytesDone: number; bytesTotal: number | null; speedBps: number;
  message: string | null; installPath: string | null;
}

interface VerifyIssue {
  path: string;                 // relative to the install folder, "/" separated
  expectedSize: number; actualSize: number | null;
  expectedSha256: string; actualSha256: string | null;
}

interface VerifyReport {
  gameId: number;
  status: "ok" | "issues" | "noManifest";
  filesChecked: number; bytesChecked: number;
  missing: VerifyIssue[]; corrupt: VerifyIssue[];
  extra: { path: string; size: number }[];
  checkedAt: string; message: string | null;
}

interface RepairReport {
  gameId: number;
  repaired: string[];           // restored relative paths
  failed: string[];             // damaged paths the archive could not restore
  fallback: boolean;            // true: a full reinstall is required
  message: string;
  verify: VerifyReport;         // state after the repair
}
```

| Command | Args | Returns |
|---|---|---|
| `get_settings` | — | `LocalSettings` |
| `save_settings` | `{ settings }` | `LocalSettings` |
| `pick_folder` | `{ title? }` | `string \| null` |
| `local_server_start` | — | `{ url, token }` |
| `local_server_stop` | — | `null` |
| `local_server_status` | — | `{ running, url }` |
| `install_game` | `{ gameId, versionId, serverUrl, token, title, version?, executable?, installDir?, downloadDir?, stream? }` | `InstallJob` (starts background; survives dialog close) |
| `cancel_install` | `{ jobId }` | `null` |
| `pause_install` | `{ jobId }` | `InstallJob` (`paused`; keeps the `.part`) |
| `resume_install` | `{ jobId }` | `InstallJob` (continues with HTTP Range) |
| `get_install_job` | `{ jobId }` | `InstallJob` |
| `list_install_jobs` | — | `InstallJob[]` (active + persisted, newest first) |
| `list_installed` | — | `InstalledGame[]` |
| `launch_game` | `{ gameId, executable?, args? }` | `{ sessionId }` |
| `stop_game` | `{ sessionId }` | `null` |
| `uninstall_game` | `{ gameId }` | `null` |
| `move_install` | `{ gameId, targetDir }` | `InstalledGame` (new path) |
| `verify_install` | `{ gameId }` | `VerifyReport` |
| `repair_install` | `{ gameId }` | `RepairReport` |
| `open_path` | `{ path }` | `null` |
| `get_disk_usage` | `{ path }` | `{ totalBytes, freeBytes }` |
| `list_recent_games` | `{ limit? }` | `InstalledGame[]`, newest launch first (default 8, max 50) |
| `get_game_playtime` | `{ gameId, days?, sessions? }` | `GamePlaytime` (defaults: 30 days, 10 sessions) |
| `check_game_updates` | — | `GameUpdate[]`; per-game failures are skipped |
| `update_game` | `{ gameId }` | `InstallJob`; installs the newest server version, keeps the previous install until the new files are in place |
| `check_for_updates` | `{ manual? }` | `UpdateStatus`; manual checks bypass the 24 h throttle |
| `install_update` | — | `UpdateStatus` after download+install; refuses when no signing key is configured |
| `restart_app` | — | `null` (restarts Ludex; Linux/macOS after an update) |
| `installer_plan` | `{ gameId?, executable? }` | `InstallerPlan`; never runs anything |
| `run_installer` | `{ gameId, executable?, interactive?, args? }` | `InstallerReport`; silent runs are Windows-only, interactive runs use no arguments and no timeout |
| `cancel_installer` | `{ gameId }` | `null`; stops a running installer at the next poll |
| `set_executable` | `{ gameId, executable }` | `InstalledGame`; the file must be inside the install folder |
| `choose_executable` | `{ gameId }` | `InstalledGame \| null`; system file dialog, records the pick |
| `presence_status` | — | `PresenceStatus` (never blocks on the IPC socket) |
| `steam_scan` | — | `SteamPreview`; reads Steam metadata, matches titles, marks imports |
| `steam_import` | `{ appIds }` | `SteamImportReport`; no download, launches through Steam |

Events:

- `install://progress` → `InstallJob`
- `install://done` → `InstallJob`
- `install://error` → `{ jobId, message }`
- `install://jobs` → `InstallJob[]` (whole queue after any change)
- `install://move-progress` → `{ gameId, bytesDone, bytesTotal, message }`
- `install://verify-progress` → `{ gameId, filesDone, filesTotal, bytesDone, bytesTotal, message }`
- `install://repair-progress` → same shape plus `phase: "downloading" | "extracting" | "restoring" | "verifying"`
- `game://exited` → `{ sessionId, gameId, minutesPlayed, secondsPlayed }`
- `local-server://status` → `{ running, url }`
- `ui://navigate` → `string` (tray menu asked for a route, e.g. `/downloads`)
- `ui://launch` → `number` (tray "Recent games" item; the UI calls `launch_game`)
- `update://available` → `UpdateStatus` (startup check found a version)
- `update://progress` → `{ downloaded, total, percent }`
- `update://installed` → `{ version, restarted }`
- `installer://done` → `{ gameId, success, exitCode }` (#7)

Native install layout: `{installDir}/{slug}/{versionLabel}/game files` plus a
desktop SQLite index at `{appDataDir}/ludex-desktop.db` (installed games,
sessions with exact seconds, settings, install jobs, file manifests, archive
fingerprints). Local archive extraction
uses `GET /api/games/{id}/versions/{vid}/local-path` when the embedded server is
in local mode; remote installs download with Range resume and delete the temp
archive by default (`keepArchives` setting false).

### Desktop integrations (#7, #12, #13, #14)

- **Silent installers.** `run_installer` tries `installerSilentArgs` (default
  `/S`, `/silent`, `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, `--silent`) in
  order, treats exit codes `0` and `3010` as success, kills an attempt after
  `installerTimeoutSecs` and moves on. After success it records the game binary
  it finds (never the installer). On a non-Windows host silent runs are refused;
  `installer_plan` still reports the plan. An interactive run sends no arguments
  and has no timeout. Nothing runs without an explicit command.
- **Discord presence.** Optional, off by default, uses the local Discord IPC
  (application id, no bot token). Payload: game title, public cover URL
  (`https`, non-private host), session start; idle when nothing runs.
  Reconnects with a 4 s → 120 s backoff. `presence_status` exposes
  `connected`, `lastError` and `retryInSeconds`.
- **Steam import.** Reads `steamapps/libraryfolders.vdf` and
  `appmanifest_*.acf`, keeps fully installed games whose folder exists, matches
  normalised titles (then edition-stripped titles) against the server library and
  imports selected games with `launchUri: steam://rungameid/<appid>`,
  `versionId: 0`, `source: "steam"`. Matched games keep their server `gameId`;
  unmatched ones get `-appId`. Uninstalling an import only removes the index
  entry. Playtime for negative ids is not synced to the server.
- **Playnite.** The plugin lives in `integrations/playnite/` and is a pure API
  client; there is no dedicated endpoint.

### Install lifecycle

- **Pause / resume.** Pausing stops the HTTP body read at the next chunk
  boundary, flushes and keeps the `.part` file; the job stays in the queue with
  phase `paused` (persisted across restarts). Resuming sends
  `Range: bytes=<offset>-` and continues with the same job id. Stream installs
  cannot be paused (nothing is written to disk to resume from) and
  `pause_install` returns an actionable error. Cancelling discards the `.part`.
- **Bandwidth limit.** `bandwidthLimitKib` caps the copy loop of archive
  downloads, repairs and stream installs. 0 means unlimited. The cap is read
  when a transfer starts.
- **Manifests.** At install completion the desktop hashes every installed file
  (relative path, size, SHA-256) into `install_manifests`. Hashing failures are
  logged and never fail the install; such installs report `noManifest` until
  reinstalled. `verify_install` compares sizes first and hashes only matching
  files; extras are reported but do not count as damage. `repair_install`
  downloads the version archive (Range resume, or the local library file),
  extracts it to a temporary folder and copies back only the missing/corrupt
  files after checking their hashes. When no archive can be obtained it returns
  `fallback: true` with a message asking for a full reinstall.
- **Game updates (#30).** `check_game_updates` compares each installed game with
  `GET /api/games/{id}/versions/latest`: a different `versionId` is a
  `newVersion`, an identical id with a different `archiveHash` is a
  `replacedArchive`. `update_game` reuses the install pipeline into staging; the
  previous folder is untouched until the new install is in place and indexed,
  then removed. Saves live outside the install folder and are never modified.
- **Playtime sessions (#26).** A session is opened at launch and closed when the
  whole process tree is empty (polled once per second), not only when the direct
  child exits. `sessions` stores `started_at`, `ended_at`, `minutes` (rounded,
  for the server) and `seconds` (exact, for local reports). `get_game_playtime`
  aggregates totals, per-UTC-day buckets and the recent sessions.
- **Tray (#15).** `closeToTray` (default true) hides the window instead of
  quitting; the tray menu offers Show/Hide, Downloads, Recent games (latest
  launch, else import/install date), Check for updates and Quit. Left click
  toggles the window and the tooltip reports active downloads.
- **App updates (#16).** `stable` follows the latest GitHub release, `early`
  follows the moving `edge` prerelease. Checks run at startup at most once per
  24 h plus on demand. Install requires a configured signing key; without one the
  status reports `configured: false` and `install_update` refuses explicitly.
- **Move.** `move_install` takes a new install root and keeps the
  `{slug}/{versionLabel}` layout. Same filesystem: an atomic rename. Different
  filesystem: copy to a hidden sibling, verify the byte count, rename into
  place, then delete the old folder; any failure leaves the original intact.
  It refuses while the game runs or an install for it is active, and updates
  the index, the executable path and the manifest path.

## Embedded server Rust API (used by the desktop app)

```rust
pub struct Config { /* env/CLI fields + builder setters */ }
impl Config {
    pub fn from_env() -> anyhow::Result<Self>;
    pub fn for_local(games_dir: PathBuf, data_dir: PathBuf, media_dir: PathBuf) -> Self;
}

pub struct ServerHandle {
    pub url: String,        // http://127.0.0.1:<port>
    pub token: String,      // JWT for the auto-provisioned local admin
    pub shutdown: tokio::sync::oneshot::Sender<()>,
}

pub async fn run(config: Config) -> anyhow::Result<()>;
pub async fn start(config: Config) -> anyhow::Result<ServerHandle>;
```

`start` binds, migrates, bootstraps the local admin when `config.local_mode`,
spawns the HTTP server, and returns once the listener is ready. Dropping/sending
on `shutdown` stops it gracefully.

## Stream install (ZIP)

`GET /api/games/{id}/versions/{versionId}/stream` returns an uncompressed tar
stream of the ZIP entries. It exists so the desktop can install a remote ZIP
without storing the archive: the client writes extracted files directly from
the tar stream. Only ZIP is supported in v0.2; non-ZIP returns 415 and the
client falls back to the normal download+extract path. `Content-Length` is set
to the exact tar size so the desktop can show a percentage.

Core API:

```rust
pub fn zip_tar_size(path: &Path) -> Result<u64, ArchiveError>;
pub fn stream_zip_as_tar<W: std::io::Write>(
    path: &Path, writer: &mut W,
) -> Result<TarStreamReport, ArchiveError>;
pub struct TarStreamReport { pub files: u64, pub bytes: u64, pub tar_bytes: u64 }
```

Tar entries must use sanitized relative paths, reject traversal/symlinks, keep
file modes when available, and never load a whole entry into memory.

## Shared Rust core API

`ludex-core` exposes (server + desktop depend on it):

```rust
pub fn parse_filename(name: &str) -> ParsedName;
pub fn detect_format(path: &Path) -> io::Result<ArchiveFormat>;
pub fn verify_archive(path: &Path, password: Option<&str>) -> Result<ArchiveReport>;
pub fn extract_archive(path: &Path, dest: &Path, opts: &ExtractOptions,
                       progress: &mut dyn FnMut(ExtractProgress)) -> Result<ExtractReport>;
pub fn pick_executable(root: &Path, title: &str) -> Option<PathBuf>;
pub fn slugify(s: &str) -> String;
```

Archive support: zip, rar (RAR4/RAR5), 7z. Extraction must be streaming, must
reject absolute paths and `..` entries, and must support a password list.
