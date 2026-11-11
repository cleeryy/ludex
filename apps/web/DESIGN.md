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
│ LUDEX     │ search            random  big picture  user  │ 60px
├───────────┼──────────────────────────────────────────────┤
│ LIBRARY ▾ │  Library                                     │
│  Home     │  18 games   860 GB   Sorted by Title         │
│  Library  │  ─────────────────────────────────────────── │
│  Recent   │  All  Action  Horror …   [state] [early]     │
│  Bookmarks│  ┌────┐ ┌────┐ ┌────┐ ┌────┐ ┌────┐          │
│  Collect. │  │3:4 │ │    │ │    │ │    │ │    │          │
│ PLAY ▾    │  └────┘ └────┘ └────┘ └────┘ └────┘          │
│  Downloads│  title  title  title  title  title           │
│  …        │                                              │
├───────────┼──────────────────────────────────────────────┤
│ Settings  │  <main>, focusable, skip link before it      │
│ Help      │                                              │
└───────────┴──────────────────────────────────────────────┘
```

- Grid: 2 / 3 / 4 / 5 / 6 columns at 390 / 640 / 1024 / 1280 / 1536. Covers are always 3:4.
- The rail becomes a five-item bottom bar under 1024 (Home, Library, Downloads or Community,
  Settings, More); “More” opens the same table as the rail, plus genres, theme and sign-out.
- The filter bar sticks to the top of the scroll area. It stacks on phones: genres first, then
  state and the early-access toggle.
- The game detail is a dark stage: backdrop, cover, title, actions; then paper for media, files,
  details and progress. The cover overlaps the stage edge.
- Admin pages are worksheets: page title, hairline, sections, tables. No card grids.

## Navigation and settings (#174)

One table, `src/lib/navigation.ts`, declares every destination: id, route, message id, icon,
group, visibility (`everyone`, `desktop`, `editor`, `admin`), `g`-prefixed shortcut and the aliases
that also count as current (the Library is `/library`, and `/all` renders the same screen). The
rail, the phone bar, the More sheet, the command palette and the shortcuts all read it, so a
destination cannot be named, hidden or keyed differently on one surface than another. Entries are
`<Link>`s: Cmd/Ctrl-click and middle-click open a new tab.

Four groups — **Library**, **Play**, **Community**, **Admin** — with a label, a chevron and
`aria-expanded`, plus Settings and Help in the footer next to the server name and version. The fold
is remembered per device (`ludex.nav.collapsed`); Admin starts folded. Genres left the rail: they
are filter chips in the Library and in the mobile sheet. The rail is one `<nav aria-label="Main
navigation">` with a named `<ul>` per group; the active entry carries `aria-current="page"` and a
2px signal mark. The page has a skip link to a focusable `<main id="main-content">`.

Settings is one addressable page: `src/lib/settings-sections.ts` declares twelve categories, their
copy, their availability (Streaming and Compatibility need the desktop app) and the exact panels
each one owns (a test asserts each panel appears once). The category is in the URL
(`/settings?section=downloads`), so onboarding, dialogs and errors deep-link instead of dropping the
user at the top of a long scroll; legacy anchors (`#help`, `#installed-heading`) still resolve. A
228px category column with a search field on desktop, a sticky chip row with the same search on
phones; picking a category moves focus to its heading (`tabindex="-1"`, no painted ring) and scrolls
the content area to the top. `aria-current="location"` marks the category. The desktop settings file
keeps one draft and one save; the save bar is rendered inside the categories that edit it (Storage,
Downloads, Saves, Integrations, Updates, Compatibility) rather than at the bottom of the page.

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

**Big Picture** (`/bigpicture`) is a full-screen console layout in four selectable shapes, all built
from the same plate system, so it reads as the same product at ten feet:

- **Shelf** (default): a large cover plate with the selected game's title and actions on ink, one
  captioned shelf below, and a strip of key hints. This is the original Big Picture.
- **Hero**: the backdrop *is* the hero — the title is centred over full-bleed key art with the meta
  line and actions, and one centred rail underneath (PlayStation-ish).
- **Grid**: a focused header keeps the selected game's plate and actions in reach while a wall of
  covers fills the screen; columns follow the viewport (8 at 4K, 7 at 1080p, 6 on a 1366 laptop,
  2 on a phone).
- **Compact**: dense rows — index, thumbnail, title, state, live progress — beside a details pane.
  The fastest layout for a large library.

The layout is a per-device preference (`localStorage`, `ludex.bigPicture.layout`, scoped per server)
shared by a small module store: the header control inside Big Picture, its quick switcher and
Settings → Appearance all write the same value, so a change applies to a running screen without a
reload. `L` on a keyboard or X on a controller opens the switcher. Direction semantics come from one
pure `stepForLayout` table: rails walk sideways and change shelf vertically, the grid walks rows by
its own column count, and the list changes shelf sideways. A/Enter, B/Escape and the shoulders mean
the same thing in every layout, the selected game and shelf survive a switch, and the gamepad ring is
restored to that game. Scroll-into-view uses `block: nearest`, so only the container that clips the
card moves.

Big Picture is overscan-aware (10-foot type, `clamp()` gutters, a 7:1 TV token set from #140), never
relies on hover, and respects `prefers-reduced-motion` for both the #142 entrance and the focus
transitions.

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
