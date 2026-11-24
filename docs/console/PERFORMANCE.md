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

## What is measured, and where (#304)

| Budget / rule | Measured on this host | Where | Verdict |
|---|---|---|---|
| Rail p95 ≤ 16.7 ms (60 fps) | **Yes**, by the rAF probe; asserted only on a hardware renderer | `09-console-perf.spec.ts`, tables below | p50 16.7 ms; p95 33.4 ms after the wall bake — missed on the software compositor, native checklist owns the real verdict |
| No long task > 50 ms on the rail (FHD) | **Yes**, `PerformanceObserver` | same | met: 0 in every FHD run |
| < 300 MB RSS at FHD | **No** — the probe measures the renderer JS heap (10.7 MB), a lower bound | JS heap here; process RSS in [NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md) D3/C4 | documented, native checklist item |
| < 380 MB RSS at UHD | **No** — same distinction | same | documented, native checklist item |
| One static blur layer, no animated filters | **Yes**, as a source/DOM rule, not a frame bisect | `styles/console-material.test.ts`, `console-backdrop.test.tsx`, long-task counts | met |
| Artwork decode off the focus path | **Partly**: the wall plate follows the settled selection (160 ms), pinned by unit tests; the frame tables include a settled plate, they do not isolate decode | `lib/wall-plate.ts` tests | documented, no A/B |
| Download progress does not re-render the library | **By construction**, selector split in the downloads store; unit-tested render isolation in `state/downloads.test.tsx` (`useActiveCount` / `useJobPhases` do not repaint on byte progress ticks); no frame-level A/B | `state/downloads.test.tsx` | met (unit), documented (frame A/B) |
| `console_v2` toggling does not mount a second library render | **Yes**, as a mount/query count | `components/console/console-flag-mount.test.tsx` | met: 1 mount, 1 query set after on → off → on |

The distinction is deliberate: "measured" is a number on this page,
"documented" is a rule with a test or a procedure but no frame/RSS figure. The
process memory budget is documented — a 10.7 MB JS heap is not a 300 MB RSS
result, and this page does not pretend otherwise.

### The device flag does not double-mount the library (#304)

`console-flag-mount.test.tsx` mounts the real `Gate` and a real router at `/`,
flips `ludex.console.shell` on → off → on, and counts:

- desktop library mounts: **1**
- console library mounts: **1**
- library query executions: **1**

Off while inside `/console` deliberately does nothing: the shell is reachable
by deep link, and redirecting out of a working console because a device flag
changed would be a second teardown, not a safety feature. The test's scope is
the mount and the query set, not frame time; the rAF probe above is the frame
measurement.

## Machine and method

| Item | Value |
|---|---|
| Commit | `240c53c` (main) for the original run; the wall-bake run is on top of it |
| Host | CT 119 `code` on pve3, Linux 7.0.14-29-pve x86_64, **8 vCPU**, no GPU |
| Renderer | `ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero)), SwiftShader driver)` |
| Browser | Chromium 158 headless (Playwright) |
| Server | `scripts/e2e.sh` harness: one admin, a seeded shelf of **48** games, real API |
| Input | fake standard pad through `navigator.getGamepads`; one held D-pad right for 3 s |
| Viewport | 1920×1080 (FHD) and 3840×2160 (UHD), `--bp-u` scaling active |
| Host load | quoted per run below; this box is shared, and it was at 5–8 (8 vCPU) |

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

Two flags exist for the investigation the numbers below came from:

| Flag | What it does |
|---|---|
| `LUDEX_E2E_PERF_WIDTH/HEIGHT` | viewport; `3840 2160` measures the TV budget |
| `LUDEX_E2E_PERF_HIDE_WALL=1` | hides `.bp-backdrop-art` for the run (the A/B control; the report records `wallpaper: "hidden"`) |

## The wall bake (#270 follow-up)

### What the numbers said

