# Download performance

How Ludex moves a game archive from the server to the desktop, where the
ceiling can be, and how to measure it (#194). The reconnect behaviour that
keeps a large transfer alive is documented in the same procedure (#195).

## The path

```
archive on disk ── ServeFile (512 KiB chunks) ── axum ── Caddy (TLS, h2) ── client
                                                                  └── reqwest stream
                                                                      └── BufWriter 1 MiB
                                                                          └── .part + Range
```

- **Server** (`crates/ludex-server/src/routes/games.rs::download_local`):
  `tower_http::services::ServeFile` with `with_buf_chunk_size(512 * 1024)`,
  exact `Range`/`If-Range`, a strong `ETag` (`size-mtime`), and no compression
  for archives. Each stream logs `bytes`, `elapsed_ms`, `range_start` and
  `outcome` (`complete`, `error`, `client_abort`) at info/warn level.
- **Proxy**: Caddy terminates TLS and forwards to the server. HTTP/2 to the
  browser/desktop, HTTP/1.1 (or h2c) to the upstream.
- **Client** (`apps/desktop/src-tauri/src/install.rs::download_into`): one
  response, one `.part`, sequential reads, 1 MiB `BufWriter`, progress every
  250 ms, `Range: bytes=<flushed>-` on every reconnect. The HTTP client sets
  TCP keepalive (30 s), `TCP_NODELAY`, and a 20 s connect timeout, but no
  global read timeout: idleness is judged per response chunk (30 s default).

A single stream is the only mode. Parallel ranged downloads are deliberately
not implemented yet: the resume and integrity tests are single-offset, and a
parallel writer needs per-range `.part` bookkeeping plus `pwrite` to stay
crash-safe. Until the matrix below shows a single stream is the bottleneck on
the target setup, parallel ranges only multiply disk head movement and server
load without a measured gain. Revisit with per-range parts once a user can show
`aria2c -x8 -s8` is materially faster than `curl` on the same URL.

## Measurement matrix

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

Do not buffer the archive and do not cut an idle-but-alive transfer. A
starting point, adjusted to the deployment:

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

## What was changed here (no user network needed)

- Server: 512 KiB file chunks (was 64 KiB), explicit `ETag`, per-stream
  disconnect logging (`bytes`, `elapsed_ms`, `range_start`, `outcome`).
- Client: TCP keepalive/`TCP_NODELAY`, idle-timeout stall detection, bounded
  reconnect with backoff, `Range` resume from the last flushed byte, and
  visible reconnect state in the job (`reconnecting`, `attempt`, `lastError`,
  `stalledForMs`).
- Reconnect abuse is bounded on both ends: exponential backoff with jitter on
  the client, and a per-address/per-version limit of 120 attempts per minute
  on the server (`429` + `Retry-After`).
- Network speeds are shown in bits per second (`Mb/s`) while sizes stay in
  bytes (#199).

## What the user must measure

On the actual setup (Proxmox host, Caddy, client), collect and record:

1. Step 1 and step 2 on a large archive, cold and warm; `curl` prints MB/s.
2. Step 3 (`aria2c -x8 -s8`) on the same URL.
3. `iperf3` LAN and WAN, plus Wi-Fi vs Ethernet.
4. `iostat -x 1` and `top` on the server during step 1.
5. Caddy access log lines for the same request (status + duration), and the
   new `download stream closed` lines from the server log.

The numbers decide the next step: a faster `aria2c` means parallel ranges are
worth implementing; equal numbers mean the link/disk/proxy is the ceiling and
the per-stream code is already saturating it.
