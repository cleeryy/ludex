# WebKitGTK Gamepad API spike (#270 §4)

Tauri on Linux renders with WebKitGTK. That is the engine the TV build ships, and
it is the one runtime where the Web Gamepad API is least proven. This page
records what ran, on what, what it proved and what it could not.

**Status: not verified on hardware.** The engine surface and the frame budget
were measured on a headless host with **no controller and no GPU**; the pad
checks that need a device are listed with their manual procedure and are still
open. Do not read this page as "WebKitGTK passed".

## Environment

| Item | Value |
|---|---|
| Engine | WebKitGTK 2.52.6 (GTK 3.24.41, `libmanette-0.2`) |
| Host | CT 119 `code` (pve3), Linux 7.0.14-29-pve x86_64, 8 vCPU, no GPU (`Matrox G200eW3`) |
| Display | Xvfb 1920×1080×24, `DRI3` unavailable, llvmpipe software GL |
| Controllers | none (`/dev/input/js*` absent, `/dev/uinput` absent) |
| Harness | `scripts/e2e.sh` mock server, 48 seeded games, commit `71075f4` |
| Probe | `e2e/webkitgtk-spike.py` (PyGObject, `evaluate_javascript`, Xvfb) |

The engine is the real packaged engine: `libwebkit2gtk-4.1.so.0` from the same
package Tauri links. The display stack is not: Xvfb has no DRI3, so WebKitGTK
falls back to software rasterization.

## Result

| # | Check | Expected | Measured | Verdict |
|---|---|---|---|---|
| 1 | API presence | `navigator.getGamepads` exists | `typeof === "function"`, `getGamepads()` → 0 pads (none connected) | **API exposed** |
| 2 | `mapping === "standard"` | standard mapping from an Xbox pad | no pad to read | **blocked: no controller** |
| 3 | Button/axis decoding | A/B/X/Y, D-pad, sticks, LB/RB, LT/RT, Start/Guide | no pad to read | **blocked: no controller** |
| 4 | Hot-plug | unplug/replug without a reload | no device, no udev in the CT | **blocked: no controller** |
| 5 | Timestamp advancement | `timestamp` increases while used | no pad to read | **blocked: no controller** |
| 6 | 60 fps rail scroll | p95 ≤ 16.7 ms on hardware | idle 61 fps / **rail ≈ 5 fps** (software, host load ≈ 22) | **not representative** |

### Frame budget

Two runs of `e2e/webkitgtk-spike.py` against the same 1080p home, 34 synthetic
`ArrowRight` presses over 3 s. The first run is the default WebKitGTK path; the
second forces the non-composited path, which is the only way rAF ticks under
Xvfb at all (`WEBKIT_DISABLE_COMPOSITING_MODE=1`).

| Run | Idle | Rail scroll | p50 | p95 | Frames > 33 ms |
|---|---|---|---|---|---|
| Default (composited) | 0 fps (rAF stalled) | 4 frames / 3.1 s | 1476 ms | 1967 ms | 4/4 |
| `WEBKIT_DISABLE_COMPOSITING_MODE=1` | 61–62 fps | 37–45 frames / 3.1 s | 200–210 ms | 235–307 ms | 33/37–40/45 |

Two runs of the forced path against the 48-game harness shelf, host load 17–21
on 8 vCPU, both with `moved: true` (the rail really scrolled from cover 01 to
cover 35).

**Why this is not a go/no-go on hardware.** WebKitGTK's accelerated compositing
needs a working GL stack; Xvfb has none. The rail still scrolls (input, selection
and layout all work), but every frame is rasterized on the CPU over a 48-card
layer and the full-screen wallpaper blur. The same measurement in Chromium with
SwiftShader on the same host is in [PERFORMANCE.md](PERFORMANCE.md); the native
Windows path is in [NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md). The production
verdict needs a Linux desktop with a GPU and a controller.

### What did work

- The app boots and the `/console` route resolves in WebKitGTK: `route=home`,
  `layout=shelf`, `room=night`.
