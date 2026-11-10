# Library operations: roots, watcher, integrity and media GC

This document covers the four server-side library operations added in the
same release:

- [Multiple library roots](#multiple-library-roots-45)
- [Folder watcher](#folder-watcher-44)
- [Integrity checks](#integrity-checks-43)
- [Media cache garbage collection](#media-cache-garbage-collection-18)

Everything is server-side; the web UI and the desktop app keep working against
the unchanged `/api/games` contract. All new endpoints are administrator-only
and require the `admin` scope for API keys, like the other admin routes.

## Multiple library roots (#45)

### Configuration

| Variable | Default | Meaning |
|---|---|---|
| `LUDEX_GAMES_DIR` | `./games` | Primary root. Always root #1 and the ingest target. |
| `LUDEX_LIBRARY_ROOTS` | empty | Extra roots, comma-separated. Order is preserved. |

`LUDEX_GAMES_DIR` is always present and always first, even when
`LUDEX_LIBRARY_ROOTS` lists it later: new downloads land in a known place.
Duplicates are removed. Paths are used as given (`/games`, `/mnt/nas/games`,
`./games`); API-added roots must be absolute.

```bash
LUDEX_GAMES_DIR=/games
LUDEX_LIBRARY_ROOTS=/mnt/nas/games,/mnt/usb/retro
```

On startup the server registers every configured root in `library_roots`
(label, path, enabled, position). **Extra roots are never created on disk**: a
missing path means "not mounted" and stays detectable by the scanner. Only
`LUDEX_GAMES_DIR` is created if missing, as before.

The previous single-directory install is upgraded transparently: migration
`0006_library_ops.sql` seeds root #1 with an empty path and the server fills it
with `LUDEX_GAMES_DIR` on the next start. Existing `game_versions` rows are
assigned to root #1 exactly once (setting `library_roots.backfilled`).

### Data model

- `library_roots` — `id`, `label`, `path`, `enabled`, `available`, `position`,
  `created_at`, `last_scan_at`, `last_error`.
- `game_versions.root_id` — the root holding the file. Unique per
  `(root_id, file_path)`: two roots may contain the same relative path.
- `game_versions.root_unavailable` — set when the root was unavailable during
  the last scan. Such versions are **not** marked `missing`, so an unplugged
  disk never looks like a deleted game.
- `games.root_id` — the root where the game was first seen (informational).

`file_path` remains relative to its root, so the API shape is unchanged.

### Scanning behaviour

A scan walks every enabled root in `position` order. For each root:

- The path is checked in a blocking task. If it is missing or not a directory,
  the root is marked `available = 0` with a `lastError`, its versions get
  `root_unavailable = 1`, and nothing else is touched. Games stay visible and
  downloads return `404 root_unavailable` instead of `file_not_found`.
- Otherwise candidates are discovered with the same rules as before (max depth
  3, hidden and redist directories skipped, `.part`/`.tmp` skipped, archives
  ≥ 1 MiB and portable `.exe`).
- Versions of scanned, available roots that vanished are marked `missing = 1`.
  A single-root scan only considers that root.
- Mirrored archives are indexed once. Deduplication hashes candidates that
  share a file name and size across different roots and keeps the first root in
  order; hashing every archive on every scan would be too expensive on a
  multi-terabyte library. The skipped mirror is still considered "present" so
  an existing row is not marked missing.

Only one scan runs at a time (`AppState::scan` lock). Targeted scans merge into
a full scan when two different roots are queued, so no root is skipped.

### Endpoints

| Method | Route | Notes |
|---|---|---|
| GET | `/api/library/roots` | admin + `admin` scope; ordered list with `versionCount` |
| POST | `/api/library/roots` | `{ path, label?, enabled? }` → 201 `LibraryRoot` |
| PATCH | `/api/library/roots/{id}` | `{ label?, path?, enabled?, position? }` → `LibraryRoot` |
| DELETE | `/api/library/roots/{id}?force=true` | 409 `root_not_empty` unless `force=true`; 204 |
| POST | `/api/library/roots/{id}/scan` | admin/editor; 202 `{ started, rootId }` |

```ts
interface LibraryRoot {
  id: number; label: string; path: string;
  enabled: boolean; available: boolean; position: number;
  createdAt: string; lastScanAt: string | null; lastError: string | null;
  versionCount: number;
}
```

Deleting a root with `force=true` keeps every game and version row (progress,
bookmarks and saves survive), marks its versions `missing = 1` and clears their
`root_id` through the foreign key. Re-running the server never reattributes
them to another root.

## Folder watcher (#44)

| Variable | Default | Meaning |
|---|---|---|
| `LUDEX_WATCHER_ENABLED` | `false` | Watch enabled roots for changes. |
| `LUDEX_WATCHER_DEBOUNCE_MS` | `5000` | Idle window used to coalesce events. |

When enabled, the server watches every enabled root recursively and triggers
the normal scan pipeline after a quiet window. Root changes made through the
API (add, rename, move, enable, disable, delete) are applied at runtime without
a restart.

Ignored paths (no scan triggered): hidden files and directories, `.ludex-tmp`,
`*.part`, `*.tmp`, `*.download`, `*.crdownload`, `~$*`. The scanner never writes
inside a root, so watcher-triggered scans cannot loop; ingest writes `.part`
files (ignored) and renames them into place (one event, coalesced).

`GET /api/library/watcher` (admin) returns:

```ts
interface WatcherStatus {
  enabled: boolean; debounceMs: number;
  roots: { id: number; label: string; path: string }[];
  lastEventAt: string | null; lastScanAt: string | null;
  pending: boolean; lastError: string | null;
}
```

`lastScanAt` is the time a scan was queued by the watcher; `lastEventAt` is the
last relevant filesystem event. While a scan is already running the trigger is
retried after the debounce window instead of being dropped.

Platform limits: Linux uses inotify, macOS FSEvents/kqueue, Windows
`ReadDirectoryChangesW`. NFS, CIFS/SMB and some FUSE mounts do not emit change
events; on those the periodic scan (`LUDEX_SCAN_INTERVAL_SECONDS`) is the only
refresh path. Very large trees can exhaust `fs.inotify.max_user_watches`; the
error is reported in `lastError`. In containers, the watched paths must be
bind-mounted, not just copied into the image.

The watcher is never started in tests or desktop local mode unless
`watcher_enabled` is set.

## Integrity checks (#43)

| Variable | Default | Meaning |
|---|---|---|
| `LUDEX_INTEGRITY_INTERVAL_HOURS` | `168` | Scheduled interval. `0` disables the schedule. |

A run verifies:

- **Archives** (`game_versions` where `missing = 0` and
  `root_unavailable = 0`): the file exists, is a regular file, its size matches
  the indexed size, and its SHA-256 matches `content_hash`. The first
  successful hash is stored as the baseline, so later runs are cheap and still
  detect truncation and bit rot.
- **Media** (`games.cover_path`, `games.background_path`,
  `games.screenshots_json`, `users.avatar_path`): the file exists and its
  SHA-256 matches `media_manifest`. The first hash becomes the baseline.

Findings are recorded, never acted on: integrity checks **do not delete or mark
anything**. A missing file does not set `game_versions.missing`; that remains
the scanner's job.

Only one run is active per process (`409 integrity_running`). The job runs in
the background; poll the report for progress. `checked` is written every 25
files.

| Method | Route | Notes |
|---|---|---|
| POST | `/api/integrity/check` | `{ scope?: "all" \| "archives" \| "media" }` → 202 `{ reportId, status }` |
| GET | `/api/integrity/reports?limit=20` | admin; newest first |
| GET | `/api/integrity/reports/{id}` | `{ report, findings }`; 404 `report_not_found` |

```ts
interface IntegrityReport {
  id: number; scope: "all" | "archives" | "media";
  status: "running" | "completed" | "failed";
  checked: number; findingsCount: number; error: string | null;
  startedAt: string; finishedAt: string | null;
}
interface IntegrityFinding {
  id: number; kind: "missing" | "corrupt" | "size_mismatch";
  targetType: "archive" | "media"; targetId: string | null;
  path: string | null; detail: string | null; createdAt: string;
}
```

A corrupt archive is reported but stays downloadable until an administrator
acts; the scanner only reclassifies files that actually disappear.

## Media cache garbage collection (#18)

| Variable | Default | Meaning |
|---|---|---|
| `LUDEX_MEDIA_GC_GRACE_HOURS` | `24` | Minimum age before an unreferenced file is deleted. |
| `LUDEX_MEDIA_GC_INTERVAL_HOURS` | `0` | Scheduled run interval. `0` disables it. |

The collector builds the set of media names referenced by the database
(`cover_path`, `background_path`, `screenshots_json`, `avatar_path`), lists the
media directory and deletes only files that are:

1. not referenced by any row, **and**
2. older than the grace period (modification time; unknown age is never
   deleted).

`POST /api/media/gc?dryRun=true|false` (admin) defaults to a dry run. The
response reports `scanned`, `referenced`, `orphanFiles`, `orphanBytes`,
`deletedFiles`, `deletedBytes` and up to 100 candidate names. A real run also
deletes the matching `media_manifest` rows. Runs are recorded in
`media_gc_runs`. Scheduled runs are skipped while a scan is running.

Safety: the grace period protects a file written by an enrichment pass that has
not committed its database row yet. A file that is referenced is never deleted,
whatever its age. Two concurrent runs are rejected with `409 media_gc_running`.

## Operational checklist

```bash
# Add a NAS root, scan it only, then check the root list.
curl -fsS -X POST https://ludex.example.com/api/library/roots \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"label":"NAS","path":"/mnt/nas/games"}'
curl -fsS -X POST https://ludex.example.com/api/library/roots/2/scan \
  -H "Authorization: Bearer $TOKEN"

# Dry-run the media collector before deleting anything.
curl -fsS -X POST 'https://ludex.example.com/api/media/gc?dryRun=true' \
  -H "Authorization: Bearer $TOKEN"

# Start an integrity run and follow it.
curl -fsS -X POST https://ludex.example.com/api/integrity/check \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"scope":"archives"}'
```

Docker note: add one volume per root and list them in `LUDEX_LIBRARY_ROOTS`
(`/games` stays the primary root). A root mounted read-only is fine; only the
ingest target is written to.