The first measurement on this page named the wallpaper as the cause: hiding
`.bp-backdrop-art` moved p95 from 50 ms to 16.8 ms. The next question was which
part of the wall — the live `filter: blur(26u) saturate(1.12)` or the fact that a
wallpaper is composited at all.

Answer: **the blur was most of it, and it is now gone.** The wall is no longer
filtered at composite time. It is painted once into a 256×144 canvas — blur and
saturation baked in — and the layer stretches that texture to the TV
(`lib/wall-plate.ts`, `components/big-picture/console-backdrop.tsx`). A scroll
composites a static bitmap instead of re-running a Gaussian over 1920×1080.

The wall also stopped re-rendering on every repeat: the plate follows the
*settled* selection (160 ms), not each card the ring crosses, so a held direction
no longer rebuilds a 200-node SVG per step.

### FHD, 1920×1080 — interleaved A/B on the same harness

Same shelf, same held D-pad, same 3 s window, two rounds, `240c53c` and the
bake, alternating to track host drift:

| Round | Build | Frames | p50 | p95 | p99 | Max | Frames > 33 ms | hostLoad |
|---|---|---|---|---|---|---|---|---|
| 1 | `240c53c` (live blur) | 115 | 16.7 | **50.1** | 66.7 | 66.7 | 28 / 115 | 7.17 |
| 1 | wall baked | 141 | 16.7 | **33.4** | 50.1 | 183.3 | 14 / 141 | 5.38 |
| 2 | `240c53c` (live blur) | 117 | 16.7 | **50.0** | 66.8 | 83.3 | 29 / 117 | 8.31 |
| 2 | wall baked | 146 | 16.7 | **33.4** | 49.9 | 66.8 | 12 / 146 | 7.71 |

Reading it:

- **p50 is unchanged at 16.7 ms** — the wall never was a constant tax, it was a
  hitch.
- **p95 halves** (50 → 33.4 ms) and **p99 loses a frame** (66.7 → 50 ms).
- **The 30 fps cliff is cut in half** (28–29 of ~116 frames → 12–14 of ~146), and
  the app renders ~25 % more frames in the same 3 s window.
- **Boot is unchanged within noise**: 578–637 ms cold and 316–370 ms warm
  before, 681–736 ms cold and 359–364 ms warm after, on the same alternating
  runs. The bake adds one 256×144 paint, not a boot cost.

### What is left, and why it is not the blur

A third pass measured the *same* build with the wall made invisible in different
ways (one fresh reload before each hold, so every variant pays its own first
scroll; 1920×1080):

| Wall state | Frames | p50 | p95 | Frames > 33 ms |
|---|---|---|---|---|
| baked, visible | 149 | 16.7 | 33.4 | 12 / 149 |
| `visibility: hidden` | 180 | 16.7 | 16.8 | 3 / 180 |
| `opacity: 0` | 170 | 16.7 | 33.3 | 7 / 170 |
| `display: none` | 179 | 16.7 | 16.8 | 1 / 179 |
| a 512×288 wall instead of full-screen | 163 | 16.7 | 33.4 | 11 / 163 |

The residual does not scale with the wall's area, its opacity, its promotion
(`will-change`), or its containment (`contain: paint`): a 147 k-pixel wall costs
the same as a full-screen one. It also is not raster — it happens on every
scroll, not on the first. What is left is the per-pixel cost of compositing
*any* visible wallpaper under a moving rail when the compositor is software: make
the layer fully invisible and the same run is back at 16.8 ms.

That is the boundary this host can measure. On a GPU the same wall is one
textured quad and costs nothing measurable, which is why the probe asserts 60 fps
only on a hardware renderer and the native checklist owns the real verdict.

### UHD, 3840×2160

Two runs each, same harness, alternating (`hostLoad` 5.4–8.0):

