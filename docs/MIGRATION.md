# Migrating from GameVault

Ludex can reuse a GameVault **games volume as-is**. In v0.1 it scans the same
archive files, so the risky part of a migration is small: run both servers side
by side, verify the library, then switch the port/proxy.

What v0.1 migrates:

- Archive files (`.zip`, `.rar`, `.7z`, portable `.exe`) and their folder layout.
- Title/year/version/edition parsed from filenames.

What Ludex does **not** migrate:

- GameVault password hashes, sessions, API keys and GameVault+ settings.
- Media files: only artwork paths are imported; Ludex re-caches provider art.

Metadata edits, users, progress states, playtime, bookmarks and news are
imported from a JSON export (see step 8). Do not delete the GameVault instance
until you are satisfied with Ludex.

## Before you start

1. **Back up GameVault** (database and volumes) and note its version.
2. **Inventory the library** and record the archive count.
3. **Plan downtime.** The first scan is read-only and can run while GameVault is
   online, but the cutover needs GameVault stopped.
4. **Do not run two ingest writers on the same volume.** GameVault's downloader
   and Ludex's ingest both create temporary files and rename archives. Scanners
   are read-only and can coexist; writers cannot.

## 1. Locate the GameVault volumes

Find the container and its mounts:

```bash
docker ps --format '{{.Names}}\t{{.Image}}' | grep -i gamevault
docker inspect <gamevault-container> \
  --format '{{range .Mounts}}{{println .Source "->" .Destination}}{{end}}'
```

Example output:

```
/srv/gamevault/games -> /games
/srv/gamevault/media -> /media
/srv/gamevault/db    -> /var/lib/postgresql/data
```

Write down the host path mounted at `/games`; call it `$GAMES_VOLUME` below.

## 2. Count the archives

Ludex indexes the same extensions but skips tiny files, hidden entries, extracted
game trees and known support folders. Establish the expected count first:

```bash
export GAMES_VOLUME=/srv/gamevault/games   # replace with your path

find "$GAMES_VOLUME" -maxdepth 3 -type f \
  \( -iname '*.zip' -o -iname '*.rar' -o -iname '*.7z' -o -iname '*.exe' \) \
  -size +1M | wc -l
```

For example, a library with **18 archives** should show 18 here. If Ludex later
shows fewer, list the candidates and compare:

```bash
find "$GAMES_VOLUME" -maxdepth 3 -type f \
  \( -iname '*.zip' -o -iname '*.rar' -o -iname '*.7z' -o -iname '*.exe' \) \
  -size +1M -printf '%s\t%p\n' | sort -n
```

## 3. Prepare Ludex directories

Use separate data and media directories. Do not point Ludex at GameVault's
media cache: the layout is GameVault-specific and Ludex writes its own artwork
cache.

```bash
sudo mkdir -p /srv/ludex/data /srv/ludex/media
sudo chown -R 1000:1000 /srv/ludex
```

Keep GameVault's media volume untouched as a fallback until the migration is
validated. Ludex re-downloads covers and screenshots from its metadata
providers; copying the old cache is not required.

## 4. Run Ludex alongside GameVault on port 8081

Mount the games volume **read-only** for the first test. Scanning and downloading
only read archives; ingest is the only writer, and it is not used yet.

```bash
docker run -d --name ludex-migrate \
  -p 127.0.0.1:8081:8080 \
  -v "$GAMES_VOLUME":/games:ro \
  -v /srv/ludex/data:/data \
  -v /srv/ludex/media:/media \
  -e LUDEX_BIND=0.0.0.0:8080 \
  -e LUDEX_GAMES_DIR=/games \
  -e LUDEX_DATA_DIR=/data \
  -e LUDEX_MEDIA_DIR=/media \
  -e LUDEX_SERVER_NAME="Ludex (migration)" \
  -e LUDEX_IGDB_CLIENT_ID="" \
  -e LUDEX_IGDB_CLIENT_SECRET="" \
  ghcr.io/cleeryy/ludex:v0.2.0
```

Or add a second service to a separate Compose file. Do not publish 8081 on the
public interface during the test; `127.0.0.1` is enough.

Check health:

```bash
curl -fsS http://127.0.0.1:8081/api/health
```

## 5. Create the admin and scan

Open `http://127.0.0.1:8081` (through an SSH tunnel if the host is remote) and
complete first-run setup. Or use the API:

