# Download performance

How Ludex moves a game archive from the server to the desktop, where the
ceiling can be, and how to measure it (#194). The reconnect behaviour that
keeps a large transfer alive is documented in the same procedure (#195).
User-facing behaviour, resume semantics and the 524 troubleshooting are in
[DOWNLOADS.md](DOWNLOADS.md).

The network-layer decision — direct vs Caddy, HTTP/2 vs HTTP/1.1 vs HTTP/3,
`aria2c`, `iperf3`, BBR — lives in
[NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md). That document owns the
topology, the decision thresholds and the security boundaries; this page keeps
the download-path specifics. The raw measurements stay in #194/#195/#211:
record new runs there, do not copy whole campaigns into this file. The one
number that changed code is the chunk-size comparison below.

## The path

```
archive on disk ── ServeFile (1 MiB chunks) ── axum ── Caddy (TLS, h2) ── client
                                                                  └── reqwest stream
                                                                      └── BufWriter 1 MiB
                                                                          └── .part + Range
```

- **Server** (`crates/ludex-server/src/routes/games.rs::download_local`):
  `tower_http::services::ServeFile` with `with_buf_chunk_size(1024 * 1024)`,
  `Range` plus a server-side `If-Range` check, a strong `ETag` (`size-mtime`),
  and no compression for archives. Each stream logs `bytes`, `elapsed_ms`,
  `range_start` and `outcome` (`complete`, `error`, `client_abort`) at
  info/warn level.
- **Proxy**: Caddy terminates TLS and forwards to the server. HTTP/2 or
  HTTP/1.1 to the browser/desktop (the native desktop client is HTTP/1.1
  today; see [NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md)), HTTP/1.1
  (or h2c) to the upstream.
- **Client** (`apps/desktop/src-tauri/src/install.rs::download_into`): one
  response, one `.part`, sequential reads, 1 MiB `BufWriter`, progress every
  250 ms, `Range: bytes=<flushed>-` on every reconnect. The HTTP client sets
  TCP keepalive (30 s), `TCP_NODELAY`, and a 20 s connect timeout, but no
  global read timeout: idleness is judged per response chunk (30 s default).

A single stream is the only mode. Parallel ranged downloads are deliberately
not implemented: the field campaign behind #217 measured `aria2c -x8` at
**1.12×** on the WAN and **0.64×** on a cold LAN, both far below the 1.5× gate,
and the cold-LAN result says eight concurrent ranges are *slower* than one
(348 vs 548 MiB/s). See
[NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md#7-parallel-ranged-downloads).

## Chunk size (#211)

The send path copies the file through a userspace buffer; its size is the one
tuning knob that showed a measurable difference on the real host
(CT 258, thin-pool `local-lvm`, `dd iflag=direct` on the same cold zone):

| Read size | Throughput |
|---|---|
| 512 KiB | **373 MB/s** |
| 1 MiB | **640 – 1 000 MB/s** (depends on the thin-pool zone) |

So `ServeFile` now uses **1 MiB** instead of 512 KiB. It is one line and costs
1 MiB of memory per in-flight stream; the default tower chunk (64 KiB) was
already too small for 40 GB archives (#194). A single stream served by Ludex at
rest, page cache cold, measured 508–580 MiB/s (3 runs) — the code is not the
ceiling. Do not raise it further without the same A/B: past 1 MiB the copy cost
stops mattering and only memory grows.

## Measurement matrix

The network rows (direct vs Caddy, HTTP/2 vs HTTP/1.1, HTTP/3, `aria2c`,
`iperf3`) are automated by `scripts/bench-download.sh`; it prints a comparison
table and the verdicts from
[NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md#11-decision-thresholds). The
rows below add the download-specific checks (cold/warm, disk, RAM disk, ZIP
stream) that the script does not cover.

Run every line from the **same client machine**, on the **same archive**, with
the machine otherwise idle. Record seconds, MB/s, CPU (`top`), disk
(`iostat -x 1`) and the Caddy access/error logs.

Legend: `<base>` is the server URL (`https://ludex.home.cleeryy.cloud` through
Caddy, `http://<server>:<port>` direct on the LAN), `<game>`/`<version>` come
from the game page, and `<ticket>` from `POST /api/games/<game>/download-ticket`.

1. **Direct, no proxy, no TLS**

   ```bash
   curl -o /dev/null -w '%{speed_download} B/s in %{time_total}s\n' \
     -H "Authorization: Bearer <token>" \
     "<base-direct>/api/games/<game>/download?versionId=<version>"
   ```

   Run it twice: one cold (disk), one warm (page cache). The difference tells
   you whether the archive read is the limit.

2. **Through Caddy, HTTP/2 then HTTP/1.1**

   ```bash
   curl -o /dev/null -w 'h2 %{speed_download}\n' "<base>/api/games/<game>/download?versionId=<ticket>"
   curl --http1.1 -o /dev/null -w 'h1 %{speed_download}\n' "<base>/api/games/<game>/download?versionId=<ticket>"
   ```

   A large gap between the two points at HTTP/2 flow control or a proxy buffer.

3. **Parallel ranges against the same URL**

   ```bash
   aria2c -x8 -s8 -k 1M -d /tmp -o bench.bin "<base>/api/games/<game>/download?versionId=<ticket>"
   ```

   Much faster than step 1 or 2 means a single stream is the limit. About the
   same means the server, the proxy, the disk or the link is the limit.

4. **Link and disk baselines**

   - `iperf3 -c <server>` on the LAN, and once over the WAN/VPN.
   - Wi-Fi vs Ethernet on the client.
   - `dd if=<archive> of=/dev/null bs=1M status=progress` on the server disk.
   - `iostat -x 1` and `top` while the download runs.
   - A 20 GB single file vs a folder of small files; a ZIP stream install is a
     different path (`/versions/<id>/stream`) and must be tested separately.

5. **RAM disk vs SSD** for the `.part` target (`downloadDir`): if the rate
   climbs on a tmpfs, the progress writer/disk is part of the ceiling.

## Caddy configuration for large streams

Do not buffer the archive and do not cut an idle-but-alive transfer. The vhost
that actually runs on the field host is minimal:

```caddyfile
ludex.home.cleeryy.cloud {
    encode zstd gzip
    reverse_proxy gamevault:8081
}
```

Caddy's defaults already do what the tuning block below sets explicitly: no
proxy read/write deadline, and a flush of each write (no buffering). Adding the
block is therefore safe but **not** what production runs — keep the two in sync
or document the difference, and measure before assuming either one matters.
The starting point, adjusted to the deployment, is:

```caddyfile
ludex.home.cleeryy.cloud {
    # `encode` skips already-compressed archive content types; never force
    # compression on /api/games/*/download.
    encode zstd gzip

    reverse_proxy 10.1.20.50:8080 {
        # Large transfers may pause between chunks; a short write timeout
        # would kill a healthy download.
        transport http {
            read_timeout 0
            write_timeout 0
            dial_timeout 10s
            keepalive 30s
            # Let each chunk leave immediately instead of filling a buffer.
            flush_interval -1
        }
    }
}
```

- `read_timeout`/`write_timeout 0` disables the proxy-side deadline; the
  client detects real stalls per read (30 s default, configurable in
  Settings → Downloads → Connection recovery). If the proxy must keep a finite
  timeout, set it well above the client's stall timeout (e.g. 5 m).
- `flush_interval -1` streams each write; a positive interval adds latency and
  can hide progress from the client's idle detector.
- Keep TLS on. Never expose the server directly to work around the proxy.
- HTTP/2 flow control on a high-latency WAN link can cap a single stream near
  `window / RTT`. If step 2 is much slower than step 1, test `--http1.1` on the
  client or raise the h2 window before blaming the server.
- On the host, `net.ipv4.tcp_congestion_control=bbr` helps lossy WAN links; it
  does nothing on a clean LAN.
- **Access logs.** The ticket lives in the query string (`?token=…`,
  `redact_url` keeps it out of the benchmark output), so a default Caddy access
  log writes a 15-minute capability in clear text. Server-side, Ludex never
  logs it; the proxy is the leak. Strip it in the log encoder (see
  [DOWNLOADS.md](DOWNLOADS.md#tickets-and-proxy-logs)):
  `log { format filter { request>uri query { delete token } } }`.

## What was changed here (no user network needed)

- Server: 1 MiB file chunks (was 512 KiB, after the field A/B above), an
  enforced `If-Range` (a stale validator returns the full 200 instead of
  splicing), explicit `ETag`, per-stream disconnect logging (`bytes`,
  `elapsed_ms`, `range_start`, `outcome`). The outcome now distinguishes a
  finished body from a real abort by comparing the sent bytes with the
  announced window; before that, every complete transfer was logged
  `client_abort`.
- Client: TCP keepalive/`TCP_NODELAY`, idle-timeout stall detection, bounded
  reconnect with backoff, `Range` resume from the last flushed byte, and
  visible reconnect state in the job (`reconnecting`, `attempt`, `lastError`,
  `stalledForMs`).
- Reconnect abuse is bounded on both ends: exponential backoff with jitter on
  the client, and a per-address/per-version limit of 120 attempts per minute
  on the server (`429` + `Retry-After`).
- Network speeds are shown in bits per second (`Mb/s`) while sizes stay in
  bytes (#199).

## Full archive hashing (#216)

Every archive now has a full SHA-256 before a client installs it. The cost is
deliberate and bounded:

- **Server:** one sequential read per archive, in a background job, 1 MiB
  buffers. It runs after a scan/ingest/adopt and backfills at most 50 legacy
  rows per scan; `POST /api/integrity/content-hashes` hashes the rest on
  demand. For `s3` rows the object is hashed while streaming — no staging copy,
  no extra disk write. Single-flight: a second request never doubles the I/O.
- **Client:** the classic download hashes while writing, so a fresh transfer
  costs no extra read. A resumed transfer (HTTP `Range`) cannot keep a partial
  hasher across restarts, so the completed `.part` is rehashed from byte 0
  before extraction — one extra sequential read of the archive (SHA-256 is
  ~1–2 GB/s on a modern CPU, well above WAN speeds; on a 40 GB archive expect
  tens of seconds of local read, overlapped with nothing else by design).
- The digest never crosses the disk as a sidecar file and is not a secret; it
  is logged by both sides for support.


## Server-side extracted cache prototype (#231)

The remaining client cost is the archive itself: a 40 GB 7z is downloaded,
verified and extracted on the desktop. The prototype measured here extracts the
archive **once on the server**, publishes a per-file manifest and lets the
client fetch files in parallel over the same authenticated HTTPS, so no archive
is stored on the client and no client CPU is spent decompressing.

It is **off by default**. Enable it only for the campaign:

```bash
# on the server, then restart
LUDEX_EXTRACTED_CACHE=1
LUDEX_EXTRACTED_CACHE_BUDGET_BYTES=536870912000   # 500 GiB, must hold the tree
LUDEX_EXTRACTED_CACHE_TTL_HOURS=336               # 14 days
```

Cost model, so the numbers are read correctly:

- **One extraction per archive digest.** The server reads the archive once to
  extract it, then once more to hash each extracted file for the manifest. That
  second pass is the prototype's known cost (per-file SHA-256 is in the
  contract); the field NAS measured ~500–580 MiB/s sequential, so budget
  roughly `2 × archive_size / 550 MiB/s` of server disk time per 40 GB archive.
- **Cache disk = extracted size**, which is about the archive size for a 7z
  repack. The budget and the LRU/TTL sweeper keep it bounded; an entry being
  served is never evicted.
- **HTTP fan-out.** The client opens 4–8 file requests in parallel, each with
  `Range` resume and its own SHA-256. The #217 job budgets apply per job id:
  8 concurrent file streams, 240 part requests per minute per job, 16 per
  address.
- **Fallback.** `409 extracted_cache_unavailable` (disabled, not extracted yet)
  means the client uses the classic download; it is not an error loop.

### Exact 40 GB 7z procedure

1. **Hash the version first** (the cache key is the archive digest):

   ```bash
   TOKEN=...        # administrator bearer token
   BASE=https://ludex.example
   GAME=12          # game id
   VERSION=34       # version id (GET /api/games/{GAME} → versions[].id)

   curl -fsS -X POST -H "Authorization: Bearer $TOKEN"      "$BASE/api/integrity/content-hashes"
   # poll until versions[].contentSha256 is non-null
   curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/games/$GAME" |      python3 -c 'import json,sys; print(json.load(sys.stdin)["versions"][0]["contentSha256"])'
   ```

2. **Extract once, in the background**, and watch the counters:

   ```bash
   curl -fsS -X POST -H "Authorization: Bearer $TOKEN"      "$BASE/api/admin/versions/$VERSION/extract"
   # repeat until status is "ready" (files/bytes climb while extracting)
   ```

   Record the one-off wall time and `vmstat 1` / `iostat -x 1` on the server
   during this step: it is the price the client no longer pays.

3. **Run both transports** with the benchmark script. It downloads and extracts
   the archive the old way, then installs the same version through the manifest
   with `--jobs` parallel file requests, verifies every file, kills the client
   mid-download to test resume, corrupts a file to test recovery, and prints the
   table and the verdict:

   ```bash
   scripts/bench-file-manifest.sh      --url "$BASE" --token "$TOKEN" --game "$GAME" --version "$VERSION"      --jobs 8 --server-ssh pve3      --work /var/tmp/ludex-bench-40g
   ```

   `--jobs 4` repeats the manifest row at the lower concurrency; run both and
   keep the better one, as the issue asks for 4/8. `--classic-only` and
   `--manifest-only` re-run a single phase against an already warm cache.

   Pass `--extract-cmd` explicitly: the script saves the archive as
   `archive.bin`, so it falls back to `unzip`, and `unzip` refuses repacks it
   cannot inflate (`exit 81`, unsupported compression method) while `7z`
   extracts the same file fine.

   ```bash
   --extract-cmd "7z x -y -o$WORK/classic/extracted $WORK/classic/archive.bin"
   ```

4. **Sample the server separately** while the script runs, so the summary is
   not the only evidence: `vmstat 1`, `iostat -x 1`, `sar -n DEV 1`, and the
   `ludex_http_requests_total{route=...}` / `ludex_http_request_duration_seconds`
   deltas from `/metrics`. On the client, `pidstat -p <curl>` (or the desktop's
   own CPU) separates download CPU from extraction CPU — the manifest path
   should show almost no CPU, which is the qualitative win even when wall time
   is close.

5. **Decide with the gate.** The file-manifest path must be **≥ 1.5× faster
   end to end** (or remove a large client cost such as CPU/extraction without
   regressing reliability) to be kept. Record the printed table, the server
   samples and the machine/network description in issue #231. Below 1.5× the
   feature stays off (`LUDEX_EXTRACTED_CACHE=0`) and the classic download
   remains the only transport.

### Reading the result

| Symptom | What it means |
|---|---|
| Manifest row barely faster, server CPU/disk near saturated | The server disk is the new bottleneck; the classic path was not the client's limit |
| Manifest much faster, client CPU near zero | The win is client decompression and disk, not the network |
| Resume row ~= missing bytes / link speed | Per-file resume works; nothing restarts from zero |
| Corrupt row `NOT detected` | A client bug, not a server one: verification must happen before commit |
| Cache `usedBytes` grows past `budgetBytes` | Eviction is refusing active entries; check for long-lived streams |

The chunk-size and stream findings above are unchanged: this prototype does not
bring back client-side parallel ranges (#217) and does not add a chunk store.

### Homelab result: 50.9 GiB archive (the 40 GB gate)

Measured on 2026-11-17, one run, one archive, against a live `v0.4.21` server.

| Item | Value |
|---|---|
| Archive | `007 First Light.zip`, game 31 / version 31, 54 668 902 063 B (50.9 GiB) |
| Extracted tree | 87 files, 57 323 553 725 B (53.4 GiB) |
| Server | CT `gamevault` (v0.4.21), `/srv/ludex` on a LVM-thin pool of five SAS disks behind a PERC H730P (all `ROTA=1`) |
| Client | throwaway 8 vCPU / 8 GiB LXC on the same host, work directory on the same thin pool, veth shaped with `tbf` at 900 Mbit/s to emulate the 1 Gbit LAN |
| Path | `https://ludex.cleeryy.cloud` → Caddy → `gamevault:8081` (TLS, HTTP/2) |
| Server one-off extraction | 20.7 min for 53.4 GiB (extract job queued at 10:12Z, cache entry created at 10:33:21Z) |

```
phase                      wall_s client_cpu  server_cpu    server_rd    server_wr   peak_disk      moved
------------------------------------------------------------------------------------------------------------
classic                  2942.877        n/a         n/a          n/a          n/a    106798.4   106804.4
manifest (8 jobs)        1453.668        n/a         n/a          n/a          n/a     54668.0    54668.0
resume (after kill)      1682.523        n/a         n/a          n/a          n/a     54668.0    54668.0
corrupt file             3796.990        n/a         n/a          n/a          n/a     54668.0    54668.0
------------------------------------------------------------------------------------------------------------
classic archive download alone: 1103.623s (rest is verification + extraction)
server extraction (one-off): 20.7 min (measured outside the script, cache already warm)
corrupted file: detected / re-downloaded
verdict: classic/manifest speedup 2.02x (gate 1.5x, strong 2.0x) - PASS
```

Reading it:

- **2.02× on this client, and the win is the client's disk, not the link.**
  Both rows shared the same 900 Mbit/s path: classic pulled the 50.9 GiB archive
  at 49.5 MB/s for 1 104 s, then spent 1 839 s verifying 50.9 GiB and unpacking
  53.4 GiB; the manifest pulled 53.4 GiB at 39.4 MB/s in 1 454 s and was done.
  Peak client disk halves, 104.3 GiB → 53.4 GiB: that is the structural gain.
- **Bytes go up, not down.** The manifest moved 53.4 GiB against a 50.9 GiB
  archive (+5 %), so on a client that is fast at both (SSD, 1 Gbit LAN) only the
  removed extraction remains and the ratio degrades towards ~1.2-1.4×. The gate
  is cleared here because spinning disks make the classic row pay the archive
  twice.
- **Server cost, per archive and per digest:** 20.7 min wall time and
  roughly `2 × extracted_size` of disk traffic, plus the tree held on the server
  (budget 200 GiB, TTL 14 days). On an all-spinning-disk server this is the
  dominant cost of the feature.
- **`client_cpu` and the server columns are `n/a` here.** The client was a
  container, so `/proc/stat` is the host's and the per-phase delta is not
  attributable, and no SSH sampler was wired. Host sampling during the run
  (`/proc/stat` + `/proc/diskstats`, 10 s) gives 14.3 % CPU busy (p95 19.3),
  pooled disk reads at 309 MB/s median (p95 566) and writes at 197 MB/s median
  (p95 560): the server was not the bottleneck.
- **Resume (1 683 s) and corrupt (3 797 s) are dominated by re-hashing the whole
  tree**, not by the transfer — every pass verifies each file before committing,
  and 53.4 GiB of SHA-256 on the same spinning disks costs minutes. They prove
  behaviour (85 kept / 2 re-fetched, then `detected / re-downloaded` and a
  converging second pass), not throughput.
- Two script defects were found and fixed during the campaign: the recovery
  check matched `download.log` with whitespace splitting, so any path with a
  space reported `NOT detected` (PR #273), and `unzip` cannot inflate this
  repack (see the `--extract-cmd` note above).
- **`--jobs 4` was not run.** The procedure asks for both and to keep the
  better one; here the 8-job row never reached the shaped link (39.4 MB/s
  against ~107 MB/s available), so concurrency was not the limiter and a
  four-job repeat would only have spent another 25 min of spinning-disk time.

**Gate reading for #231.** The 1.5× gate is cleared **on a storage-bound
client** (2.02×, peak disk halved, recovery verified). It is not cleared by this
campaign on a desktop-class client, where both rows are network-bound and the
manifest only removes extraction. Recommendation carried to the issue: keep the
cache **opt-in** (`LUDEX_EXTRACTED_CACHE`, default `0`), document the
per-archive server cost above, and re-run the gate on a real desktop before
making the manifest the default transport.


## Archive extraction (#232)

The classic path downloads an archive, verifies it and unpacks it on the
desktop. Extraction was single-threaded for every format, with a 1 MiB write
buffer and one entry at a time. That is the right place to look when a 40 GB
repack takes minutes after the download already finished.

### What changed

- **ZIP: parallel entries.** The entry list is validated in one pass (names,
  `..`, links, `ensure_within`) and the directory entries are created before
  any worker starts, so an unsafe entry is rejected before a byte is written
  and the failure never depends on which worker was scheduled first. Up to
  eight workers then inflate independent entries, each with its own
  `ZipArchive` handle (the `zip` crate hands out one mutable entry borrow at a
  time, so a shared handle would serialise the inflate again). Work is claimed
  from an atomic index, progress is aggregated on the calling thread from
  atomics plus a coalescing channel, and a failure is recorded with its entry
  index so the lowest index wins. Auto worker count is
  `available_parallelism`, capped at 8, and stays at one for archives under
  4 MiB where thread start-up costs more than the inflate;
  `ExtractOptions::zip_workers` / `LUDEX_EXTRACT_WORKERS` override it.
- **7z: native `7zz`/`7z` when installed.** 7-Zip's LZMA2 decoder is faster
  than `sevenz-rust2` and `-mmt=on` decodes independent blocks in parallel.
  The binary is detected once per environment value (`LUDEX_SEVEN_ZIP` forces a
  path, `crate`/`off` forces the Rust decoder), progress is parsed from
  `-bsp1` percentages, and every expected file is checked for presence and
  size afterwards. Any failure — missing binary, exit code ≥ 2, a file that is
  not there, an encrypted archive (7-Zip would take the password on its command
  line) — falls back to the crate. Nothing is bundled.
- **RAR: unchanged backend, honest about it.** RAR5 solid archives are one
  compressed stream; entry N+1 cannot be decoded before entry N. libunrar
  stays, directories are pre-created, libunrar writes each entry directly (no
  scratch copy on this path), and cancel is polled between entries. A 4 MiB
  user-space buffer is not applicable here: libunrar owns the write syscalls,
  so the archive layer cannot size them.
- **I/O.** 4 MiB `BufWriter` per extracted entry and 1 MiB read chunks (was
  64 KiB); 4 MiB is where a sequential write stops issuing a syscall every
  8 KiB. The #223 digest sidecar already prevents a second full read: an
  archive whose sidecar matches is not rehashed, and a fresh download hashes
  while writing, so extraction never re-reads the archive to verify it.
- **Progress, ETA, cancel.** `ExtractProgress` now carries
  `bytes_per_second`, `entries_per_second` and `eta_seconds` (the average since
  the call, not a rolling window, so the ETA does not jump when a large entry
  starts). `ExtractOptions::cancel` is checked between entries and every 1 MiB
  chunk and surfaces as `ArchiveError::Cancelled`; the desktop maps it to a
  cancelled job and keeps the staging directory. The job message reads
  `Extracting 1234/5678 files · 412 MB/s · 2 min left`.

### Measured on the dev box

Host: CT 119, 8 vCPU, Proxmox `home`, thin-provisioned SSD, page cache warm,
archive read and extracted on the same filesystem, Linux, release build.
Archive: 512 MiB (8 × 64 MiB) of mixed text/random data, 157 MiB deflated ZIP.
Command: `scripts/bench-extract.sh --bin <target>/release/examples/extract
--workers 1,2,4,8 --runs 2 --cancel-after 5 <archive>`.

| backend | workers | wall | MB/s | cores | peak RSS |
|---|---|---|---|---|---|
| zip | 1 | 1.33 s | 123 | 1.0 | 5–6 MiB |
| zip | 2 | 0.69–0.70 s | 235 | 1.9–2.0 | 8 MiB |
| zip | 4 | 0.44–0.53 s | 310–373 | 3.2–3.3 | 12–13 MiB |
| zip | 8 | 0.35 s | 469 | 4.4 | 23 MiB |

Best of 2–3 runs; the spread is the thin-pool write cache, not the code.

**Parallel vs sequential ZIP: 3.80× — passes the 1.5× gate.** Copies are
bandwidth-bound past four workers (4.4 cores busy, 469 MB/s), which is the
expected shape on an SSD; a slower disk or a NAS flattens it sooner.

7z, same payload (`7z a -mx=5 -mmt=on`), 512 MiB extracted:

| backend | wall | MB/s | cores | peak RSS |
|---|---|---|---|---|
| sevenz-rust2 | 17.1–17.3 s | 9.4–9.5 | 1.0 | 13 MiB |
| system `7z` 23.01, non-solid | 3.4–3.6 s | 45–48 | 4.3–4.5 | 337 MiB |
| system `7z` 23.01, `-ms=on` (solid) | 4.2 s | 38.8 | 3.8 | 337 MiB |

**Native vs crate 7z: 4.8–5.0× non-solid, 4.1× solid — passes the gate.** Two
effects stack: a faster decoder (~1.7×) and parallel block decode. 7-Zip ≥ 22
decodes LZMA2 on several threads even for a solid block; the older `p7zip`
16.02 does not, so on a distro that ships only 16.02 expect the decoder speedup
and no `-mmt` win on solid archives. Peak RSS is 7z's dictionary (~337 MiB for
`-mx=5`), not ours; a `-mx=9` scene repack needs more (1 GiB+), which is why the
crate fallback stays.

Cancellation: **0.00 s measured latency** (sub-10 ms, below the 10 ms clock
resolution of `time`) after a 0.17 s request on 8 workers, exit code 3
(`Cancelled`), staging directory kept.

RAR: not benchmarked on this host (no RAR creator installed); the fixtures
cover correctness. RAR5 solid extraction is sequential by design — libunrar
saturates one core, and claiming otherwise would be a lie. The only wins there
are I/O and cancel, not threads.

### Inflate backend: measured, kept `flate2`

`libdeflate` is a one-shot (whole-buffer) decoder, which does not fit the
streaming `Read` path without loading entries in memory, so the honest
comparison is `zlib-ng` as a drop-in for `flate2`/`miniz_oxide`. Measured with
`zip = { features = ["deflate-zlib-ng"] }` on the same archive:

| workers | miniz_oxide | zlib-ng | gain |
|---|---|---|---|
| 1 | 1.33 s | 1.14 s | 1.17× |
| 8 | 0.35 s | 0.32 s | 1.09× |

Below the 1.5× gate, and it would add `cmake` + a C compiler to every target
the desktop app builds for (Windows, macOS, Linux) for a 9 % end-to-end win on
an already parallel path. **Kept `flate2`/`miniz_oxide`.** To revisit, add the
feature to `crates/ludex-core/Cargo.toml` and re-run the table above.

### Disk, Defender and long paths (advice, with the trade-offs)

- **Local SSD, not SMB/NAS.** Extraction reads the whole archive once. When
  `downloadDir` is a network share and the install is local, the archive is
  read over SMB for the digest *and* again for extraction; `downloadStaging =
  installVolume` (#226, the default since #235) writes the `.part` next to the
  install and deletes it after extraction, so both passes are local.
- **Windows Defender.** Real-time scanning runs on every extracted file; on a
  100k-file tree that can dominate the wall time. Excluding the install and
  download folders removes it. Trade-off: files written there are not scanned,
  so an infected repack lands unscanned — only do this for a library you trust,
  and run a scheduled full scan instead. Never disable Defender globally. Add
  the exclusion for the two folders, not for `C:\`.
- **Long paths (Windows).** Enable `LongPathsEnabled=1`
  (`HKLM\SYSTEM\CurrentControlSet\Control\FileSystem`) before extracting deep
  game trees; without it the OS returns `ERROR_PATH_NOT_FOUND` at 260
  characters, which surfaces as an extraction failure naming the entry.
- **Antivirus/backup agents** that open every new file (OneDrive sync of the
  install folder is the usual culprit) turn a streaming write into a random-I/O
  storm. Keep the install root out of cloud-sync folders.

### The 40 GB procedure

1. Put the archive on the disk you actually install from (local SSD) and pick a
   scratch root on the same filesystem, e.g.
   `ARCHIVE=/srv/games/Game-Repack.zip`, `SCRATCH=/var/tmp/ludex-extract-40g`.
2. Build the harness once, in release:

   ```bash
   CARGO_TARGET_DIR=/root/ludex-target cargo build --release -p ludex-core --example extract
   BIN=/root/ludex-target/release/examples/extract
   ```

3. Run the rows, one format per archive. `--runs 1` is enough at this size; the
   run is minutes, not milliseconds.

   ```bash
   # ZIP: 1 worker (sequential) vs 2/4/8 (parallel); cancel latency at the end
   scripts/bench-extract.sh --bin "$BIN" --dest "$SCRATCH" \
     --workers 1,2,4,8 --runs 1 --cancel-after 20 "$ARCHIVE"

   # 7z: crate vs the system binary, then repeat with a solid archive
   scripts/bench-extract.sh --bin "$BIN" --dest "$SCRATCH" --runs 1 "$ARCHIVE.7z"

   # RAR: one sequential row; the script prints the honest note
   scripts/bench-extract.sh --bin "$BIN" --dest "$SCRATCH" --runs 1 "$ARCHIVE.rar"
   ```

4. While a row runs, sample it: `iostat -x 1`, `vmstat 1`,
   `pidstat -p <pid> 1` (CPU per thread is the evidence for or against
   parallelism), `nproc`, and free memory.
5. Cold vs warm: `sync; echo 3 > /proc/sys/vm/drop_caches` between rows (Linux,
   root; on Windows use RAMMap `-E`) and record both. A cold run is the honest
   number for a first install.
6. Defender A/B, Windows: one row with the install/download folders excluded
   (`Add-MpPreference -ExclusionPath`), then remove the exclusion
   (`Remove-MpPreference -ExclusionPath`) immediately after and record both.
   State the exclusion in the result; do not leave it in place by accident.
7. Keep the printed table, the `iostat`/`pidstat` samples and the host
   description (CPU, disk model, filesystem, Defender state, cold/warm) in
   #232. The gate is 1.5× on the same archive and host; ZIP and 7z must show
   it, RAR is expected not to.
8. Cancellation: the `--cancel-after 20` row prints the measured latency
   (`wall − 20 s`); pass it in #232 with the staging directory size it left
   behind.

### Reading the result

| Symptom | What it means |
|---|---|
| ZIP speedup < 2× with 8 workers | The disk or the network share is the ceiling; check `iostat` and `pidstat` per thread |
| ZIP barely faster, one core busy | Workers are not being used: check `LUDEX_EXTRACT_WORKERS`, the archive entry count and the 4 MiB auto threshold |
| 7z native row ≈ crate row | The system binary is p7zip 16 (no MT decode) or the archive is solid; check `7z i` |
| 7z native much faster but RSS > 1 GiB | `-mx=9` scene archive: the dictionary, not a leak; the crate path stays as a memory-safe fallback |
| RAR slower than everything | Expected: solid, sequential, one core |
| Cancel latency > 1 s | A single entry larger than the remaining work: cancel is polled per 1 MiB chunk, so this should not happen — report it |

## Hashing while extracting (#240)

The install used to hash every extracted file a second time, after extraction,
to build the per-file manifest (`Hashing X of Y files` in `finalizing`). That
was a full extra read of the installed tree — 40–100 GB — while the
extraction had already streamed every byte once.

The extractor now tees each written byte into a per-file SHA-256 and
`ExtractReport::hashes` carries `{path, size, sha256}`; the manifest is
assembled from that result, so no second pass exists. RAR hashes the single
copy libunrar already wrote (scratch → hash → rename), the native 7-Zip path
hashes during its presence/size verification read, and the per-entry stream
hashes while writing and only re-reads entries it adopts from an earlier
attempt. A legacy install, a repair or a single-file copy still walks the tree
(`manifest_for_install(None, …)`), because no transport digest exists.

Measured on the dev box (CT 119, Xeon Silver 4208, **no SHA-NI**, so SHA-256
runs at ~145 MiB/s/core; page cache warm). 64 entries × 4 MiB = 256 MiB ZIP,
same archive and host for both rows; "old" = extract without hashes then
`rehash_tree` with the production 4 MiB reader, "new" = extraction with the
tee:

| workers | old (extract + rehash) | new (extract + hash) | saved |
|---|---|---|---|
| 1 | 2.112 s (0.327 + 1.785) | 2.058 s | 3 % |
| 8 | 1.895 s (0.099 + 1.796) | 0.444 s | **77 % (1.45 s)** |

Read it correctly:

- The win is parallelism for ZIP: the old walk hashed on one thread, the tee
  hashes on the workers. On a CPU with SHA-NI the same 256 MiB hashes in
  ~0.2 s on one core, so both columns shrink and the saving approaches the
  removed **read**.
- For RAR (sequential by design) the hash CPU is unchanged and the tee merely
  overlaps it with the write; the saved cost is the second read of the tree,
  which the warm page cache hides here and a NAS/SMB or cold SSD does not.
  Do not promise a wall-clock number for RAR from this table.
- A 40 GB ZIP extrapolates to roughly 4–5 minutes saved on a host without
  SHA-NI and 20–30 s with it, before counting the disk/Defender cost of the
  removed pass.

Reproduce with the ignored measurement (release, prints the table above):

```bash
cargo test -p ludex-core --release -- --ignored --nocapture benchmark_hash_while_extracting
```

The install log now carries `archive_read_bytes`, `tree_written_bytes`,
`tree_read_bytes` (0 for a normal install) and `hashed_bytes` (0 when the
digests came from the extraction), plus `from_extraction` and
`trusted_archive_sha256` on the `install manifest recorded` line.

## What the user must measure

On the actual setup (Proxmox host, Caddy, client), collect and record:

1. `scripts/bench-download.sh --direct <direct-url> --token <token>
   --iperf3 <host> --http3 --runs 3 <caddy-url>` — the table and verdicts.
   `--self-check` validates the script offline before using it.
2. The same rows by hand if the script cannot reach a URL (matrix steps 1–3);
   `curl` prints MB/s. Run the Caddy row cold and warm.
3. `iperf3` LAN and WAN, plus Wi-Fi vs Ethernet (the script runs it with
   `--iperf3`).
4. `iostat -x 1` and `top` on the server during the Caddy run.
5. Caddy access log lines for the same request (status + duration), and the
   new `download stream closed` lines from the server log.
6. For extraction, the `scripts/bench-extract.sh` table of one large archive
   per format (procedure and gate above); `--self-check` validates the parsing
   and verdict logic offline.

The numbers decide the next step: a faster `aria2c` means parallel ranges are
worth implementing; equal numbers mean the link/disk/proxy is the ceiling and
the per-stream code is already saturating it. That campaign has been run
(#217): WAN **1.12×**, cold LAN **0.64×**, both below the gate, so single-stream
stays the only mode. The exact thresholds (including the 1.5× `aria2c` gate and
the three `direct/caddy` bands) are in
[NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md#11-decision-thresholds).
