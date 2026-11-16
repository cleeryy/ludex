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
| 720p / 1080p / 4K + 5% overscan | **Automated** (capture only) | `e2e/tests/07-console-visual.spec.ts` |
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
uploads `e2e/test-results`). They are **evidence**, not baselines: a human
reviews the safe-area frames for clipped hero titles, hints and focus rings.

Approved images for the README and docs go through the #265 pipeline
(`pnpm screenshots`, optimized, manifest) into `docs/assets/screenshots/`.
Never commit a raw 4K PNG.

Overscan rule of thumb: if it is information, it must survive the 90% crop; if
it only exists in the outer 5%, it will not be read from a couch.

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
- The visual captures are not pixel-diffed. Baselines are approved by a human
  through #265; a diff gate can be added once the Cabinet shell stops moving.
