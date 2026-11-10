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
  downloadUrl: string; // /api/games/{id}/download?versionId={id}
}

interface GameDetail extends GameSummary {
  description: string | null;
  screenshots: string[]; // media URLs
  trailers: { title: string; url: string; thumbnailUrl: string | null }[];
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
| GET | `/api/media/{name}` | cached image/file (public, immutable cache headers) |
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
| GET | `/api/games/{id}/versions/{versionId}/local-path` | embedded/local mode + loopback only → `{ path }`, else 404 |

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
| GET | `/api/stats` | — | `{ games, versions, totalBytes, users, byState: Record<ProgressState, number> }` |

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
| POST | `/api/library/scan` | admin/editor; 202 `{ started: boolean }` |
| GET | `/api/library/scan` | `ScanStatus` |
| GET | `/api/library/folders` | `{ gamesDir, mediaDir, dataDir }` (admin) |

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
}

interface InstalledGame {
  gameId: number; versionId: number; title: string; version: string | null;
  installPath: string; executable: string | null; sizeBytes: number;
  installedAt: string; lastLaunchedAt: string | null;
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

Events:

- `install://progress` → `InstallJob`
- `install://done` → `InstallJob`
- `install://error` → `{ jobId, message }`
- `install://jobs` → `InstallJob[]` (whole queue after any change)
- `install://move-progress` → `{ gameId, bytesDone, bytesTotal, message }`
- `install://verify-progress` → `{ gameId, filesDone, filesTotal, bytesDone, bytesTotal, message }`
- `install://repair-progress` → same shape plus `phase: "downloading" | "extracting" | "restoring" | "verifying"`
- `game://exited` → `{ sessionId, gameId, minutesPlayed }`
- `local-server://status` → `{ running, url }`

Native install layout: `{installDir}/{slug}/{versionLabel}/game files` plus a
desktop SQLite index at `{appDataDir}/ludex-desktop.db` (installed games,
sessions, settings, install jobs, file manifests). Local archive extraction
uses `GET /api/games/{id}/versions/{vid}/local-path` when the embedded server is
in local mode; remote installs download with Range resume and delete the temp
archive by default (`keepArchives` setting false).

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
