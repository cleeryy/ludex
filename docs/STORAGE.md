# Object storage for game archives (#47)

Ludex can keep game archives in an S3-compatible bucket instead of (or in
addition to) the local library roots. Metadata, artwork and cloud saves stay on
local disk; only the game archives travel to object storage.

The local library file is **never deleted** by Ludex. It remains the fallback
for rows indexed before S3 was enabled, for archives whose upload failed, and
for rolling back to local storage.

## Backends

| Backend | `LUDEX_STORAGE_BACKEND` | Where archives live |
|---|---|---|
| Local (default) | `local` | `LUDEX_GAMES_DIR` and the extra library roots |
| S3-compatible | `s3` | Bucket configured by the `LUDEX_S3_*` variables |

`LUDEX_STORAGE_BACKEND` accepts the aliases `filesystem`/`disk` for local and
`minio`/`garage`/`r2`/`b2` for S3; they all mean the same two code paths.

## Configuration

| Variable | Default | Description |
|---|---|---|
| `LUDEX_STORAGE_BACKEND` | `local` | `local` or `s3`. |
| `LUDEX_S3_ENDPOINT` | AWS S3 | Custom endpoint, e.g. `https://minio.lan:9000` or `http://garage:3900`. |
| `LUDEX_S3_REGION` | `us-east-1` | Signing region. Most self-hosted servers accept any value. |
| `LUDEX_S3_BUCKET` | — | Bucket name. Required for `s3`. |
| `LUDEX_S3_ACCESS_KEY` | — | Access key id. Required for `s3`. |
| `LUDEX_S3_SECRET_KEY` | — | Secret access key. Required for `s3`. |
| `LUDEX_S3_PATH_STYLE` | `false` | Path-style addressing (`endpoint/bucket/key`). Set `true` for MinIO, Garage and most S3 gateways. |
| `LUDEX_S3_PREFIX` | empty | Optional key prefix, without leading/trailing slashes. |

The server makes **no network call at startup**. Missing bucket/access
key/secret values fail `Config::from_env` before the listener binds; an S3
client that cannot be built (for example an invalid endpoint) logs a warning and
leaves the server on local storage. An unreachable endpoint only fails the first
upload or download, and those failures fall back to the local file when it still
exists.

Credentials are read from the environment only. They are never written to the
database, logs, `/api/config`, `/api/health/details` or the backup manifest.

### MinIO

```bash
LUDEX_STORAGE_BACKEND=s3
LUDEX_S3_ENDPOINT=http://10.1.20.40:9000
LUDEX_S3_REGION=us-east-1
LUDEX_S3_BUCKET=ludex
LUDEX_S3_ACCESS_KEY=ludex
LUDEX_S3_SECRET_KEY=change-me
LUDEX_S3_PATH_STYLE=true
LUDEX_S3_PREFIX=prod
```

The bucket must exist; Ludex does not create it. Keep it private: downloads are
proxied through the server, never redirected to a signed URL.

### Cloudflare R2 / Backblaze B2

Use their S3 endpoint, `LUDEX_S3_PATH_STYLE=false`, and credentials scoped to the
bucket (`Object Read & Write`). B2 requires the region reported by the bucket
(for example `us-west-004`).

## What gets uploaded

A scan (scheduled, watcher or ingest-triggered) uploads every archive that is:

- newly indexed,
- replaced on disk (size or mtime changed), or
- still marked `storage_backend = local` while the backend is `s3` (this is how
  an existing library is migrated on the next scan).

The object key is deterministic per content generation:

```text
{prefix}/archives/{sha256(root_id:path:size:mtime)[:24]}-{sanitised file name}
```

`game_versions` records `storage_backend`, `object_key` and `etag`
(migration `0010_storage_ops.sql`). A failed upload logs a warning, leaves the
row on `local` and is retried by the next scan; the scan itself still succeeds
and the game stays downloadable from disk.

Enabling S3 on a large existing library therefore uploads the whole library
during the first scan. Do it during a maintenance window and watch
`ludex_scan_running` / `ludex_scan_files` in `/metrics`.

## Downloads and stream installs

`GET /api/games/{id}/download` proxies the object through the server:

- a single `Range: bytes=…` header is forwarded as an S3 range request
  (start-end, start- and -suffix forms), returning `206` with `Content-Range`,
  or `416` for an unsatisfiable range;
- no `Location` redirect and no presigned URL is returned, so bucket
  credentials and object keys never reach clients;
- a missing object falls back to the local library file when it still exists;
  otherwise it returns `404 file_not_found`;
- an unreachable or failing backend falls back to the local file when present;
  otherwise it returns `502 storage_unavailable` (or `503` when the S3 client
  itself is unavailable).

Rows with `storage_backend = local` (including every row from before this
feature) continue to use `ServeFile` on the library root, including `If-Range`.
`If-Range` is ignored on S3 proxied reads; clients that need it should compare
the ETag themselves.

Stream install (`/api/games/{id}/versions/{id}/stream`) needs random access to
the ZIP/7z central directory. For S3 rows the archive is staged under
`{LUDEX_DATA_DIR}/tmp/`, streamed as tar and deleted when the response ends.
Plan for free space in the data volume equal to one archive per concurrent
stream request.

## Storage layout

```text
bucket/
  {prefix}/archives/<hash>-<file>.zip
  {prefix}/backups/ludex-backup-<timestamp>.tar.gz   # ludex-backup --upload-s3
```

The media cache (`LUDEX_MEDIA_DIR`) and cloud saves (`{DATA}/saves`) stay local
because they are small, frequently read and needed when S3 is unavailable.

## Rollback

Set `LUDEX_STORAGE_BACKEND=local` and restart. New downloads are served from
the local files, which were never removed. Existing `s3` rows keep using the
bucket until a scan re-indexes them; to force local reads on those rows:

```sql
UPDATE game_versions SET storage_backend = 'local', object_key = NULL, etag = NULL;
```

Objects already in the bucket are not deleted by Ludex (there is no S3 garbage
collection yet). Remove them with the bucket's lifecycle rules or `mc rm`.

## Limitations

- No object garbage collection: replaced archives leave the previous object in
  the bucket. Use a bucket lifecycle policy on the `archives/` prefix.
- An upload interrupted by a crash may leave an orphan multipart upload; most
  S3 servers expose a lifecycle rule for incomplete multipart uploads.
- Stream installs from S3 stage the full archive on local disk.
- `If-Range` is not honoured for S3 proxies.
- S3 is not used in local/desktop mode.

## Tests

`storage.rs` unit tests use `object_store`'s in-memory backend, so CI never
needs a real bucket. They cover upload, ranged reads (bounded, offset, suffix),
404, invalid ranges, local no-op and staging downloads. Route tests cover the
proxied `206`/`404`/`503` responses, and the scanner test covers upload plus
`storage_backend`/`object_key`/`etag` persistence.