```bash
read -rsp 'Admin password: ' LUDEX_ADMIN_PASSWORD; echo
curl -fsS -X POST http://127.0.0.1:8081/api/setup \
  -H 'content-type: application/json' \
  -d "{\"username\":\"admin\",\"password\":\"$LUDEX_ADMIN_PASSWORD\",\"serverName\":\"Ludex\"}"
```

Store the returned token in a shell variable without printing it:

```bash
TOKEN=$(curl -fsS -X POST http://127.0.0.1:8081/api/auth/login \
  -H 'content-type: application/json' \
  -d "{\"username\":\"admin\",\"password\":\"$LUDEX_ADMIN_PASSWORD\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')
```

Trigger and poll a scan:

```bash
curl -fsS -X POST http://127.0.0.1:8081/api/library/scan \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{}'

curl -fsS http://127.0.0.1:8081/api/library/scan \
  -H "authorization: Bearer $TOKEN" | python3 -m json.tool

curl -fsS http://127.0.0.1:8081/api/stats \
  -H "authorization: Bearer $TOKEN" | python3 -m json.tool
```

## 6. Verify the library

- Compare `/api/stats` `games`/`versions` with the archive count from step 2.
  A version is one archive; a game groups versions by normalized title and year.
- Spot-check titles with brackets and versions: `Game (2016)`, `Game v1.2.3`,
  `Game [GOTY]`. The parser is conservative; it never invents metadata.
- Open a game, confirm the cover/metadata, and download one archive with a Range
  request:

  ```bash
  curl -fsS -D - -o /dev/null \
    -H "authorization: Bearer $TOKEN" -H 'Range: bytes=0-1023' \
    "http://127.0.0.1:8081/api/games/<id>/download?versionId=<versionId>"
  ```

  Expect `206 Partial Content` and `Content-Range: bytes 0-1023/...`.

If the count is lower, check for archives under 1 MiB, nested deeper than three
levels, hidden folders, or inside `Redist`, `_CommonRedist`, `Support`, `Tools`,
extracted game trees and `unins*`. Those are intentionally skipped. Move or
rename files if they should be indexed.

