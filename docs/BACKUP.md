# Backup and restore (#49)

`ludex-backup` creates a consistent snapshot of the server state and restores
it on the same or another machine. Game archives are **not** included: they are
large and already live on the library roots or in the S3 bucket.

## What a backup contains

```text
ludex-backup-20261110-120000.tar.gz
├── backup.json      # format version, counters, every file + SHA-256
├── config.json      # non-secret server configuration
├── ludex.db         # consistent SQLite snapshot (VACUUM INTO)
├── media/           # cached artwork (optional, --no-media)
└── saves/           # cloud save snapshots
```

- `backup.json` is the trust anchor: restore verifies the size and SHA-256 of
  every listed file and rejects unexpected extra files.
- `config.json` is informational. It never contains `LUDEX_JWT_SECRET`,
  `LUDEX_ADMIN_PASSWORD`, IGDB secrets, OIDC secrets or S3 credentials. Restore
  does not apply it: the environment stays the source of truth.
- The database is copied with `VACUUM INTO`, which reads a consistent snapshot
  even while WAL writers are active. `PRAGMA integrity_check` is expected to
  return `ok` on the restored copy (covered by a concurrent-write test).
- `jwt.secret` is intentionally not archived: restoring invalidates every
  session, and a leaked backup would otherwise leak the signing key.
- Game archives stay on disk/S3. After a restore, run a scan so the database
  and the library roots agree again.

## Usage

The binary is built as part of the workspace:

```bash
cargo build --release -p ludex-server --bin ludex-backup
```

In the Docker image it is available as `/usr/local/bin/ludex-backup`:

```bash
docker compose exec ludex ludex-backup backup --output /data/backups/manual.tar.gz
```

### Create

```bash
# Uses LUDEX_DATA_DIR / LUDEX_MEDIA_DIR / LUDEX_DATABASE_URL from the environment.
ludex-backup backup

# Explicit destination.
ludex-backup backup --output /srv/backups/ludex-$(date +%F).tar.gz

# Validate and measure without writing the archive.
ludex-backup backup --dry-run

# Skip the media cache (smaller archive, it is rebuildable).
ludex-backup backup --no-media

# Upload with the configured S3 storage (requires LUDEX_STORAGE_BACKEND=s3).
ludex-backup backup --upload-s3
```

The default destination is
`{LUDEX_DATA_DIR}/backups/ludex-backup-<YYYYmmdd-HHMMSS>.tar.gz`. Set
`LUDEX_BACKUP_DIR` to keep archives on a separate volume; the admin UI
(Admin → Backups) reads and writes the same directory. The report is
printed as JSON on stdout: path, bytes, file count, creation time and the
uploaded key when `--upload-s3` succeeded.

`backup --dry-run` still opens the database and builds the staging tree (it must
verify what it would archive) but does not write the `.tar.gz`.

### Verify

```bash
ludex-backup verify /srv/backups/ludex-2026-11-10.tar.gz
```

This extracts to a temporary directory, checks every checksum and prints the
manifest. It never touches the data directory.

### Restore

Stop the server first. Restoring a database under a live process is not
supported.

```bash
# Inspect what would happen.
ludex-backup restore /srv/backups/ludex-2026-11-10.tar.gz --dry-run

# Restore into the configured data/media directories.
systemctl stop ludex.service
ludex-backup restore /srv/backups/ludex-2026-11-10.tar.gz
systemctl start ludex.service
```

Restore refuses to write when `{DATA}/ludex.db`, `{DATA}/saves/` or the media
directory already contain data, unless `--force` is passed. With `--force` the
database is replaced, media/saves files from the archive overwrite their
counterparts, and files that are not in the archive are left in place.

`--dry-run` extracts to a temporary directory, verifies all checksums and reports
the counters without writing anything. It still fails on a non-empty destination
without `--force`, exactly like a real restore.
`--no-media` skips restoring the media cache (the database still references the
files; they will be re-downloaded by the next metadata pass or restored from a
previous backup).

Restored files are written to:

| Archive entry | Destination |
|---|---|
| `ludex.db` | `{LUDEX_DATA_DIR}/ludex.db` (mode 0600) |
| `saves/**` | `{LUDEX_DATA_DIR}/saves/**` |
| `media/**` | `{LUDEX_MEDIA_DIR}/**` |
| `backup.json` | `{LUDEX_DATA_DIR}/last-restore.json` (informational) |
| `config.json` | `{LUDEX_DATA_DIR}/last-restore-config.json` (informational) |

Stale `ludex.db-wal` / `ludex.db-shm` files are removed before the database is
swapped in.

### Overrides

Every command accepts the usual overrides, useful for restoring on another
host:

```bash
ludex-backup restore backup.tar.gz \
  --data-dir /srv/ludex/data \
  --media-dir /srv/ludex/media \
  --database-url "sqlite:///srv/ludex/data/ludex.db?mode=rwc"
```

`--data-dir` also resets the database URL to `{data-dir}/ludex.db`.

## Scheduling

Example systemd timer (run as the `ludex` user):

```ini
# /etc/systemd/system/ludex-backup.service
[Unit]
Description=Ludex nightly backup
After=ludex.service

[Service]
Type=oneshot
User=ludex
EnvironmentFile=/etc/ludex/backup.env
ExecStart=/usr/local/bin/ludex-backup backup --output /srv/backups/ludex-nightly.tar.gz
```

```ini
# /etc/systemd/system/ludex-backup.timer
[Unit]
Description=Nightly Ludex backup

[Timer]
OnCalendar=*-*-* 03:30:00
Persistent=true

[Install]
WantedBy=timers.target
```

```bash
sudo systemctl enable --now ludex-backup.timer
```

The `--output` path is used literally (no strftime expansion); create dated
names with a wrapper script if you need history.

Keep at least one off-host copy (`rclone`, `restic`, or `--upload-s3`). A
backup on the same disk as the database is not a backup.

## Size and performance

- The archive contains the database, media cache and cloud saves. The media
  cache dominates: 18 games with 4K backgrounds produced roughly 300 media
  files in the homelab, but libraries with thousands of games can reach several
  gigabytes.
- Staging uses temporary space under `LUDEX_DATA_DIR` (or the system temp
  directory if the data directory does not exist). Plan for roughly the size of
  the uncompressed snapshot in addition to the final `.tar.gz`.
- Compression is gzip level 6 (default). The database compresses well; JPEG and
  WebP artwork does not.
- `VACUUM INTO` needs a read lock; concurrent writers wait briefly. SQLite's
  busy timeout is 30 s in the backup connection.

## Restore checklist

1. Stop `ludex.service` (or the container).
2. `ludex-backup restore … --dry-run` and check the counters.
3. Restore with `--force` if the data directory is not empty.
4. Confirm the library roots and `LUDEX_STORAGE_BACKEND` match the backed-up
   configuration (`config.json`).
5. Start the server, log in (sessions were invalidated), run a library scan and
   check `/api/health/details`.
6. Keep the old `ludex.db` until the restored instance has been verified.

## Tests

`crates/ludex-server/tests/backup.rs` covers:

- round-trip backup → verify → restore into a fresh directory;
- checksum tampering rejection;
- dry-run restore writing nothing;
- refusing a non-empty destination without `--force` and accepting `--force`;
- snapshot consistency under concurrent inserts (`PRAGMA integrity_check` on the
  archived database);
- `--upload-s3` to an in-memory object store.
