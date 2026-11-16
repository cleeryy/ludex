# Ludex Console 2.0 — brand and UX blueprint

> **Status:** the Cabinet system shipped in #268. Rooms replaced presets, the
> six layouts became Stage / Wall / List, and the 20u type step is gone. Read
> "Shipped in #268" in [the blueprint](../README.md) before quoting a preset or
> a layout from this research record; the decisions here are the rationale, not
> the current API.

**Author:** Designer (read-only turn). No repository files were modified.
**Date:** 2026-11-16. **Repo:** `7199a2c` + `#237` merged (Studio default, 10-foot type, pad escape).
**Scope:** product identity, Console 2.0 visual language, design system, IA, screen blueprints, customization, motion/sound, implementation plan.

---

## Verdict in twelve lines

1. **The sound is the art direction.** `ui-sound-specs.ts` already describes the product: *"dry archive — a short transient plus one pitched body."* Dry, matte, one event. That is a complete visual brief, and it is the only part of the current Console the user likes that is also frozen. Derive the look from the audio, not the other way around.
2. **Do not reskin Soft and do not build a third preset.** Delete Soft, delete the five pastel accents, cut six layouts to two, delete the layout picker. The problem was never the hue; it was six competing decisions shipped behind a preference row.
3. **One identity, two densities.** Keep the Ludex name and the drawn wordmark. The desktop stays the archive (dense, hairlines, 11.5px meta). The Console is the *device room*: same letters, same yellow, same dot, ten times the air.
4. **Recommended direction: Cabinet.** Warm charcoal, matte anodised panels, one lit object, 14–20u radii, one blur spent on the wallpaper behind everything, safety yellow as the only chromatic event, and the wordmark's full stop promoted to the product's only ornament.
5. **The dot is the brand.** It is the cursor ("here"), the progress head ("not yet"), the badge ("new"), the wordmark's period. One 8u circle that means "position". It gives the product its "cute" without a single pastel.
6. **Offline is a feature, not an error.** A DRM-free library that says *"Offline · 42 games still play"* is a brand statement no storefront can copy. Build the state properly.
7. **Home is three bands, one axis:** stage (the selected game), rail (recents), system band (Search · Library · Downloads · Settings · Power). No six-tile grid, no 01–06 indexes, no KPI composition anywhere.
8. **Mono is for numbers only.** Uppercase tracked JetBrains Mono labels are the single strongest "generated dashboard" tell left in the build. Bricolage for display, Instrument Sans for words, Mono for numerals/units/shortcuts.
9. **Customization = 4 rooms × 8 knobs**, live-previewed, no Apply button, no per-element colour pickers, no CSS injection. "100% customizable" means every surface you can point at resolves to one of those twelve decisions.
10. **Motion table stays frozen**; three entries are added (press, detent, boot mark). No new cue either — the existing eleven, fired at the start of the motion they describe.
11. **Every anti-slop rule must be a test**, because this repo already enforces its design system with tests. A rule that is not a test will be gone in three releases.
12. **What needs hardware:** Tauri install sheet, the floating overlay over a live game, launch/focus loss, power actions, controller families, 4K blur cost, real overscan. Everything else is verifiable in the browser.

---

## 0. What already landed, and what it does not fix

`#237` fixed the largest audit findings: Studio is the default, Soft no longer owns a palette, console type is expressed in `--bp-u` (28/24/20u), the install dialog no longer traps the pad (B escapes it), the SystemBar no longer collides at 390px. Credit where it is due — do not re-litigate those. Note that escaping a dialog is not completing it: install is still a desktop surface with a 10-foot escape hatch, which is why §5.4 replaces it outright.

What `#237` did not touch, and what Console 2.0 is actually about:

| Still true today | Evidence | Why it reads as slop |
| --- | --- | --- |
| Six equal home tiles with a corner icon, a `01`–`06` index and an uppercase mono caption | `home-row.tsx:110-138` | It is a KPI grid. Position is already spatial; the index is decoration. |
| Uppercase mono at 20u with `0.08–0.18em` tracking used for *labels* everywhere | `system-bar.tsx:94`, `home-row.tsx:128`, `settings-app.tsx:108` | At 3 m this is a grey texture, not words. It is also the "technical dashboard" signature. |
| Six layouts, each with its own composition, chosen from an overlay | `layout.ts:15`, `layout-switcher.tsx` | Asks the player to be a designer before they can play. Six is a decision the product refused to make. |
| Board art used as a full-bleed wall with a 26u blur plus a 18u blur on the plate plus six tiles over it | `index.css:1065-1080` | Blur is spent everywhere, so it means nothing; and it is the most expensive thing on a 4K panel. |
| Console settings shows two rows on an empty 70% canvas | `settings-app.tsx` | A settings *list* is fine; an empty room around it is not. |
| Decorative indexes repeated on Power and in search results | `power-app.tsx:79-81` | Repetition of a meaningless ornament is the classic generated-UI tell. |

So: **the type is now big enough; the composition, the material budget, the copy register and the iconography are the work.**

---

## 1. Rebrand scope

### 1.1 Recommendation: one identity, two densities — not two brands

|  | Desktop / web | Console 2.0 |
| --- | --- | --- |
| Job | curate, install, configure, debug | browse, launch, play |
| Polarity | follows the OS, light or dark | device-owned, **dark-first** |
| Density | dense tables, 11.5px meta, 207 bordered panels | one object at a time, 28u body |
| Material | hairlines + 8 shadow recipes, no glass | 3 levels, **one** blurred wall, matte |
| Type | Bricolage display / Instrument body / Mono data | same three families, 6 sizes, mono = numerals only |
| Motion | 120–240 ms, restrained | same table **plus** springs on physical surfaces |
| Sound | none | the eleven-cue language |
| Brand presence | wordmark in the rail | monogram in the bar, wordmark only at boot and About |

The two must share: the name, the wordmark and its dot, the semantic token vocabulary, the signal yellow, the type families, the motion/easing tokens, the cue table. They may differ in: density, polarity default, materials, which verbs exist.

**Why not a separate Console brand.** A second name would need its own logo, own copy, own docs — and the Console only exists because the library does. The "device room" framing gets the separation without the cost: same house, different furniture.

**Why not a full desktop rebrand.** The audit is right that the desktop is the coherent half. Rebranding it while rebuilding the Console produces two half-finished languages. Touch the desktop only where the shared system changes: wordmark behaviour, the 16-glyph icon set, the mono-for-numbers rule, and the dot.

### 1.2 Name

**Keep "Ludex".** Five letters, two syllables, no diacritics, pronounceable in FR/EN/ES, reads as *ludus* + *index* (play + catalogue), and the drawn alphabet already exists in source. Renaming would cost the wordmark, the app icon, the repo, the docs, the deck, the 3,864 i18n messages that mention the name in help copy, and the user's own habit — for no gain in meaning.