- `navigator.getGamepads` is **not** gated away by the engine; a pad that
  appears in WebKitGTK will reach the same `useGamepad` polling loop Chromium
  uses. That was the one engine-level risk that could have forced a Rust
  fallback before visuals freeze.
- Keyboard movement of the rail works (`ArrowRight` through the real input
  scope), so the input loop itself is engine-agnostic.

## Manual procedure (desk with controller and GPU)

Run this before flipping `console_v2` on for a Linux TV device.

1. Build the desktop app with the console bundle:
   ```bash
   pnpm --filter @ludex/web build
   cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --release
   ```
2. Start a seed library (any server with a few hundred games), then launch:
   ```bash
   ./apps/desktop/src-tauri/target/release/ludex --console
   ```
   Add `?gamepadDebug=1` to the route (`/#/console?gamepadDebug=1`) to show the
   pad id, mapping, profile, axes/buttons and the last action.
3. Connect pads one at a time and fill the table below:
   - Xbox pad (xpad) — expect `mapping: "standard"`, 17 buttons, 4 axes.
   - DualSense (hid-playstation) — same mapping, check the profile resolves to
     DualSense.
   - A generic/8BitDo pad in XInput mode — expect standard mapping; in Switch
     mode expect raw indices and the generic fallback profile.
4. For each pad: press every control once, then unplug and replug mid-session.
   Watch the diagnostics panel go to "no pad" and back without a reload.
5. Take two debug frames a few seconds apart and compare `timestamp`.
6. Hold D-pad right on the Stage rail for 10 s. In DevTools → Performance,
   record the window and read:
   - p95 frame time (target ≤ 16.7 ms; a 60 fps budget),
   - long tasks > 50 ms (target: none on the rail),
   - `requestAnimationFrame` callbacks per frame (target: one loop, one
     callback per frame — the Console's input loop).
7. Open the quick-actions sheet (Start) and press Down once: the ring must move
   exactly one row. This is the single-dispatch gate on the real engine.

Record the result here and in the #270 thread:

| Date | Device | WebKitGTK | Driver | mapping | hot-plug | timestamp | p95 frame | Verdict |
|---|---|---|---|---|---|---|---|---|
| | | | | | | | | |

## Decision

- **Gamepad API:** engine exposes it; the pad-level checks are open. No Rust
  `gilrs` fallback is justified by evidence yet.
- **Frame budget:** unresolved. The software path is 5 fps at 1080p, but it
  cannot distinguish "WebKitGTK is slow" from "no GPU". Measure on the target
  Linux desktop before locking the visual budget; if the p95 stays above
  33 ms there, profile the wallpaper blur and the rail re-centring before
  considering a compositor workaround.

If checks 2–4 fail on hardware (raw indices only, or a hot-plug that needs a
reload), the fallback is: read pads in Rust with `gilrs` and emit a normalized
input event over the Tauri bridge. The web layer keeps the same action names
(`confirm`, `back`, `left`, ...), so the profiles, hints and the fake-pad tests
do not change. Design it before the visuals freeze; after that it is a
rewrite.

## Repeat this spike

```bash
scripts/e2e.sh &                                  # mock server + seeded library
source <the harness env it prints/uses>           # LUDEX_E2E_BASE_URL, LUDEX_E2E_TOKEN
xvfb-run -a -s "-screen 0 1920x1080x24" \
  WEBKIT_DISABLE_COMPOSITING_MODE=1 \
  python3 e2e/webkitgtk-spike.py report.json
```

The library must be big enough to scroll the rail (the harness seeds one game;
run `LUDEX_E2E_PERF=1 pnpm --filter @ludex/e2e exec playwright test
tests/09-console-perf.spec.ts` against the same server to seed 48, or point it
at a real library). The script reports `scroll.moved`; when it is false the rail
never moved and the frame numbers are idle frames — it prints that warning.

The script prints JSON and writes the same document to `report.json`. Keep the
`WEBKIT_DISABLE_COMPOSITING_MODE=1` variant in the report: without it, rAF does
not tick under Xvfb and every frame number is meaningless.
