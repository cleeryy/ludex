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
| Best for | Large repacks, slow/flaky WAN, proxies with short timeouts | Small/medium ZIPs, fast LAN, saving disk space |
| Default | Yes | Opt-in (`Settings → Downloads → Stream installs`) |

A stream install is not a different download protocol; it is a different way to
consume the same authenticated archive. When the stream path cannot work
(unsupported format, missing manifest, a proxy that cannot serve an entry within
its budget), Ludex falls back to the classic resumable download instead of
looping.

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
  checkpoint is read, and the UI shows "Resuming from X".

## Stall detection and recovery

- The client judges idleness **per response chunk**, not with a global read
  timeout: no bytes for ~30 s means a stall (configurable in
  Settings → Downloads → Connection recovery).
- A stalled/dropped response is retried with exponential backoff and jitter,
  bounded per entry/job. The next request resumes at the last flushed offset.
- HTTP `408`, `425`, `429`, `5xx`, connection reset, TLS/body decode errors and
  proxy `502`/`504`/`524` are retryable. `401`, `403`, `404` and `415` are not:
  the job fails with the server's reason instead of looping.
- The job exposes `reconnecting`, `attempt`, `lastError` and `stalledForMs`;
  the UI shows "Connection lost, resuming from X%" instead of a frozen 0 B/s.
- The server rate-limits reconnect abuse per address/version (`429` +
  `Retry-After`) and logs `bytes`, `elapsed_ms`, `range_start` and `outcome`
  (`complete`, `error`, `client_abort`) for each stream.

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

## Troubleshooting

| Symptom | Check |
|---|---|
| `HTTP 524` on `/stream/entries/...` | Origin/proxy budget; try classic; check server staging/decompress time and Caddy/Cloudflare logs |
| `error decoding response body` | Connection closed mid-body; stall/retry log; server `client_abort`/`error`; disk space |
| `0 B/s` for >30 s | Job `stalledForMs`, retry count, server log, Caddy timeout; the UI should show reconnecting |
| Resume starts at 0% | Job id/history duplicates; `resume_install` mode; `.part` path; report with logs (#208/#205) |
| Download fails at the same byte | Archive/version changed, server file truncated, or a proxy body limit; compare hash/size |
| Stream fails immediately | Format unsupported or manifest missing; classic fallback should run |
| Slow but stable | Run `scripts/bench-download.sh` and the matrix in PERFORMANCE; check LAN vs Caddy, disk and CPU |

Logs: `%APPDATA%\cloud.cleeryy.ludex\logs\ludex.log` (Windows) or the desktop
log directory; never paste tokens. Include the job id, game/version, transport,
last offset and the server/Caddy status.

## Field acceptance (40 GB)

1. Start a 40 GB classic download over the real WAN path.
2. Drop the client network for 30 s three times; confirm same job id, offset
   resumes, one history row, final hash/size.
3. Restart the desktop app mid-download; press Resume; confirm the same job and
   offset.
4. Repeat with stream enabled: confirm per-entry resume, then a forced `524`
   falls back to classic once.
5. Reorder a second queued game to the top; confirm it starts next.
6. Record the result in #211 (epic) with the server/Caddy logs.

## Related

- [PERFORMANCE.md](PERFORMANCE.md) — buffers, 512 KiB server chunks, Caddy
  config, benchmark matrix, BBR/HTTP2 notes.
- [NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md) — direct vs proxy, VPN,
  decision thresholds.
- [CONTRACT.md](CONTRACT.md) — upload/download/stream endpoints and fields.
- Issues: #202 queue, #203 stream resume, #204 stream drops, #205/#208 same-job
  resume, #211 field acceptance.