Two things to check, not to design around:

1. **Trademark search (EU/US, class 9/41).** "Ludex" is used by card-collecting software in the US. Different product class, but have it checked before spending on merchandise or a domain purchase.
2. **Route naming.** Resolve the audit's split now: user-facing **Console**, route `/console` with `/bigpicture` as a permanent alias, internals stay `bp-*`. One name in the UI, one alias in the router.

### 1.3 Wordmark: keep the drawing, fix the system

The drawing is good — compressed geometric, squared U, chamfered D, signal full stop. It is not being used like a mark.

**Rules**

- **Minimum size.** The full wordmark never renders below **40u** (≈40px at 1080p). At 17px in the system bar it is a grey smear; the current build does exactly that (`system-bar.tsx:126`).
- **Three appearances only, in Console:** boot (stacked lockup, 96u mark above 48u wordmark), Settings → About (horizontal, 40u), and the error/empty wall (48u). Nowhere else.
- **The bar carries the monogram, not the wordmark.** The chamfered D + dot at 28u, `aria-label="Ludex"`. A mark is legible at 3 m; a 17px wordmark is not.
- **Clear space** = two dot diameters on every side. Nothing overlaps it, no artwork behind it, no border around it.
- **Colour.** Ink or paper only. The dot is **always** `--signal`. Never recolour the wordmark to the accent, never apply a gradient, never outline it.
- **10-foot cut.** One heavier weight (the current 22u stroke reads thin at 3 m), the counters opened by ~8%, the dot enlarged to 22/100. Ship as a second SVG path set (`wordmark-console.svg`), not a font.
- **App icon.** Mark on `bg-deep`, dot bottom-right, rounded-square mask for Windows/macOS, no text.

**"Console" is not a sub-brand.** Delete the `Console` eyebrow next to the wordmark (`system-bar.tsx:130`). The word appears once at boot ("Ludex Console"), and in Settings → About ("Console mode"). A second logo for a mode is how products get three logos.

### 1.4 Decisions the user must make

| # | Decision | My recommendation | Cost of the other answer |
| --- | --- | --- | --- |
| 1 | Keep the name "Ludex"? | **Yes** | Full identity reset, ~2 days of asset/route/i18n work, plus the risk of a worse name chosen under deadline. |
| 2 | Console dark-first even when the app theme is light? | **Yes** (device-owned room, Daylight available) | Console inherits a paper-white TV UI at night, which is the worst possible 10-foot default. |
| 3 | Delete the Soft preset and the five pastel accents? | **Yes, both** | Two products forever, and coral/mint/lilac stay the strongest slop signal in the build. |
| 4 | Cut six layouts to two (+ List)? | **Yes** | Six compositions to art-direct, six to test at three resolutions, and no committed home. |
| 5 | Console-only rebrand or whole product? | **System-level identity shared; desktop density untouched** | A desktop rebrand freezes the Console work for a quarter. |
| 6 | Device-pairing sign-in (code on phone) in v1? | **v1.1**, not v1 | v1 keeps the password field; ship the console anyway. |
| 7 | State "offline-first" as a brand promise? | **Yes** | Losing the only differentiator a self-hosted library has. |
| 8 | Ship the Daylight room in v1? | **v1.1** (Night + Projector in v1) | Two full token maps to validate in the first pass. |

---

## 2. Three visual directions

All three: no purple/teal SaaS gradient, no six-tile grid, no decorative indexes, no glass everywhere, no neon. Each is a complete system you could hand to an engineer.

### 2.1 Direction A — **Cabinet** *(recommended)*

**Mood.** A well-made piece of furniture for your games. A Braun hi-fi front, a Switch dock, a mid-century record cabinet: one object is lit, everything else is matte and quiet. The room is dark because a TV is a lamp.

**References.** Braun/rams device panels (one lit indicator per function); the Switch's *physical* logic (large isolated objects, one red, generous air); Teenage Engineering for the material honesty (visible fasteners, no fake gloss); the archive half of Ludex's own desktop (hairline + signal).

**Palette.** The existing family, tuned for 10 feet; no new brand colour.

| Token | Night (default) | Projector (TV-safe) | Daylight (v1.1) |
| --- | --- | --- | --- |
| `bg` | `#0d0c0a` | `#080807` | `#e8e3d5` |
| `bg-deep` | `#070706` | `#000000` | `#ddd7c6` |
| `surface` | `#16140f` | `#141310` | `#f2eddf` |
| `surface-2` | `#1d1a13` | `#1b1914` | `#f8f4e8` |
| `elevated` | `#242017` | `#232018` | `#fffdf6` |
| `line` | `#2b2820` | `#3a362b` | `#d3cbb8` |
| `line-2` | `#3d392c` | `#57513f` | `#b7ae99` |
| `ink` | `#f3eee2` | `#fbf7ec` | `#191712` |
| `ink-2` | `#bcb4a3` | `#cfc7b4` | `#4e4839` |
| `ink-3` | `#948c7b` → TV `#aca494` | `#aca494` | `#6b6453` → TV `#5a5342` |
| `signal` | `#ffc61a` | `#ffd23f` | `#f0c000` |
| `on-signal` | `#14120a` | `#14120a` | `#191712` |
| `danger` | `#ff6a5c` | `#ff8074` | `#a82c1c` |

Computed pairs (night): ink/bg ≈ 17:1 · ink-2/surface-2 ≈ 8.7:1 · ink-3/surface-2 ≈ 5.4:1, lifted to ≈ 7.3:1 by the TV map · signal/bg ≈ 12:1 · on-signal/signal ≈ 11.6:1. **Rule: no console text pair below 7:1**, which is the skill's target, not AA's 4.5.

**Type.**
- Display: Bricolage Grotesque 600, optical sizing on, tracking **−0.02em** (the current −0.04/−0.045 reads as fashion, not as type).
- UI: Instrument Sans 400/500, tracking 0, sentence case.
- Data: JetBrains Mono 400, **tabular**, tracking +0.02em, for numerals, units (`GB`, `MB/s`), percentages, clocks, shortcuts only.
- **No uppercase words** except two-letter glyph labels and units. This one rule removes most of the remaining dashboard read.

**Materials.** Matte anodised panels. One soft-touch surface (the selected object) that is *opaque* and slightly raised. No glass on tiles, ever. Exactly **one** blurred layer in the whole screen: the wallpaper behind everything, painted once, never animated.

**Radius.** `cover 6u` · `object 14u` · `surface 20u` · `sheet 24u` · pill. Nothing exceeds 24u. Cuteness comes from a generous radius on the *one* object under the cursor, not from rounding everything.

