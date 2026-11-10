# Object storage for the media cache (#47)

Ludex keeps the **library volume** (game archives and portable executables) on
local disk: downloads support HTTP `Range` and are streamed from files. The
**media cache** (covers, backgrounds, screenshots, avatars) can live in any
S3-compatible bucket instead of `LUDEX_MEDIA_DIR`.

The default is unchanged: `LUDEX_STORAGE_BACKEND=local`.

## Backends

| `LUDEX_STORAGE_BACKEND` | Behaviour |
|---|---|
| `local` (default) | Artwork is written atomically to `LUDEX_MEDIA_DIR` and streamed by `GET /api/media/{name}`. |
| `s3` | Artwork is written to the bucket, streamed through the server, or redirected to a public URL. |

## Configuration

```bash
LUDEX_STORAGE_BACKEND=s3

# Required
LUDEX_S3_BUCKET=ludex
LUDEX_S3_ACCESS_KEY=...
LUDEX_S3_SECRET_KEY=...

# Optional
LUDEX_S3_ENDPOINT=https://s3.eu-west-3.amazonaws.com   # MinIO: http://minio:9000
LUDEX_S3_REGION=us-east-1                              # default
LUDEX_S3_PATH_STYLE=true                               # default; false for virtual-hosted buckets
LUDEX_S3_PREFIX=ludex                                  # key prefix; default "ludex"
LUDEX_S3_PUBLIC_BASE_URL=https://cdn.example.com/ludex # enables 302 redirects
```

Object keys are `{prefix}/media/{name}`. Media names are content-addressed
(sha256 of the source URL + extension), so they are immutable and safe to cache
forever.

Startup fails with a clear message when `LUDEX_STORAGE_BACKEND=s3` is missing
the bucket, access key, secret key or a valid endpoint. Credentials are never
logged; `Debug` output masks them.

### Public reads

By default every media read is **streamed through the server**, so the bucket
can stay private and the existing authentication/caching headers apply. Set
`LUDEX_S3_PUBLIC_BASE_URL` when the bucket (or a CDN in front of it) is public
and you prefer clients to fetch artwork directly:

- `GET /api/media/{name}` answers `302` with `Location: {base}/{prefix}/media/{name}`
  and `Cache-Control: public, max-age=300`.
- The URL is not presigned: bucket policy, CDN tokens or a private network are
  your responsibility. Presigned GET URLs are not implemented yet.

## Migrating an existing local cache

The media cache is disposable (enrichment re-downloads missing artwork), but a
large IGDB pass is slow, so the CLI can copy the local directory into the
bucket:

```bash
# Upload only, keep the local files as a warm copy:
LUDEX_STORAGE_BACKEND=s3 LUDEX_S3_BUCKET=... ... \
  ludex-backup migrate-media --from /srv/ludex/media

# Upload, then delete each file that was uploaded or already present remotely:
ludex-backup migrate-media --from /srv/ludex/media --delete-source
```

The command is idempotent: files already present in the bucket are skipped.
Unsupported names (nested paths, hidden files) are reported and left alone.

## What is (and is not) stored remotely

| Data | Location |
|---|---|
| Covers, backgrounds, screenshots, avatars | media backend (`local` or `s3`) |
| Game archives and portable executables | library roots, always local disk |
| Database, saves, ingest state | `LUDEX_DATA_DIR` |
| Backups | local file, optionally uploaded with `ludex-backup backup --s3` |

Media garbage collection (`POST /api/media/gc`) and the media half of the
integrity check only run on the `local` backend. Object stores do not expose
file ages or references, so cleaning an S3 cache is left to the provider
(bucket lifecycle rules, or a scheduled `aws s3 rm` over old keys). The GC
endpoint answers `400 media_gc_unsupported` instead of silently doing nothing;
the integrity check simply skips media on S3.

`GET /metrics` exposes `ludex_media_cache_bytes` / `ludex_media_cache_files`
only for the local backend: listing a bucket on every scrape would cost one
request per page.

## Failure behaviour

- `exists_media` errors are treated as a cache miss; the download is retried
  and the upload error is logged (enrichment is best-effort and never fails a
  scan).
- `GET /api/media/{name}` returns `404` for a missing object and `500` for a
  backend error, with the error in the server log.
- Nothing is cached locally when `s3` is selected, so a bucket outage degrades
  artwork only; library browsing, downloads and saves are unaffected.

## Limits

- Only the media cache is remote. Archives are streamed from disk with `Range`
  support and are not proxied through S3.
- No S3 presigning, no server-side encryption configuration, no lifecycle
  management.
- Tested with `object_store`'s in-memory and local implementations; a real
  bucket (AWS, MinIO, Ceph) has not been exercised in CI.
