<!-- Suggested title: [v0.5] Big Picture layouts: a Switch-style dock and a PlayStation-style spotlight -->

## Goal

Two new ways to live in the library at ten feet, alongside Shelf / Hero / Grid / Compact:

- **Dock** — a Switch-like home: one large game row, the selected tile lifted and readable, a system icon dock underneath, shelves changed with the shoulders.
- **Spotlight** — a PlayStation-like home: the selected game takes over the background, a strong selected tile sits in a bottom row, system icons sit over the art at the top, and a one-line activity strip with quick actions lives under the title.

They must feel like two consoles, not two skins: different hierarchy, different focus emphasis, different rhythm. They reuse the existing palette, typefaces, cards, hint strip, input router and layout store. No new colours, no new fonts, no new dependency.

## Current state

**Dependency:** `28-bp-console.md` P1 owns `system-zones.ts`, the home row and the `zone` variant of `LayoutStep`. This issue adds the two layouts against that contract; if the shell lands first, Phase 1 shrinks to the two new `stepForLayout` cases.

| Piece | Today | Evidence |
| --- | --- | --- |
| Registry | four ids, default `shelf` | `components/big-picture/layout.ts:11-17` |
| Geometry | `stepForLayout`, page jumps, clamp | `layout.ts:113-213` |
| Selection + scroll + ring | one hook for all layouts | `navigation.ts:80-228` |
| Shared pieces | cards, tabs, meta, hint strip | `parts.tsx:34-422` |
| Views | four components, one props contract | `model.ts:12-32`, `layout-shelf.tsx`, `layout-hero.tsx`, `layout-grid.tsx`, `layout-compact.tsx` |
| Picker | header trigger + `L` + Settings cards | `layout-switcher.tsx:24-138`, `layout-panel.tsx:16-71` |
| Persistence | per device, per server, shared store | `use-layout.ts:38-90`, `layout.ts:19-65` |
| Preview cards | live-token miniatures | `layout-preview.tsx:94-112` |
| Loading | per-layout skeleton | `layout-skeleton.tsx:20-89` |

Two traps the compiler will not catch when a fifth and sixth layout are added:

1. `LayoutSkeleton` ends in a shelf fallback (`layout-skeleton.tsx:76-89`): a new id silently renders the wrong skeleton.
2. The screen’s body is an if/else ending in `ShelfLayout` (`big-picture.tsx:815-823`): a new id silently renders Shelf.

Both become exhaustive `switch` / `Record<BigPictureLayout, …>` in Phase 1, so a future layout cannot ship half-wired.

One existing copy bug to fix here: Settings says the layout switches with “L on a keyboard or X on a controller” (`layout-panel.tsx:68`). X opens search (`input.ts:107-108`); the pad path is Menu/Guide → Layout (`controller.ts:288-291`). Correct the sentence while touching the panel.

## Design

### Names

| id | label | reference | why the name |
| --- | --- | --- | --- |
| `dock` | Dock | Switch home | the system row docks under the games |
| `spotlight` | Spotlight | PlayStation home | the selected game is lit, the rest of the shelf is not |

No platform trademark appears in an id, a label, a translation key or a class name. Reference names stay in this issue.

Registry order (which is also the picker order, `layout.ts:13-14`): `shelf, dock, spotlight, hero, grid, compact`. Default stays `shelf` (`layout.ts:17`), so no existing device changes appearance on upgrade. `sanitizeLayout` already handles unknown values (`layout.ts:31-33`); no migration.

### Zone order

The two layouts are not only visual: they change where the home row sits, so the zone model from `28-bp-console.md` must be per-layout.

| Layout | Zones, top → bottom | Up | Down |
| --- | --- | --- | --- |
| shelf, hero | home → library | to home | nothing |
| grid | home → library | row above, or home at row 0 | row below, clamp |
| compact | home → library | row above, or home at row 0 | row below, clamp |
| **dock** | library → home (bottom) | nothing | to home |
| **spotlight** | home (over art) → library → actions | to home | to actions |

Everything else is unchanged: left/right moves inside the row, LB/RB changes shelf, LT/RT jumps a page (`layout.ts:168-193`). In `shelf`/`hero`, Up no longer cycles shelves; LB/RB already is the advertised control (`parts.tsx:139-145`) and the shelf tabs are clickable for pointer users.

### Dock