**Iconography.** 16 glyphs drawn for the product (see §3.7), 2u stroke, 1u terminal radius, filled variant for the active state, one accent detail (a dot).

**Motion.** The frozen table plus three additions (press squash, rail detent, boot mark). Surfaces spring; focus never does.

**Sound relationship.** Exact. The cues are dry transients; the visuals are matte surfaces with no gloss, no gradient, no glow. Nothing about this direction needs the cue table retuned.

**Failure mode.** Can read as *too* severe if the air band and the single lit object are dropped. The warmth lives in the charcoal (not pure black) and the paper-warm ink.

**Why it is recommended.** It is the only direction the existing, frozen sound language already speaks; it is the only one that does not require a second palette; it survives bright box art; and it reads as "cute minimal" at 3 m because exactly one thing glows.

### 2.2 Direction B — **Lantern** *(the honest alternative — and the trap)*

**Mood.** A softly lit room at night: paper lamp, dim warm light, everything slightly out of focus. The Switch's cosiness taken literally.

**References.** Nintendo Switch home, iOS 17 widget softness, Paper by FiftyThree, warm-dark reading apps.

**Palette.** `#17151c` bg, `#221f28` surface, `#f6efe9` ink, `#ff9e7d` peach accent, plus a per-game artwork tint at 18% chroma. Radii 26–34u. Blur 30u on the wall and 16u on the sheet. Type in a rounded grotesque (Bricolage at 500 is the closest available).

**Why it looks good in a mockup.** Warm, soft, forgiving of bad artwork. It photographs well.

**Why it fails here.**
1. **It is the current slop's better-looking cousin.** Peach + cream + translucency + 30u radii is the exact recipe that produced the "AI slop" reading; a better-executed version of the same recipe is still the same recipe.
2. **It cannot survive bright box art** without the opaque-plate fix that `#237` already had to make — which removes the translucency that *is* the direction.
3. **It needs different sound.** A glowing, soft, diffuse interface with a dry 45 ms tick is a mismatch. The user likes the cues and they are frozen; the visual must move, not the audio.
4. **It has no edge,** so focus has to be carried by glow and scale, which is the least legible focus treatment at 10 feet.

**What survives into A:** the warmth of the charcoal and the paper ink, and generous radii on large objects. That is Lantern's good half, without the veil.

### 2.3 Direction C — **Enamel** *(bold alternate; borrow its boot, not its palette)*

**Mood.** An arcade cabinet's attract mode printed as a Swiss poster. Enamel paint, hard edges, one saturated colour, type as architecture.

**References.** Teenage Engineering, Playdate's packaging, arcade marquees, Müller-Brockmann posters, Panic's hardware.

**Palette.** `#111114` near-black, `#f4f1e8` paper, one enamel `#2f6bff` (or acid `#d7ff3e`), hard 0–2u radius, zero blur, 1u ink hairlines. Type at 96–140u, uppercase, tight leading. Motion: snap, 90–140 ms, no springs. Sound would want louder, snappier cues.

**Why not as the whole system.** It shouts, and a library should be quiet. Text-heavy surfaces (downloads, settings, install) look like posters, not tools. It also throws away the spring work the user likes and would want the cue table retuned. Its uppercase display type at 10 feet is superb *for one screen*.

**What survives into A:** the boot screen. Direction A's boot is Enamel's composition in Cabinet's palette: mark huge, one line of copy, nothing else.

### 2.4 Recommendation

**Ship Cabinet, with Enamel's boot and Lantern's warmth.** Concretely: Cabinet's tokens and materials, a boot sequence built like an Enamel poster (huge mark, one line, 1.2 s), and the warmed charcoal/paper ink that makes Lantern feel human. Ban the veil.

**What would change my mind:** if the room the console lives in is bright most of the time (daylight living room, no curtains), Daylight becomes the primary room and the material budget shifts from "one lit object in the dark" to "paper and ink" — Cabinet still works, but the *default* polarity flips. Ask the user which room the TV is in before locking the default.

---

## 3. Design system

### 3.1 Tokens

Keep the existing 27-token semantic vocabulary (`theme/types.ts`). Console adds exactly three and removes one.

| Change | Token | Purpose |
| --- | --- | --- |
| add | `--bp-rim` | 1px warm highlight on the selected/lifted object (signal at 55% over surface). Gives the "anodised edge" without a shadow. |
| add | `--bp-wall-tint` | artwork-derived colour, used **only** inside the wallpaper gradient, clamped to 18% chroma. Never reaches a control. |
| add | `--bp-safe-x` / `--bp-safe-y` | overscan-aware gutters (see §3.8). |
| remove | `--sheen` usage in Console | the only sheen left is 6% on the sheet's top edge. Glass is gone. |

Everything else is the active app theme. **The console never ships its own colour map** — that decision (`#237`) is correct and must survive the rebrand.

### 3.2 Type

