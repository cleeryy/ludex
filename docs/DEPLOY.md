# Deploying Ludex

Ludex is one process and three directories:

| Path | Purpose | Backup? |
|---|---|---|
| `/data` | SQLite database, generated JWT secret | **Yes** |
| `/games` | Your archives (`.zip`, `.rar`, `.7z`, portable `.exe`) | **Yes** (your files) |
| `/media` | Cached covers, backgrounds, screenshots | No, rebuildable |

The container listens on `8080`, runs as uid/gid `1000`, and writes only to those
three directories (plus `/tmp`).

## Requirements

- Docker Engine 24+ with Compose v2, or a Linux host with glibc 2.39+ for the
  GNU binary (Ubuntu 24.04+, Debian 13+). On older distributions, use the Docker
  image; it is built on Debian bookworm.
- A reverse proxy for TLS if the server is reachable from the internet.
- Optional: IGDB credentials (see below) for richer metadata.
- Optional: PostgreSQL 14+ instead of the default SQLite file (see
  [POSTGRES.md](POSTGRES.md)).

## Docker Compose (recommended)

```bash
git clone https://github.com/cleeryy/ludex.git
cd ludex
mkdir -p games media data
sudo chown -R 1000:1000 games media data
cp .env.example .env
# edit .env: server name, optional IGDB credentials
docker compose up -d
```

Open `http://<host>:8080` and create the first admin account. Setup is only
available while zero users exist.

Pin a version in production instead of tracking `latest`:

```yaml
# compose.yaml
services:
  ludex:
    image: ghcr.io/cleeryy/ludex:v0.2.0
```

`docker compose up -d` uses the local image if present and pulls it otherwise.
`docker compose up -d --build` builds from the checkout instead.

### Ports

The Compose file publishes `8080:8080`. To use another host port, change the
left side:

```yaml
ports:
  - "127.0.0.1:8081:8080"
```

Binding to `127.0.0.1` keeps the unencrypted port off the LAN; put Caddy/nginx
in front. The container itself always listens on `0.0.0.0:8080`.

### Permissions

The image runs as `1000:1000`, and `compose.yaml` repeats it explicitly. Bind
mounts are not remapped by Docker, so the host directories must be writable by
that uid:

```bash
sudo chown -R 1000:1000 games media data
```

If your host user has another uid, either change ownership to that uid and set
`user: "<uid>:<gid>"` in `compose.yaml`, or run the container as your user. Do
not run it as root.

`/games` can be mounted read-only (`./games:/games:ro`) if you only scan and
download existing archives. Mount it read-write when you use the ingest
downloader, which writes `.part` files and renames completed archives.

## Reverse proxy and TLS

Caddy obtains and renews certificates automatically:

```caddyfile
ludex.example.com {
    encode zstd gzip

    reverse_proxy 127.0.0.1:8080 {
        # Do not buffer downloads: archives are large and Range requests
        # should reach the server immediately.
        flush_interval -1
        # A large transfer may pause between chunks; a proxy write deadline
        # shorter than the client's stall timeout (30 s default) would kill a
        # healthy download. 0 disables it; see docs/PERFORMANCE.md.
        read_timeout 0
        write_timeout 0
    }
}
```

If Caddy runs in another container, proxy to the service name
(`ludex:8080`) on a shared Docker network instead of `127.0.0.1`.

nginx equivalent (streaming-friendly):

```nginx
server {
    listen 443 ssl http2;
    server_name ludex.example.com;

    client_max_body_size 10m;          # JSON API bodies; archives are fetched by URL ingest
    proxy_request_buffering off;
    proxy_buffering off;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 1h;
    }
}
```

Notes:

- **Cloudflare proxying** may buffer large responses. For multi-gigabyte
  archives, use a DNS-only record or bypass the proxy for the download path.
- Forward `X-Forwarded-Proto` so generated ticket URLs use `https`.
- Do not add HTTP Basic auth in front of the API: the desktop app and download
  tickets use the API directly. Use Ludex accounts.

## Binary deployment (Linux)

Download the release archive and checksum, verify, and install:

```bash
VERSION=v0.2.0
curl -fLO "https://github.com/cleeryy/ludex/releases/download/${VERSION}/ludex-server-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
curl -fLO "https://github.com/cleeryy/ludex/releases/download/${VERSION}/ludex-server-${VERSION}-x86_64-unknown-linux-gnu.tar.gz.sha256"
sha256sum -c "ludex-server-${VERSION}-x86_64-unknown-linux-gnu.tar.gz.sha256"
tar -xzf "ludex-server-${VERSION}-x86_64-unknown-linux-gnu.tar.gz" -C /tmp
sudo install -m 0755 /tmp/ludex-server /usr/local/bin/ludex-server
```

Create a service user and directories:

