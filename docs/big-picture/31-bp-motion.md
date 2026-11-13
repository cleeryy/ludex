<!-- Suggested title: [v0.5] Big Picture motion system: console-grade transitions, interruptible and sound-synced -->

## Goal

Every state change in Big Picture gets one deliberate, interruptible, sound-synced transition: the boot reveal, the focus move, the shelf/page change, the layout switch, the screen push/pop, overlays, the power confirmation and the settings sections. The reference is a current console — the interface reacts on the frame of the press, never waits for a tween, and sounds when the thing it describes happens.

This document is the human-readable version of `apps/web/src/components/big-picture/motion.ts`. That file is the source of truth; `motion.test.ts` fails if the tables drift.

## Rules

1. Only `transform` and `opacity` animate. Never width, height, top, left, filter or backdrop-filter; never `transition: all`.
2. One vocabulary. Components do not invent a duration, a curve or a travel distance; they ask `transitionOf(kind)` or the `.bp-focus-move` class.
3. Interruptible. A new input replaces the running target on the next frame. The motion hub records the cut for the dev probe, but it never gates input (`accepts()` is always true).
4. No transition on a held key. `keydown` with `repeat` (and the pad's auto-repeat) lands on the final frame: Motion receives `duration: 0`, and `data-bp-instant` removes the CSS transitions for that one commit.
5. Sound-synced. The surface owns its cue and fires it once at the start of the motion it describes. The command router only speaks for movement (`move`, `tabChange`), so a press cannot play twice.
6. Calm. No animation on the clock or the battery, no idle drift in the library, no motion on a pointer hover beyond the existing colour and sheen.

## Tokens

| Token | Value | Used by |
| --- | --- | --- |
| `MOTION_DURATION.fast` | 120 ms (`--dur-1`) | focus, page, exit floor |
| `MOTION_DURATION.base` | 180 ms (`--dur-2`) | shelf, layout |
| `MOTION_DURATION.slow` | 240 ms (`--dur-3`) | entrance copy |
| `EASE_OUT` | `cubic-bezier(0.16, 1, 0.3, 1)` (`--ease-out`) | enter |
| `EASE_STANDARD` | `cubic-bezier(0.2, 0.6, 0.2, 1)` (`--ease`) | move |
| `MOTION_STAGGER` | 22 ms | card cascade, same as `.stagger-in` |
| `BOOT_DURATION` | 200 ms | system bar, home row, hint strip |
| `MOTION_DISTANCE.shelf` | 18 px | rail slide |
| `MOTION_DISTANCE.push` | 28 px | app push |
| `MOTION_DISTANCE.panel` | 14 px | overlay rise |
| `MOTION_DISTANCE.confirm` | 18 px | power confirmation |

Every enter tween sits between 120 and 320 ms. Exits are opacity-only fades, 120–140 ms, and never travel: a push and a pop cannot disagree about which way is "back".

## Transition table

| Kind | Duration | From | Cue (start) | Reduced motion |
| --- | --- | --- | --- | --- |
| `boot` | 200 ms | `y 8`, opacity 0 | — | fade |
| `focus` | 120 ms | CSS `.bp-focus-move` (scale/lift, opacity) | `move` | none |
| `shelf` | 180 ms | `x ±18`, opacity 0.55 | `tabChange` | fade |
| `page` | 120 ms | opacity 0.7 | `tabChange` | fade |
| `layout` | 180 ms | `scale 0.995`, opacity 0.55 | `confirm` | fade |
| `screenPush` | 220 ms | `x +28`, opacity 0.4 | `openGroup` | fade |
| `screenPop` | 200 ms | `x −20`, opacity 0.55 | `closeGroup` | fade |
| `overlayOpen` | 200 ms | `y 14`, opacity 0.5 | `overlayOpen` | fade |
| `overlayClose` | 160 ms | `y 8`, opacity 0.6 | `overlayClose` | fade |
| `powerConfirm` | 220 ms | `y 18`, `scale 0.992`, opacity 0.4 | `confirm` | fade |
| `settings` | 160 ms | `y 6`, opacity 0.6 | `tabChange` | none |

The page change is a scroll, not a content remount: the grid keeps its covers and the browser's smooth scroll carries the selection. When a direction is held, that scroll becomes instant too.

## Reduced motion

`prefers-reduced-motion` maps every kind to a 120 ms opacity fade or to no motion at all. Focus is `none` on purpose — the ring and the lift are the feedback, and a fade would only delay the highlight. The decorative cues (`openGroup`, `closeGroup`, `overlayOpen`, `overlayClose`, `tabChange`) are already suppressed by the sound manager (#198); the functional ones (`move`, `confirm`, `back`, `error`, `power`, `launch`) stay.

## Interruption and input

- The screen opens a transition with `beginMotion(kind)` *before* the state change that starts it. The hub stores the kind, the time, the generation and the transition it cut.
- A late completion from a transition that was cut cannot clear the newer one (`end(kind)` only matches the active kind).
- The library and the system apps are two stacked layers. The library stays mounted and `inert` under an app, so a pop reveals the same covers and the same selection without a remount. The arriving app is interactive on its first frame; the leaving app has `pointer-events: none`.
- Overlays and the layout picker are `AnimatePresence` children. Escape closes them, and the next press is already routed by the new state while the panel fades.
- A repeated key/held direction sets `data-bp-instant` for one commit and passes `instant` to Motion. The focus lands immediately; no tween restarts thirty times a second.

## Verification

`motion.test.ts` covers the token table (including the CSS mirror), the reduced-motion mapping, the interruption state machine and cue ordering, including "one cue per surface, no adjacent repeat".

The dev probe (`?motionDebug=1`) reports the last transition, the worst frame and the count over 17/33 ms. A transition is measured in two windows because they have different costs:

| Window | What it contains | Measured (1920×1080, headless Chromium, software raster) |
| --- | --- | --- |
| Mount (first ~90 ms) | React commit and first paint of the arriving surface | 47–94 ms task, 1–5 ms layout |
| Tween (next 280 ms) | the Motion animation itself | 10–25 ms task total, 0.1–0.7 ms layout |

`getAnimations()` on the moving elements reports exactly two running animations: `opacity` and `transform`. No layout property is written during the tween.

## Files

- `apps/web/src/components/big-picture/motion.ts` — tokens, table, frames, cue map, interruption hub.
- `apps/web/src/components/big-picture/motion-probe.tsx` — `?motionDebug=1` frame probe.
- `apps/web/src/components/big-picture/entrance.ts` — card/stage cascade, now sourced from the same tokens.
- `apps/web/src/screens/big-picture.tsx` — the layer stack, held-input policy, transition calls.
- `apps/web/src/components/big-picture/layout-*.tsx` — shelf rail slide on a real shelf change only.
- `apps/web/src/components/big-picture/overlays.tsx`, `layout-switcher.tsx` — enter/exit and scrim.
- `apps/web/src/styles/index.css` — `--dur-*`, `--ease*`, `.bp-focus-move`, zone lift, `data-bp-instant`.