```text
┌ system bar ──────────────────────────────────────────────────────────────┐
├──────────────────────────────────────────────────────────────────────────┤
│  RECENTLY PLAYED · 12                                     03 / 24         │
│  Hades II                                                                │
│  2024 · Early access · 41 h played · Installed                           │
│                                                                          │
│      ▮▮▮   ███████   ▮▮▮                                                 │
│      dim   focused   dim                                                 │
├──────────────────────────────────────────────────────────────────────────┤
│   [Games] [Search] [Downloads] [Updates] [Controller] [Settings] [Power]  │
├ hint strip ──────────────────────────────────────────────────────────────┤
```

- The game row is bottom-anchored above the dock, not vertically centred: the tiles sit next to the system row they belong to, and the title block grows upward. Scale, not reading order, is what makes the tiles land first.
- Title block, left-aligned at the gutter: eyebrow (`SelectedEyebrow`, `parts.tsx:34-46`), `<h1>` at `clamp(1.6rem, 3.4vw, 4.2rem)`, `MetaLine` (`parts.tsx:48-72`), one primary action. No plate, no description paragraph: Dock is for choosing, not reading.
- Tiles: 3:4 `GameCover`, height `calc(var(--bp-u) * 336)`, radius 2 px, caption only under the focused tile (neighbours get the title on hover/focus only). Focused scale `1.12`, `translateY(-4px)`, neighbours `opacity .55`; the ring is the existing `FocusRing` (`parts.tsx:156-163`).
- Position readout `03 / 24` in `.num`, right-aligned on the eyebrow line (the shelf/hero readouts already do this, `layout-shelf.tsx:91-93`).
- No page dots, no carousel arrows, no rounded tiles: Ludex is hairlines and index marks, not plastic.

### Spotlight

```text
┌ full-bleed background of the selected game, crossfaded ──────────────────┐
│                                        [search] [library] [downloads] …  │
│                                                                          │
│                                                                          │
│   LAST PLAYED 2 DAYS AGO · 41 H · 6 / 24 ACHIEVEMENTS                    │
│   Hades II                                                               │
│   2024 · Early access · Installed                                        │
│   [ Play ]  [ Game page ]  [ Options ]                                   │
│                                                                          │
│        ▮▮▮     ███████     ▮▮▮                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

- Background takeover: the full-bleed art is the stage; `hero-shade` (`styles/index.css:298-311`) keeps the bottom third legible. The previous game fades out under the new one over 240 ms, opacity only.
- The title block sits above the row, left-aligned, with a **single activity line** in mono (last played, hours, achievements). This is the PlayStation “what did I do here” line; it must come from real data, or the line is omitted, never faked.
- Quick actions: primary (Play/Install/Download/Installing, the same node the screen already builds, `big-picture.tsx:747-774`), Game page, and a third action that opens the quick menu scoped to the game. Actions are a focusable row (zone `actions`), not decoration.
- Top function icons are the **same** `HomeRow` component as the shell, `placement="overlay"`: 22 px icons, 10 px labels, no plate, right-aligned over the art. One list, one implementation, three placements.
- Tiles are slightly larger than Dock (`calc(var(--bp-u) * 380)`), focused scale `1.08`, no vertical travel: the background does the moving.

### Focus scaling and rhythm

| | Dock | Spotlight |
| --- | --- | --- |
| Focused tile | `scale(1.12) translateY(-4px)` | `scale(1.08)` |
| Neighbours | `opacity .55` | `opacity .50` |
| Ring | `FocusRing` + global `[data-gamepad-focus]` | same |
| Row follow | `scrollIntoView({ inline: "center", block: "nearest" })` (already `navigation.ts:178-184`) | same |
| Edge padding | `scroll-padding-inline: var(--bp-gutter-x)` so the ring never touches the edge | same |
| Focus duration | 150 ms, `transform, opacity` | 150 ms, `transform, opacity` |

Both rows clamp at the ends; they do not wrap. A game row encodes position, unlike the system ring (`28-bp-console.md`, home row).

### Transitions

| Moment | Cue | Duration / easing | Reduced motion |
| --- | --- | --- | --- |
| Enter Big Picture | existing `reveal()` cascade (`entrance.ts:22-75`), once per open | 240 ms `--ease-out` | fade only |
| Focus move | scale + opacity | 150 ms `--ease` | instant, ring only |
| Shelf change | row fades and shifts 12 px | 180 ms | instant |
| Spotlight background | crossfade two `ArtPlate` layers | 240 ms, opacity only | 0 ms |
| Layout switch | no entrance replay (`settled`, `big-picture.tsx:462-469`) | — | — |

Never animate `filter`, `box-shadow`, `height` or `backdrop-filter`; only `transform` and `opacity`. No `transition: all`.

### Responsive

| Viewport | Tile height | Fully visible | Title | Notes |
| --- | --- | --- | --- | --- |
| 1280×800 (Steam Deck) | 269 | 5 | 26 px | dock 77 px, icons 24 |
| 1920×1080 | 336 | 6 | 42 px | reference |
| 2560×1440 | 447 | 6 | 58 px | |
| 3840×2160 | 672 | 7 | 84 px | `--bp-u` = 2 |
| 1920×1200 (16:10) | 336 | 6 | 42 px | extra height goes to the title block |
| 3440×1440 (21:9) | 447 | 8 | 58 px | the row uses the full width, no island |

The row scrolls internally; the page never scrolls horizontally. `overflow-x-hidden` on the layout root. Captions `truncate` with `min-w-0`.

### How the engine maps

`stepForLayout` (`layout.ts:146-163`) grows one variant:

```ts
export type LayoutStep =
  | { type: "move"; index: number }
  | { type: "shelf"; delta: -1 | 1 }
  | { type: "zone"; target: "home" | "actions" }   // new
  | { type: "none" };
