---
name: console-mode-ui
description: Design and build the 10-foot, gamepad-first Ludex Console 2.0 UI/UX.
---

# Ludex Console 2.0 — UI/UX Rules (Cabinet)

These rules supersede the v0.4 "Big Picture" assumptions. They are product rules, not suggestions.

## 1. One identity, two densities
- Keep the name **Ludex** and the drawn wordmark. Console is a *device room*, not a second brand.
- Desktop stays dense (archive/hairlines/small meta); Console is dark-first, one object at a time, ten times the air.
- Shared: name, wordmark, dot, signal yellow, type families, semantic tokens, motion/sound vocabulary.
- Console bar carries the **monogram** (chamfered D + dot) at 28u, never a 17px wordmark. Wordmark only at boot, About, and empty/error walls (≥40u, Console cut: heavier stroke, larger dot).

## 2. Visual direction: Cabinet
- Warm charcoal, matte anodised panels, one lit object per screen, radii **6/14/20/24u**, depth by luminance — a sheet is a lighter step with a 1px ring, never a 40u shadow.
- **One** blurred layer: the wallpaper behind the shell. No blur on panels; never animate `backdrop-filter`.
- **Signal yellow is the only chromatic event.** No Soft preset, no pastel accent list. `theme/console-room.ts` owns the material.
- The dot is the only ornament: cursor, progress head, unread badge, wordmark full stop.
- No six equal KPI tiles, no `01–06` indexes, no uppercase tracked mono labels, no gradient SaaS surfaces, no glass everywhere.

## 3. 10-foot hierarchy
- Type floor: **24u** secondary metadata, **28u** body/actions, **34u** surface title, **48u+** hero. No text below 24u, no ad-hoc `clamp()` — every heading is `text-bp-*`.
- Max 1 primary object + 1 rail. Two levels of hierarchy per screen; no dashboard.
- Contrast target **7:1** on near-black, in every room and theme (`theme/console-contrast.test.ts`).
- Mono is for numerals/units/shortcuts only; words use the UI face.
- Respect TV safe areas/overscan: the outer **5 %** band is decorative; the bar and footer bleed their background, their content stays inside. Test 1280×720, 1920×1080, 4K.

## 4. IA: stage + rail + band
- Home = **stage** (selected game, primary action), **rail** (5.5 covers at comfortable density, caption under the selected cover only), **band** (Search · Library · Downloads · Settings · Power) at the bottom.
- Library = shelf/grid + shelf chips; Collections is a chip, not a system destination.
- Game = one primary action + tabs (Overview · Saves · Artwork · Controller); no desktop game route.
- Downloads, Settings, Power, Search, Notifications are console surfaces, not desktop routes.
- Compositions: exactly three — **Stage**, **Wall**, **List**. No layout picker; the toggle lives in Settings.
- Settings = 4 rooms × 8 knobs (room, accent, wall, corners, density, motion, sound, haptics), live, reset per section, no Apply.

## 5. Focus and input parity
- One **focus store**; DOM focus follows it in one place. Never querySelector sweeps.
- Never move focus without user input. If the focused item disappears, move to a neighbour only on a discrete directional press; otherwise hide the ring.
- **B/Escape always returns to the exact previous target** (rail index, scroll, tab).
- Full D-pad, left stick, keyboard arrows, mouse, TV remote. Controller glyphs per family; A/B swap respected.
- No route a controller can reach but not operate. No desktop dialog or `/game/:id` escape.
- Exactly **two modals**: install confirmation and power confirmation. Everything else is a non-modal sheet; the library stays mounted and input is never silently disabled.
- One hint strip, max 3 hints; when a sheet is open, the page footer hides. One notice surface; no toasts in Console.
- OSK: 28u keys, delete/clear, recents; never trap focus in a portalled widget.

## 6. Customization without a theme engine
- 4 rooms × 8 knobs. A room sets the wall, corners, density, motion, sound and haptics at once; the player's accent and motion survive a room change.
- Live preview, no Apply button, reset per section. No per-element colour pickers, no CSS injection.
- "From artwork" accent: dominant chromatic cluster, OKLCH chroma ≤ 0.16, lifted to 7:1, cached per game; a cover with no colour falls back to the signal with one line of copy.
- Persist device-local (`ludex.console.room` v3, per server); account sync optional, never required.

## 7. Motion and sound
- Keep the existing motion table and the 11 sound cues; do not rewrite them.
- Cues fire at the start of the motion they describe; one cue per gesture; tile-move and launch are hash-frozen.
- The Motion knob maps onto the platform preference: Full keeps springs, Calm is the reduced fade, Off lands instantly.
- Springs on physical surfaces (overlay/panel/screen push/launch); focus stays a short tween; exits are opacity-only.
- Reduced motion → 120ms opacity; reduced transparency → opaque surfaces; reduced contrast → defined borders. Never make blur/parallax load-bearing.

## 8. Performance is accessibility
- 60fps rail scroll on the weakest target (WebKitGTK/Tauri Linux TV). Spike Gamepad API `mapping==="standard"`, hot-plug, timestamp before locking visuals; if broken, read pads in Rust/gilrs.
- One static blur layer; max 3 composited layers; no animated filters; artwork decode off the focus path; virtualize long grids.
- TV memory budget: target <380 MB UHD / <300 MB FHD; avoid full-screen bitmap walls.
- Separate query/render boundaries: download progress must not re-render the library.

## 9. Anti-slop tests
Every rule that matters is a test (`styles/console-cabinet.test.ts`, `styles/type-scale.test.ts`, `styles/console-material.test.ts`, `theme/console-contrast.test.ts`):
- no text below 24u; no uppercase mono words; no ad-hoc pixel type or `clamp()`;
- no six equal tiles/indexes; no layout picker; no Soft/pastel tokens or `data-console-appearance`;
- no `aria-modal` desktop dialog inside Console; no desktop route from Console;
- no toast in Console; one hint strip; one bracket; no second ring;
- no Lucide inside console chrome (drawn glyph set);
- 7:1 contrast; 720p/1080p/4K + 5 % overscan screenshots (`e2e/console-screenshots.mjs`).

A rule without a test will be gone in three releases.

## 10. Source of truth
- Research and blueprint: `docs/console/` (external, current, design, architecture).
- This skill is the short contract; if the blueprint changes, update both.
- Brand assets: `e2e/cabinet-assets.mjs` regenerates every icon and the Sunshine poster from the drawn mark.
