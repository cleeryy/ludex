# Downloads

How Ludex moves an archive from the server to a desktop, what happens when the
connection drops, and what to check when a large install fails. The performance
measurements and Caddy tuning live in [PERFORMANCE.md](PERFORMANCE.md); the
network topology (direct vs Caddy, HTTP/2/3, VPN) lives in
[NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md).

## Two transports

| | Classic download | Stream install |
|---|---|---|
| What it keeps | The archive in a `.part` file, then extracts it | Extracts entries as they arrive; no full archive retained |
| Resume unit | Byte offset via HTTP `Range` | Completed entries + a `.part` per in-flight entry |
| Formats | ZIP, RAR, 7z, portable `.exe` | ZIP per-entry; RAR/7z fall back to the sequential tar stream or to classic |
| Verification | Full archive SHA-256 before extraction | Per-entry size + CRC-32 before rename; manifest recorded |
| Best for | Large repacks, slow/flaky WAN, proxies with short timeouts | Small/medium ZIPs, fast LAN, saving disk space |
| Default | Yes | Opt-in (`Settings → Downloads → Stream installs`) |

A stream install is not a different download protocol; it is a different way to
consume the same authenticated archive. When the stream path cannot work
(unsupported format, missing manifest, a proxy that cannot serve an entry within
its budget), Ludex falls back to the classic resumable download instead of
looping.

## How the big launchers do it (and what we copy)

They converged on the same shape: **immutable hash-addressed units + a
per-version manifest + plain HTTPS in parallel + per-unit verification.**

| | Unit | Identity | Resume | Parallel | Delta |
|---|---|---|---|---|---|
| Steam (SteamPipe) | ~1 MiB chunks in depots | chunk SHA-1, depot manifest | chunk | many chunk GETs, multi-CDN | chunk reuse + binary deltas |
| Epic (BPS) | chunks in a binary manifest | chunk hash/GUID + SHA-1 | chunk | worker pool | per-chunk delta optimization |
| GOG (gogdl) | V2 chunks / V1 `main.bin` | MD5 per chunk/file | chunk/offset | worker pool | per-file patch |
| Battle.net (TACT) | encoded files/archives | CKey/EKey MD5 | file/block Range | multi-CDN | ZBSDIFF patches |

The lessons that matter for a self-hosted archive server:

