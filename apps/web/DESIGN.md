# Ludex web — design notes

## Direction

**Late-night archive.** Ludex is a private shelf, not a storefront. The interface is built to
feel like a well-kept physical collection at night: warm-black ink, bone paper, thin rules, and
one safety-yellow signal colour used for the things that matter (the primary action, the active
shelf, the current state). Nothing is glassy, nothing floats in a gradient, nothing is rounded
for the sake of it.

The organising idea is the **plate**: every game cover is a numbered plate in one printed series
(`LUDEX / No. 014`). Covers are objects with a hairline edge and a sheen when you pick them up.
The same system draws artwork for real games that have no cached cover yet, so the shelf never
shows a broken image.

The default is **Midnight** — warm black, bone ink, hairline rules. **Paper** is the designed
light counterpart: warm paper, dark ink, and covers that keep their colour so the library reads
like a printed catalogue. **Nord**, **Gruvbox** and **Amber** are complete alternative palettes,
not filters over Midnight; each one is contrast-checked as a full set. Users can also add a
custom accent, tint the surfaces, or import a validated theme file (#140).

The game detail hero is a deliberate exception: a **dark stage** in every theme, because artwork
needs darkness around it. `ThemeScope` re-declares the base palette for that section, with the
user's accent, instead of a hand-written colour block.

## Tokens

Defined once in `src/theme/themes.json`; `scripts/theme.mjs` writes the static fallback CSS and
the runtime applies the resolved map as custom properties. Tailwind v4 maps them through
`@theme inline` (`bg-bg`, `text-ink-2`, `border-line`, …), so no component paints a literal
colour. `src/styles/no-hardcoded-colors.test.ts` enforces that rule.

### Colour

| Token | Ink | Paper | Use |
|---|---|---|---|
| `--bg` | `#0a0a09` | `#e7e2d4` | page |
| `--bg-deep` | `#050505` | `#ddd7c5` | wells, art plates |
| `--surface` | `#121210` | `#f1ede0` | inputs, plates |
| `--surface-2` | `#191916` | `#f6f2e6` | raised states |
| `--line` | `#272620` | `#cbc3ad` | hairlines |
| `--line-2` | `#3b3930` | `#ada48b` | control borders |
| `--ink` | `#ece8de` | `#16150f` | primary text |
| `--ink-2` | `#a8a396` | `#55513f` | secondary |
| `--ink-3` | `#918b7e` | `#635d4c` | meta |
| `--signal` | `#ffc61a` | `#f0c000` | primary action, active marks |
| `--on-signal` | `#14130c` | `#16150f` | text on signal |
| `--signal-text` | `#ffc61a` | `#705200` | signal as text (AA on paper) |
| `--danger` | `#f0564a` | `#a82c1c` | destructive, errors |
| `--ok` | `#79c99f` | `#2c7050` | healthy, installed |

Signal is a **fill**, never large type. On paper it is darkened for text use so contrast stays AA.
The table shows the two designed defaults; Nord, Gruvbox and Amber use the same token names, and a
theme file can replace every value as long as the contrast checks pass.

Grain is a single fixed SVG noise overlay at 4.5% (3.5% on paper), `mix-blend-mode: overlay`.

### Type

| Role | Family | Notes |
|---|---|---|
| Display | Bricolage Grotesque Variable (wdth 75–100, wght 200–800) | headings, cover titles, numbers that are not data |
| UI | Instrument Sans Variable | body, labels, buttons |
| Data | JetBrains Mono Variable | sizes, dates, paths, counts, keyboard hints, table heads |

All three are bundled with `@fontsource-variable`; no network font requests.

Scale (desktop): display `clamp(2rem, 3.4vw, 3.25rem)` for page titles, `clamp(2.4rem, 4.6vw, 4.25rem)`
for a game title, section titles 17px, body 15px/1.5, secondary 13px, meta 11.5–12.5px, mono data
10–11.5px. Line length is capped at 62ch for prose. Page titles use tracking `-0.03em`; body never
goes below 12px.

Cover typography scales with the plate: title `11cqw`, series label `4.4cqw`, footer `3.9cqw`, all
via container queries, so a 24px thumbnail and a 600px plate share one layout.

### Space and shape

4px base. Page gutters 40px desktop / 24px tablet / 18px phone; rail 236px; top bar 60px.
Radii are 2px (plates, inputs, buttons) and 3px for dialogs. There is no soft card shadow anywhere;
depth comes from hairlines, plate edges and one hover shadow under covers.

### Motion

120ms for colour/state, 180ms for overlays, 240ms for page and cover entrances. Only `transform`
and `opacity` animate; the sheen is a single 620ms pass. One orchestrated entrance per screen
(staggered grid, capped at 8 items). Every transform is disabled under
`prefers-reduced-motion: reduce`, including the cover sheen.

## Layout

```
┌───────────┬──────────────────────────────────────────────┐
│ LUDEX     │ search            random  sort  account      │ 60px
├───────────┼──────────────────────────────────────────────┤
│ Browse    │  Library                                     │
│  Library  │  18 games   860 GB   Sorted by Title         │
│  Recent   │  ─────────────────────────────────────────── │
│  Saved    │  All  Action  Horror …   [state] [early]     │
│  Index    │  ┌────┐ ┌────┐ ┌────┐ ┌────┐ ┌────┐          │
│  News     │  │3:4 │ │    │ │    │ │    │ │    │          │
│ Genres    │  └────┘ └────┘ └────┘ └────┘ └────┘          │
│ Admin     │  title  title  title  title  title           │
└───────────┴──────────────────────────────────────────────┘
```

- Grid: 2 / 3 / 4 / 5 / 6 columns at 390 / 640 / 1024 / 1280 / 1536. Covers are always 3:4.
- The rail becomes a five-item bottom bar under 1024; “More” opens a sheet with genres, admin,
  theme and sign-out.
- The filter bar sticks to the top of the scroll area. It stacks on phones: genres first, then
  state and the early-access toggle.
- The game detail is a dark stage: backdrop, cover, title, actions; then paper for media, files,
  details and progress. The cover overlaps the stage edge.
- Admin pages are worksheets: page title, hairline, sections, tables. No card grids.

## Components

Built in-house, no UI kit: button (primary/default/ghost/quiet/danger), icon button, input,
textarea, field, checkbox, switch, segmented control, badge, kbd, progress (determinate and
indeterminate), skeleton, dialog, confirm dialog, popover (portalled, viewport-clamped, Escape,
outside click, focus restore), dropdown menu, select/listbox, tooltip, toast viewport, empty state,
error state, page header, section header.

States are explicit: hover changes border or text, never lifts text; active is a 1px shift of colour;
focus is a 2px signal outline with 2px offset; disabled is 40% opacity and no pointer events;
loading keeps the button width and swaps the icon for a spinner.

### Cover plates

`GameCover` renders a real image when the API provides one (through `assetUrl()`), otherwise a
generated plate: one of 12 palettes × 12 motifs (grid, halftone, chevron, arcs, horizon, cross,
split, type, topo, waves, blocks, orbit), seeded by the slug. The series index, title, year and
genre are typeset over it, with a scrim top and bottom. Screenshots use the same system with a
cycled motif per still, so a demo library has a coherent “press kit”.

### Data and state

- `src/lib/api.ts` is the only network surface. `VITE_USE_MOCKS` selects the mock adapter
  (default on in dev, off in production). The mock has 18 games, users, news, ingest jobs that
  advance, a scanner that finishes, and two localStorage switches for review states:
  `ludex.demo.needsSetup` and `ludex.demo.empty`.
- `assetUrl()` prefixes every media and download URL with the configured API base. In Tauri the
  page origin is `tauri://localhost`, so relative `/api/media/...` paths must never be used raw.
- React Query owns server state; auth, theme and toasts are small context stores. Bookmarking is
  optimistic.
- `src/lib/native.ts` is the Tauri bridge: every command in the contract, typed, dynamic-imported,
  no-op or friendly rejection in a browser. Installed games are merged into the library by game id.



## Installs, Home and Big Picture

**Installs are background jobs.** `DownloadsProvider` owns one list for the whole app, fed by
`list_install_jobs`, the `install://*` events and a 1s poll while anything is active. The same job
appears in three places, all reading that list: the rail badge, the compact strip at the bottom of
the shell (title, aggregate percent, speed, ETA, click for `/downloads`), and the Downloads screen
(active rows with bytes/speed/ETA/cancel/open folder, history with resume/open folder/clear
finished). The install dialog starts a job and can be closed; the job keeps running. Starting an
install for a game that already has an active job returns the existing one.

**Home** is the default route: a dark plate hero for the most recently played installed game (or
the last played game), with Play/Install/Download, followed by a horizontal Recently played shelf
and an All games grid. It uses the same plates and type as the library; it is not a separate visual
system.

**Big Picture** (`/bigpicture`) is a full-screen console layout: a large cover plate, the selected
game's title and actions on ink, one shelf with captions, and a strip of key hints. Arrow keys,
Enter, Escape and the Gamepad API (stick, d-pad, A/B/Y) drive it. It reuses the dark stage and the
plate system, so it reads as the same product at ten feet.

**Desktop settings** live in Settings: install and download folders with a folder picker and free
space, stream installs (ZIP tar stream; saves disk, uses bandwidth, no mid-file resume), keep
archives, and an archive password list tried in order. Paths are editable text so the settings can
be inspected in a browser against the mock adapter.

## Voice

Short, confident, sentence case. Actions keep their name through the flow (“Install” → “Installing”
→ “Installed”). Errors say what happened and what to do; they never apologise and never say
“Oops”. Empty states are invitations, not mood. No exclamation marks, no emoji, no fake statistics:
counts, sizes and times shown in the UI all come from the data on screen.

## Accessibility

Landmarks (`header`, `nav`, `main`, `aside`), labelled dialogs and menus, `aria-current`,
`aria-pressed`, `aria-checked`, `aria-activedescendant` in the command palette, `role="progressbar"`
with values (or indeterminate), visible focus rings, 44px targets for primary controls, and text
contrast checked in both themes. Reduced motion is honoured globally and in JS-driven overlays.