Scale, in `--bp-u`. `1u ≈ 1px at 1920`; the clamp makes the physical size identical at 720p, 1080p and 4K (verified: 28u = 20.2px at 720p on a 55" ≈ 28px at 1080p ≈ 56px at 4K, all ≈ 10 arcmin x-height at 3 m).

| Role | Token | 1080p px | x-height at 3 m | Use |
| --- | --- | --- | --- | --- |
| Boot | `--bp-text-boot` | 72 | 26′ | wordmark only |
| Hero | `--bp-text-hero` | 56 | 20′ | selected game title (max 2 lines, 20ch) |
| Title | `--bp-text-title` | 34 | 12′ | sheet titles, section heads |
| Body | `--bp-text-body` | 28 | 10′ | **every label a player must read to act** |
| Secondary | `--bp-text-meta` | 24 | 8.7′ | second line under a label, never the only text |
| Numerals | `--bp-text-micro` | 20 | 7.3′ | counts, clocks, `1/24`, sizes. Never a word. |

**Rules**
1. Nothing below 20u. Nothing below 24u may carry information that is not already stated in words elsewhere on screen.
2. Line height 1.0 for hero/title, 1.25 for body/meta.
3. Tracking: −0.02em hero/title only; 0 everywhere else; +0.02em on numerals.
4. **Sentence case for words; uppercase only for 2-letter glyph labels and units.**
5. Max 2 lines of title, max 3 lines of description, then truncate with the full text in the detail sheet.
6. Numbers are always tabular, always mono, always with a unit.

### 3.3 Spacing

`--bp-s1 4u · s2 8u · s3 12u · s4 16u · s5 24u · s6 32u · s7 44u · s8 64u · s9 96u`.

- Page gutter = `--bp-safe-x` (≥ 44u).
- Vertical rhythm: 16u inside a group, 32u between groups, 64–96u between bands.
- Row min-height 64u; primary button min-height 72u; band item 96u wide.
- **Air band:** at least 96u of untouched space above the rail on home. This is what makes it read as a console rather than a dashboard; it is a hard minimum, not a preference.

### 3.4 Radii

`cover 6u` · `object 14u` · `surface 20u` · `sheet 24u` · `pill 999`.

- Covers keep 6u: box art must stay a rectangle with softened corners, not a pill.
- Only *surfaces* (panels that hold other things) go above 14u.
- Nothing exceeds 24u. Beyond that, a 55" TV starts to look like a phone.
- Radius is a *room* decision (4 steps: 0 / soft / round / soft-max), applied through `--bp-radius-*`; components never hardcode one.

### 3.5 Elevation, materials and the blur budget

| Level | What | Recipe |
| --- | --- | --- |
| L0 wall | the wallpaper | flat `bg-deep`, optional blurred art at 40% under an 80% stage wash. **The only blurred layer.** |
| L1 chrome | bar, band, hint strip | opaque `bg-deep`, 1px `line`, no shadow |
| L2 object | tiles, covers, cards | opaque `surface`, 1px `line`, `--bp-rim` when selected, 2u lift, shadow 24u/30% |
| L3 sheet | overlays, install, settings panels | opaque `surface-2`, 1px `line-2`, 40u shadow, **blur only if a wall is visible behind** |

**Budget**
- At most **2** blurred layers on screen: the wall, plus (only when a sheet is open) the sheet. Never a third.
- Never `backdrop-filter` on an element that moves (`transform`) or scrolls.
- **While a game is running: blur budget = 0.** The overlay window is 92% opaque; you never blur someone's game.
- `prefers-reduced-transparency: reduce` → both layers opaque, same borders and radii.
- `prefers-contrast: more` → `line` becomes `ink-2`, rim becomes `ink`, wash becomes 90%, no tint.

### 3.6 Focus: the bracket

The ring is the reason the current build shows a "double ring that reads as an error state". Replace it with a **bracket**, which cannot double up:

- 6u signal block on the object's **leading edge**, 40% of its height, vertically centred.
- 1px `--bp-rim` around the perimeter (signal 55%).
- 4u lift and 1.5% scale — **only** on covers/tiles, never on rows. No scale under reduced motion.
- Label flips `ink-2 → ink`; all other labels on screen step down to `ink-3` so exactly one thing is bright.
- **List rows**: 3u leading bar + `surface-2` fill. No outline at all.
- Global `:focus-visible` still exists for keyboard parity, but `[data-bp-card]`, `[data-bp-row]` and `[data-overlay-active]` suppress it — one ring per object, always.
- Focus is `--focus` (signal). Because the primary button is also signal, **a focused object and a signal fill are never adjacent**: 24u gap, or the primary button is the *only* signal fill on screen.

### 3.7 Controller glyphs

Replace the Lucide-glyph-in-a-kbd-box with a drawn set. 24u grid, 2u stroke, 1u terminal radius, drawn at 28u.

| Glyph | Notes | Glyph | Notes |
| --- | --- | --- | --- |
| A / B / X / Y | letter in a 28u circle; A/B swap for Nintendo family | LB / RB | rounded rect, 3u corner |
| LT / RT | same rect, "trigger" notch | D-pad | plus, 2u |
| L-stick / R-stick | circle + 3u nub | Menu | two lines (hamburger, 2u) |
| View | two overlapping rects | Guide | the dot in a ring |
| Kbd | keyboard slab, 3 rows of 5 dots | More | three 6u dots |

- Never colour a glyph except the active one (signal). All others `ink-3`.
- The hint strip shows **max 3 hints**, ordered by frequency, `A` first when there is a primary. Everything else lives behind `More` (the three dots) and opens a sheet titled "Controls".
- A hint whose action is unavailable is **not shown** (never a disabled glyph).
- `aria-label` on each hint group: "Controls: A play, B back, Menu options".

### 3.8 Safe area and overscan

- `--bp-safe-x: max(44u, 3vw)`, `--bp-safe-y: max(24u, 3vh)`.
- Setting: **Overscan — Off / 3% / 5%** (default 3%). At 5% nothing interactive may enter the outer band; the hint strip gets its own background so a crop never eats it.
- Title-safe = content; action-safe = anything focusable. The rail scrolls *under* the gutter, so a lifted cover never clips.
- The bar and band are inside action-safe; the wall art bleeds past both.

### 3.9 Preferences

| Preference | Behaviour |
| --- | --- |
| `prefers-reduced-motion` | per-kind table (`REDUCED_MOTION`): 120 ms fades or instant; focus instant; launch plate becomes a 2-frame cut + hold, no FLIP, static filament segment |
| `prefers-reduced-transparency` | both blur layers opaque; art filter off; tint off |
| `prefers-contrast: more` | ink borders, no wash, TV map, focus bracket 3u |
| `forced-colors` | all materials to system colours; focus to `Highlight`; the dot becomes a 2px border (v1.1) |
| Text size (in-app) | Console type multiplier 1.0 / 1.15 / 1.3 applied to `--bp-u` **only** (geometry stays), which grows type without breaking the grid |

---

## 4. Navigation and IA

### 4.1 The map

```
Boot ─┬─ Sign-in (device code, v1.1)
      └─ Home ─┬─ Stage (selected game) ── Play / Install ── Launch plate
               ├─ Rail (Recent / All / shelf) ── select ── stage
               ├─ Library (grid or list + shelf chips + Collections)
               ├─ Game surface (Overview · Saves · Artwork · Controller)
               ├─ Search (keyboard + recents + A–Z buckets)
               ├─ Downloads (queue, bulk actions)
               ├─ Settings (Rooms · Appearance · Controllers · Streaming · About)
               ├─ Power (Sleep · Restart · Quit · Sign out)
               ├─ Notifications (sheet from the avatar)
               └─ Overlay (in-game window: Downloads · Now playing · Controls)
```

Five destinations, six surfaces. **No layout picker. No dashboard.** Collections is a shelf inside Library, not a system app.

### 4.2 Home composition (1920×1080, 1u = 1px)

```
┌──────────────────────────────────────────────────────────────┐  bar 56u
│ ◐ mark      (status: wifi · 2↓ 61% · pad 72% · clock · avatar●) │
├──────────────────────────────────────────────────────────────┤
│                                                              │  air band 96u
│  CONTINUE PLAYING            ← 24u, sentence case, ink-2      │
│  Hades II                    ← 56u, 2 lines max, ink          │
│  2025 · Installed · 12 h     ← 24u, ink-2                     │
│  [ ▶ Play ]  [ Manage ]      ← 72u tall, primary signal fill  │
│                                                              │  stage = 55%
│                                                              │
│   ▐  ██   ██   ██   ██   ██   ██                             │  rail (5.5 visible)
│     └ selected: lift 8u, rim, label under it only             │
├──────────────────────────────────────────────────────────────┤
│ ⌕ Search   ▤ Library   ↓ Downloads·2   ⚙ Settings   ⏻ Power  │  band 96u
│ A play   B back   ☰ options                                   │  hints 3 max
└──────────────────────────────────────────────────────────────┘
```

- Covers 220×330 (2:3), 24u gap, caption **only** under the selected cover.
- Pagination: 5 dots, 6u, centred in the band. Never `1 / 24` at the top of the screen.
- The bar shows *state* only; the band shows *destinations* only. No item appears twice.
- At 1280×720 the same layout with `u = 0.72`; the rail shows 4.5 covers. At 4K it shows 5.5 at double size.

### 4.3 Focus model

| Level | Zones | Rules |
| --- | --- | --- |
| Home | `stage-actions` → `rail` → `band` | Up/down moves between zones; left/right never leaves the current zone except at the rail edge, which detents (2u bounce + soft haptic) instead of moving |
| Library | `chips` → `grid` → `band` | Roving tabindex; a grid row holds its column index when moving up/down |
| Game | `tabs` → `rows` | Shoulders switch tabs; never a second column (the classic console trap) |
| System surfaces | the row list | Left/right cycles a choice value in place, always live, no confirm |
| Sheets | the sheet's rows | Sheet owns the ring; the surface behind is `inert` |

- **B always returns to the exact previous target**, including rail index and scroll offset.
- **No route a controller can reach but not operate.** Desktop-only verbs are either console-native or absent. Anything genuinely desktop-only gets a QR bridge (v1.1), never a dead navigation.
- Mouse/pointer parity is automatic: every focusable is a real `<button>`, hover highlights, click activates, wheel scrolls the rail. Keyboard parity: arrows, Enter/Space, Escape, `/` search, `M` menu, `Esc` back.

### 4.4 Modal rules

Exactly **two** modals: install confirmation and power confirmation. Everything else is a non-modal sheet — the library stays mounted and live, because a couch player must never be trapped (§ the audit's install dead end).

- A modal must be completable with `A`, cancelled with `B`, and never contain a portalled widget that escapes focus.
- One hint strip only. When a sheet is open, the page footer hides.
- One notice surface: bottom-leading, above the band, max one at a time, `B` dismisses, 6 s hold for transient, persistent for errors. **No toasts in Console.**

### 4.5 Boot and first run

**Boot, 1.5 s, no spinner.**
1. 0 ms: black.
2. 80 ms: mark fades in (240 ms), dot pulses once (`scale 0.9→1`, 600 ms, ease-out).
3. 1200 ms hold: one line at 24u, `ink-3`: "Ludex Console".
4. 240 ms crossfade to the shell; the rail entrance cascade plays (existing 434 ms).

If the library query resolves before 600 ms, shorten the hold — never flash the mark. Never show a spinner; skeletons are the *shape* of the content (stage block + 6 plates).

**First run, four honest states, no wizard:**
| State | What the stage shows |
| --- | --- |
| Not signed in | "Sign in on this device" — code + URL (v1.1) or the password field (v1). B = change server. |
| No server | "Can't reach your library" + Retry / Change server / Exit. Error text behind "Details". |
| Empty library | "Your library is empty" + Scan a folder (desktop) / Import from a server (web) / Open the desktop app (browser). |
| Offline, games installed | Normal home. Bar chip "Offline"; stage line "42 installed games still play". |

No coach marks, no tour, no modal onboarding. The hint strip teaches the pad in one line.

---

## 5. Screen-by-screen blueprint (v1)

Every screen: bar + band persist (except boot/launch), one hint strip, one focus ring.

### 5.1 Home
**On screen:** stage (art 40% + wash, eyebrow, 56u title, 24u meta, primary + secondary), rail, band.
**Hierarchy:** title → primary action → rail. The rail is the second read, not the first.
**Interaction:** left/right walks the rail and the stage follows with a 120 ms crossfade (existing `SpotlightBackdrop`, reused); `A` plays/installs; `X` opens the game surface; `☰` opens the command sheet; shoulders change shelf.
**Loading:** stage block + 6 cover plates as skeletons, no shimmer (a moving shimmer is decoration at 3 m; use a 1px top line that fills once).
**Empty:** as §4.5.
**Error:** one sentence + Retry; the raw message behind "Details".

### 5.2 Library
**On screen:** title "Library" 34u + count 24u; shelf chips (Recent · All · Installed · Not installed · Missing · each Collection); grid 6×3 at 1080p; bottom detail strip for the focused cover (title, year, state, `A` action).
**Interaction:** roving grid; shoulders page by shelf; `Y` filters; `X` toggles grid/list; missing archives **stay in the grid** with a `Missing` chip (the audit's #11 — never hide a game whose drive is offline).
**Loading:** 18 cover-shaped skeletons, same grid.
**Empty:** "Nothing here yet" + the filter that produced it as a chip + "Clear filter".
**Error:** inline banner above the grid, grid still shows what it has.

### 5.3 Game surface
**On screen:** full-bleed art top 55% with the title over it (56u), meta 24u, action row (`Play`/`Install` primary, `Manage` secondary), tabs (Overview · Saves · Artwork · Controller), rows 64u.
**Hierarchy:** one primary action; the second is a hairline plate, never equal weight.
**Interaction:** shoulders switch tabs; rows are one focus stop; `B` pops to the exact rail index.
**Rules:** no desktop dialog, no route to `/game/:id`; install opens the console sheet. `Open in desktop` does not exist here.
**Loading:** the surface renders immediately with the art it has and a skeleton for the tab body.
**Error:** the action row shows `Retry` in place of `Play`, with one sentence.

### 5.4 Install sheet
**On screen:** title "Install {game}" 34u, a drive list (rows: name, free space, `Recommended` chip), a space line ("48.2 GB needed · 212 GB free"), a size/quality choice if available, `Install` primary, `Cancel` secondary.
**Interaction:** drive rows are the focus list; `A` selects and confirms; `B` cancels at any point; no native `<select>`, no portal.
**States:** estimating (one line "Checking…", 2 s max), insufficient space (reason + the biggest free drive suggested), blocked (reason + the fix).
**This replaces `InstallDialog` on console entirely.** The desktop keeps its dialog.

### 5.5 Launch plate
**On screen:** cover FLIPs to full bleed; 0.35 dim; eyebrow "Launching"; title 56u; 2u filament; one status line 28u.
**Interaction:** `B` only after the 8 s give-up; everything else swallowed.
**States:** launching / slow ("Still starting — this can take a moment") / failed (reason + `Back`) / cancelled.
**Reduced motion:** 2-frame cut, no FLIP, static filament segment, same copy.

### 5.6 Downloads
**On screen:** aggregate line 34u ("2 downloading · 61% · 14 min left"), then rows: 40u percent numerals, title 28u, one phase line 24u, `⋯` action menu. Bulk `Pause all` / `Clear finished` top-right.
**Interaction:** `A` on a row opens its menu (Pause, Cancel, Retry, Open folder, Remove); `X` pauses all; the menu is a sheet, not 9 inline buttons.
**Rules:** one primary metric per row; no chart in the header (move the sparkline into the aggregate line as a 1px baseline, or drop it); a row never shows 100% and "waiting for a slot" at once — the phase wins and the percentage is replaced by an indeterminate bar.
**Empty:** "Nothing downloading" + one line "Installs run in the background while you play."

### 5.7 Search
**On screen:** query line 34u, on-screen keyboard (28u keys), recents (3), result grid 4×2.
**Interaction:** dpad on the keyboard, `A` types, `LB/RB` jump sections, `B` clears then closes.
**Empty:** recents + "Type to search" — never an empty void.
**Result:** returns the grid with the selection, not a new screen.

### 5.8 Settings
**On screen:** title, one line of what this section does (24u), section tabs, rows 64u with the value on the right, footer facts (version, server, account, pad) as a 4-up line — not four boxes.
**Sections:** Rooms (presets + preview), Appearance (accent, wall, corners, density, band), Controllers (profile, swap, haptics, cue volume + 3 previews), Streaming, About.
**Rules:** changes apply **live**; no Apply button; each section has `Reset section`; the preview strip (240×135 mini-home) renders the real components; no roadmap text, no preset internals in copy ("SHELF · SOFT" does not exist).

### 5.9 Power
**On screen:** four rows 88u: Sleep, Restart, Quit, Sign out — glyph, label 28u, one-line detail 24u.
**Rules:** confirmation starts on **Cancel**; destructive row is not adjacent to the safe one (24u gap); `B` cancels; focus is restored to the exact prior target after cancel.
**No** indexes, no equal tile grid.

### 5.10 Notifications
**On screen:** sheet from the avatar, max 3 visible, each: dot (unread), title 28u, one line 24u, time 20u. `A` opens the related surface, `B` closes.
**Rule:** never over the focused object; never more than one notification surface.

### 5.11 Overlay (in-game window)
**On screen:** 560u opaque panel, 3 tabs, `Downloads · Now playing · Controls`; 40% scrim **inside the window only**.
**Rules:** type in `em` so the scale setting actually scales (the audit's #19); gamepad: dpad tabs, `A` choose, `B` close; mouse: hover/click/scroll; the game is never paused; no blur over the game.

### 5.12 Errors and offline (cross-cutting)

- One sentence at 28u, one action, `B` always backs out.
- The raw error goes behind `Details` (a sheet) at 24u — never 11px mono on the main screen.
- **Offline is a state, not an error:** bar chip + "42 installed games still play" on the stage; installs and streaming are disabled with a reason in place, not a modal.
- The desktop toast system never renders inside the console root.

---

## 6. Customization model

### 6.1 The two axes

**Rooms (4 presets).** A room changes ~12 decisions at once: polarity, material, radius family, type weight, wall treatment, density, motion character, sound mix, band layout.

| Room | For | What it changes |
| --- | --- | --- |
| **Night** (default) | dark room, evening | Cabinet charcoal, one lit object, blur wall, 14/20u, springs |
| **Projector** | long throw, low contrast, big screen | pure black, +1 type step effective, thicker lines, **no blur**, no tint, 3u focus |
| **Daylight** (v1.1) | bright living room | paper palette, ink hairlines, no blur, flat panels, 8/14u |
| **Arcade** (optional) | the fun one | Enamel: hard edges, enamel blue, poster boot, no springs |

**Overrides (8 knobs).** Always win over the room:
1. **Accent** — Theme (default) · Signal · **From artwork**.
2. **Wall** — Artwork / Solid / Gradient; blur 0–3; dim 0–100; tint on/off.
3. **Corners** — 4 steps (0/8/14/20).
4. **Density** — Comfortable / Compact / Dense (changes items per rail and gaps; **never** type below 24u).
5. **Band** — which 5 items, reorderable, `More` if fewer than 3.
6. **Motion** — Full / Calm / Off (Calm = springs off, 120 ms only).
7. **Sound** — Full / Reduced (chrome only) / Off, plus volume and 3 previews.
8. **Haptics** — Off / Low / Medium / High.

### 6.2 "From artwork" accent

The one algorithmic idea worth building, and the replacement for five pastel colours. Extract the dominant chromatic cluster from the selected cover (canvas, cached per game id), push it to `oklch`, clamp chroma to 0.16, and validate it against both the surface and the stage at 7:1. If it fails, fall back to signal and say so in the row ("Not enough colour in this cover" — in one line, no error). It makes the console feel like *your* library without shipping a colour picker.

### 6.3 What "100% customizable" means, and what it does not

It means: **every surface a player can point at is decided by a room or one of the eight knobs, and changes visibly before they commit.**

It does not mean: per-component colour pickers, per-screen backgrounds, custom fonts, user CSS, a theme file format, a marketplace, or an API. Those are desktop activities, and a console is operated with a d-pad. Say this out loud in the settings copy once ("Every screen follows your room") and then stop explaining.

### 6.4 Persistence

- Record `ludex.console.room` **v3**: `{ v: 3, room, overrides: { accent, wall, corners, density, band, motion, sound, haptics } }`.
- Migrate v2 (`console-appearance`) → v3: `studio` → Night, `soft` → Night + corners soft; coral/mint/sky/lilac/sun → Theme (with a one-time notice: "Your accent moved to Theme. Pick From artwork in Appearance.").
- **Per device, per server** (a phone must not inherit a TV's materials — the existing rule, keep it).
- **Preview writes nothing**; the section commits on the frame of the change; `Reset section` reverts only that section; the full reset is in About.
- **Copy this room to another device** is an explicit action in About (account-level, opt-in), never a silent sync.

---

## 7. Motion, sound and haptics

### 7.1 Frozen

The table in `motion.ts` is **not retuned**. Durations 120/180/240, the two easings, the spring surfaces, `REDUCED_MOTION`, the interruption hub and the cue-at-start rule all stay. The audits confirm this is the part that works.

Three entries are **added**:

| New kind | Duration | Transform | Cue | Reduced |
| --- | --- | --- | --- | --- |
| `press` | 90 ms down / spring 0.28·0.18 up | `scale 0.97` | `click` | none |
| `detent` | 160 ms | `x ±2u` back to 0 | `move` at −3 dB | none |
| `bootMark` | 240 ms in, 600 ms pulse, 1200 ms hold | `opacity`, `scale 0.9→1` | none | instant |

### 7.2 Transition inventory (all existing except where marked)

| Moment | Kind | Numbers |
| --- | --- | --- |
| Focus move | `focus` | 120 ms tween, `EASE_OUT`, **never a spring** |
| Shelf / page | `shelf` | 180 ms, 18u, 0.55 opacity |
| Layout (removed) | — | deleted with the layout picker |
| Screen push | `screenPush` | spring 0.36 / 0, 28u x |
| Screen pop | `screenPop` | 200 ms, 20u, opacity only on exit |
| Sheet open | `panel` | spring 0.34 / 0.12, 14u y |
| Launch | `launch` | FLIP 90–600 ms, spring 0.55 / 0.08, dim 0.35, filament 1.1 s |
| Power confirm | `powerConfirm` | 220 ms, 18u, 0.992 |
| Settings section | `settings` | 160 ms, 6u |
| Queue row | `queueRow` | spring 0.26 / 0 |
| Boot | `boot` + `bootMark` | 200 ms shell, 1.5 s total |

**Rules that stay:** only transform/opacity; no `transition: all`; a held key lands on the final frame; a new input interrupts the previous tween; progress bars never spring; the outgoing surface never travels.

### 7.3 Launch handoff

Cover FLIP from the rail (90–600 ms) → full bleed → 0.35 dim → filament → title 56u → status 28u. Minimum hold 900 ms; 8 s ceiling with the calm truth; `B` only after the give-up. One messaging channel: the plate. **No toast on failure.**

### 7.4 Sound

The cue table stays exactly as specified (11 cues, dry archive, ≤3 partials, ≤−10 dBFS, `move`/`launch` hash-frozen). New rules only:

- **One sound per press.** The surface owns its cue through the motion table; the router speaks only for movement (already true — keep it).
- **Edge press with no movement is silent** (already true) — a rail detent gets `move` at −3 dB plus haptics, not a new cue.
- **Sound fires at the start** of the motion it describes. Never at the end, never on exit.
- **Console silently mutes while a game runs** (already true) — surface it once in Settings → Controllers: "Quiet while a game is running".
- Settings exposes 3 previews (move, confirm, launch), volume in 5% steps, and "Reduce cues" (chrome off, essentials on).

### 7.5 Haptics

Never on focus (fatiguing at 130 ms repeat). Only on:

| Event | Pattern |
| --- | --- |
| Rail edge / detent | 20 ms, 30% |
| Confirm | 2× 18 ms, 60% |
| Launch | 40 ms ramp 20→70% |
| Error | 1× 35 ms, 80% |
| Install complete | 2× 25 ms, 50% |

Strength Off / Low / Med / High maps to the existing dual-rumble scales.

---

## 8. Anti-slop rules — the do-not-ship list

Each is enforceable. The repo already tests its design system; these become tests, not a style guide PDF.

1. **No equal-tile grid anywhere.** No 6-up, 4-up or 3-up row of same-sized cards carrying different functions. System destinations are a band; content is a stage + rail.
2. **No decorative ordinals.** `01`–`06` never appear. Position is spatial. (Grep test: `padStart(2, "0")` in console components.)
3. **No uppercase mono labels.** Mono is for numerals, units, percentages, clocks, shortcuts. Words are Instrument Sans, sentence case.
4. **No type below 20u; no word set below 24u.** Static scan for `text-[` in `components/console/**` fails the build.
5. **No fixed px type in Console.** Everything through `--bp-text-*`.
6. **Max 2 blurred layers; zero while a game runs.** Never blur a moving or scrolling element.
7. **Radius ≤ 24u; covers ≤ 8u.** No 30–34u cards.
8. **One accent.** No coral/mint/lilac/sky/sun menu. Accent is Theme, Signal, or From artwork.
9. **No second palette.** Console inherits the app tokens, always.
10. **No preset names, roadmap notes or design rationale in user copy.** No "SOFT", no "A full theme editor is not built yet".
11. **No centred logo + message empty state.** Empty states are one sentence + one action, anchored where the content would be.
12. **No dead-end focus.** Every focusable has a `B` path; every level has a back; no route a controller can reach but not operate.
13. **No modal a controller cannot complete,** and no modal beyond install/power confirm.
14. **No focus trap, no double ring.** One bracket per object.
15. **Max 3 hints; one strip.** The page footer hides under a sheet.
16. **No toasts in Console.** One notice surface, bottom-leading.
17. **No icons without words at 10 feet,** and no two destinations sharing a glyph. 16 drawn glyphs; Lucide is not used inside console chrome.
18. **No gradients, glows, neon, glass tiles, noise-as-decoration.** The grain stays at ≤ 0.045 and never over text.
19. **No `transition: all`, no animating layout properties, no spring on progress.**
20. **No chart in a section header.** Data lives where the data is.
21. **No hidden state.** If a feature is unavailable, it is visible with a reason in place — never removed silently, never a modal.

---

## 9. Implementation plan

### 9.1 Files and components

**New**
- `theme/console-room.ts` — rooms + overrides, v3 record, v2 migration, sanitize, preview-writes-nothing.
- `theme/artwork-accent.ts` — dominant colour extraction, oklch clamp, 7:1 validation, per-game cache.
- `styles/console.css` — the whole `bp-*` block moved out of `index.css` (≈400 lines today), plus bracket/glyph/band rules.
- `components/console/glyphs.tsx` — 16 drawn glyphs + hint strip.
- `components/console/{stage,rail,band,bracket,notice,sheet,room-preview}.tsx`
- `screens/console/{home,library,game,downloads,settings,power,search,boot}.tsx`
- `lib/console-focus.ts` — the zone graph, extracted from `big-picture.tsx`.

**Changed**
- `theme/console-appearance.ts` → re-exported shim, deleted at P5.
- `components/brand.tsx` — wordmark cut, monogram, boot lockup, clear-space rules.
- `components/big-picture/home-row.tsx` → `components/console/band.tsx` (indexes and corner icons deleted).
- `components/big-picture/{system-bar,settings-app,power-app,overlays,parts}.tsx` → rewritten under `components/console/`.
- `apps/web/src/screens/big-picture.tsx` (2,744 lines) → split into `screens/console/*` + `use-console-shell` + `use-console-input` + `use-console-launch`. Delete the 6-layout registry; keep `shelf` and `grid` as `Stage` and `Wall`, plus `list` for accessibility.

**Deleted**
`theme/console-appearance.ts` presets/accents · `layout-switcher.tsx`, `layout-preview.tsx`, `layout-{dock,spotlight,hero,compact}.tsx` · `onboarding-wizard` from the console path · `InstallDialog` usage in console · the `Soft` theme in `themes.json` (desktop may keep it as a light theme; it is no longer a console preset) · the five console accent hexes.

### 9.2 Phases

| Phase | Scope | Est. | Exit criterion |
| --- | --- | --- | --- |
| **P0** | Tokens, `console.css`, glyphs, bracket, room record behind `?console=2` | 2–3 d | A static home renders with real tokens at 1080p/4K; contrast tests green |
| **P1** | Home: stage, rail, band, boot | 5–6 d | Boot → home with a pad only; no dead ends; screenshots at 3 sizes |
| **P2** | Library, game surface, install sheet, launch plate | 5–7 d | Install and launch completable with a pad on **Windows/Tauri** |
| **P3** | Downloads, settings, power, notifications | 4–5 d | Every system surface has a `B` path; no modal beyond 2 |
| **P4** | Search, overlay window, offline/error states | 3–4 d | Overlay scale actually scales; offline play verified |
| **P5** | Rooms, overrides, preview, artwork accent, migration | 3–4 d | v2 record migrates; preview = applied; reset per section |
| **P6** | A11y, perf, overscan, screenshots, delete old shell | 3–5 d | axe clean; 4K blur budget verified; old `big-picture.tsx` gone |

≈ 5 weeks solo; ≈ 2.5 with two lanes. **Lane conflicts:** `styles/index.css`/`console.css`, `routes/mod.rs` if `/console` is added server-side, `apps/web/src/lib/types.ts` (only if the room record goes to the account), `CHANGELOG.md`. Serialize those.

### 9.3 Tests and screenshots

**Unit (vitest, colocated)**
- `console-tokens.test.ts` — every pair used, including `ink-3` on `surface-2`, `danger`/`ok` on `surface-2`, and the **composed** wallpaper case (art → wash → text) at 7:1 for all built-in themes × 3 rooms.
- `console-type.test.ts` — no `text-[` in `components/console/**`; no token below 20u; the scale is monotonic; `u` clamp holds the physical size at 720/1080/2160.
- `console-material.test.ts` — ≤ 2 blurred layers; no `backdrop-filter` on `[data-bp-scroll]`; overlay window opaque over a game.
- `console-focus.test.ts` — the zone graph: every level has a `B`; every zone is reachable and exits; band wraps; no focusable without an action.
- `console-room.test.ts` — v2→v3 migration, sanitize, preview-writes-nothing, reset-scope.
- `motion.test.ts` — unchanged, plus the three new kinds and their reduced mappings.
- `console-copy.test.ts` — no ordinal indexes, no preset names, no roadmap strings in console messages; hint strip ≤ 3.

**E2E (Playwright)**
- `e2e/console-2.spec.ts`: boot → home → library → game → install sheet → launch → back; pad emulation via keyboard map; every screen asserts one visible hint strip and one bracket.
- Screenshot matrix: 1280×720, 1920×1080, 3840×2160 × {Night, Projector} × {normal, reduced-motion, reduced-transparency, contrast-more} + 3% and 5% overscan crops — about 24 frames, stored in `docs/assets/console/`.
- **Worst-case fixture:** 1 game / 400 games / 40-char titles / missing art / one missing archive / offline / install failed / 8 queued jobs. This is where "10 feet" breaks, and it is the only fixture that matters for the anti-slop rules.
- axe pass on home, library, game, settings, power.

**Commands:** `pnpm --filter @ludex/web test`, `pnpm --filter @ludex/web build`, `scripts/e2e.sh`, plus the existing workspace fmt/clippy/test gate.

### 9.4 What needs Windows / TV hardware (cannot be validated in the browser)

1. **Install sheet** — real volumes, free space, staging volume, blocked states (Tauri only).
2. **Launch handoff** — process spawn, focus loss, the 8 s ceiling on a real slow game.
3. **Floating overlay** — a second window over a running game, input routing, opacity.
4. **Power actions** — sleep, restart, quit, focus restore on Windows.
5. **Controller families** — Xbox, DualSense, Switch Pro: glyph swap, A/B swap, haptics strength.
6. **Overscan** — a real TV in "just scan"/16:9, plus a projector.
7. **4K blur cost** — integrated GPU, 60 fps with the wall + sheet open.
8. **Legibility at 3 m** — the only test for the 28u floor; a person, not a screenshot.

Everything else (tokens, type, focus graph, copy, motion, screenshots) is verifiable in the browser and should be, before hardware.

### 9.5 Risks

| Risk | Mitigation |
| --- | --- |
| "Cabinet" reads too severe on a real TV | Validate the 3 m legibility pass early (P1), with the user in the room; keep the warmth in charcoal + paper ink |
| The 2-layout cut feels like a feature loss | Ship `Stage` + `Wall` + `List`, and make them *unmistakably* different; the deleted six were never distinguishable at a glance |
| `big-picture.tsx` split breaks the pad | Extract the zone graph first (pure, testable), then the render; keep e2e pad tests green per phase |
| Room presets become a theme engine | The 8-knob ceiling is in code: overrides is a fixed interface, not a record of arbitrary keys |
| Offline promise overpromises | Only claim what the launcher can do: installed games launch; art and metadata may be cached or generic |

---

## Appendix A — token deltas vs `midnight`

Only what changes for Night. Everything else is inherited.

```
bg          #0a0a09 → #0d0c0a   (+1 warm, less green)
bg-deep     #050505 → #070706
surface     #121210 → #16140f
surface-2   #191916 → #1d1a13
elevated    #1f1f1b → #242017
line        #272620 → #2b2820
line-2      #3b3930 → #3d392c
ink         #ece8de → #f3eee2
ink-2       #a8a396 → #bcb4a3
ink-3       #918b7e → #948c7b  (TV map: #aca494)
signal      #ffc61a → #ffc61a  (unchanged — one accent, no new brand colour)
```

## Appendix B — copy rules

- Sentence case, no exclamation, no "Oops", no "Let's".
- Verbs are the player's: "Play", "Install", "Resume", "Remove".
- Never name an internal: no "preset", "layout", "theme engine", "cache".
- A failure states the cause and the next action in one sentence; the raw error is behind `Details`.
- Numbers carry units; sizes use one decimal; times are "14 min", not "0:14:03".
- Translate, don't concatenate: `t("Quit {app}", { app: "Ludex" })`, never `t("Quit") + " Ludex"`.

## Appendix C — open questions for the user

1. Which room is the TV in — dark, bright, or projector? (Decides the default room.)
2. Is a 55" the reference, or is there a 4K 65"+ in play? (Decides whether 4K is the design target or a scale check.)
3. Does the console ever run on a handheld / Steam Deck? (Decides whether the 720p layout is a real target or a fallback.)
4. Are there other people using the TV (profiles), or is this a single-player console? (Decides whether the profile switcher is a band item.)
5. Does "cute" mean *warm and soft* or *toy-like and playful*? (Decides the press-squash amplitude and whether the dot gets a face-like blink. I would keep the dot abstract.)
