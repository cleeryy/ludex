# Network architecture: downloads and remote play (#200)

Status: **accepted** (2026-11-12), **measured** (2026-11-13). The decision was
taken before any number existed for the user's setup; the field campaign on
v0.4.7 has since filled the matrix (sections 7 and 10) and confirmed it:
single-stream stays the only download mode, Caddy stays the only ingress, h3
stays opportunistic. Numbers below that are labelled *measured* come from that
campaign; the rest are arithmetic.

Related: [#194](https://github.com/cleeryy/ludex/issues/194) (download
performance), [#195](https://github.com/cleeryy/ludex/issues/195) (stall
detection and resume), [#179](https://github.com/cleeryy/ludex/issues/179)
(remote play / UPnP wizard), [PERFORMANCE.md](PERFORMANCE.md),
[DEPLOY.md](DEPLOY.md), [REMOTE-ACCESS.md](REMOTE-ACCESS.md).

## Decision in one paragraph

Keep **Caddy + HTTPS as the only public ingress for downloads**, authenticated
and resumable, and keep **single-stream** as the default. Do **not** add STUN or
TURN: they solve peer-to-peer NAT traversal, and Ludex has no peer-to-peer
download path; Moonlight/Sunshine does not use ICE either, so STUN does not help
remote play. Do **not** expose the Ludex server directly, and do **not** map a
port for downloads: the Caddy hop is only a suspect until `direct` and `Caddy`
are compared on the same URL. For remote play, **VPN first**; the #179 UPnP
wizard is the convenience fallback, with its lease, verification and rollback
rules. HTTP/3 is opportunistic (Caddy advertises it when UDP 443 is reachable);
it is not a default requirement. Parallel ranges were justified only at
`aria2c -x8` ≥ 1.5× the best single stream: the field run measured **1.12×** on
the WAN and **0.64×** on a cold LAN, so they are **not implemented** and the
desktop stays single-stream.

The desktop installer is **HTTP/1.1 today**: `reqwest` is built without the
`http2` feature, so the ~36 MB/s in #194/#195 is a single-TCP-flow number, not
an HTTP/2 window number. The h2 window arithmetic in section 4 applies to the
browser path and to `curl --http2`, and becomes relevant to the desktop only if
`http2` is enabled there. The first candidates for the desktop plateau are
therefore TCP BDP/congestion control, Caddy, the disk/CPU and the ISP.

### Direct answers to the question that opened the issue

- **Would opening a port (or UPnP) make downloads faster?** Only if the
  direct-vs-Caddy A/B shows Caddy is the limit; otherwise it adds exposure and
  no speed. Do not do it for downloads.
- **Would STUN help?** No. There is no peer to hole-punch to; the server is
  already reachable.
- **Would TURN help?** No; it relays and adds latency.
- **Would QUIC/HTTP3 help?** Possibly on a lossy/high-RTT WAN with UDP 443
  open; not on a clean LAN, and not for the native desktop client today.
  Measured LAN: +18% (112.5 vs 95.1 MB/s); from the WAN it is unreachable
  because no UDP 443 forward exists (section 6).
- **Would parallel ranges help?** Only if `aria2c -x8` is ≥1.5× a single
  stream. Measured: 1.12× WAN, 0.64× cold LAN — no, and the cold-LAN number
  says they actively hurt.
- **What about remote play?** VPN first; UPnP (#179) as fallback; never an
  HTTP tunnel.

## 1. Scope and explicit assumptions

In scope:

- client → Ludex server downloads over HTTP(S), through Caddy;
- remote play (Moonlight → Sunshine) reachability and transport choice;
- the decision matrix from #200, the measurements that separate its rows, and
  the security/NAT/ISP constraints of each option.

Out of scope: a custom relay/CDN, a P2P download swarm, replacing Caddy or TLS,
exposing Sunshine's web UI, and implementing an optimisation before the
measurement that justifies it.

Assumptions, stated so a wrong one can be challenged:

1. The **~36 MB/s plateau and the drops** are the runs recorded in #194/#195.
   They are not repeated here, and this repository has no access to that
   network. Any number quoted below is arithmetic, not a measurement.
2. Caddy terminates TLS on the same LAN as the server, one hop away
   (loopback or 1 GbE). A Caddy in another datacenter changes the comparison.
3. The native desktop download uses `reqwest` with `default-features = false`
   and **no `http2` feature** (`apps/desktop/src-tauri/Cargo.toml`), so it
   negotiates HTTP/1.1 with Caddy. The browser/webview path may use h2 or h3.
   Any h2 window statement below is about the browser/curl path, not the
   native installer as built.
4. The archive is a single large file on a local disk. The S3/object-storage
   path (`download_from_storage`) has its own bottleneck and is not covered by
   the local-file rows.
5. The public side is IPv4 NAT unless the user says otherwise. IPv6 is
   optional and measured separately.
6. No ISP shaping is assumed. If the link is shaped, `iperf3` and the download
   will both show it; record it instead of guessing.
7. The benchmark client is wired. Wi-Fi adds its own ceiling and must be
   measured as a separate row.
8. "Direct" means the plain HTTP server port (`LUDEX_BIND`, normally
   `127.0.0.1:8080` or a private address). The server has no TLS listener of
   its own; a public direct HTTPS endpoint would still need a proxy or a TLS
   terminator, which defeats the point.

## 2. Current path and what is already known

```
client (reqwest / browser)
   │  HTTPS 443, HTTP/2 (Caddy may also serve HTTP/3)
   ▼
Caddy (TLS; no proxy timeout, flushes each write)
   │  HTTP/1.1 to the upstream by default
   ▼
ludex-server
   │  GET /api/games/{id}/download?versionId=…
   │  ServeFile, 1 MiB chunks, Range + enforced If-Range, strong ETag
   ▼
archive on disk
```

Evidence in this repository:

- `crates/ludex-server/src/routes/games.rs::download_local` uses
  `ServeFile::with_buf_chunk_size(1024 * 1024)`, forwards `Range` and only
  honours it under a matching `If-Range`, sets an `ETag` (`size-mtime`) and logs
  every stream (`bytes`, `elapsed_ms`, `range_start`, `outcome`). It is a
  buffered `tower-http` copy, not `sendfile(2)`. The 1 MiB chunk was measured
  against 512 KiB on the field host (373 → 640-1000 MB/s of raw `dd`, see
  [PERFORMANCE.md](PERFORMANCE.md#chunk-size-211)).
- `apps/desktop/src-tauri/src/state.rs` builds `reqwest::Client::builder()`
  with keepalive, `TCP_NODELAY` and a 20 s connect timeout, and no HTTP/2
  window override.
- `apps/desktop/src-tauri/Cargo.toml` enables reqwest features
  `["rustls-tls", "stream", "json", "gzip", "multipart"]` with
  `default-features = false`; **`http2` is not enabled**. `cargo tree -e
  features -i reqwest@0.12.28` confirms it, and reqwest's `client.rs` only
  adds `"h2"` to the ALPN list under `#[cfg(feature = "http2")]`. The native
  download is therefore HTTP/1.1.
- If `http2` is enabled later, `Cargo.lock` resolves `reqwest 0.12.28` to
  `hyper 1.12.0`, whose client defaults are `DEFAULT_STREAM_WINDOW = 2 MiB`
  and `DEFAULT_CONN_WINDOW = 5 MiB` (`hyper/src/proto/h2/client.rs`). That is
  the window the client would advertise; it is not the current path.
- Caddy's `reverse_proxy` transport defaults to HTTP/1.1 upstream. The
  Caddy↔server leg is a loopback/LAN flow, not the public WAN flow.
- Remote play already has a VPN-first, UPnP-fallback design with explicit
  refusal codes and cleanup; see `docs/CONTRACT.md` ("Remote play safety
  (#179)") and `apps/desktop/src-tauri/src/remote_play.rs`.

What is **not** known from the original report: whether 36 MB/s was the ISP,
Caddy, the h2 window (browser path), the server disk, the client disk, the CPU,
or the Wi-Fi. #194/#195 recorded the symptom, not the layer. Section 10 measures
it; the 2026-11-13 campaign answered the download part on the field host (LAN,
WAN, protocol and parallelism rows) and left the desktop-only scenarios open.

**This ADR names the limiting layers only where a row fired.** On the measured
host the ceiling is the **ISP upload** on the WAN (400–480 Mb/s) and the
**storage/CPU** on the LAN (a single stream reaches ~550 MiB/s, far above the
62–112 MB/s of the contended matrix). The archive path is not code-limited.

## 3. Decision matrix

| Option | What it solves | What it does not | Cost/risk | Decision |
|---|---|---|---|---|
| Direct HTTPS / Caddy (current) | Simple, TLS, auth, one public 443 | Removes no ISP/proxy limit; adds one hop | Low | **Keep** as the only public ingress |
| Port-forward / UPnP the server | Removes the Caddy hop; exposes the server | Not faster if Caddy is not the bottleneck; loses TLS and the proxy's protections | Medium | **Reject for downloads.** UPnP only for Sunshine (#179) |
| Parallel ranged downloads | Single-flow TCP window/BDP limits | ISP/disk/server CPU limits | Medium, client only | **Measure first**; implement only at ≥1.5× |
| TCP BBR / buffer / sendfile tuning | Lossy/high-RTT WAN, syscall overhead | Local LAN line rate if disk/CPU bound | Low | **Adopt only on measured loss** (WAN), no default change |
| HTTP/3 (QUIC) | Loss recovery, multiplexing, 0-RTT; better on flaky links | Some proxies/webviews; UDP-blocked networks; CPU; not a single-stream magic fix | Medium | **Opportunistic**, keep h2 guaranteed |
| STUN | Discovers the public IP:port for hole punching between peers | Does not forward a port, does not relay, no client-server speedup | Low | **Reject**: no P2P path in Ludex |
| TURN relay | Connectivity when direct ICE fails | Adds latency and bandwidth; poor for game streaming | High | **Reject**: no use case |
| UPnP / NAT-PMP / PCP | Automatic port mapping for Sunshine (UDP/TCP) | Unreliable on CGNAT/double NAT; security; not downloads | Medium | **Keep only in #179** |
| VPN / mesh (WireGuard, Tailscale) | Remote play/control without opening ports; direct P2P or relay | Setup; relay latency; not a CDN | Low/Medium | **Recommended for remote play** |
| IPv6 | Direct reachability without NAT | Needs routable IPv6 at both ends | Low | **Optional**, measure |
| HTTP/2 enablement + client window (desktop) | Single-stream cap at `window / RTT` on high-RTT links, once h2 is on | Disk/CPU/ISP caps; h2 is not automatically faster | Low/Medium | **Candidate**, not shipped here |

## 4. Is Caddy/direct HTTPS the bottleneck?

Short answer: **Caddy is not a fixed 36 MB/s ceiling, and it cannot be declared
guilty without the A/B.** It is a suspect because it is the only extra hop,
but so are the ISP, the single TCP flow, the disk and the CPU.

How to decide:

1. Run the same archive, same client, same moment, once against
   `https://…` (Caddy) and once against `http://server:8080` (direct).
2. Compare the best of each pair (h2/h1.1 on Caddy, h1.1 direct). For the
   native desktop, use the h1.1 rows; for a browser, include h2.
3. **If direct is within 10% of Caddy (0.90–1.10×), Caddy is not the limit.**
   Keep Caddy; the difference is inside run-to-run noise and TLS/HTTP overhead.
4. If direct is 0.50–0.90× Caddy, Caddy is *still* not the limit: the direct
   path is slower, so investigate it (disk, CPU, network namespace, bind
   address, missing keep-alive) before drawing a conclusion from it.
5. Below 0.50× the direct path is broken or throttled; the row is unusable as
   a reference until it is fixed.
6. If direct is more than 10% *faster*, Caddy is doing something measurable.
   Check, in this order: `flush_interval -1` is set (no buffering), Caddy CPU
   (`pidstat -C caddy 1`), the h2 window on a browser client, and the upstream
   transport (h1.1 vs h2c). Do not bypass Caddy before those are excluded.

The script prints exactly these bands. Do not read "direct is 45% slower" as
"within 10%": a 0.55× row means the bypass would be slower, which is the
strongest evidence that Caddy is not the bottleneck — not evidence that it is.

Why 10%: a single large stream varies by several percent between runs from
page-cache state, TCP slow start, and background load. A 10% band keeps the
decision from flipping on noise. Use `--runs 3` and the best run for the
comparison if the link is noisy.

What Caddy actually costs: one userspace copy per direction, TLS record
framing (AES-NI on x86 makes this cheap), HTTP/2 framing, and scheduling. It
does not buffer the archive when `flush_interval -1` is set. On a small VM the
cost is visible as Caddy CPU; on a modern host it is usually far above a
single 1 GbE stream. Measure, do not assume.

### The h2 window (browser path only)

The native desktop installer is HTTP/1.1 (section 2), so the h2 window cannot
explain the 36 MB/s recorded in #194/#195. The arithmetic matters for the
browser path and for `curl --http2`, and it becomes a risk only if `http2` is
enabled in the desktop client.

For a single HTTP/2 stream, the receiver advertises a per-stream flow-control
window. A sender cannot have more than one window of unacknowledged data in
flight, so the ceiling is approximately:

```
single-stream ceiling ≈ stream_window / RTT
```

With hyper's default 2 MiB window:

| RTT | 2 MiB window | 8 MiB window |
|---|---|---|
| 1 ms (LAN) | ~2.1 GB/s | ~8.4 GB/s |
| 10 ms | ~210 MB/s | ~839 MB/s |
| 30 ms | ~70 MB/s | ~280 MB/s |
| 58 ms | **~36 MB/s** | ~145 MB/s |
| 100 ms | ~21 MB/s | ~84 MB/s |

For the browser/curl path, a 36 MB/s plateau would be consistent with a
**2 MiB h2 window at ~58 ms RTT**. For the native desktop path, the same number
is consistent with a ~300 Mbit/s ISP cap, a disk near 36 MB/s, a CPU-bound
proxy, or a single-flow TCP BDP limit. The matrix separates them:

- `h1.1` is much faster than `h2` → h2 flow control (browser path), not Caddy.
- `iperf3 -P1` ≈ the download → the link/ISP is the limit.
- `dd iflag=direct` ≈ the download → the server disk is the limit.
- `pidstat` shows a saturated core → CPU is the limit.
- `direct` ≈ `Caddy` → the proxy is not the limit.

Measured on the field host: `h1.1/h2 = 0.84×` (h2 is *faster*, so no flow-control
problem), `direct/caddy = 0.55×` (the bypass is 45% slower), and a download at
~5% of `iperf3 -P1`. None of the Caddy/h2 suspects fired.

Caveat: `curl`'s HTTP/2 stack is nghttp2, not hyper. Its window may differ from
a browser's. The `h1.1` vs `h2` A/B is still valid because it removes h2 flow
control entirely. The native desktop baseline is the **h1.1** row (M3/M5), not
the h2 row (M4); use `aria2c` (HTTP/1.1, 8 connections) as the parallel-range
reference.

## 5. STUN, TURN, UPnP: what they solve (and do not)

### STUN

STUN (RFC 8489) is a client-server protocol used by ICE to discover the
**reflexive transport address** (the public IP:port a NAT assigns) and to
probe NAT behaviour. It does not create a port mapping, does not forward
traffic, does not relay, and does not authenticate anything. It only tells a
peer what address the outside world sees so two peers can *try* to send packets
to each other.

- **Downloads:** the Ludex server is already reachable through Caddy. There is
  no second peer to hole-punch to, so STUN adds nothing. It cannot make a
  client→server TCP/HTTP stream faster.
- **Remote play:** Moonlight/Sunshine is **not** ICE-based. It uses fixed
  TCP/UDP ports (47984/47989/48010 TCP; 47998/47999/48000/48010 UDP, plus
  48002 for the microphone). STUN has no place in that protocol. The relevant
  options are a VPN, UPnP (#179), or a manual port-forward.

### TURN

TURN (RFC 8656) allocates a **relayed** transport address and forwards every
packet through a public relay when direct connectivity fails. It is the
fallback of last resort for WebRTC. For Ludex it would mean paying for a
public relay, adding a network hop to every download (or game frame), and
carrying the user's bandwidth twice. It is unsuitable for game streaming
(latency and jitter) and unnecessary for downloads (Caddy already relays
HTTP). **Not adopted.**

### UPnP / NAT-PMP / PCP

These ask the home router to create an inbound port mapping. They work only
when the router exposes an IGD, the WAN address is public, and the mapping is
allowed. They fail on CGNAT, double NAT, and routers with UPnP disabled or a
"public IGD" exposed to the WAN. They are also a security surface: a mapping
is a public door.

- **Downloads:** irrelevant. Caddy already accepts inbound 443. Mapping 8080
  would expose the unencrypted server and bypass the proxy.
- **Remote play:** this is exactly what #179 implements, with a short lease,
  verification, rollback, refusal on CGNAT/double NAT/public IGD, and a hard
  list of ports that never get mapped (TCP 47990 Sunshine Web UI, UDP 5353
  mDNS). Keep it as the convenience fallback, not the default.

## 6. QUIC / HTTP/3

HTTP/3 runs over QUIC (RFC 9000, HTTP/3 in RFC 9114). It brings:

- **no TCP head-of-line blocking across streams** (one lost packet does not
  stall unrelated streams);
- faster loss recovery and no retransmission ambiguity;
- 0-RTT resumption for repeat connections;
- connection migration (Wi-Fi ↔ cellular).

What it does **not** do: it does not remove **intra-stream** ordering. A single
large download is one stream; lost packets still stall that stream until
recovery, just as with TCP. The gain on one stream comes from better loss
detection and recovery, not from multiplexing.

When it helps:

- lossy or high-RTT WAN links (mobile, congested Wi-Fi, long-haul);
- many concurrent requests (the web UI), where cross-stream HOL matters;
- clients that already support h3 and a network that passes UDP 443.

When it hurts or does nothing:

- a clean LAN where TCP is already at line rate;
- a single stream limited by disk, CPU, or the ISP;
- networks that block or throttle UDP 443 (corporate, hotel, some ISPs);
- CPU-constrained hosts: QUIC is userspace and costs more per byte than
  kernel TCP;
- the native desktop download: `reqwest`'s `http3` feature is not enabled in
  `apps/desktop/src-tauri/Cargo.toml`, so the Tauri download path stays
  HTTP/1.1 (neither `http2` nor `http3` is enabled).
  Browsers/webviews may use h3 if the platform supports it.

Caddy serves HTTP/3 automatically when it terminates TLS and advertises it via
`Alt-Svc`. Requirements: **UDP 443 must reach Caddy end-to-end**. In Docker,
publish `443:443/udp` in addition to `443:443/tcp`; on a host firewall, allow
UDP 443. If UDP is blocked, clients fall back to h2 automatically; `Alt-Svc` is
advisory, so this is safe.

**Field caveat (measured).** `alt-svc: h3=":443"` is advertised on
`ludex.cleeryy.cloud`, but OPNsense forwards **TCP 80/443 only** — there is no
UDP 443 rule. Consequence: HTTP/3 works on the LAN and is unreachable from the
WAN, so a remote client silently stays on h2. Measured LAN gain: 112.5 vs
95.1 MB/s (1.18×) on a 48 GB archive, on a path that is already disk-bound —
real but not decisive. Decide explicitly: either add the UDP 443 NAT rule (and
the Caddy `443/udp` publish) or drop the claim that remote clients can use h3.
Do not treat a LAN-only h3 number as a WAN result.

Decision: keep h2 as the guaranteed path, leave h3 enabled/opportunistic, and
adopt h3 only if the matrix shows ≥15% on the user's lossy path **and** UDP 443
works. No server code change is needed.

## 7. Parallel ranged downloads

`aria2c -x8 -s8` splits the file into ranges and fetches them over 8
connections. It helps when a single flow cannot fill the link:

- high RTT and loss: one TCP congestion window cannot grow to the bandwidth-
  delay product, so 8 flows aggregate more;
- an h2 stream window cap (each stream has its own window);
- a per-connection proxy or server limit.

It does not help when:

- the link is already saturated (`iperf3` ≈ download);
- the server disk or CPU is the limit;
- Caddy is the limit (one proxy process still does the work, often more of it);
- the ISP shapes the subscriber's aggregate, not a single flow.

Costs: 8× read syscalls and disk seeks (bad on HDD, fine on SSD/NVMe), more
server CPU and memory, more failure modes, and per-user fairness. Resume needs
per-range bookkeeping: one `.part.N` per range, `pwrite` at the range offset,
resume by reading each part's length, and the existing SHA-256 verify after
concatenation. It is a client-side feature with a server-side concurrency cap,
not a protocol change.

**Threshold: if `aria2c -x8` is >1.5× the best single stream, implement
parallel ranges. Between 1.2× and 1.5×, fix the single flow first (BBR; the h2
window only matters on the browser path). Below 1.2×, the link/disk/CPU/proxy
is the limit and parallel ranges only add load.**

### Verdict (#217): not implemented

The campaign measured the threshold on the real infrastructure (v0.4.7,
2026-11-13) and it does not fire:

| Path | Single stream | `aria2c -x8` | Ratio |
|---|---|---|---|
| WAN (VPS OVH, distinct link, same 4.9 GB archive) | 54.1 MB/s (432 Mb/s) | 57.6 MiB/s (483 Mb/s) | **1.12×** (bit rate) |
| LAN, cold, host idle (8 GiB slices) | **548 MiB/s** | 348 MiB/s | **0.64×** |
| LAN matrix, host under load | 112.5 MB/s (h3) | 119.9 MB/s | 1.19× (vs h1.1 1.50×) |

- WAN: the fiber upload is saturated at ~400–480 Mb/s; no client-side
  parallelism can exceed it.
- LAN cold: eight concurrent ranges are **1.6× slower** than one stream. The
  single stream already reaches ~4.4 Gb/s; extra readers add seek/CPU contention
  on the thin pool.
- The 1.50× row is measured against `direct h1.1`, not against the best single
  stream, and while two real user downloads saturated the storage. It does not
  describe the nominal regime.

**Decision: the desktop client stays single-stream.** The server prerequisites
stay in place and are cheap: `X-Ludex-Job-Id`, `?partIndex=`, the job-scoped
budget, the 8-part cap and the `parallelPartsMax` capability. They cost nothing
when unused, are tested, and make a future measurement a client-only change.
Reopen only if a path shows ≥1.5× on *disjoint cold slices* (section 10).

**Where the cold-LAN throughput actually goes:** the read chunk size, not the
request count. `dd` on the same zone moved from 373 MB/s at 512 KiB to
640–1000 MB/s at 1 MiB, and `ServeFile` now uses 1 MiB
([PERFORMANCE.md](PERFORMANCE.md#chunk-size-211)).

**Server readiness (#217).** The server foundations are in place; the client
implementation stays gated on the measurement above. A single-stream client
keeps the #195 budget unchanged (120 requests per address + version per
minute). A client that names its job with `X-Ludex-Job-Id` (or `?jobId=`) gets
a job-scoped budget instead:

| Scope | Budget |
|---|---|
| one job on one address | 240 part requests/min |
| one address across all jobs | 600 part requests/min |
| one job on one address | 8 concurrent part streams (`parallelPartsMax`) |
| one address across all jobs | 16 concurrent part streams |

`GET /api/config` publishes `parallelPartsMax`, `?partIndex=` labels a range
in the log, and each `download stream closed` line carries `job`, `part`,
`range_start`, `range_end`, `bytes`, `elapsed_ms` and `outcome` (never a
token). A refused part is `429 too_many_download_attempts` with `Retry-After`;
it backs off one part, not the job. The server still serves one range per
request (no `multipart/byteranges`).

## 8. Recommended architecture

### 8.1 Downloads

```
Internet / LAN
   │  HTTPS 443/tcp (h2 or h1.1; h3 opportunistic on 443/udp)
   ▼
Caddy ── TLS, no proxy timeout, flushes each write, optional BBR on the host
   │  HTTP/1.1 (default) to 127.0.0.1:8080 or the private address
   ▼
ludex-server ── auth, Range, If-Range, ETag, 1 MiB chunks, per-stream logging
   ▼
/games (local disk or S3)
```

Rules:

1. **One public ingress: Caddy on 443.** Keep the server on
   loopback/private/VPN. With Compose, publish `127.0.0.1:8081:8080` (the
   container listens on `0.0.0.0`); with the binary, set
   `LUDEX_BIND=127.0.0.1:8080` or a private/VPN address. Never publish 8080 to
   the internet.
2. **Every download is authenticated.** JWT/device token or a 15-minute
   ticket; Range resume is supported. Single-stream clients keep the
   per-address/version reconnect limit (120/min, #195); a named parallel job
   uses the job-scoped budgets and 8-part cap above (#217).
3. **No STUN/TURN/UPnP for downloads.** No P2P swarm.
4. **Measure before tuning.** Order of work:
   a. prove whether Caddy is the limit (direct vs Caddy);
   b. prove whether the single flow is the limit (download vs `iperf3`; h1.1
      vs h2 on the browser path);
   c. only then parallel ranges (≥1.5×) or BBR (measured loss, WAN only);
   d. h3 only if the path is lossy and UDP 443 works.
5. **Do not enable compression on archives.** Caddy's `encode` skips
   already-compressed types; never force it on `/api/games/*/download`.
6. **Do not add a second proxy hop** (Cloudflare proxying, another tunnel) in
   front of large downloads unless it is measured; buffering proxies are a
   common cause of stalls.

**Doc vs production (7.7).** The vhost that runs on the field host is
`encode zstd gzip` + `reverse_proxy gamevault:8081`, with **no `transport`
block**. Caddy's defaults already mean "no proxy read/write deadline, flush each
write", so the tuning block in [PERFORMANCE.md](PERFORMANCE.md) changes nothing
measurable there; it is a starting point for a host whose defaults differ, not a
description of production. Nothing in the measurements above depends on it.

### 8.2 Remote play

```
Moonlight client ── WireGuard / Tailscale ── Sunshine host        (recommended)
        └─ fallback: UPnP mapping with lease + verification (#179)
        └─ fallback: manual port-forward (advanced, firewall + strong PIN)
```

- **VPN first.** No public ports, no UPnP, encrypted, and it works behind
  CGNAT when the VPN has a relay (Tailscale DERP). A relay adds latency, so
  prefer direct WireGuard or a LAN/VPN path for streaming; the relay is a
  connectivity fallback, not a performance path.
- **UPnP only through #179**, with its refusals (CGNAT, double NAT, no IGD,
  public IGD), short lease, read-back verification and rollback. Ports:
  TCP 47984/47989/48010; UDP 47998/47999/48000/48010 (+48002 mic). Never
  TCP 47990 (Sunshine Web UI) or UDP 5353 (mDNS).
- **Manual port-forward** is the advanced path when UPnP is unavailable:
  forward the same ports to the Sunshine host, keep a firewall default-deny,
  use a strong Sunshine PIN, and set `origin_web_ui_allowed=lan`,
  `origin_pin_allowed=lan`, `wan_encryption_mode` ≥ 1. Close the ports when
  done.
- **No HTTP tunnel for Moonlight.** OpenTunnel/Cloudflare Tunnel/ngrok are
  TCP-oriented, add buffering and latency, and cannot carry the UDP GameStream
  ports. A tunnel is for the Ludex HTTP API, not for game streaming
  ([REMOTE-ACCESS.md](REMOTE-ACCESS.md)).
- **CGNAT:** inbound port mapping is impossible. Use a VPN with a relay, or
  IPv6 if both ends have routable addresses. #179 reports `cgnat` and
  recommends the VPN; do not fight it with UPnP.
- **Upload-bound.** Game streaming sends video from the Sunshine host to the
  client, so the host's ISP **upload** rate matters, not its download plan.
  Measure it from the client with `iperf3 -c <server> -R` (reverse: the server
  sends), and record the upload rate next to the download rate.
- **Wired beats Wi-Fi.** Wi-Fi adds jitter and loss exactly where Moonlight is
  least tolerant; prefer Ethernet on the host and, when possible, on the
  client. A 5 GHz/6 GHz link can work, but measure it instead of assuming.
- **IPv6:** if both ends have routable IPv6, Moonlight can connect directly
  without NAT; still firewall the Sunshine ports and keep Sunshine encryption
  on. Downloads still go through Caddy (or a TLS terminator); do not expose the
  plain server on IPv6.

## 9. Security boundaries

- **No unauthenticated port.** The only public listeners are Caddy 443/tcp
  (and 443/udp if h3). 8080 stays loopback/private/VPN.
- **Downloads stay authenticated and resumable.** Tickets are short-lived and
  scoped; the server never serves a path without auth; `/data` is never served
  by Caddy.
- **No new admin surface.** No Sunshine web UI exposure, no UPnP mapping of
  47990/5353, no "auth off" switch, no HTTP tunnel as an authentication
  bypass.
- **Per-user limits.** The reconnect limiter (120 attempts/min per
  address/version) stays. If parallel ranges are added, cap streams per user
  and per version, and count them against the same limiter.
- **Rate-limit keying.** Behind a proxy, the client IP is the proxy's address;
  apply a per-real-IP limit at Caddy as well. Ludex does not trust
  `X-Forwarded-For` for its limiter unless the proxy overwrites it.
- **STUN/TURN are not shipped.** If a future P2P feature needs TURN, use
  ephemeral per-user credentials, quota the relay, and never relay downloads
  through it.

## 10. Measurement matrix

Run every row from the **same client machine**, on the **same archive**, with
the machine otherwise idle. Use `scripts/bench-download.sh` for the download
rows; it prints the table and the verdicts. Record the raw output in the issue.

Set up the variables once:

```bash
BASE=https://ludex.example.com          # through Caddy
DIRECT=http://10.1.20.50:8080           # the server, no proxy
GAME=<game-id>
VERSION=<version-id>
TOKEN=<jwt>
ARCHIVE=/srv/games/<file>.zip
```

Get a 15-minute ticket (the URL is relative; prefix it with `$BASE`):

```bash
TICKET=$(curl -fsS -X POST "$BASE/api/games/$GAME/download-ticket" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d "{\"versionId\":$VERSION}" | jq -r .url)
CADDY_URL="$BASE$TICKET"
DIRECT_URL="$DIRECT/api/games/$GAME/download?versionId=$VERSION"
```

| ID | Question | Command | Record |
|---|---|---|---|
| M0 | What is the RTT? | `ping -c 20 ludex.example.com` | min/avg/max ms, loss % |
| M1 | What can one TCP flow do? | `iperf3 -c <server> -t 20 -P 1` | Mbit/s |
| M2 | What can 8 flows do? | `iperf3 -c <server> -t 20 -P 8` | Mbit/s |
| M3 | Direct server, no proxy | `curl --http1.1 -o /dev/null -w '%{http_code} %{speed_download} %{time_total}\n' -H "Authorization: Bearer $TOKEN" "$DIRECT_URL"` | HTTP, MB/s, s |
| M4 | Caddy, h2 | `curl --http2 -o /dev/null -w '%{http_code} %{speed_download} %{time_total}\n' "$CADDY_URL"` | HTTP, MB/s, s |
| M5 | Caddy, h1.1 | `curl --http1.1 -o /dev/null -w '%{http_code} %{speed_download} %{time_total}\n' "$CADDY_URL"` | HTTP, MB/s, s |
| M6 | Caddy, h3 (optional) | `curl --http3-only -o /dev/null -w '%{http_code} %{speed_download} %{time_total}\n' "$CADDY_URL"` | MB/s, or "curl has no h3" / "UDP blocked" |
| M7 | Parallel ranges | `time aria2c -x8 -s8 -k 1M --file-allocation=none -d /tmp -o bench.bin "$CADDY_URL"` | MB/s, wall time |
| M8 | Server disk read | `dd if="$ARCHIVE" of=/dev/null bs=1M iflag=direct status=progress` | MB/s |
| M9 | Server CPU during M4 | `pidstat -p "$(pgrep -f ludex-server)" 1 10` | %CPU per core |
| M10 | Caddy CPU during M4 | `pidstat -C caddy 1 10` | %CPU per core |
| M11 | Client CPU during M4 | `top` / Task Manager | %CPU |
| M12 | Client `.part` target | repeat M4 with `downloadDir` on tmpfs | MB/s |
| M13 | ZIP stream path | `curl -o /dev/null -w '%{speed_download}\n' -H "Authorization: Bearer $TOKEN" "$DIRECT/api/games/$GAME/versions/$VERSION/stream"` | MB/s (separate path) |
| M14 | Caddy view | Caddy access log for the M4 request | status, duration |
| M15 | Server view | `download stream closed` log line | range_start/range_end, bytes, job, part, elapsed_ms, outcome |
| M16 | Retransmits (WAN) | `ss -tin dst <server-ip>` or `nstat -az TcpRetransSegs` | retrans % |
| M17 | UDP 443 path | `curl -s -o /dev/null -D - -r 0-0 "$CADDY_URL" \| grep -i alt-svc` then M6 | alt-svc present? M6 works? |
| M18 | IPv6 (optional) | `curl -6 ...` and `ping6` to the server | reachable? MB/s |

One-command version for M1–M7 (the script also runs iperf3 when installed):

```bash
scripts/bench-download.sh --direct "$DIRECT_URL" --token "$TOKEN" \
  --iperf3 <server-host> --http3 --runs 3 "$CADDY_URL"
```

Notes:

- **Client path.** The native Tauri installer uses HTTP/1.1 (reqwest without
  the `http2` feature); M3/M5 are its rows. M4 (h2) is the browser/curl path.
  Record which client produced the original 36 MB/s.
- **Cold vs warm.** Run M4 twice: once cold (drop caches on the server:
  `sync; echo 3 | sudo tee /proc/sys/vm/drop_caches`, or restart the
  container) and once warm. Record both. The difference separates disk from
  network.
- **iperf3 placement.** Run the iperf3 server on the same host/network as
  Ludex, not on your laptop. If it runs on the Proxmox host instead of the
  container, say so; the container's virtual NIC is part of the path.
- **`dd` caveat.** `iflag=direct` bypasses the page cache; `ServeFile` does
  not, so compare the warm `cat "$ARCHIVE" >/dev/null` too. On filesystems
  that reject `O_DIRECT`, drop `iflag=direct` and say so.
- **Ticket expiry.** A ticket lasts 15 minutes. For a run longer than that,
  use the Authorization header (aria2c: `--header="Authorization: Bearer
  $TOKEN"`) or request a new ticket.
- **S3 path.** If the version is on object storage, M3–M7 measure the server's
  S3 fetch + relay, not the local disk. Record it separately.
- **Disjoint slices for an A/B.** A cold-vs-warm A/B on *the same* byte range is
  invalid: the first run fills the page cache (the host had 156 GB of RAM and
  55 GB in `buff/cache`), and the second serves from memory. The naive
  "single then 8 parts on one 8 GiB slice" measured 1700–2860 MiB/s for the
  parallel run — pure cache, not disk. Every mode must read a **slice that has
  never been read** in that campaign (the accepted run used 30/70/110 GiB for
  single and 50/90/130 GiB for 8 parts). `drop_caches` is not a substitute on a
  shared host with live downloads.
- **Proxy budget.** Through Cloudflare, a request that buffers for ~100 s gets
  a **524** even though the origin is fine; a DNS-only hostname (no orange
  cloud) has no such budget. Say which one the row used, because a 524 changes
  the apparent throughput.

### Field results (2026-11-13, v0.4.7, real infra)

Reference archive: "It Takes Two", 47 935 843 067 bytes (44.64 GiB), version 7.
Server CT 258 (`gamevault`, 10.1.20.58, 8 vCPU/12 GB, virtio 10 Gb/s, cubic),
Caddy v2.11.7 on CT 2010, client LAN CT 125 on the same VLAN, WAN client on a
distinct VPS (OVH). LAN matrix rows ran **while two real user downloads
saturated the storage**, so they compare protocols, not a ceiling.

| LAN row | HTTP | GiB | s | MB/s | ×best |
|---|---|---|---|---|---|
| caddy h2 | 200 | 44.64 | 480.7 | 95.1 | 0.71 |
| caddy h1.1 | 200 | 44.64 | 573.5 | 79.7 | 0.59 |
| caddy h3 | 200 | 44.64 | 406.3 | 112.5 | 0.84 |
| direct h1.1 | 200 | 44.64 | 734.5 | 62.2 | 0.46 |
| aria2c -x8 caddy | 200 | 44.64 | 381.4 | 119.9 | 0.89 |
| aria2c -x8 direct | 200 | 44.64 | 340.7 | 134.2 | 1.00 |

| WAN row (VPS OVH) | Result |
|---|---|
| `Range` 512 MiB, h2 | 30.6 → 49.0 MB/s (burst 411 Mb/s, then ~256 Mb/s) |
| `Range` 512 MiB, h1.1 | 30.7 – 31.9 MB/s |
| Full archive, single stream | 953 s → 47.97 MiB/s (402 Mb/s), sha256 matches |
| Cuphead 4.9 GB, single stream | 91.2 s → 54.1 MB/s (432 Mb/s) |
| Cuphead 4.9 GB, `aria2c -x8 -s8` | 81.7 s → 57.6 MiB/s (483 Mb/s), **1.12×** |

Baselines measured on the same day: `iperf3 -P1` 19.9 Gb/s, `-P8` 70.7 Gb/s
(the script used to print 7 990 Mb/s for `-P8` because it parsed the first
stream; fixed), `dd bs=512K iflag=direct` 373 MB/s vs `bs=1M` 640–1 000 MB/s on
an idle host, single-stream Ludex 508–580 MiB/s cold. Conclusion: the WAN
ceiling is the fiber upload (~400–480 Mb/s), the LAN ceiling is the storage, and
the 62–112 MB/s of the matrix is contention, not code.

### Numbers to record

For every row: **MB/s**, **seconds**, **GB**, **HTTP status**, **server %CPU**,
**Caddy %CPU**, **client %CPU**, and the filesystem of `downloadDir`. Plus:

- RTT min/avg/max and loss (M0);
- iperf3 single and 8 flows (M1/M2);
- server disk MB/s cold and warm (M8);
- retransmit percentage if WAN (M16);
- Caddy version (`caddy version`), server version, kernel, CPU model, disk
  model, `nproc`, `free -h`;
- client OS, wired/Wi-Fi, and the Wi-Fi generation if wireless;
- ISP down/up plan, CGNAT yes/no, router model;
- whether UDP 443 works (M17) and whether h3 was actually negotiated;
- the Caddy access-log line and the server `download stream closed` line for
  the M4 run;
- whether the original 36 MB/s was LAN or WAN, wired or Wi-Fi, Tauri or
  browser.

## 11. Decision thresholds

Apply these to the table; the script prints them automatically.

| Question | Metric | Threshold | Action |
|---|---|---|---|
| Is Caddy the limit? | `direct / caddy` | > 1.10 | Investigate buffering/TLS CPU/h2 before bypassing Caddy |
| | | 0.90 – 1.10 | Caddy is not the limit; keep it |
| | | 0.50 – 0.90 | Caddy is not the limit either: the **direct** path is slower, fix/inspect it |
| | | < 0.50 | Direct path broken or throttled; its rows are unusable |
| Is h2 flow control the limit? | `h1.1 / h2` (browser/curl) | ≥ 1.25 | Fix the single flow: larger client h2 window, or test h1.1. Not applicable to the native desktop (h1.1 only) |
| | | < 1.25 | h2 is not the limit |
| Are parallel ranges worth it? | `aria2c / best single` | ≥ 1.50 | Implement parallel ranges (separate issue) |
| | | 1.20–1.50 | Marginal; fix the single flow first |
| | | < 1.20 | Not justified; link/disk/CPU/proxy is the limit |
| Is the link the limit? | `download / iperf3 -P1` | ≥ 0.90 | Link-limited; no code change will beat it |
| Is one flow BDP-limited? | `iperf3 -P8 / -P1` | ≥ 2.00 | BBR or parallel ranges may help |
| Does h3 help? | `h3 / h2` | ≥ 1.15 and UDP OK | Keep h3 enabled/opportunistic |
| Does h3 hurt? | `h3 / h2` | < 0.90 | Keep h2; h3 optional |
| Is the server disk the limit? | `download / dd warm` | ≥ 0.90 | Disk-limited; no network change |
| Is the CPU the limit? | server or Caddy %CPU | ≥ 80% of one core | CPU-limited; tune/upgrade before network changes |
| Is the client disk the limit? | M12 vs M4 | M12 much faster | The `.part` writer/disk is part of the ceiling |

Two exact rules from the issue, restated so they are not lost:

- **If direct is within 10% of Caddy, Caddy is not the limit** (and a direct
  path that is 45% slower is proof of the same conclusion, not a contradiction).
- **If `aria2c -x8` is >1.5× a single stream, implement parallel ranges.**

## 12. What the user must provide

This repository cannot measure another user's LAN, WAN, ISP, Caddy host or
disks. The 2026-11-13 campaign supplied items 1–6 and 9 for the reference host
below; a *new* deployment still needs its own rows. Provide:

1. The full output of `scripts/bench-download.sh` (table + verdicts), with
   `--direct`, `--iperf3` and `--http3` when possible.
2. `ping` RTT and loss to the public hostname, and the same over the VPN if
   remote play is involved.
3. `iperf3 -P1` and `-P8` results, and where the iperf3 server ran.
4. Server hardware: CPU model, `nproc`, `free -h`, disk model/filesystem,
   `dd` cold and warm.
5. Caddy version and config (redact secrets), and whether Caddy runs on the
   same host/container network as Ludex.
6. ISP plan (down/up), CGNAT yes/no, router model, and whether UDP 443 works
   (`curl --http3-only`).
7. Whether the 36 MB/s run was LAN or WAN, wired or Wi-Fi, in the Tauri app
   or a browser; the archive size and the `downloadDir` filesystem.
8. For remote play: the #179 wizard status (public IP, CGNAT/double NAT, IGD),
   the VPN type (WireGuard/Tailscale/other), and the client network.
9. The Caddy access-log line and the server `download stream closed` line for
   the same run.

Without items 1–3 a new deployment stays at "measure first"; do not implement
parallel ranges or QUIC on a guess.

Revisit triggers for the reference host: `aria2c` ≥1.5× on **disjoint cold
slices**; WAN retransmits ≥0.5%; UDP 443 forwarded and h3 measured from the
WAN; routable IPv6 on both ends; 10 GbE LAN; S3-backed versions; a user report
that the plateau is on a LAN.

## 13. Decision and consequences

Adopted:

- Caddy + HTTPS is the only public ingress for downloads; the server port
  stays loopback/private/VPN.
- Downloads remain authenticated, resumable, single-stream by default.
- No STUN, no TURN, no UPnP for downloads, no P2P swarm.
- Remote play is VPN-first; #179 UPnP is the convenience fallback; no HTTP
  tunnel for Moonlight.
- HTTP/3 is opportunistic (Caddy default), h2 is the guaranteed path.
- BBR only on a measured lossy WAN, never as a default on a clean LAN.
- Parallel ranges only at ≥1.5×; measured 1.12× WAN and 0.64× cold LAN, so the
  client stays single-stream and the server prerequisites are kept dormant.

Rejected, with reasons:

- Direct/port-forward for downloads: loses TLS and the proxy, and the direct
  path measured **0.55×** Caddy — it is slower, not faster.
- STUN/TURN: no P2P use case; TURN relay cost and latency.
- UPnP for downloads: wrong tool, exposes the plain server.
- HTTP tunnel for Moonlight: TCP/buffering, cannot carry the UDP ports.
- Replacing Caddy or TLS: out of scope and not justified by any measurement.

The behaviours that changed with #211 are on the server side (enforced
`If-Range`, honest `complete`/`client_abort` outcomes, 1 MiB read chunks); the
network decisions above are unchanged. The script self-check is the test for the
doc tooling.

Candidate experiments, not shipped (each needs its own measurement and issue):

1. **Enable HTTP/2 in the desktop client.** `apps/desktop/src-tauri/Cargo.toml`
   does not enable reqwest's `http2` feature, so the installer is HTTP/1.1.
   Enabling it adds `"h2"` to ALPN; if it is enabled, also set
   `.http2_adaptive_window(true)` (or an 8 MiB initial stream window) so the
   hyper default 2 MiB window does not cap a high-RTT WAN. Expected: neutral
   on a clean LAN; could help or hurt on a WAN. This is a behaviour change and
   needs a before/after of the native installer (`http2` off/on) at the same
   RTT, plus M4 for the browser/curl h2 path; it is not the current cause of
   36 MB/s.
2. **Parallel ranges.** Only if M7 ≥ 1.5× M4/M5. Measured 1.12×/0.64×: not
   triggered. If it ever fires, the design is N ranges, one `.part.N` per
   range, `pwrite` at the offset, resume per part, the existing SHA-256 verify,
   a server-side per-user stream cap, and a fallback to single stream on
   416/429/error.
3. **BBR.** Only if M16 shows ≥0.5% retransmits and RTT > 20 ms. Apply on the
   sending host (the server for downloads), not blindly on the client.
4. **HTTP/3 for the native client.** Requires `reqwest`'s `http3` feature
   (quinn) in the desktop build and a measured gain over the current HTTP/1.1
   path; today the native path is h1.1 only.
5. **Larger `ServeFile` chunks or `sendfile`.** Only if M9 shows the server
   CPU is the limit and the disk is not. The 1 MiB shipped in #211 is the
   measured optimum for this thin pool; going further only adds per-stream
   memory.

Revisit triggers: `aria2c` ≥1.5×; WAN retransmits ≥0.5%; UDP 443 blocked;
routable IPv6 on both ends; 10 GbE LAN; S3-backed versions; a user report that
the plateau is on a LAN.

## 14. References

- [#194](https://github.com/cleeryy/ludex/issues/194) — download performance,
  the 36 MB/s plateau and the first matrix. The raw runs live there.
- [#195](https://github.com/cleeryy/ludex/issues/195) — stall detection,
  bounded reconnect and resume; the 120/min limiter.
- [#179](https://github.com/cleeryy/ludex/issues/179) — remote play wizard,
  UPnP lease/verification/rollback, CGNAT/double-NAT refusals.
- [PERFORMANCE.md](PERFORMANCE.md) — the download path, Caddy config and the
  download-specific measurements.
- [DEPLOY.md](DEPLOY.md) — reverse proxy, ports, TLS and the deployment
  checklist.
- [REMOTE-ACCESS.md](REMOTE-ACCESS.md) — VPN, reverse proxy and tunnel threat
  model for the HTTP API.
- `docs/CONTRACT.md` — "Remote play safety (#179)" and the exact port list.
- RFC 8489 (STUN), RFC 8656 (TURN), RFC 9000 (QUIC), RFC 9114 (HTTP/3).
- `hyper 1.12.0`, `src/proto/h2/client.rs`: `DEFAULT_STREAM_WINDOW = 2 MiB`,
  `DEFAULT_CONN_WINDOW = 5 MiB` (only if `http2` is enabled).
- `scripts/bench-download.sh` — the automated matrix and its `--self-check`.