```bash
sudo useradd --system --uid 1000 --home-dir /var/lib/ludex \
  --shell /usr/sbin/nologin ludex
sudo mkdir -p /var/lib/ludex/data /srv/ludex-media /srv/games
sudo chown -R ludex:ludex /var/lib/ludex /srv/ludex-media
# /srv/games only needs to be writable if you use ingest
sudo chown -R ludex:ludex /srv/games
```

Environment file, readable by root only (systemd reads it before dropping
privileges):

```bash
sudo install -d -m 0755 /etc/ludex
sudo tee /etc/ludex/ludex.env >/dev/null <<'EOF'
LUDEX_BIND=127.0.0.1:8080
LUDEX_DATA_DIR=/var/lib/ludex/data
LUDEX_GAMES_DIR=/srv/games
LUDEX_MEDIA_DIR=/srv/ludex-media
LUDEX_SERVER_NAME=Ludex
# LUDEX_IGDB_CLIENT_ID=
# LUDEX_IGDB_CLIENT_SECRET=
EOF
sudo chmod 600 /etc/ludex/ludex.env
```

systemd unit:

```ini
# /etc/systemd/system/ludex.service
[Unit]
Description=Ludex game library server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=ludex
Group=ludex
EnvironmentFile=/etc/ludex/ludex.env
ExecStart=/usr/local/bin/ludex-server
Restart=on-failure
RestartSec=3
StateDirectory=ludex
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/ludex /srv/ludex-media /srv/games
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now ludex
sudo systemctl status ludex
curl -fsS http://127.0.0.1:8080/api/health
```

Adjust `ReadWritePaths` if your directories differ. Remove `/srv/games` from it
when the library is mounted read-only.

## Windows server (optional)

The desktop app's **Local library** mode is the supported Windows single-machine
setup and does not need a separate server. To run a LAN server on Windows, build
from source and set the paths in PowerShell:

```powershell
cargo build --release -p ludex-server --features embed-web

$env:LUDEX_BIND = "0.0.0.0:8080"
$env:LUDEX_DATA_DIR = "D:\Ludex\data"
$env:LUDEX_GAMES_DIR = "D:\Ludex\games"
$env:LUDEX_MEDIA_DIR = "D:\Ludex\media"
$env:LUDEX_SERVER_NAME = "Ludex"
.\target\release\ludex-server.exe
```

Allow the port on the private network only:

```powershell
New-NetFirewallRule -DisplayName "Ludex" -Direction Inbound `
  -Protocol TCP -LocalPort 8080 -Action Allow -Profile Private
```

Put a TLS reverse proxy in front before exposing it beyond the LAN. Windows
binaries are not published as server artifacts in v0.1; the desktop installers
are.

## Configuration reference

| Variable | Default | Notes |
|---|---|---|
| `LUDEX_BIND` | `0.0.0.0:8080` | `host:port`. Use `127.0.0.1:8080` behind a proxy. |
| `LUDEX_DATA_DIR` | `./data` (binary) / `/data` (image) | SQLite DB + JWT secret. |
| `LUDEX_GAMES_DIR` | `./games` / `/games` | Scanned recursively, depth 3. |
| `LUDEX_MEDIA_DIR` | `./media` / `/media` | Artwork cache. Safe to delete while stopped. |
| `LUDEX_SERVER_NAME` | `Ludex` | Shown in the UI and `/api/setup/status`. |
| `LUDEX_JWT_SECRET` | generated in data dir | HS256 secret. Set to keep sessions across a data wipe; 32+ random bytes. |
| `LUDEX_IGDB_CLIENT_ID` | unset | IGDB metadata provider. |
| `LUDEX_IGDB_CLIENT_SECRET` | unset | Server-side only. |
| `LUDEX_INGEST_ALLOW_PRIVATE` | `false` | Permit ingest URLs on private/link-local IPs. |
| `LUDEX_SCAN_INTERVAL_SECONDS` | `300` | Automatic scan interval; `0` disables periodic scans. |
| `LUDEX_DATABASE_URL` | `sqlite://$LUDEX_DATA_DIR/ludex.db?mode=rwc` | `postgres://…` runs on PostgreSQL instead of SQLite; see [POSTGRES.md](POSTGRES.md). |
| `LUDEX_DATABASE_BACKEND` | inferred from the URL | `sqlite` or `postgres`; only needed for a scheme-less URL. A value contradicting the URL is a startup error. |
| `LUDEX_DATABASE_MAX_CONNECTIONS` | `8` | Connection pool ceiling (both backends). |
| `LUDEX_ADMIN_USERNAME` / `LUDEX_ADMIN_PASSWORD` | unset | Optional first-run admin bootstrap (zero users only). Prefer the UI; env values are visible to `docker inspect`. |
| `LUDEX_WEB_DIR` | unset | Serve the UI from disk instead of the embedded build. |
| `LUDEX_LOCAL_MODE` | `false` | Desktop embedded mode; leave unset on a server. |
| `RUST_LOG` | `ludex_server=info,tower_http=warn` | Log filter. |

Unknown `LUDEX_*` variables are ignored.

### IGDB credentials