- **The hash is the identity.** A full archive SHA-256, verified before install,
  closes the "fails at the same byte" class and is a prerequisite for parallel
  parts (#216, shipped). The server publishes it and the client refuses to
  extract bytes that do not match.
- **The manifest is the contract.** For Ludex the archive is the unit; the
  per-entry stream checkpoint is already a small manifest. Publish entry
  size/CRC32 and verify each entry; do not build a chunk store for one server.
- **Plain HTTP, several requests.** Parallelism comes from multiple Range
  requests, not an exotic protocol. Only worth it when measured (#217): the
  `aria2c -x8 ≥ 1.5×` rule in NETWORK-ARCHITECTURE. Measured 1.12× (WAN) and
  0.64× (cold LAN), so it is **not** implemented and Ludex stays single-stream.
- **Staging + commit.** Keep versions as immutable archives; never re-zip or
  patch an archive in place. Replacing a 40 GB archive for a one-file fix is a
  Steam-documented anti-pattern.
- **Do not copy:** CDN/cell topology, DRM/encryption keys, P2P, preload,
  fixed-size chunk stores, or an A→B delta pipeline. They solve scale problems
  Ludex does not have (and a delta on a multi-GB compressed archive is often
  larger or slower than the archive).

## Job identity and resume

- One installed game/version/transport has **one job id** from queue to history.
  Starting an install while a failed/interrupted job for the same version
  exists adopts that job and resumes it instead of inserting a second row.
- `resume_install` accepts a persisted job, not only an in-memory paused one:
  the worker is rebuilt from the stored parameters and the `.part`/checkpoint.
- Classic: the offset is the flushed `.part` size; every reconnect sends
  `Range: bytes=<offset>-`. A server that ignores Range or answers `416` is
  handled explicitly (restart only with confirmation).
- Stream: a `CompletedEntries` bitmap plus `.part` files resume at entry and
  byte granularity. Entries already on disk are verified and skipped; a changed
  archive/manifest restarts from zero with a clear message.
- A full app restart does not lose the job. The queue row is restored, the
  checkpoint is read, and the job re-enters the queue automatically (unless it
  was paused/cancelled or `autoResumeDownloads` is off), so the UI shows
  "Resuming from X" without a click.

## Full archive verification (#216)

Every version carries a full SHA-256 (not the sampled `sha256-sample:` update
fingerprint). The server computes it in a background job and exposes it as
`contentSha256` plus the `X-Ludex-Archive-Sha256` header on the download and
tar-stream responses.

- **Classic:** the download hashes while writing. The digest is only trusted
  when the file was written once from byte 0; a resumed or reconnected transfer
  is rehashed from byte 0 after the last byte arrives (no partial hash state is
  persisted, which also makes an app restart harmless). Verification runs before
  extraction, so a corrupt archive is never installed.
- **Mismatch:** the `.part` is deleted (never extracted), the expected digest is
  refreshed from the server, and the download retries once from zero. A second
  failure ends the job with the expected and actual SHA-256 in the message.
- **Changed vs corrupt:** the download response advertises the digest of the
  bytes the server is sending. If that digest matches what reached the disk but
  not the version metadata, the server replaced the archive between the two
  reads and the message says so; otherwise the transfer was corrupted.
- **Legacy rows:** a version without a hash (backfill pending, or indexed before
  #216) logs a warning and installs without strong verification. It stays
  downloadable; the server backfills it on the next scan.
- **Coverage on the field host (2026-11-13):** only **10 of 59 versions** carry
  a `content_sha256` (49 missing, i.e. every archive indexed before #216 and not
  re-scanned since). The strong-verification path therefore proved itself on 10
  archives, not all of them. Backfill with one sequential read per archive
  (`POST /api/integrity/content-hashes`, single-flight, ~1.7 TB on that host) —
  run it off-peak, it saturates the pool; the scan-time backfill only chips
  away at 50 rows per scan. Until then, a version with `contentSha256: null` is
  downloadable but only size/CRC-verified.
- **Stream:** no archive is retained, so the full hash is not applicable. Each
  entry is size- and CRC-32-verified before rename and the manifest is recorded
  (the per-entry checkpoint already proves completion).
- Both sides log the digest (not a secret) for support.

## Stall detection and recovery

- The client judges idleness **per response chunk**, not with a global read
  timeout: no bytes for ~30 s means a stall (configurable in
  Settings → Downloads → Connection recovery).
- A stalled/dropped response is retried with exponential backoff and jitter
  (1 s doubling, capped at 5 min). The next request resumes at the last flushed
  offset; the `.part` is never restarted while it is valid.
- **Long-horizon retry (#212).** Retrying continues for
  `downloadRetryWindowSecs` (default **6 h**, `0` = until the app closes). A
  dropped WAN, a Caddy restart or a ten-minute outage therefore needs no manual
  Resume. The window is set in Settings → Downloads → Automatic retries.
- **Only permanent errors stop immediately:** `400`, `401`, `403`, `404`,
  `415`, a changed archive (`ETag`/size/start offset, after two explicit
  restarts), a rejected resume offset (`416` after an explicit restart), and a
  full disk. The message names the layer and the `.part` is kept.
- A circuit breaker stops after **three identical permanent errors**, so a buggy
  server or archive cannot spin the retry loop.
- `408`, `425`, `429`, `5xx`, connection reset, TLS/body decode errors and proxy
  `502`/`504`/`524` are retryable. A `429`/`503` `Retry-After` is honoured
  (capped at 30 min).
- **Visibility.** While a retry waits the job stays in the active queue with
  `reconnecting`, `attempt`, `lastError`, `stalledForMs`, `httpStatus` and
  `nextRetryAt`. The row shows "Retrying in 42 s, attempt 7" and a **Retry now**
  button; cancel stops the job immediately (including during a backoff wait).
- **Offline.** The web shell forwards `navigator.onLine` to
  `set_download_network_state`. While offline, retries park
  (`waitingForNetwork`) without counting a failed attempt and resume on the next
  `online` event.
- The server rate-limits reconnect abuse per address/version (`429` +
  `Retry-After`) and logs `bytes`, `elapsed_ms`, `range_start`, `range_end` and
  `outcome` (`complete`, `error`, `client_abort`) for each stream. `complete`
  means the bytes sent equal the announced window (`Content-Range` length, or
  the archive size for a full `200`); `client_abort` is the residual case, i.e.
  the client stopped early. A dropped response after the last byte is a
  complete transfer, not an abort — that distinction is what makes the log
  usable for diagnosis.

## Stream retry and the 524 budget (#211/#212)

- Each entry gets five short attempts (0.5 s → 8 s backoff), then the **job**
  backs off and runs again against the same checkpoint: completed entries are
  skipped and per-entry `.part` files resume at their byte offset. The job-level
  retry uses the same 6 h window and the same visibility fields.
- A stream attempt only falls back to classic or fails when the path is
  structurally unusable: unsupported format/manifest, or the whole job received
  **zero bytes** after its entry attempts.
- Each entry request must produce its first body chunk within **20 s**. Cloudflare
  answers a 524 at ~100 s; five 20 s attempts plus backoff fit inside that
  budget, so a stuck proxy ends in the classic fallback instead of a hung job.
  A 524 in the middle of a working entry is retried at the exact byte.
- Server side, `GET .../stream/entries/{index}` logs `stream entry response`
  (`index`, `path`, `size`, `start`, `length`, `partial`, `elapsed_ms`) before
  the body, and `download stream closed` (`status`, `bytes`, `elapsed_ms`,
  `outcome`) when the body ends or the client aborts. Use those two lines to
  tell a proxy timeout (no entry response) from a client abort (entry response,
  then `client_abort` **with `bytes` below the announced range**).

## Tickets and proxy logs

The download ticket is a 15-minute capability. Ludex never logs it, but the
**path** it travels in does:

| Client | How it authenticates | Ticket in a URL? |
|---|---|---|
| Desktop (Tauri) | `Authorization: Bearer <session token>` on every request | **No** — `install.rs`, `maintenance.rs` (repair) and `stream_install.rs` build `/api/games/{id}/download?versionId=…` and attach the header |
| Browser / web shell | `POST /api/games/{id}/download-ticket` then a plain `<a href>` navigation | **Yes**, `?token=…` — a browser download cannot set a header |
| Share link | same ticket flow, scoped to one version | **Yes**, `?token=…` |

So the desktop path is already clean; the leak comes from browser downloads
hitting a proxy that logs the full URI. `/var/log/caddy/access.log` on the
field host recorded `"uri":"/api/games/7/download?versionId=7&token=eyJ…"`, i.e.
a live capability in clear text. Filter it at the proxy rather than relying on
the 15-minute TTL:

```caddyfile
log {
    output file /var/log/caddy/access.log
    format filter {
        wrap json
        fields {
            # `query` parses request>uri as a map so one key can be dropped
            # (Caddy >= 2.6 log filters). `replace token REDACTED` also works.
            request>uri query {
                delete token
            }
        }
    }
}
```

`query` is parsed as a map in the Caddy log encoder, so `delete` removes only
that key and keeps the rest (`versionId`, `jobId`, `partIndex`) — that is what
makes the log useful. On a Caddy older than the log filter module, redact at
collection time (Vector/filebeat) instead of leaving the URI intact. Verify with
one ranged request and `grep token` on the log: it must show no `token=eyJ`.
Rotate the log as usual; an unfiltered historical log contains expired tickets
and should be purged.

If a future browser flow can afford it, the same `fetch` + streaming
`FileSystemWritableFileStream` trick avoids the URL entirely, but it cannot
replace the anchor for a 40 GB download today.

## Resume on launch (#212)

- On exit the desktop marks rows that were still active, keeping their queue
  position. At startup, with `autoResumeDownloads` on (default), it rebuilds
  those workers from `install_job_params` and re-queues them at the recorded
  position. The job id, `.part` offset and stream checkpoint are reused, so the
  Downloads history keeps one row and no bytes are fetched twice.
- A user-paused or cancelled job is never auto-resumed. A real failure (404,
  disk full, changed archive) carries its own message and is not auto-resumed
  either; the row stays in history with its Retry action.
- Turn it off in Settings → Downloads → After a restart (the job then waits for
  an explicit Resume).

## Diagnostics bundle (#211 item 6)

`diagnostics_status` returns a `downloads` array (at most 10 jobs) with the
redacted `lastError`, `attempt`, `offset` (bytes on disk), `bytesTotal`,
`httpStatus`, `nextRetryAt`, `phase` and `stream`. Tokens, URLs and absolute
paths are redacted; the token stored in `install_job_params` is never included.
That is what to attach to a bug report instead of pasting logs.

## Queue

- Jobs are persisted with a `queue_position`; the scheduler takes the lowest
  position when a slot frees. Moving a queued job to the top/up/down changes the
  position and survives a restart.
- One install runs at a time by default. "Start now" asks to pause the current
  job first; it never silently cancels it.
- Pause keeps the `.part`/checkpoint; cancel also keeps the resumable state
  unless the user explicitly deletes it. Removing a history row never deletes
  the files.

## Cloudflare 524 and large stream entries

`524` is an **origin timeout** (Cloudflare waits ~100 s), not a client network
drop. It usually means the origin took too long to stage the archive or to
serve/decompress the requested entry. Consequences:

- Classic downloads are unaffected (a normal Range response streams at once) and
  are the recommended path for large repacks through a proxy.
- A stream entry that cannot start within the proxy budget is retried, then the
  job falls back to classic; it must not stay stuck.
- The durable fixes are on the server side (start the entry response early,
  avoid staging the whole object, correct `Content-Length`, no buffering) and in
  the automatic fallback, not in asking the user to click Resume repeatedly.
- A DNS-only/direct hostname removes the 100 s proxy budget but also removes
  Cloudflare's protection; use it deliberately, keep TLS/auth, and never expose
  the Sunshine web UI or the internal Ludex port.

**Field caveat (2026-11-13).** `ludex.cleeryy.cloud` resolves **directly** to
the origin (90.14.215.218, `server: Caddy`, no `cf-ray`): there is no Cloudflare
in front of it, so a 524 cannot occur and the "burst 524" scenario is not
reproducible there. It needs a separate orange-cloud hostname, or the fallback
must be tested by injecting a timeout at the proxy instead. Two related
consequences:

- **h3 from the WAN does not work**: `alt-svc: h3` is advertised, but OPNsense
  forwards TCP 80/443 only — no UDP 443. Remote clients stay on h2; the +18%
  measured for h3 is LAN-only. Either add the UDP 443 NAT rule (and the Caddy
  `443:443/udp` publish) or stop presenting h3 as a remote option.
- **No proxy buffering to blame**: a stalled classic download on that hostname
  is a disk/CPU/server problem, not a proxy budget. Check the server first.

## Troubleshooting

| Symptom | Check |
|---|---|
| `HTTP 524` on `/stream/entries/...` | Origin/proxy budget; try classic; check server staging/decompress time and Caddy/Cloudflare logs |
| `error decoding response body` | Connection closed mid-body; stall/retry log; server `client_abort`/`error`; disk space |
| `0 B/s` for >30 s | Job `stalledForMs`, retry count, server log, Caddy timeout; the UI should show reconnecting |
| Resume starts at 0% | Job id/history duplicates; `resume_install` mode; `.part` path; report with logs (#208/#205) |
| Download fails at the same byte | Archive/version changed, server file truncated, or a proxy body limit; the SHA-256 verification error shows expected/actual and whether the server changed the archive |
| `failed SHA-256 verification` | Retry from zero happened automatically; if it persists, the archive on the server differs from its recorded hash (run `POST /api/integrity/content-hashes`) or a proxy rewrites the body |
| Stream fails immediately | Format unsupported or manifest missing; classic fallback should run |
| Slow but stable | Run `scripts/bench-download.sh` and the matrix in PERFORMANCE; check LAN vs Caddy, disk and CPU |

Logs: `%APPDATA%\cloud.cleeryy.ludex\logs\ludex.log` (Windows) or the desktop
log directory; never paste tokens. Include the job id, game/version, transport,
last offset and the server/Caddy status.

## Field acceptance (40 GB)

This is the matrix that closes #211 items 1 and 4. It must be run on the real
Proxmox/Caddy/Cloudflare setup and on a second connection; automated tests
cannot prove a WAN path.

**Run 2026-11-13 on v0.4.7** — "It Takes Two", 47 935 843 067 bytes, server CT
258 (`gamevault`), Caddy v2.11.7 on CT 2010, LAN client CT 125, WAN client on a
VPS (OVH, distinct link). Evidence is in the issue; the archive's recorded
`content_sha256` was `7c793676…21ea`.

| # | Scenario | Status | Evidence |
|---|---|---|---|
| 1 | Classic 40 GB, LAN | **PASS** | 47 935 843 067 B, sha256 identical, 216 s (211.6 MB/s, half warm) |
| 2 | Classic 40 GB, WAN | **PASS** | 47 935 843 067 B, sha256 identical, 953 s, 402 Mb/s |
| 3 | 20 drops | **PASS** | 20 kills, 0 offset regression, exact-byte resume, final 256 s, sha256 identical |
| 4 | 10-minute outage | **PASS** | offset frozen at 6 400 044 056 B for 600 s, same offset on resume, 310 s, sha256 identical |
| 5 | App restart at 50 % | **NOT RUN** | needs the desktop app |
| 6 | Paused across restart | **NOT RUN** | needs the desktop app |
| 7 | Stream, 524 burst | **NOT REPRODUCIBLE** | hostname is DNS-direct (no Cloudflare) and no UDP 443 forward; needs a proxied test hostname |
| 8 | Stream restart | **PARTIAL** | stream endpoints validated server-side (manifest 6 678 entries, tar 2.08 GB, entry ranges 206/416 exact); the client checkpoint needs the desktop app |
| 9 | Queue reorder | **NOT RUN** | needs the desktop app |
| 10 | Wrong version | **PASS** | coherent ticket + unknown `versionId` → `404 version_not_found` |
| 11 | Throughput decision | **PASS** | LAN + cold A/B matrices; see [NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md#verdict-217-not-implemented) |

Protocol checks that also passed on the same run: strong `ETag` +
`X-Ludex-Archive-Sha256` match, `416` past EOF with `bytes */size`, matching
`If-Range` → 206, no ticket → 401, wrong-version ticket → 403 `ticket_mismatch`,
`invalid_job_id`/`invalid_part_index` → 400, exactly 240 requests/min then 429
with `Retry-After`, the 9th concurrent part → 429 (`parallelPartsMax` = 8),
ZIP manifest and entry ranges exact. The one defect found there — a bogus
`If-Range` returned 206 — is fixed in this batch.

Rows 5, 6, 8 and 9 are desktop-only; automated protocol tests cover the server
half and the remaining half is a manual run of the app. Record for each row: job
id, final offset, retry count, final hash/size, elapsed time, client and
server/Caddy log excerpts (never a token). Close #211 only after the table is
filled in the issue.

## Related

- [PERFORMANCE.md](PERFORMANCE.md) — buffers, 1 MiB server chunks, Caddy
  config, benchmark matrix, BBR/HTTP2 notes.
- [NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md) — direct vs proxy, VPN,
  decision thresholds, the #217 verdict.
- [CONTRACT.md](CONTRACT.md) — upload/download/stream endpoints and fields.
- Issues: #202 queue, #203 stream resume, #204 stream drops, #205/#208 same-job
  resume, #211 field acceptance, #216 full archive SHA-256, #217 parallel
  ranges.

## Sources for the launcher comparison

- Valve, SteamPipe upload/content system (chunks, manifests, HTTP, deltas):
  <https://partner.steamgames.com/doc/sdk/uploading>
- Valve, updating best practices (pack-file churn, update size):
  <https://partner.steamgames.com/doc/store/updates>
- Epic BuildPatch Tool (chunks, PatchGeneration, delta optimisation):
  <https://dev.epicgames.com/docs/epic-games-store/publishing-tools/uploading-binaries/bpt-instructions-140>
- Epic manifest/chunk layout (community reimplementation):
  <https://github.com/derrod/legendary/blob/master/legendary/models/manifest.py>
- GOG downloader (chunks/patches):
  <https://github.com/Heroic-Games-Launcher/heroic-gogdl>
- Blizzard TACT/CASC (community reverse engineering):
  <https://wowdev.wiki/TACT>
