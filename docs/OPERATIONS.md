# Operations: metrics and backups

Day-two operations for a self-hosted Ludex server: scraping Prometheus metrics
and running backups/restores with the `ludex-backup` CLI.

Deployment, reverse proxy and permissions live in [`DEPLOY.md`](DEPLOY.md);
pull-based updates live in [`GITOPS.md`](GITOPS.md).

## Metrics (#48)

`GET /metrics` serves the Prometheus text exposition format
(`text/plain; version=0.0.4`). It is not under `/api` and is never public.

### Authentication

| Configuration | Accepted credentials |
|---|---|
| `LUDEX_METRICS_TOKEN` set | `Authorization: Bearer <token>` (constant-time compared) |
| unset (default) | an administrator session JWT or an `ldx_` API key with the `admin` scope |

Prometheus cannot log in, so set a dedicated scrape token:

```bash
# .env / compose environment
LUDEX_METRICS_TOKEN=$(openssl rand -hex 32)
```

Rotating the token only requires a restart; no other token is affected.

### Series

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `ludex_build_info` | gauge | `version` | Always 1; the server version |
| `ludex_uptime_seconds` | gauge | | Process uptime |
| `ludex_http_requests_total` | counter | `method`, `route`, `status` | Requests by route template |
| `ludex_http_request_duration_seconds` | histogram | `route` | Latency, 5 ms → 10 s buckets |
| `ludex_active_downloads` | gauge | | Archive downloads streaming right now |
| `ludex_games_total` | gauge | | Indexed games |
| `ludex_game_versions_total` | gauge | | Versions that are not missing |
| `ludex_users_total` | gauge | | Accounts |
| `ludex_ingest_jobs` | gauge | `state` | `queued`, `downloading`, `verifying`, `done`, `error`, `cancelled` |
| `ludex_media_cache_files` | gauge | | Cached artwork (local backend only) |
| `ludex_media_cache_bytes` | gauge | | Cache size (local backend only) |
| `ludex_media_backend_info` | gauge | `backend` | Always 1; `local` or `s3` |
| `ludex_scans_total` | counter | | Library scans started by this process |
| `ludex_scan_duration_seconds` | gauge | | Duration of the last scan |
| `ludex_webhook_deliveries_total` | counter | `outcome` | `delivered`, `retry`, `dropped` |
| `ludex_db_pool_connections` | gauge | `state` | `open`, `idle` |

Route labels use the matched template (for example `/api/games/{id}`), not the
raw path, so cardinality stays bounded. Static web assets share `route="unmatched"`.

### Example scrape configuration

```yaml
scrape_configs:
  - job_name: ludex
    metrics_path: /metrics
    scheme: https
    bearer_token_file: /etc/prometheus/ludex-metrics-token
    static_configs:
      - targets: ["ludex.example.com"]
```

Inside the same Docker network, scrape `http://ludex:8080/metrics` directly and
keep the token in a file readable by the Prometheus user (mode `0400`).

### Useful queries

```promql
# Requests per minute by route
sum by (route) (rate(ludex_http_requests_total[5m]))

# p95 latency
histogram_quantile(0.95, sum by (le, route) (rate(ludex_http_request_duration_seconds_bucket[5m])))

# 5xx rate
sum(rate(ludex_http_requests_total{status=~"5.."}[5m])) / sum(rate(ludex_http_requests_total[5m]))

# Ingest backlog
sum(ludex_ingest_jobs{state=~"queued|downloading|verifying"})
```

## Backup and restore (#49)

`ludex-backup` produces a single deterministic `tar.gz`:

```text
manifest.json        format version, Ludex version, createdAt, size + SHA-256 per entry
data/ludex.db        consistent snapshot (`VACUUM INTO`, WAL-safe)
data/<other files>   jwt.secret, ingest state, ...
media/<files>        cached artwork (when --media-dir is given)
saves/<files>        optional on-disk save exports
```

Cloud saves are stored in SQLite, so they are always covered by the database
snapshot; `--saves-dir` exists for operators who also keep exported saves on disk.