```

Per layout:

- `dock`: horizontal → `move`; `down` → `zone home`; `up` → `none`.
- `spotlight`: horizontal → `move`; `up` → `zone home`; `down` → `zone actions`.
- `shelf`/`hero`: horizontal → `move`; `up` → `zone home`; `down` → `none`.
- `grid`: horizontal → `move`; vertical → `gridStepIndex`; if `delta < 0` and the index did not change, `zone home`.
- `compact`: horizontal → `shelf`; vertical → `stepIndex`; if `delta < 0` and the index did not change, `zone home`.

`pageStepForLayout` (`layout.ts:178-193`) treats `dock` and `spotlight` like the rails (`RAIL_PAGE_STEP`).

`navigation.move` returns the zone target instead of `void` (`navigation.ts:138-145`), and `runCommand` calls `nav.move(direction)` instead of `nav.actions[...]` so it can read it (`big-picture.tsx:597-599`); `actions` keeps its shape for every other caller. Selection is untouched by a zone step, so returning from the home row lands on the same game.

Also required for a complete sixth layout:

| File | Change |
| --- | --- |
| `layout.ts:11-14` | union + registry order |
| `layout.ts:146-193` | new cases + zone variant |
| `navigation.ts:138-159` | return the zone target; no other change |
| `input.ts:15` | no new overlay; `layout` command unchanged |
| `layout-preview.tsx:94-99` | `BODIES` is a `Record<BigPictureLayout, …>`: compiler forces two previews |
| `labels.ts:9-32` | two labels + two descriptions |
| `layout-switcher.tsx:17-22` | typed icon `Record`; two icons (`PanelBottom`, `Sparkles` or `GalleryVerticalEnd`) |
| `layout-skeleton.tsx:20-89` | exhaustive switch, `assertNever` |
| `big-picture.tsx:815-823` | `LAYOUT_VIEWS: Record<BigPictureLayout, ComponentType<…>>`, no fallback |
| `parts.tsx` | no new card: `RailCard` already fits both |
| `controller.ts:183-185` | keyboard arrows: rails/dock/spotlight `["←","→"]`; grid/compact all four |

No new hint id. The home row is visible on screen, which is the affordance; the strip stays at six hints (drop `page` from the library context, `controller.ts:200-210`, which currently advertises eight).

### Layout picker and persistence

- Six cards, same registry, in both surfaces: the in-screen picker (`layout-switcher.tsx:107`) and Settings → Appearance (`layout-panel.tsx:33`). Grid becomes `grid-cols-2 sm:grid-cols-3 xl:grid-cols-6`; the picker keeps `w-[min(1000px,94vw)]`, so a card is still ~150 px at 1080p.
- Previews: `DockBody` (one large plate, two dim neighbours, a four-icon dock strip) and `SpotlightBody` (full-bleed block, three top chips, three tiles with the middle one lifted). Decorative only, `aria-hidden` (already `layout-preview.tsx:104`).
- The picker highlight is an index into the registry (`big-picture.tsx:451-473`); adding entries needs no other change.
- Selecting a layout keeps shelf and game: `nav.index` lives in the screen, not in the layout, and the refocus effect only runs when the pad owns focus (`big-picture.tsx:731-743`). Add a regression test.
- Copy: “Saved on this device” stays true (`layout-panel.tsx:67-69`); fix the controller path in that sentence.

## Scope

### Phase 1 — engine and exhaustiveness (no new layout, 1 PR)

- `layout.ts`: union, registry, `zone` variant, `dock`/`spotlight` cases, page step.
- `navigation.ts`: zone return.
- `layout-skeleton.tsx`, `big-picture.tsx` body: exhaustive maps.
- `controller.ts`: keyboard arrow rules.
- Tests: every direction for all six layouts; grid/compact top-edge zone; page step; selection untouched by a zone step.

### Phase 2 — Dock (1 PR)

- `layout-dock.tsx`, `parts.tsx` reuse, `HomeRow placement="dock"`, the shelf label above the title, responsive sizes.
- Tests: render smoke via the existing layout tests, plus a Playwright step that switches with `L`, confirms the title, and walks the row to both ends.

### Phase 3 — Spotlight (1 PR)

- `layout-spotlight.tsx`, crossfade, activity line, actions row (zone `actions`), home row over art.
- Extend `ArtPlate` with `priority?: boolean` (`fetchPriority="high"`, `decoding="async"`); preload the previous/next art on idle so a fast d-pad roll does not pop.
- Tests: actions zone reachable and returning; background layer count never exceeds two.

### Phase 4 — picker, previews, docs (1 PR)

- Two previews, labels, descriptions, icons, 6-card grid, corrected Settings sentence.
- CHANGELOG entry, `docs/CONTRACT.md` untouched (no API change).
- e2e: all six layouts selectable from Settings and from the in-screen switcher.

## Acceptance

- [ ] `dock` and `spotlight` appear in Settings → Appearance and in the in-screen switcher, with previews and descriptions, in the registry order.
- [ ] Both work controller-only: every action reachable (play/install, game page, search, downloads, profiles, layout, settings, exit) with no pointer.
- [ ] Switching layouts preserves shelf and selected game; the ring returns to the same card (pad only).
- [ ] `dock`: Up does nothing, Down enters the dock, Left/Right clamp at the ends, LB/RB changes shelf, LT/RT pages.
- [ ] `spotlight`: Up reaches the system icons, Down reaches the actions, Left/Right moves the row, the background crossfades once per selection.
- [ ] No layout renders the wrong view or the wrong skeleton (TypeScript exhaustive; `assertNever`).
- [ ] 1280×800, 1920×1080, 2560×1440, 3840×2160, 1920×1200 and 21:9 show no clipped caption, no horizontal page scroll, no ring under the hint strip.
- [ ] `prefers-reduced-motion`: no scale, no crossfade, no row slide; focus is ring + opacity only.
- [ ] No new colour token, no new font, no new dependency; shelf/hero/grid/compact unchanged apart from the shared gutter.
- [ ] `pnpm --filter @ludex/web check`, `test`, `build`; i18n gate; e2e green.

## Out of scope

- Nintendo or Sony assets, icons, sounds, layouts copied pixel-for-pixel, or brand names in the UI.
- A seventh layout, a layout per account, or layout sync across devices (the choice stays per device, `layout.ts:19-24`).
- Animated video backgrounds, parallax, reflection effects, or art-driven colour extraction.
- Changing `shelf`, `hero`, `grid` or `compact` beyond the shared gutter.
- A new card component: both layouts compose `RailCard` / `GameCover` / `ArtPlate`.
- Sound on focus (belongs to `30-bp-sound.md`).

## Risks

| Risk | Mitigation |
| --- | --- |
| Up/Down losing shelf cycling in `shelf`/`hero` surprises keyboard users | LB/RB and `[`/`]` remain; the shelf tabs are visible; changelog note |
| Two full-screen `ArtPlate` layers on a weak GPU | opacity-only crossfade, one outgoing layer max, `will-change` avoided, art preloaded ±1 |
| 6 cards overflow the picker at 1024 px | 2/3/6 columns; previews stay 74 px (`layout-preview.tsx:104`) |
| `home`/`actions` zones conflict with the quick menu overlay | overlay input routing already owns every direction while open (`input.ts:76-94`); zones are library-only |
