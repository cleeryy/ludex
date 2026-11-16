---
name: console-mode-ui
description: Design and build the 10-foot, gamepad-first Ludex Console 2.0 UI/UX.
---

# Ludex Console 2.0 — UI/UX Rules

These rules supersede the v0.4 "Big Picture" assumptions. They are product rules, not suggestions.

## 1. One identity, two densities
- Keep the name **Ludex** and the drawn wordmark. Console is a *device room*, not a second brand.
- Desktop stays dense (archive/hairlines/small meta); Console is dark-first, one object at a time, ten times the air.
- Shared: name, wordmark, dot, signal yellow, type families, semantic tokens, motion/sound vocabulary.
- Console bar carries the **monogram** (chamfered D + dot) at 28u, never a 17px wordmark. Wordmark only at boot, About, and empty/error walls (≥40u).

## 2. Visual direction: Cabinet
- Warm charcoal, matte anodised panels, one lit object per screen, 6/14/20/24u radii, soft shadows only for depth.
- **One** blurred layer: the wallpaper behind the shell. No blur on every panel; never animate `backdrop-filter`.
- **Signal yellow only** chromatic event. Delete the Soft preset, the five pastel accents, coral/mint/lilac.
- The dot is the only ornament: cursor, progress head, unread badge, wordmark full stop.
- No six equal KPI tiles, no `01–06` indexes, no uppercase tracked mono labels, no gradient SaaS surfaces, no glass everywhere.

## 3. 10-foot hierarchy
- Type floor: **24u** secondary metadata, **28u** body/actions, **48u+** hero titles at 1080p. No text below 24u.
- Max 1 primary object + 1 rail. Two levels of hierarchy per screen; no dashboard.
- Contrast target **7:1** on near-black; use luminance steps, not drop shadows, for elevation.
- Mono is for numerals/units/shortcuts only; words use the UI face.
- Respect TV safe areas/overscan (≥5% margin) and test 1280×720, 1920×1080, 4K.

## 4. IA: stage + rail + band
- Home = **stage** (selected game, primary action), **rail** (recents/shelf, 5.5 visible), **band** (Search · Library · Downloads · Settings · Power).
- Library = shelf/grid + shelf chips; Collections is a shelf, not a system app.
- Game = one primary action + tabs (Overview · Saves · Artwork · Controller); no desktop game route.
- Downloads, Settings, Power, Search, Notifications are console surfaces, not desktop routes.
- Boot → Home in ~1.2s, no spinner; first run is four honest states (signed out / no server / empty / offline).

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
- 4 rooms × 8 knobs: Rooms/preset, accent (theme/from artwork/curated), wall (art/solid/gradient), corners, density, motion, sound, haptics.
- Live preview, no Apply button, reset per section. No per-element color pickers, no CSS injection.
- "From artwork" accent is extracted and runtime-validated at 7:1.
- Persist device-local; account sync optional, never required.

## 7. Motion and sound
- Keep the existing motion table and the 11 sound cues; do not rewrite them. Add only press, detent, boot-mark.
- Cues fire at the start of the motion they describe; one cue per gesture; tile-move and launch are hash-frozen.
- Springs on physical surfaces (overlay/panel/screen push/launch); focus stays a short tween; exits are opacity-only.
- Reduced motion → 120ms opacity; reduced transparency → opaque surfaces; reduced contrast → defined borders. Never make blur/parallax load-bearing.

## 8. Performance is accessibility
- 60fps rail scroll on the weakest target (WebKitGTK/Tauri Linux TV). Spike Gamepad API `mapping==="standard"`, hot-plug, timestamp before locking visuals; if broken, read pads in Rust/gilrs.
- One static blur layer; max 3 composited layers; no animated filters; artwork decode off the focus path; virtualize long grids.
- TV memory budget: target <380 MB UHD / <300 MB FHD; avoid full-screen bitmap walls.
- Separate query/render boundaries: download progress must not re-render the library.

## 9. Anti-slop tests
Every rule that matters is a test:
- no text below 24u; no uppercase mono labels outside numerals;
- no six equal tiles/indexes; no layout picker; no Soft/pastel tokens;
- no `aria-modal` desktop dialog inside Console; no desktop route from Console;
- no toast in Console; one hint strip; one focus ring;
- 7:1 contrast; 1080p/720p/4K + overscan screenshots; keyboard/controller E2E.
A rule without a test will be gone in three releases.

## 10. Source of truth
- Research and blueprint: `docs/console/` (external, current, design, architecture).
- This skill is the short contract; if the blueprint changes, update both.
