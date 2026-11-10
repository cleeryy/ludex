# Ludex API + desktop contract (v0.1)

Single source of truth for the server, web UI and Tauri shell. All JSON is
`camelCase`. All routes are under `/api` unless stated. Auth is
`Authorization: Bearer <jwt>` except public routes.

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
  lastPlayedAt: string | null;
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
| GET | `/api/games` | query: `q`, `genre`, `state`, `bookmarked`, `earlyAccess`, `sort` (`title\|releaseDate\|rating\|addedAt\|lastPlayed`), `order` (`asc\|desc`), `page` (1), `limit` (48, max 200) → `{ items, total, page, limit }` |
| GET | `/api/games/{id}` | `GameDetail` |
| GET | `/api/games/random` | same filters as list → `GameSummary`; 404 when empty |
| PATCH | `/api/games/{id}` | editor; editable metadata fields + `launch` object; returns `GameDetail` |
| POST | `/api/games/{id}/refresh-metadata` | editor; 202 `{ queued: true }` |
| POST | `/api/games/{id}/download-ticket` | `{ versionId? }` → `{ url, expiresAt }` (15 min, no auth needed on the URL) |
| GET | `/api/games/{id}/download?versionId=&token=` | file stream, Range supported, `Content-Disposition` |
| GET | `/api/games/{id}/versions/{versionId}/stream?token=` | Stream install: an uncompressed `application/x-tar` of the archive entries, no archive stored on the client. ZIP and 7z are supported; RAR/unknown return 415 and the client falls back. Exact `Content-Length`. Same auth rules as download. |
| GET | `/api/games/{id}/versions/{versionId}/local-path` | embedded/local mode + loopback only → `{ path }`, else 404 |

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
}

interface InstalledGame {
  gameId: number; versionId: number; title: string; version: string | null;
  installPath: string; executable: string | null; sizeBytes: number;
  installedAt: string; lastLaunchedAt: string | null;
}

interface InstallJob {
  jobId: string; gameId: number; versionId: number;
  phase: "queued" | "downloading" | "extracting" | "finalizing" | "done" | "cancelled" | "error";
  bytesDone: number; bytesTotal: number | null; speedBps: number;
  message: string | null; installPath: string | null;
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
| `get_install_job` | `{ jobId }` | `InstallJob` |
| `list_install_jobs` | — | `InstallJob[]` (active + persisted, newest first) |
| `list_installed` | — | `InstalledGame[]` |
| `launch_game` | `{ gameId, executable?, args? }` | `{ sessionId }` |
| `stop_game` | `{ sessionId }` | `null` |
| `uninstall_game` | `{ gameId }` | `null` |
| `open_path` | `{ path }` | `null` |
| `get_disk_usage` | `{ path }` | `{ totalBytes, freeBytes }` |

Events:

- `install://progress` → `InstallJob`
- `install://done` → `InstallJob`
- `install://error` → `{ jobId, message }`
- `game://exited` → `{ sessionId, gameId, minutesPlayed }`
- `local-server://status` → `{ running, url }`

Native install layout: `{installDir}/{slug}/{versionLabel}/game files` plus a
desktop SQLite index at `{appDataDir}/ludex-desktop.db` (installed games,
sessions, settings). Local archive extraction uses
`GET /api/games/{id}/versions/{vid}/local-path` when the embedded server is in
local mode; remote installs download with Range resume and delete the temp
archive by default (`keepArchives` setting false).

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
