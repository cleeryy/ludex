# Console TV performance (#270 §3)

The Console is a 10-foot UI on a device that may be a small Windows box behind a
TV. This page is the repeatable measurement of its frame budget and memory, with
the machine and the commit that produced each number. It is not a benchmark
sheet: a number here is either from `e2e/tests/09-console-perf.spec.ts` or it is
labelled as manual.

## Budgets (from #270)

| Budget | Target | Unit |
|---|---|---|
| Rail scroll | 60 fps (16.7 ms frame) | p95 ≤ 16.7 ms on the weakest target |
| Memory, FHD | < 300 MB | process, 1920×1080, after 30 s idle |
| Memory, UHD | < 380 MB | process, 3840×2160, after 30 s idle |
| Long tasks on the rail | none > 50 ms | main thread |

The probe asserts the frame budget only on a hardware renderer. This host has no
GPU, so the numbers below are recorded and analysed instead of turned into a
false pass; the native process budget is in
[NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md).

## Machine and method

| Item | Value |
|---|---|
| Commit | `71075f4` (main, harness from #271) |
| Host | CT 119 `code` on pve3, Linux 7.0.14-29-pve x86_64, **8 vCPU**, no GPU |
| Renderer | `ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero)), SwiftShader driver)` |
| Browser | Chromium 158 headless (Playwright) |
| Server | `scripts/e2e.sh` harness: one admin, a seeded shelf of **48** games, real API |
| Input | fake standard pad through `navigator.getGamepads`; one held D-pad right for 3 s |
| Viewport | 1920×1080 (FHD) and 3840×2160 (UHD), `--bp-u` scaling active |
| Host load | 10.6–13.9 (8 vCPU) during the reported runs |

The probe:

1. writes 48 deterministic 1.2 MB archives into the harness games directory and
   triggers `POST /api/library/scan` — the same path a real library takes;
2. signs in, loads `/#/console`, measures cold boot and a warm reload to the
   first cover;
3. starts a requestAnimationFrame sampler, holds D-pad right for 3 s and records
   every frame delta;
4. reads long tasks (`PerformanceObserver`, `longtask`), the settled rAF picture,
   and the JS heap after a forced GC at 5 s and after 30 s idle.

Tracing and video are off for this test (`test.use`), because Playwright's
recorder instruments every frame and would measure itself.

## Results

### FHD, 1920×1080

Three consecutive runs on the same commit and harness:

| Metric | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Cold boot to first cover | 987 ms | 667 ms | 635 ms |
| Warm reload to first cover | 507 ms | 362 ms | 323 ms |
| First Contentful Paint (cold / warm) | 252 / 408 ms | 272 / 172 ms | 248 / 272 ms |
| Rail frames sampled (3 s hold) | 121 | 126 | 119 |
| Frame p50 | **16.7 ms** | **16.7 ms** | **16.7 ms** |
| Frame p95 | **66.7 ms** | **50.1 ms** | **66.7 ms** |
| Frame p99 | 66.7 ms | 66.7 ms | 66.7 ms |
| Worst frame | 66.7 ms | 66.7 ms | 66.7 ms |
| Frames > 16.7 ms | 87 / 121 | 94 / 126 | 83 / 119 |
| Frames > 33.3 ms | 26 / 121 | 24 / 126 | 27 / 119 |
| Long tasks during scroll | 0 | 0 | 0 |
| Long tasks during boot | 1 · 73 ms | 1 · 67 ms | 1 · 55 ms |
| JS heap at 5 s / 30 s idle | 10.7 / 10.7 MB | 10.7 / 10.7 MB | 10.7 / 10.7 MB |
| rAF chains at idle | 1 | 1 | 1 |
| rAF callbacks / frame | 1 | 1 | 1 |

Reading it:

- **p50 is a solid 60 fps.** Half the frames land on the 16.7 ms vsync boundary.
- **p95 misses 60 fps** (50–67 ms, i.e. 15–20 fps on one frame in twenty). Two
  frames in five exceed 30 fps. That is a visible hitch on a TV when the rail
  scrolls.
- **No long tasks during the scroll.** The JS main thread is not the bottleneck:
  the frames are missed in raster/compositing, which is exactly what a
  GPU-less SwiftShader host does with a full-screen blur.
- **Memory is flat.** The renderer's JS heap is 10.7 MB before and after the 30 s
  idle window; nothing grows while the Console sits on Home.

The spread is the shared host, not the app: a later run at load 12.3 measured
p95 83.3 ms with p50 still 16.7, and runs taken while another lane compiled Rust
(load 17–22) reached p95 133–233 ms. The p50 held 16.7 ms in every settled run;
quote the p95 with its `hostLoad`.

### UHD, 3840×2160

One run at the same commit, load 11.3 (8 vCPU):

| Metric | Value |
|---|---|
| Cold / warm boot to first cover | 869 / 594 ms |
| Rail frames sampled | 36 in 4.4 s |
| Frame p50 / p95 / p99 | 16.7 / **483.4** / 583.4 ms |
| Frames > 33.3 ms | 13 / 36 |
| Long tasks during scroll | 12 · 2998 ms total, longest 519 ms |
| JS heap at 5 s / 30 s idle | 10.1 / 10.1 MB |
| rAF chains at idle | 1 |

4K on SwiftShader is a software-raster worst case: the rail drops to ~8 fps with
main-thread stalls of half a second, because every covered frame rasterizes a
3840×2160 wallpaper blur on the CPU. The JS heap stays at ~10 MB. Do not read
this as a 4K product verdict; read it as "the software path cannot be used to
approve 4K", which is why the UHD budget is a native checklist item.

## Which budget is missed, and why

| Budget | Verdict | Evidence |
|---|---|---|
| 60 fps rail (p95 ≤ 16.7 ms) | **Missed on this host** (p95 50–67 ms). p50 holds 60 fps; the misses are raster, not JS. | Table above; zero long tasks during scroll. |
| No long tasks > 50 ms on the rail (FHD) | **Met.** | 0 long tasks in all three FHD runs. |
| < 300 MB FHD | **Not measured as a process.** JS heap is 10.7 MB, far below the budget, but that is only the renderer's JS objects. | Native checklist item D3/C4. |
| < 380 MB UHD | **Not measured as a process.** JS heap is 10.1 MB. | Native checklist item D3. |

### Cause: the wallpaper blur layer

The rail sits above one full-screen, blurred wallpaper (`.bp-backdrop-art`,
`blur(26u)`, the only blurred layer in the product, `console-room.ts`). It never
animates, but the rail re-centres on every repeat (`scrollIntoView({ inline:
"center", behavior: "smooth" })` in `navigation.ts`), so the compositor moves a
48-card layer continuously over that full-screen blur. With a GPU the blur is a
cached texture; with SwiftShader it is CPU work on every composited frame.

A controlled A/B on the same harness, same held D-pad, same 3 s window, with the
wallpaper layer hidden by a style override and the two runs interleaved twice to
track host drift:

| Round | Variant | Frames | p50 | p95 | Max | Frames > 33 ms |
|---|---|---|---|---|---|---|
| 1 | baseline | 132 | 16.7 | 50.0 | 66.7 | 27 / 132 |
| 1 | wallpaper hidden | 172 | 16.7 | **16.8** | 33.4 | 6 / 172 |
| 2 | baseline | 132 | 16.7 | 50.0 | 66.7 | 24 / 132 |
| 2 | wallpaper hidden | 171 | 16.7 | **16.8** | 50.0 | 8 / 171 |

Hiding the wallpaper moves p95 from 50 ms to 16.8 ms — the 60 fps budget — and
cuts the frames over 30 fps by roughly 4×. That is the named cause; a secondary
run that removed only the `filter` (keeping the layer) improved the same metric
by half, which points at the blur raster rather than the rail's own transforms.

This is not a call to delete the Cabinet wallpaper. It is the named cause to
check first on a real GPU host: if the p95 still misses there, the options are
(a) scroll the rail only when the selected card leaves the comfortable band
instead of re-centring on every repeat, and (b) promote the wallpaper to its own
composited layer (`will-change`/`contain`) so the blur is rasterized once. Both
are product/perf changes outside this evidence lane.

## What the probe measured that is healthy

- **One input loop.** Exactly one requestAnimationFrame chain at idle in every
  run, with one callback per frame. Before #267 the shell, library, overlay and
  tour each polled; a single chain is the observable proof that the scope
  architecture did what it claimed.
- **Boot.** 0.6–1.0 s to the first cover cold, 0.3–0.5 s warm, on a shared
  8-vCPU box with no GPU. First Contentful Paint is 0.17–0.45 s.
- **Memory shape.** Flat at ~10.7 MB JS heap across the 30 s idle window at FHD
  and ~10.1 MB at UHD; no growth while the Console is idle.
- **Main thread.** Zero long tasks on the FHD rail; the only boot long task is
  55–73 ms.

## Repeat it

```bash
# Full harness, with the probe enabled (writes 48 archives, idles 30 s):
LUDEX_E2E_PERF=1 scripts/e2e.sh

# Against an already running harness:
source <harness env>          # LUDEX_E2E_BASE_URL, LUDEX_E2E_TOKEN, LUDEX_E2E_GAMES_DIR
LUDEX_E2E_PERF=1 pnpm --filter @ludex/e2e exec playwright test tests/09-console-perf.spec.ts

# 4K:
LUDEX_E2E_PERF=1 LUDEX_E2E_PERF_WIDTH=3840 LUDEX_E2E_PERF_HEIGHT=2160 \
  pnpm --filter @ludex/e2e exec playwright test tests/09-console-perf.spec.ts
```

The JSON is printed as `console-perf {...}`, attached to the run as
`console-perf.json`, and written under the test output directory. It carries
`commit`, `host`, `hostLoad`, `renderer` and the raw summary so a number is never
detached from its machine.

When reporting a run, include the `hostLoad`: this box is shared, and runs taken
while another lane compiles Rust (load 13–22 on 8 vCPU) produce p95 values 2×
worse than the settled runs above. The probe records it for that reason.

## Related

- [WEBKITGTK.md](WEBKITGTK.md) — the same rail in WebKitGTK under Xvfb; engine
  surface measured, frame budget not representative without a GPU.
- [NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md) — the real Windows + Sunshine
  measurement, including the process memory budget.
- [evidence.md](evidence.md) — the rest of the #270 gates.