| Round | Build | Frames | p50 | p95 | p99 | Frames > 33 ms | Long tasks in scroll | hostLoad |
|---|---|---|---|---|---|---|---|---|
| 1 | `240c53c` | 26 | 116.6 | 283.3 | 283.4 | 19 / 26 | 6 · 1060 ms | 7.96 |
| 1 | wall baked | 23 | 116.7 | 616.6 | 633.3 | 19 / 23 | 1 · 54 ms | 7.01 |
| 2 | `240c53c` | 24 | 149.9 | 333.3 | 366.7 | 17 / 24 | 11 · 2327 ms | 7.13 |
| 2 | wall baked | 28 | 100.1 | 633.2 | 633.3 | 22 / 28 | 0 | 6.51 |
| — | wall baked, wall hidden | 94 | 16.7 | 83.3 | 116.7 | 34 / 94 | 0 | 5.45 |
| — | wall baked, wall hidden | 83 | 33.3 | 83.3 | 183.4 | 33 / 83 | 0 | 6.08 |

The main-thread picture is the clear one: at 4K the live blur was blocking the
main thread for 1.1–2.3 s inside a 3 s scroll (6–11 long tasks, longest 335 ms);
baking it leaves 0–1 long tasks (0–54 ms). The frame *rate* is outside the range
where a comparison means anything: the rail runs at 7–8 fps with the wall and
26–29 fps without it, so the software compositor is saturated either way and the
two builds land within each other's noise.

**Do not read a 4K verdict from this host**; the UHD memory and frame budgets are
native checklist items. 4K also remains the case where a bigger plate would pay:
the texture is magnified 17× at UHD, and `plateBlur`/`WALL_PLATE` are the two
constants to revisit once a GPU host can measure the difference.

## The original run (commit `71075f4`, before the bake)

Kept for comparison: these are the numbers that named the cause.

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

A controlled A/B on the same harness, same held D-pad, same 3 s window, with the
wallpaper layer hidden by a style override and the two runs interleaved twice:

| Round | Variant | Frames | p50 | p95 | Max | Frames > 33 ms |
|---|---|---|---|---|---|---|
| 1 | baseline | 132 | 16.7 | 50.0 | 66.7 | 27 / 132 |
| 1 | wallpaper hidden | 172 | 16.7 | **16.8** | 33.4 | 6 / 172 |
| 2 | baseline | 132 | 16.7 | 50.0 | 66.7 | 24 / 132 |
| 2 | wallpaper hidden | 171 | 16.7 | **16.8** | 50.0 | 8 / 171 |

The causality that A/B established — the wallpaper layer, not the rail's own
transforms, and not the main thread — is what the bake above acts on.

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

## Which budget is missed, and why

| Budget | Verdict | Evidence |
|---|---|---|
| 60 fps rail (p95 ≤ 16.7 ms) | **Improved, still missed on this host** (p95 33.4 ms, from 50.1). p50 holds 60 fps; the remainder is compositing a visible wallpaper on a CPU compositor, not the blur and not JS. | A/B above; `visibility: hidden` control at 16.8 ms. |
| No long tasks > 50 ms on the rail (FHD) | **Met.** | 0 long tasks in every run. |
| < 300 MB FHD | **Not measured as a process.** JS heap is 10.7–11.3 MB, far below the budget, but that is only the renderer's JS objects. | Native checklist item D3/C4. |
| < 380 MB UHD | **Not measured as a process.** JS heap is 10.7 MB. | Native checklist item D3. |

## The fix, in code

| Piece | Where | What it does |
|---|---|---|
| Plate painter | `apps/web/src/lib/wall-plate.ts` | `coverRect` (the `object-fit: cover` maths), `parseWallFilter` (reads the blur the stylesheet asked for), `plateBlur` (CSS px → texture px), `paintWallPlate` (one blurred draw), `canvasFilterSupported` (behavioural probe). Pure except the probe, unit tested. |
| Wall component | `apps/web/src/components/big-picture/console-backdrop.tsx` | Settles the selection, serializes the live plate (SVG) or reuses the decoded backdrop (image), paints the 256×144 texture, and hides the live plate only once the bake succeeded. |
| Stylesheet | `apps/web/src/styles/index.css` | `.bp-backdrop-art` is a static texture layer; the live `filter: blur()` survives only under `[data-bp-wall="live"]` — the frames before the bake lands, and engines that cannot blur a canvas. |

