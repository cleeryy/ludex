# GitOps: pull-based edge updates for CT258

The homelab deployment on **CT258** updates itself from a public, prerelease
GitHub artifact. There is no self-hosted runner, no deploy key and no secret in
the update path: the timer runs on the host, pulls the `edge` release, verifies
its SHA-256 and restarts `ludex.service` only when the binary changed.

`/games`, `/data` and `/media` are never read or written by the updater. Only the
server binary and its checksum/backup files in the install directory change.

## How the edge release is produced

On every push to `main`:

1. `rust`, `web` and **`E2E`** must pass. `edge-release` has
   `needs: [rust, web, e2e]`, so a failed E2E run cannot publish an edge build.
2. `edge-release` checks whether anything server-affecting changed since the
   previous `edge` tag (`crates/`, `apps/web/`, Cargo/pnpm manifests and locks,
   `docker/`). Documentation-only pushes skip the build; the job still succeeds.
3. The server is built with the web UI embedded:
   `cargo build --release -p ludex-server --features embed-web`.
4. The artifact is packaged as:

   ```text
   ludex-server-edge-x86_64-unknown-linux-gnu.tar.gz
   ludex-server-edge-x86_64-unknown-linux-gnu.tar.gz.sha256
   ```

5. The `edge` tag is force-moved to the current `main` commit and a prerelease
   named **Ludex edge** is created or updated. Assets are uploaded with
   `--clobber`.

The public, authentication-free download URLs are:

```text
https://github.com/cleeryy/ludex/releases/download/edge/ludex-server-edge-x86_64-unknown-linux-gnu.tar.gz
https://github.com/cleeryy/ludex/releases/download/edge/ludex-server-edge-x86_64-unknown-linux-gnu.tar.gz.sha256
```

`edge` is a rolling prerelease. It is convenient for the homelab, not a stable
release channel.

## Timer and service on CT258

The units live in [`deploy/gitops/`](../deploy/gitops/):

- `ludex-update.service` — one-shot, runs `/usr/local/bin/ludex-update`.
- `ludex-update.timer` — every 2 minutes after boot, with up to 45 s of
  randomized delay.

### Install (on CT258, after the v0.2.0 deployment)

```bash
# From a checkout of the repository, on the host:
sudo install -m 0755 scripts/ludex-update.sh /usr/local/bin/ludex-update
sudo install -m 0644 deploy/gitops/ludex-update.service /etc/systemd/system/ludex-update.service
sudo install -m 0644 deploy/gitops/ludex-update.timer /etc/systemd/system/ludex-update.timer

sudo install -d -m 0755 /etc/ludex
sudo tee /etc/ludex/update.env >/dev/null <<'EOF'
LUDEX_UPDATE_CHANNEL=edge
# LUDEX_UPDATE_TAG=edge
# LUDEX_UPDATE_INSTALL_DIR=/srv/ludex
# LUDEX_UPDATE_SERVICE=ludex.service
# LUDEX_UPDATE_HEALTH_URL=http://127.0.0.1:8080/api/health
EOF
sudo chmod 600 /etc/ludex/update.env

sudo systemctl daemon-reload
sudo systemctl enable --now ludex-update.timer
```

The installer is intentionally not automated: the timer is enabled on the host
by the operator, after the initial `ludex.service` deployment exists.

### Check what would happen

```bash
sudo /usr/local/bin/ludex-update --check
```

Exit codes:

| Code | Meaning |
|---|---|
| `0` | up to date |
| `10` | an update is available (dry-run only) |
| `1` | download, checksum, extraction or configuration error |

`--check` does not take the update lock, does not write to the install directory
and does not touch the service. It downloads only the small `.sha256` file.

## What an update does

1. `flock -n /run/ludex-update.lock`; concurrent runs exit without doing work.
2. Download `ludex-server-<tag>-x86_64-unknown-linux-gnu.tar.gz.sha256`.
3. Compare the remote hash with `/srv/ludex/ludex-server.sha256` (or the binary
   hash if the checksum file is missing). If equal, exit 0.
4. Download the tar, verify the **tar's** SHA-256, extract `./ludex-server`.
5. Install it as `/srv/ludex/ludex-server.new`; back up the current binary to
   `ludex-server.previous` and the checksum to `ludex-server.previous.sha256`.
6. `systemctl stop ludex.service`, atomically swap the binary, write the new
   checksum, `systemctl start ludex.service`.
7. Poll `LUDEX_UPDATE_HEALTH_URL` for up to 30 s.
8. On a failed health check: stop the service, restore
   `ludex-server.previous`, restore the previous checksum, start the service and
   exit 1. The timer retries on its next tick.

If the health check succeeds, the previous binary remains on disk for a manual
rollback. It is overwritten on the next successful update.

## Rollback

Automatic rollback happens on a failed health check. Manual rollback:

```bash
sudo systemctl stop ludex.service
sudo install -m 0755 /srv/ludex/ludex-server.previous /srv/ludex/ludex-server
sudo cp -a /srv/ludex/ludex-server.previous.sha256 /srv/ludex/ludex-server.sha256
sudo systemctl start ludex.service
curl -fsS http://127.0.0.1:8080/api/health
```

Restoring the checksum matters: without it the next timer tick would compare the
new remote hash with an old local checksum and update again.

## Logs

```bash
journalctl -u ludex-update.service -n 100 --no-pager
journalctl -u ludex-update.service -f
journalctl -u ludex.service -n 100 --no-pager
systemctl list-timers ludex-update.timer
```

Every run logs the channel, tag, install directory, download decisions and
health result to the journal.

## Pin stable instead of edge

Edge is the default. To follow a stable release:

```bash
sudo tee /etc/ludex/update.env >/dev/null <<'EOF'
LUDEX_UPDATE_CHANNEL=stable
LUDEX_UPDATE_TAG=v0.2.0
EOF
sudo chmod 600 /etc/ludex/update.env
sudo systemctl start ludex-update.service   # apply immediately
```

Stable assets use the release tag in the file name, for example:

```text
https://github.com/cleeryy/ludex/releases/download/v0.2.0/ludex-server-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
```

Changing `LUDEX_UPDATE_TAG` is the upgrade action. The timer keeps checking that
tag until you change it. To go back to edge, remove `LUDEX_UPDATE_TAG` and set
`LUDEX_UPDATE_CHANNEL=edge`.

## Backups

The updater only swaps the binary; it never backs up `/data`. Schedule
`ludex-backup backup` (database snapshot + manifest) independently and keep the
archives outside `/srv/ludex`, otherwise a rollback or a reinstall can take them
with it. Runbook: [`OPERATIONS.md`](OPERATIONS.md#backup-and-restore-49).

## Safety notes

- The updater never touches `/games`, `/data` or `/media`. It only restarts the
  server; SQLite migrations run normally at startup.
- A release artifact is only accepted if its tar SHA-256 matches the `.sha256`
  published with the same release. Both are fetched over HTTPS.
- The edge release is public and contains no secrets. The update path has no
  GitHub token, deploy key or self-hosted runner.
- The timer runs as root because it must stop/start the systemd unit and write
  the binary. The service itself keeps running as the unprivileged Ludex user.
- If GitHub is unreachable, the update is skipped and the current service keeps
  running. If the new binary fails its health check, the previous binary is
  restored automatically.
- The unit hard-codes `ReadWritePaths=/srv/ludex`. If you override
  `LUDEX_UPDATE_INSTALL_DIR`, update that path in the unit as well.
- Do not run the timer on a host where `ludex.service` is not the intended
  production unit. Override `LUDEX_UPDATE_SERVICE` and paths in
  `/etc/ludex/update.env`.
