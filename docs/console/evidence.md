# Console 2.0 evidence (#270)

Phase 0.6 of [the blueprint](README.md#phases-and-gates): the fake-pad harness,
the TV screenshot matrix, the performance probe and the two hardware checklists.
This page says what is automated, how to repeat it, and which checks still
belong to a human.

Related issues: [#270](https://github.com/cleeryy/ludex/issues/270) (this
evidence), [#267](https://github.com/cleeryy/ludex/issues/267) (`/console`
shell + single input owner), [#268](https://github.com/cleeryy/ludex/issues/268)
(Cabinet), [#269](https://github.com/cleeryy/ludex/issues/269) (console
surfaces), [#265](https://github.com/cleeryy/ludex/issues/265) (approved
screenshots).

## Status

| Gate | State | Where |
|---|---|---|
| Fake pad drives the real input loop | **Automated** | `e2e/pad.ts`, `e2e/tests/06-console.spec.ts` |
| Direct boot, deep link, Escape/back | **Automated** | `e2e/tests/06-console.spec.ts` |
| 1920×1080 evidence frame | **Automated** | `e2e/tests/06-console.spec.ts` |
| Overlay single-dispatch (one press, one action) | **Automated** | `e2e/tests/06-console.spec.ts` |
| Home → launch, install → Downloads, settings persistence, power confirm | **Automated** | `e2e/tests/10-console-flows.spec.ts`, [E2E.md](E2E.md) |
| 720p / 1080p / 4K + 5% overscan, pixel-diffed (Stage/Wall) | **Automated gate** | `e2e/console-screenshots.mjs --gate`, this page |
| 720p / 1080p / 4K + 5% overscan (List/Settings/Power) | **Capture only** | `e2e/tests/07-console-visual.spec.ts` |
| 24u floor, Cabinet anti-slop, desktop leaks | **Automated** | `e2e/tests/08-console-antislop.spec.ts` |
| TV boot / rail frames / long tasks / heap / rAF loops | **Automated**, opt-in | `e2e/tests/09-console-perf.spec.ts`, [PERFORMANCE.md](PERFORMANCE.md) |
| WebKitGTK Gamepad API + rail frames | **Partial, not hardware** | `e2e/webkitgtk-spike.py`, [WEBKITGTK.md](WEBKITGTK.md) |
| Windows + Sunshine/Moonlight | **Manual, not run** | [NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md) |

Nothing is `fixme` any more: every gate that the 2.0 shell can satisfy has a
passing test, and the checks that need hardware are documented as manual rather
than skipped behind a green run.

## Fake-pad suite

`e2e/pad.ts` installs one `page.addInitScript` that replaces
`navigator.getGamepads` with a pad-shaped object (`mapping: "standard"`, 17
buttons, 4 axes, `timestamp`, hot-plug events). The application's own
`useGamepad` loop reads it through the normal Gamepad API — the repeat tracker,
profile resolution, focus routing and cue haptics all run for real. No React
hook is mocked.

Every state change waits two animation frames before returning, so an assertion
cannot race the rAF poll. `tap`/`hold`/`chord` do the press, the hold and the
release inside the page, which keeps the press visible for at least two polls on
a busy CI runner.

| Helper | What it does |
|---|---|
| `installFakePad(page, options)` | Registers the stub before `goto`; returns the controller. Options: `id`, `index`, `mapping`, `connected`, `swapAB`, `axes`. |
| `pad.press(input)` / `pad.release(input)` | Press or release and wait two frames. `input` is a named button (`"a"`, `"dpad right"` → `"right"`) or a raw index. |
| `pad.tap(input, holdMs = 40)` | One press/release; the default hold is below the 380 ms repeat delay. |
| `pad.hold(input, ms)` | Keep pressed (crosses the repeat delay when `ms > 380`). |
| `pad.chord(inputs, holdMs)` | Press several buttons in the same frame, release together. |
| `pad.connect()` / `pad.disconnect()` | Hot-plug: fires `gamepadconnected` / `gamepaddisconnected`; a disconnected slot reads as `null`. |
| `pad.setAxes([lx, ly, rx, ry])`, `pad.setStick("left", x, y)` | Analog input; deadzone/curve are applied by the real profile. |
| `pad.setButton("rt", 0.5)` | Analog button travel without a press/release pair. |
| `pad.setABSwap(true)` | Nintendo layout: `a` reports on button 1, `b` on button 0. |
| `pad.bumpTimestamp()` | Advances `timestamp` without changing input. |
| `pad.state()` / `pad.events()` | The pad the page sees, and the hot-plug history. |
| `pad.reset()` | Releases every button and centres the sticks. |

Run the suite:

```bash
scripts/e2e.sh                     # full browser suite (builds web + server)
pnpm e2e                           # reuse a running harness (same env vars)
# one spec, while the harness servers are up:
pnpm --filter @ludex/e2e exec playwright test tests/06-console.spec.ts
```

The shell is `/console` (#267); `/bigpicture` only redirects.
`e2e/tests/console-helpers.ts` resolves the path from
`LUDEX_E2E_CONSOLE_PATH` (default `/console`) and carries the screenshot/PNG
helpers and `isConsoleUrl`.

### Tests added

| Test | Asserts |
|---|---|
| boots straight into the console shell from a direct URL | Direct `/console` boots the shell, not the desktop; no desktop sidebar; title is the console title. |
| deep links open a surface and Escape returns to the shell root | `/console/settings` opens Settings; Escape closes the surface without leaving the shell. |
| Escape at the shell root backs out | B/Escape at the root opens the Power confirm (default Cancel) or returns to the desktop; never a silent exit. |
| fake pad drives the real input loop | With `?gamepadDebug=1`, a fake D-pad press shows up as a decoded action in the diagnostics panel; hot-unplug/hot-plug is observed. |
| captures the 1920×1080 evidence frame | PNG is exactly 1920×1080 and not a blank frame. |
| overlay single-dispatch | Start opens the quick-actions sheet; one D-pad Down moves the ring exactly one row; B returns to the same cover. Catches both a double dispatch and a dead controller path. |
| Home → Play opens the launch plate; B closes it onto the same cover | A on the selected game opens the plate from the cover, the plate reaches its honest "slow" line, B closes it and `data-gamepad-focus` is back on the same card. `launch_game` was called once. |
| Install starts a job that reaches the Status bar and Downloads | The install dialog starts a job; `install://progress` moves it to 75 %; the Status bar count and the Downloads surface both show it. Native bridge, real downloads provider. |
| a Console setting survives a reload | Changing Room to Projector updates the shell attribute and the scoped `ludex.console.room` record; a full reload still shows Projector. |
| Power → Sleep confirms on Cancel; B cancels | The confirmation starts on Cancel, B closes it, the shell still drives Power and `system_sleep` was never invoked. |
| 24u floor / Cabinet rules / desktop leaks | Reads the rendered `/console` DOM: no visible text under 24 px at 1080p, no layout picker or Soft/pastel token, a room attribute from `theme/console-room.ts`, and no toast, desktop dialog, pad layer or desktop route. |

## Screenshot matrix

`e2e/tests/07-console-visual.spec.ts` captures three viewports, each as a full
frame and as the 90% area a 5% overscan TV shows:

| Viewport | Full frame | Safe area (5% inset per edge) |
|---|---|---|
| 1280×720 | `720p-full.png` | `720p-safe-90.png` (1152×648) |
| 1920×1080 | `1080p-full.png` | `1080p-safe-90.png` (1728×972) |
| 3840×2160 | `4k-full.png` | `4k-safe-90.png` (3456×1944) |

Files land in `e2e/test-results/console-visual/<test>/` (gitignored; CI
uploads `e2e/test-results`). They are evidence frames a human reviews for
clipped hero titles, hints and focus rings; the approved set for the gate is
below.

Approved images for the README and docs go through the #265 pipeline
(`pnpm screenshots`, optimized, manifest) into `docs/assets/screenshots/`.
Never commit a raw 4K PNG.

Overscan rule of thumb: if it is information, it must survive the 90% crop; if
it only exists in the outer 5%, it will not be read from a couch.

### Visual gate (#303)

`e2e/console-screenshots.mjs --gate` renders the demo build
(`VITE_USE_MOCKS=true`, no server) in the same conditions the Cabinet lane used
to approve the images — Night room, en-US, reduced motion, fixed clock — and
compares **Stage** and **Wall** at 1280×720, 1920×1080 and 3840×2160 plus each
5 % overscan crop against
[`docs/assets/screenshots/console/`](../assets/screenshots/console/).

```bash
# Capture a fresh set first if apps/web/dist is stale:
VITE_USE_MOCKS=true pnpm --filter @ludex/web build

# Compare (fails on a real layout change):
node e2e/console-screenshots.mjs --gate --skip-build

# Re-approve after an intentional redesign (one command, all views + manifest):
LUDEX_VISUAL_UPDATE=1 node e2e/console-screenshots.mjs
```

Both sides are normalised to 720 px wide with a Lanczos downscale and a 1 px
blur before the diff. A pixel counts as changed when any channel moves by more
than **24/255**; a frame fails when more than **0.5 %** of its pixels changed.
The normalisation is why the gate survives palette quantisation (the baselines
are 88-quality palette PNGs) and font hinting; it keeps structural facts — a
cover is ~3 % of a frame at that width, a moved band strip ~5 %.

Tuned and run on the CT 119 host (`ANGLE`/SwiftShader, Chromium headless) on
2026-11-19:

| Run | Worst frame | Result |
|---|---|---|
| Gate 1 | 0.152 % (stage-720-overscan) | pass |
| Gate 2 | 0.076 % (stage-1080-overscan) | pass |
| Missing first cover | 23.093 % | fail |
| Band shifted 12 px | 1.116 % | fail |
| Hero title moved 12 px | 1.052 % | fail |
| Band label hidden | 0.639 % | fail |

### Design System #333 + Console 2.0 re-approval (2026-11-24)

After unifying Console 2.0 on the #333 ARC × Swiss token system (removing the
warm-amber `warmTokens` cast, adding the 3 px `.arc-stripe` header, aligning
radii to `--r-xs/sm/md/lg` scaled by `--bp-u`, adding Power → `Sign out`,
console-native Game/Install/Downloads/Notifications surfaces, and honest
Offline mode), all 15 baselines (5 views × 3 TV resolutions + 5 % overscan
crops) were regenerated and verified with Chromium 158 headless:

- **Visual baselines**: `LUDEX_VISUAL_UPDATE=1 node e2e/console-screenshots.mjs`
  — 15 captures (720p, 1080p, 4K + 5 % overscan), **0 overscan violations**,
  0 page errors.
- **Visual pixel gate**: `node e2e/console-screenshots.mjs --gate --skip-build`
  — **12/12 gated frames PASS**, worst frame **0.010 %** (`stage-720-overscan`,
  `wall-720-overscan`, vs 0.5 % limit).
- **Asset budget**: `node scripts/check-assets.mjs` — PASS (all screenshots
  under 300 KB).
- **Playwright fake-pad & DOM gates** (against `VITE_USE_MOCKS=true` demo
  build):
  - `06-console.spec.ts` (6/6 PASS: direct boot, deep link + Escape, root back,
    fake pad input loop, 1080p evidence frame, overlay single-dispatch).
  - `07-console-visual.spec.ts` (3/3 PASS: 720p / 1080p / 4K + 5 % overscan).
  - `08-console-antislop.spec.ts` (3/3 PASS: 24u type floor at 1080p, Cabinet
    rules, zero desktop leaks).
  - `10-console-flows.spec.ts`: settings reload persistence and Power → Sleep
    confirmation verified against the demo build; full server-backed install
    and launch flows (`09-console-perf.spec.ts` and server-seeded `10` flows)
    remain gated on a host with `cargo` / Rust server (`scripts/e2e.sh`).

The gate is **not wired into the PR jobs**. Two reasons, both documented rather
than hidden: the demo build overwrites `apps/web/dist` with a different adapter
than the E2E job needs, and GitHub's runner image is not the machine the
baselines were approved on — font rasterisation there is a different renderer,
so a failure would say "different fonts", not "different layout". Run it on the
dogfood/TV host or a dedicated job with a pinned image; the command above is the
whole protocol.

A failed frame writes a red-on-grey diff to
`e2e/test-results/console-visual-gate/<frame>.diff.png`; the terminal lists the
ratio and pixel count per frame.

#### Re-approve a baseline

1. Confirm the change is intentional (a Cabinet redesign, a new shell element),
   not a regression the gate just caught.
2. Run `LUDEX_VISUAL_UPDATE=1 node e2e/console-screenshots.mjs` (no
   `--skip-build`: the script must rebuild the demo adapter, or the baselines
   would come from the real server build). It rewrites all five views at three
   resolutions, the overscan crops and `manifest.json` with the new commit and
   timestamp.
3. Review the changed PNGs in the PR diff. A baseline update without a visible
   image change in the diff is a process smell.
4. Run `node e2e/console-screenshots.mjs --gate --skip-build` once more; it must
   pass on the fresh set.
5. Note the re-approval and its reason in the PR body. `node
   scripts/check-assets.mjs` enforces the 300 KB per-image budget.

List, Settings and Power stay capture-only: the Settings footer prints the
release version, so a gate over it would fail on every version bump for a
reason that is content, not layout. Stage/Wall are the two compositions a
player browses; they are the pixel gate.

## Performance probe

`e2e/tests/09-console-perf.spec.ts` seeds a 48-game shelf in the harness games
directory, scans it through the public API, then measures on 1920×1080:

- boot to home (cold and warm) with Navigation Timing + FCP,
- rail scroll frame deltas while a fake pad holds a direction (p50/p95/p99/max,
  frames over 16.7 ms and over 33.3 ms),
- long tasks during boot and during the scroll,
- `performance.memory.usedJSHeapSize` after a forced GC at 5 s and after 30 s
  idle,
- the number of distinct requestAnimationFrame chains still scheduling work and
  the callbacks per frame.

It is opt-in because it writes 48 archives and idles for 30 s:

```bash
LUDEX_E2E_PERF=1 scripts/e2e.sh
# or, against an already running harness:
LUDEX_E2E_PERF=1 pnpm --filter @ludex/e2e exec playwright test tests/09-console-perf.spec.ts
```

The report is printed as `console-perf {...}` and attached to the test as
`console-perf.json`. The measured numbers, the machine and the sub-budget
breakdown live in [PERFORMANCE.md](PERFORMANCE.md).

## Hardware gates

The two checks that need hardware are their own documents now, so the procedure
is one source of truth:

- [WEBKITGTK.md](WEBKITGTK.md) — WebKitGTK 2.52.6 Gamepad API surface and rail
  frames on this host, the blocked pad checks, and the exact desk procedure.
  Status: **not verified on hardware**.
- [NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md) — Windows + Sunshine/Moonlight at
  1080p60 and 4K, controller hot-plug, overlay over borderless, sleep/quit,
  second-instance promotion, focus return, with the evidence each check needs.
  Status: **not run**.

## What this lane does not cover

- Playwright runs Chromium only: it proves the web input loop, not WebKitGTK
  or WebView2. Those are the two hardware gates above.
- The process memory budgets (< 380 MB UHD / < 300 MB FHD) are measured as the
  renderer's JS heap by the probe — a lower bound. The `ludex.exe` commit size
  at 1080p and 4K is the real measurement and lives in
  [NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md).
- The visual gate covers Stage and Wall only. List, Settings and Power are
  approved by a human through #265; the Settings footer carries the release
  version, so pixel-gating it would fail on every version bump.
- The native bridge proves the native *branches*, not a process spawn, a real
  archive or an OS sleep. Those remain the hardware checklist's job.
