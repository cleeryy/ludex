# Console 2.0 evidence (#270)

Phase 0.6 of [the blueprint](README.md#phases-and-gates): the fake-pad harness,
the TV screenshot matrix and the two hardware checklists. This page says what
is automated today, what is still `fixme`, and exactly how to run each gate.

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
| 720p / 1080p / 4K + 5% overscan | **Automated** (capture only) | `e2e/tests/07-console-visual.spec.ts` |
| Overlay single-dispatch | `fixme` until #267 | `e2e/tests/06-console.spec.ts` |
| 24u floor, Cabinet anti-slop, desktop leaks | `fixme` until #267/#268 | `e2e/tests/08-console-antislop.spec.ts` |
| WebKitGTK Gamepad API spike | **Manual** | checklist below |
| Windows + Sunshine/Moonlight | **Manual** | checklist below |

A `fixme` test is reported as skipped: the harness is ready and CI stays green.
Enable each one in the PR that makes it pass — never delete it (a rule without
a test is gone in three releases).

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

Today the shell is `/bigpicture`. When #267 lands `/console`, point the suites
at it instead of editing them:

```bash
LUDEX_E2E_CONSOLE_PATH=/console pnpm --filter @ludex/e2e test
```

`e2e/tests/console-helpers.ts` also carries the screenshot/PNG helpers and
`isConsoleUrl`, so a route move is a one-line change there.

### Tests added

| Test | Asserts |
|---|---|
| boots straight into the console shell from a direct URL | Direct `/bigpicture` (or `/console`) boots the shell, not the desktop; no desktop sidebar; title is the console title. |
| deep links open a surface and Escape walks back | `?app=settings` opens Settings; Escape returns to the shell root, then either the desktop (v0.4) or the Power confirm surface (2.0). |
| fake pad drives the real input loop | With `?gamepadDebug=1`, a fake D-pad press shows up as a decoded action in the diagnostics panel; hot-unplug/hot-plug is observed. |
| captures the 1920×1080 evidence frame | PNG is exactly 1920×1080 and not a blank frame. |
| `fixme` overlay single-dispatch | One press with the overlay open must move one step (double-dispatch gate, #267). |
| `fixme` 24u floor / Cabinet rules / desktop leaks | Skill §9 anti-slop gates; the v0.4 shell still violates them. |

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

## WebKitGTK Gamepad API spike

Tauri on Linux (WebKitGTK) is the weakest target and the one the TV build
ships. Run this before locking the Console 2.0 visuals; record the result in the
table below and in #270.

### Procedure

1. `pnpm --filter @ludex/web build`, then `pnpm --filter @ludex/desktop dev`
   (Tauri dev window, the same WebKitGTK engine as the packaged app).
2. Open Console mode, then append `?gamepadDebug=1` to the route
   (`/#/bigpicture?gamepadDebug=1`). The diagnostics panel shows pad id, index,
   mapping, resolved profile, axes/buttons, last action, and the stream state.
3. Connect an Xbox pad (xpad driver) and a DualSense (hid-playstation) if
   available; repeat with a generic/8BitDo pad in XInput mode.

### Checks

| # | Check | Pass condition | Go/No-go |
|---|---|---|---|
| 1 | API presence | `navigator.getGamepads` exists and returns the pad; `mapping === "standard"`. | **Go** if standard, otherwise the profile layer still works via raw indices — document it. |
| 2 | Button/axis mapping | A/B/X/Y, D-pad, sticks, LB/RB, LT/RT, Start/Guide decode to the expected actions in the panel. | **Go** if all of them; any dead input blocks the visual lock. |
| 3 | Hot-plug | Unplug/replug mid-session: the panel goes to "no pad" and back without a reload; inputs resume. | **Go** if it recovers; if it needs a reload, the Rust/gilrs fallback is required before ship. |
| 4 | Timestamp | `timestamp` advances while the pad is used (compare two debug frames). | **Go** if it advances; the app does not read it for pad selection (some builds refresh every frame), so a static value is a warning, not a blocker. |
| 5 | 60 fps rail | Hold a D-pad direction on a 5.5-cover rail; the scroll holds ~60 fps (DevTools performance panel, no long frames > 20 ms). | **Go** at 1080p on the weakest device; otherwise profile before the visual lock. |
| 6 | Focus/back | B/Escape returns to the exact previous target; no dead end after a dialog. | **Go** if every surface backs out. |

Fallback if 1–3 fail: read pads in Rust with `gilrs` and emit a normalized
input event over the Tauri bridge; the web input layer keeps the same action
names so the tests and profiles do not change. Design it before the visuals are
frozen — after that it is a rewrite.

Record:

| Date | Device | WebKitGTK/Tauri version | 1 | 2 | 3 | 4 | 5 | Verdict |
|---|---|---|---|---|---|---|---|---|
| | | | | | | | | |

## Windows + Sunshine/Moonlight checklist

The TV path is: Windows host runs Sunshine and publishes the Ludex window;
Moonlight on the TV decodes it. The Console must survive a real gamepad, a
real overlay and a real sleep.

### Prerequisites

- Windows host with the Ludex desktop build (WebView2), a controller, Sunshine
  paired with the Moonlight client.
- Moonlight set to 1080p60 for the first pass, 4K for the second.
- A game configured to launch borderless (the overlay path).

### Checklist

| # | Step | Pass condition | Done |
|---|---|---|---|
| 1 | Launch Ludex normally, then start it again with `--console` | The second instance promotes the existing window to Console mode; no duplicate window, no data loss. | ☐ |
| 2 | Moonlight 1080p60: navigate the Console for two minutes | No stutter, no dropped input, rail scroll smooth; no visible compression artefacts on focus. | ☐ |
| 3 | Moonlight 4K: same pass | Readable at 10 feet, no layout overflow, overscan safe. | ☐ |
| 4 | Hot-plug a controller mid-session | Input resumes without a reload; the SystemBar updates. | ☐ |
| 5 | Open the in-app overlay over a borderless game (Sunshine overlay path) | Overlay is visible above the game, controller works in it, closing returns focus to the game. | ☐ |
| 6 | Launch a game from the Console, then quit it | Focus returns to the Console at the same target; no black frame, no lost fullscreen. | ☐ |
| 7 | Power → Sleep, then wake | Session resumes; the Console is still the active screen; no stuck input. | ☐ |
| 8 | Power → Quit (after a confirm that defaults to Cancel) | Process exits cleanly, Sunshine ends the stream, no orphan window. | ☐ |
| 9 | Pull the network cable mid-download | Offline state is honest; the library stays usable; no toast storm. | ☐ |
| 10 | Start the stream without a controller, then connect one | The Console stays keyboard-navigable; the pad takes over on first press. | ☐ |

Sign-off: date, host build, Sunshine/Moonlight versions, and any deviation in
the #270 thread.

## What this lane does not cover

- Playwright runs Chromium only: it proves the web input loop, not WebKitGTK
  or WebView2. The two checklists above are the native gates.
- Memory budgets (< 380 MB UHD / < 300 MB FHD) need a real Tauri process; a
  browser tab measurement is not evidence.
- The visual captures are not pixel-diffed. Baselines are approved by a human
  through #265; a diff gate can be added once the Cabinet shell stops moving.