The archive is written with sorted entries, zeroed ownership, `mtime = 0` and a
timestamp-free gzip header: the same content produces byte-identical archives
apart from `createdAt`.

### Build

The server image and release archives currently ship only `ludex-server`.
Build the CLI from the same source tree:

```bash
cargo build --release -p ludex-server --bin ludex-backup
install -m 0755 target/release/ludex-backup /usr/local/bin/ludex-backup
```

Defaults: `LUDEX_DATA_DIR=./data`, `LUDEX_MEDIA_DIR=./media`,
`LUDEX_BACKUP_OUT=./backups`.

### Back up

```bash
# Generated name in a directory (ludex-backup-<timestamp>.tar.gz)
ludex-backup backup --data-dir /srv/ludex/data --media-dir /srv/ludex/media \
                    --out /srv/backups --keep 7

# Explicit archive path; --media-dir is included, --saves-dir is optional
ludex-backup backup --data-dir /srv/ludex/data \
                    --out /srv/backups/manual.tar.gz

# Upload to the configured S3 backend (streamed multipart, no memory spike)
ludex-backup backup --data-dir /srv/ludex/data --media-dir /srv/ludex/media \
                    --out /srv/backups --s3
```

The server does **not** need to be stopped: the database snapshot is taken with
`VACUUM INTO` from a second connection, which is safe with WAL.

`--keep N` only applies when `--out` is a directory, and only deletes files
matching `ludex-backup-*.tar.gz` (oldest first).

### Scheduled backups (systemd)

```ini
# /etc/systemd/system/ludex-backup.service
[Service]
Type=oneshot
User=ludex
EnvironmentFile=/etc/ludex/backup.env
ExecStart=/usr/local/bin/ludex-backup backup \
  --data-dir /srv/ludex/data --media-dir /srv/ludex/media \
  --out /srv/backups --keep 7
```

```ini
# /etc/systemd/system/ludex-backup.timer
[Timer]
OnCalendar=*-*-* 03:30:00
Persistent=true

[Install]
WantedBy=timers.target
```

`systemctl enable --now ludex-backup.timer`. Keep `/etc/ludex/backup.env` mode
`0600` when it holds S3 credentials.

### Verify

```bash
tar -tzf /srv/backups/ludex-backup-2026-01-01T03-30-00Z.tar.gz | head
ludex-backup restore --archive /srv/backups/ludex-backup-....tar.gz \
                     --data-dir /srv/ludex/data --dry-run
```

A dry run validates `manifest.json`, every size and every SHA-256 without
writing anything. A corrupted or truncated archive is refused before any file
is extracted.

### Restore

1. Stop the server. `restore` takes the same `flock` the server holds on
   `data/ludex.lock` and refuses to run while an instance is alive.
2. Restore:

   ```bash
   systemctl stop ludex
   ludex-backup restore --archive /srv/backups/ludex-backup-....tar.gz \
                        --data-dir /srv/ludex/data \
                        --media-dir /srv/ludex/media \
                        --force
   systemctl start ludex
   ```

3. The previous directories are kept next to the new ones as
   `<dir>.pre-restore-<timestamp>`; delete them once the instance looks healthy.

Notes:

- `--force` is required when `data/ludex.db` already exists.
- If the archive contains `media/` or `saves/` entries, the matching
  `--media-dir` / `--saves-dir` flag is mandatory: restore never guesses where
  to write those files.
- Extraction happens next to each target and the swap is a `rename`, so a
  failed restore leaves the previous directories in place.
- Every archive entry is path-checked (no absolute paths, no `..`, no symlinks).

## Disaster recovery checklist

1. Reinstall the server (see `DEPLOY.md`) and stop it.
2. Restore the last archive with `--force`.
3. Start the server, check `GET /api/health` and log in with a known account.
4. Rescan the library if the volumes moved: `POST /api/library/scan`.
5. Point `LUDEX_STORAGE_BACKEND` back at the same bucket (media) and verify a
   cover loads.
6. Keep the pre-restore directories until the next successful backup.