Constraints the bake keeps:

- **One blurred layer, still.** The blow-up is inside the texture; there is no
  `backdrop-filter` anywhere, and no filter on a moving element.
- **Same wall, same look.** `parseWallFilter` reads the room's own value, so
  Night keeps its 26u blur and a room with no blur keeps a sharp, full-resolution
  plate (the bake declines and the live layer stays). The texture-space radius is
  resolution-independent, so 720p and 4K look the same.
- **Fallbacks.** No canvas 2D context, no canvas filter support, a failed SVG
  decode or a failed image load all leave `data-bp-wall="live"`, i.e. exactly the
  layer that shipped before this change.
- **Accessibility.** `prefers-reduced-transparency` and `prefers-contrast: more`
  still zero the wall; nothing new animates, so there is nothing to reduce.
- **No new strings.** No user-facing text changed.

## What the probe measured that is healthy

- **One input loop.** Exactly one requestAnimationFrame chain at idle in every
  run, with one callback per frame. Before #267 the shell, library, overlay and
  tour each polled; a single chain is the observable proof that the scope
  architecture did what it claimed.
- **Boot.** 0.6–1.0 s to the first cover cold, 0.3–0.5 s warm, on a shared
  8-vCPU box with no GPU. First Contentful Paint is 0.17–0.45 s.
- **Memory shape.** Flat at ~10.7 MB JS heap across the 30 s idle window at FHD
  and ~10.1 MB at UHD; no growth while the Console is idle.
- **Main thread.** Zero long tasks on the FHD rail at either commit, and zero at
  4K after the bake; the only boot long task is 51–73 ms.

## Repeat it

```bash
# Full harness, with the probe enabled (writes 48 archives, idles 30 s):
LUDEX_E2E_PERF=1 scripts/e2e.sh

# The A/B control: the same run with the wallpaper layer hidden.
LUDEX_E2E_PERF=1 LUDEX_E2E_PERF_HIDE_WALL=1 scripts/e2e.sh

# Against an already running harness:
source <harness env>          # LUDEX_E2E_BASE_URL, LUDEX_E2E_TOKEN, LUDEX_E2E_GAMES_DIR
LUDEX_E2E_PERF=1 pnpm --filter @ludex/e2e exec playwright test tests/09-console-perf.spec.ts

# 4K:
LUDEX_E2E_PERF=1 LUDEX_E2E_PERF_WIDTH=3840 LUDEX_E2E_PERF_HEIGHT=2160 \
  pnpm --filter @ludex/e2e exec playwright test tests/09-console-perf.spec.ts

# The device-flag mount check (no server, no browser):
pnpm --filter @ludex/web exec vitest run src/components/console/console-flag-mount.test.tsx
```

The JSON is printed as `console-perf {...}`, attached to the run as
`console-perf.json`, and written under the test output directory. It carries
`commit`, `host`, `hostLoad`, `renderer`, `wallpaper` and the raw summary so a
number is never detached from its machine.

When reporting a run, include the `hostLoad`: this box is shared, and runs taken
while another lane compiles Rust (load 13–22 on 8 vCPU) produce p95 values 2×
worse than the settled runs above. The probe records it for that reason.

## Related

- [WEBKITGTK.md](WEBKITGTK.md) — the same rail in WebKitGTK under Xvfb; engine
  surface measured, frame budget not representative without a GPU.
- [NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md) — the real Windows + Sunshine
  measurement, including the process memory budget.
- [evidence.md](evidence.md) — the rest of the #270 gates.