1. Open <https://dev.twitch.tv/console/apps> and create an application.
2. Copy the **Client ID** and generate a **Client Secret**.
3. Put both in `.env` (Compose) or `/etc/ludex/ludex.env` (binary) and restart.
4. `GET /api/config` reports `features.igdb: true` when configured.

Without IGDB, Ludex falls back to the public Steam Store search and finally to
filename-only metadata. Steam needs no key. Keep the secret server-side; it is
never sent to clients.

## Backups and restore

Back up `/data` (database + JWT secret) and `/games`. `/media` is a cache.

PostgreSQL deployments have no database file in `/data`: dump the database with
`pg_dump` and keep `/data` for the JWT secret, saves and uploads. `ludex-backup`
refuses to run against PostgreSQL by design; the exact commands are in
[POSTGRES.md](POSTGRES.md#backup-and-restore).

The built-in CLI creates a consistent, checksummed snapshot of the database,
`media/`, `saves/` and a non-secret config manifest (game archives stay on
`/games` or in S3):

```bash
docker compose exec ludex ludex-backup backup --output /data/backups/manual.tar.gz
docker compose exec ludex ludex-backup verify /data/backups/manual.tar.gz
```

See [`BACKUP.md`](./BACKUP.md) for restore, `--dry-run`, `--force` and
`--upload-s3`.

Cold backup (consistent, simplest):

```bash
docker compose stop ludex
sudo tar -czf "ludex-data-$(date +%F).tar.gz" -C . data
docker compose start ludex
```

Hot backup with the SQLite CLI (safe with WAL, no downtime):

```bash
mkdir -p backups
sqlite3 data/ludex.db ".backup 'backups/ludex.db'"
```

Keep the database and the JWT secret together: restoring one without the other
invalidates existing sessions (users simply log in again; no data is lost).

Restore:

```bash
docker compose stop ludex
sudo mv data data.broken
sudo tar -xzf ludex-data-YYYY-MM-DD.tar.gz
sudo chown -R 1000:1000 data
docker compose start ludex
```

Test a restore on a different port before relying on it. Store backups
encrypted, off the host, and include the Ludex version that produced them.

## Upgrade

1. Read the release notes and take a backup.
2. Change the image tag (or `docker compose pull` for `latest`).
3. `docker compose up -d`.
4. Watch `docker compose logs -f ludex` and `curl -fsS localhost:8080/api/health`.

Database migrations run automatically at startup and are forward-only. To roll
back, restore the pre-upgrade `/data` backup and the previous image tag. Never
run two server instances against the same `/data` directory.

For the CT258 pull-based updater (edge prerelease, SHA-256 verification,
automatic rollback, systemd timer), see [`GITOPS.md`](GITOPS.md).

## Healthcheck and monitoring

- Endpoint: `GET /api/health` → `{"status":"ok","version":"...","uptimeSeconds":...}`.
- Compose: `docker compose ps` shows `healthy`; the image healthcheck uses
  `curl` against `127.0.0.1:8080`.
- If you change `LUDEX_BIND`'s port, override the healthcheck accordingly.
- Logs are JSON-free structured `tracing` output on stdout; `RUST_LOG` controls
  verbosity. `RUST_LOG=ludex_server=debug` for troubleshooting.

## Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| Container restarts, `permission denied` | `./data`, `./media` or `./games` not writable by uid 1000. `chown -R 1000:1000`. |
| `unhealthy` but UI works | Healthcheck port differs from `LUDEX_BIND`; override the compose healthcheck. |
| Setup returns 409 | Users already exist. Log in, or restore an empty data dir. |
| Scan finds fewer games than expected | Files under 1 MiB, deeper than 3 levels, hidden, or inside `Redist`, `_CommonRedist`, `Support`, `Tools`, extracted trees, `unins*` are skipped by design. |
| Metadata stays empty | No IGDB credentials and Steam lookup failed/rate-limited. Check logs, set IGDB, or edit metadata manually. |
| Download stalls at a proxy | Buffering enabled. Use Caddy `flush_interval -1` or nginx `proxy_buffering off`. |
| Login loop after moving data | JWT secret changed (data dir wiped). Log in again; existing tokens are invalid by design. |
| `database is locked` | Two Ludex processes share one data dir. Run one. |
| JSON request rejected (413) | API JSON bodies are capped at 10 MiB; archives are fetched server-side by URL ingest, not uploaded as JSON. |

## Security checklist

- [ ] HTTPS everywhere; port 8080 bound to localhost or a private network.
- [ ] Strong admin password; one account per person; editor role only when needed.
- [ ] Backups encrypted and access-controlled (they contain password hashes).
- [ ] `/data` never served by the reverse proxy or a file server.
- [ ] Image pinned to a version and updated deliberately.
- [ ] `/games` mounted read-only when ingest is not used.
- [ ] No Docker socket, no host network, no root user.
- [ ] IGDB secret only in server environment, never in the web UI or git.