Metadata enrichment may take a few minutes after the scan. If covers stay empty,
configure IGDB (see [DEPLOY.md](DEPLOY.md#igdb-credentials)) and refresh.

## 7. Cut over

1. Stop GameVault's downloader and container:

   ```bash
   docker compose -f /path/to/gamevault/compose.yaml stop
   # or: docker stop <gamevault-container>
   ```

2. Confirm nothing is still writing to `$GAMES_VOLUME`:

   ```bash
   sudo lsof +D "$GAMES_VOLUME" 2>/dev/null | head
   ```

3. Stop and remove the migration container:

   ```bash
   docker stop ludex-migrate && docker rm ludex-migrate
   ```

4. Deploy Ludex with the normal [compose.yaml](../compose.yaml): mount
   `$GAMES_VOLUME` at `/games` **read-write** if you want ingest, keep `/data`
   and `/media` from the test, and publish the production port (usually
   `8080:8080` or `127.0.0.1:8080:8080`).

   ```yaml
   services:
     ludex:
       volumes:
         - /srv/gamevault/games:/games
         - /srv/ludex/media:/media
         - /srv/ludex/data:/data
   ```

5. Point the reverse proxy at Ludex. If the hostname stays the same, this is an
   upstream change (for example `reverse_proxy 127.0.0.1:8080`), not a DNS
   change. If you move to a new hostname, lower the DNS TTL beforehand.

6. Log in, run one final scan, download an archive, and start a game from the
   desktop app.

Keep the GameVault container and database stopped but intact until you have used
Ludex for a few days. Rolling back is then just starting GameVault and restoring
the proxy upstream.

## 8. Import GameVault metadata and progress

The importer reads the JSON document produced by
[`scripts/gamevault-export.sql`](../scripts/gamevault-export.sql); it never
connects to GameVault's PostgreSQL database. Export first, review the dump, run
the import in dry-run mode, then apply it.

### 8.1 Export the JSON dump

The export script is read-only. GameVault is a Prisma application, so check the
table/column names header in the script against your version (`\dt` and
`\d "Game"` in psql) before running it.

```bash
# Read the connection details from the running GameVault container instead of
# putting a password on the command line.
docker exec -u postgres <gamevault-db-container> \
  psql -d gamevault -At -f - < scripts/gamevault-export.sql > gamevault.json
chmod 600 gamevault.json
```

The result is one `gamevault-export` v1 JSON document with `users`, `games`
(including their file paths and sizes), `progress`, `bookmarks` and `news`.
It contains **usernames, e-mail addresses and metadata edits**: treat it as a
secret and delete it once the import is verified.

Keep a normal `pg_dump` as a rollback safety net (encrypted, outside the repo):

```bash
docker exec -u postgres <gamevault-db-container> \
  pg_dump -Fc -d gamevault > "gamevault-$(date +%F).dump"
age -p -o "gamevault-$(date +%F).dump.age" "gamevault-$(date +%F).dump"
shred -u "gamevault-$(date +%F).dump"
```

### 8.2 Dry run

Run the importer against the **same** `LUDEX_*` environment as the server
(`LUDEX_DATABASE_URL`, `LUDEX_DATA_DIR`, `LUDEX_GAMES_DIR`, `LUDEX_MEDIA_DIR`).
A dry run writes nothing and prints matched / unmatched / ignored rows plus the
account and progress plan:

```bash
ludex-server import-gamevault --dump gamevault.json --dry-run
# machine-readable:
ludex-server import-gamevault --dump gamevault.json --dry-run --json
```

Games are matched by relative archive path first, then by normalized title plus
an identical archive size. Ambiguous matches are reported as unmatched and are
never guessed: fix the report (usually a missing or renamed archive) and run the
dry run again.

### 8.3 Apply

```bash
ludex-server import-gamevault --dump gamevault.json
```

What is imported:

- **Accounts**: matched by username; new accounts get a random temporary
  password printed **once** by the command. GameVault password hashes are never
  reused. `admin`/`editor` roles map to the same Ludex role, everything else
  becomes `user`; disabled GameVault accounts stay disabled.
- **Progress and playtime** with GameVault states mapped to Ludex:
  `UNPLAYED → unplayed`, `PLAYING → playing`, `COMPLETED → completed`,
  `ABORTED_TEMPORARY → on hold`, `ABORTED_PERMANENT → abandoned`,
  `INFINITE → playing`.
- **Bookmarks** and **news posts** (author kept when the account was matched).
- **Metadata edits** (description, artwork paths, lists, rating, provider
  mapping, launch/installer fields), with priority over provider metadata.
  Artwork files are not copied; a metadata refresh replaces the paths.

The import is idempotent: running the same dump twice does not duplicate
accounts, progress, bookmarks or news, and does not reset passwords it already
created.

### 8.4 Verify

- `/api/stats` `games`/`versions` still matches the archive count.
- Spot-check a few games: description/cover reflect GameVault edits, progress
  states and playtime are present, bookmarks survived.
- Sign in with one generated temporary password, then change it in
  Settings → Account (or ask the user to).

## 9. Users, roles and passwords

GameVault password hashes cannot be reused (different scheme), so every account
created by the import receives a random temporary password printed once. Ask
each user to change it after their first sign-in. Roles map conservatively:
`admin → admin`, `editor → editor`, everything else → `user`. Accounts are local
Ludex accounts; link them to SSO later from the identity provider if needed.

## Rollback

1. Stop Ludex.
2. Start GameVault with its original database and volumes (untouched).
3. Restore the reverse proxy upstream.
4. Keep Ludex's `/data` for inspection; do not point GameVault at it.

Because the games volume is shared read-only during the test, no archive is
modified until you enable Ludex ingest. After cutover, any archive added through
Ludex is a normal file in `$GAMES_VOLUME` and GameVault can still see it, but
GameVault's database will not know about it until its own scan.

## Checklist

- [ ] GameVault database and volumes backed up.
- [ ] Archive count recorded (for example 18).
- [ ] Ludex test instance on 8081, games mounted read-only.
- [ ] Admin created, scan finished, `/api/stats` matches the expected count.
- [ ] Metadata spot-checked; IGDB configured if desired.
- [ ] Range download returns 206.
- [ ] GameVault stopped; no writers on the games volume.
- [ ] Ludex on the production port with read-write games (if ingest is used).
- [ ] Proxy/DNS switched; login and download verified.
- [ ] GameVault PostgreSQL dump encrypted and stored with backups.
- [ ] `ludex-server import-gamevault --dry-run` reviewed (matched/unmatched).
- [ ] Import applied; progress, bookmarks and metadata spot-checked.
- [ ] Temporary passwords handed out; users changed them.
- [ ] Rollback procedure tested or at least documented.
