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

The numbers decide the next step: a faster `aria2c` means parallel ranges are
worth implementing; equal numbers mean the link/disk/proxy is the ceiling and
the per-stream code is already saturating it. That campaign has been run
(#217): WAN **1.12×**, cold LAN **0.64×**, both below the gate, so single-stream
stays the only mode. The exact thresholds (including the 1.5× `aria2c` gate and
the three `direct/caddy` bands) are in
[NETWORK-ARCHITECTURE.md](NETWORK-ARCHITECTURE.md#11-decision-thresholds).
