# Ludex v0.4.14 — UI/UX audit (read-only)

**Scope:** `apps/web/src/styles/**`, `apps/web/src/theme/**`, `apps/web/src/components/**`, `apps/web/src/screens/**`, Console/Big Picture, overlay, settings, install/downloads, accessibility, responsive.
**Repo state:** `7199a2c chore(release): v0.4.14`. No repository files were modified.
**Method:** code reading + rendered screenshots of the web build in mock mode (Vite dev, `VITE_USE_MOCKS` dev default) + Playwright measurements. Screenshots at 1440×900, 1920×1080, 1280×720, 3840×2160 and 390×844 are in `/tmp/opencode/audit/shots/`. No Tauri shell, real controller, or screen reader was available; Tauri-only flows (install dialog, overlay window, launch) are assessed from code, not observed end-to-end.

**Headline:** the desktop app has a genuine, coherent identity. The Console's **default "Soft" preset** abandons it for a generic friendly-console recipe (cream + coral, 18–34px radii, milky blur, six equal tiles) and is the main source of the "AI slop" reading. The motion, sound and controller systems are real assets and are not the problem. The rest of the audit is a list of specific, mostly small execution defects.

---

## 0. What is genuinely good (do not throw away)

- **Token contract.** Every colour comes from `theme/themes.json` via `theme/types.ts` (`THEME_TOKEN_KEYS`), with a generated fallback stylesheet and a boot script. `styles/no-hardcoded-colors.test.ts` fails the build on literals; there are effectively none in components.
- **Contrast is enforced, not aspirational.** `theme/registry.ts:83-109` + `styles/tokens.test.ts` check 25 pairs at 4.5:1 (3:1 focus) for every built-in theme; user accents/tints are validated in `theme/derive.ts`.
- **Motion system.** `components/big-picture/motion.ts` is a real vocabulary (durations 120/180/240ms, two easings, spring surfaces, interrupt/instant rules), mirrored by `--dur-*`/`--ease*` tokens in `styles/index.css:44-49`, tested against each other. `entrance.ts` is a capped 434ms cascade; `launch-overlay.tsx` does a FLIP handoff with a ceiling and a failure hold. This is why the animations feel good.
- **Sound and haptics.** `lib/ui-sound-specs.ts` holds a documented cue table (peak dBFS, polyphony, dedupe, frozen `move`/`launch` hashes); `lib/gamepad-core.ts:403-460` maps actions to haptic patterns. Consistent, restrained, and functional under reduced motion.
- **Focus architecture.** Gamepad focus is a first-class state (`data-gamepad-focus`, `focusGamepadTarget`), zones are explicit (`system-zones.ts`), overlays are deliberately non-modal, and the launch plate swallows stray input. This is better than most desktop apps.
- **Primitives exist** for the common page furniture: `PageHeader`, `EmptyState`, `ErrorState`, `SectionHeader` (`components/page.tsx`), `Dialog`, `Button`, `Field`, `Progress`, `Skeleton`.

The remaining problems are execution and one large direction decision.

---

## 1. Design-system quality

### 1.1 Coherent

| Area | Verdict | Evidence |
|---|---|---|
| Colour semantics | Strong | 27 tokens, dark/light/TV variants, accent/tint derivation, runtime rejection below AA. |
| Motion tokens | Strong | `motion.ts` + CSS tokens + tests; durations capped 120–320ms, only transform/opacity. |
| Focus ring / keyboard | Strong | global `:focus-visible`, `[data-gamepad-focus]` (`styles/index.css:203`), custom card ring. |
| Elevation language (desktop) | Coherent | hairlines + 8 shadow recipes, no glass on the app shell. |
| RTL / locale groundwork | Good | logical properties, RTL bridge, Arabic font stack, i18n catalogue of 3,864 messages. |
| Sound/haptics | Strong | see §0. |

### 1.2 One-off / missing

**Typography has no scale.** There are **1,915 arbitrary `text-[Npx]` utilities across 27 distinct sizes**, including eight half-pixel sizes:

```
371 × 11.5px   330 × 12.5px   271 × 12px    232 × 13px
217 × 10.5px   207 × 11px     108 × 10px     41 × 13.5px
 36 × 9.5px     30 × 14px      23 × 15px      8 × 34px ...
```

That is not a type system; it is hand-tuning frozen in markup. Consequences: the same semantic role (metadata) is 9.5, 10, 10.5, 11 or 11.5px depending on the file; headings carry five different negative trackings (`-0.015em` to `-0.045em`); there is no mid-level scale between 15px body and 34–92px display. The three families (Bricolage Grotesque / Instrument Sans / JetBrains Mono) are well chosen — the *sizes* are improvised.

**Spacing is Tailwind defaults plus local overrides.** No spacing tokens; section rhythm varies (`mt-8`, `mt-10`, `py-5`, `gap-6`, `gap-8`) with no rule. `--page-x` changes at 1024/640 and Console has a third system (`--bp-u`, `bp-pad-x`), which is defensible but undocumented as a grid.

**Radii are near-duplicates.** 109 × `rounded-[2px]`, 18 × `rounded-[1px]`, 11 × `rounded-[3px]`, plus pills and Console's 2/12/18/24/26/34 scale. At 1x these read as accidents, not a scale.

**Elevation has no levels.** `shadow-cover`, `shadow-hero`, `shadow-plate`, `shadow-pop`, `shadow-dialog`, `shadow-panel`, `shadow-toast`, `shadow-palette`, `shadow-debug` (`styles/index.css:222-340`). `panel` and `dialog` differ by 0/10px; `pop` and `toast` overlap in intent. A 3-level scale would cover every use.

**Iconography is stock and ambiguous.** Lucide only, no optical sizing beyond `size-3.5/4/5`, and the registry maps different destinations to the same glyph: `download` and `ingest` both `Download`; `community` and `users` both `Users`; `health` and `observability` both `Activity` (`components/shell/nav-icons.tsx:35-49`). In the console command menu, "Game page" and "Controller profile" share `Gamepad2` (`overlays.tsx:181-203`). At 10 feet an icon is most of the label.

**Surfaces multiply.** 207 occurrences of `border border-line` in components; `surface`, `bp-panel`, `bp-tile`, `bp-row`, `bp-sheet`, `palette-plate` and raw bordered divs coexist with no rule for which is which. There is no `Surface`/`Panel` primitive.

**Sound and haptics have no user-facing grammar.** The sound *engine* is excellent; the settings expose "UI sounds / Volume / Test sound" but no per-cue previews or a "reduce cues" option, and the console silently mutes them while a game runs (intentional per `lib/ui-sound.ts`, but invisible to the user).

---

## 2. Console: why the default style reads as "AI slop"

### 2.1 Which style is being judged

The default is **Soft**:

```ts
// theme/console-appearance.ts:62-70
/** Soft is the console default: the Switch-like, rounded, translucent look. */
export const DEFAULT_CONSOLE_APPEARANCE = { preset: "soft", accent: "theme", background: "artwork", radius: "soft" };
```

The code comment names the reference outright. Screenshot: `13-console-1920-soft.png`, `42-console-soft-midnight.png`, `26-console-mobile.png`. If the complaint was about **Studio** (`33-console-studio-artwork.png`, `64-console-studio-dark-app.png`), the diagnosis is narrower: Studio already looks like the app; its remaining slop markers are the tile grid/ornament and the micro type (§2.4).

### 2.2 The concrete tells (Soft)

1. **Borrowed palette, unrelated to the brand.** Soft forces the `soft` built-in palette — cream `#efe9dd`, coral `#e2665a` (`theme/themes.json`) — and disables the theme row in the console (`screens/big-picture.tsx:1755-1764`: "The Soft console style uses its own cream palette"). On a midnight/yellow desktop, entering Console flips the product to a cream/coral one (`42-console-soft-midnight.png`). Coral + cream + rounded is the default "friendly AI app" palette of the last three years.
2. **Muddy mid-tones from translucent light over dark art.** `--bp-material-card` is `elevated` at 72% (`styles/index.css:987-1005`) over a blurred, dark wallpaper. Sampled pixels: tile background ≈ `#cdc0bb`, page ≈ `#3a3030` — neither light nor dark, a taupe fog. There is no committed value structure, which is what makes it read as generated rather than art-directed.
3. **Glass everywhere.** Blur 22px on chrome/sheets, 26px on the wallpaper, 18px on art, and **14px on every floating home tile** (`styles/index.css:1206-1222`). Six simultaneous filters behind a six-tile row also contradicts the stylesheet's own "three chrome surfaces" budget.
4. **Softness without hierarchy.** 18–24px card radii, 18px panels, 26–34px "round", pill tabs in Soft (`index.css:1185-1200`), soft shadows. Every edge is negotiable; nothing has an edge.
5. **Dashboard composition.** Six equal-width cards (`home-row.tsx`), each with a corner icon, a two-digit index, a label and an uppercase mono caption. It is a KPI grid, not a console launcher. The same recipe repeats on Power (`power-app.tsx:57-95`) and search results (`search-overlay.tsx:118-130`).
6. **Decorative index numbers.** `01`–`06` appear on home tiles (`home-row.tsx:112`), power tiles (`power-app.tsx:80`), compact rows and search results. They carry no information (position is already spatial) and are the single most recognisable "generated UI" ornament.
7. **Micro-typography as texture.** Console metadata is 9.5–13px: tile detail `text-[9.5px]` (`home-row.tsx:112`), progress/meta `text-[10.5px]` (`parts.tsx:80,138`), system bar `text-[10px]` (`system-bar.tsx:94,127`), footer `text-[10.5px]` (`parts.tsx:489`), hint strip `text-[10.5px]`. The project's own console skill asks for **28px body/metadata minimum at 1080p**. This is 3x under spec, so at couch distance the labels are decoration, not information.
8. **Type does not scale with the console unit.** Geometry uses `--bp-u` (`index.css:96-99`): home tile height 104×u (208px at 4K vs 104px at 1080p), bar 56×u. Type is fixed px. Measured at both 1080p and 4K: tile label 15px, detail 9.5px, footer 10.5px — identical. On a 4K TV the same 9.5px label now sits in a tile twice as tall.
9. **Ornamental honesty problem.** Settings copy explains the aesthetic to the user: "Rounded, translucent surfaces over blurred artwork, with a cream palette." (`labels.ts:56-59`), "Back to Soft, artwork, theme accent and soft corners." (`big-picture.tsx:1770`), and ships a roadmap note "A full theme editor and per-element overrides are not built yet." (`console-appearance-panel.tsx:176`). This is design rationale leaking into product copy.
10. **The home tile advertises internals.** The Settings tile subtitle is `${layoutLabel(layout)} · ${consolePresetLabel(preset)}` (`big-picture.tsx:2049`) — rendered as "SHELF · SOFT" in the UI. A player should never see the preset name there.
11. **Hero composition is unbalanced.** At 1920×1080 the hero copy occupies the left ~35%, the right ~60% is a blur wash, and the rail is cropped at the right edge (13 covers, no pagination cue). The selected game's art is reduced to a 220px cover plus a background wash, while the six chrome tiles dominate the top third.
12. **The alternative proves the point.** Studio (`33-…`) uses hairlines, black, safety yellow, 2px corners and looks like the same product as the desktop app. Soft looks like a different product that borrowed the app's data.

### 2.3 What is *not* slop

The entrance cascade, focus move, cover lift, shelf slide, launch FLIP/filament, spring surfaces, cue design and controller routing are authored and specific. They must survive any visual redesign. Relevant assets: `motion.ts`, `entrance.ts`, `launch-overlay.tsx`, `parts.tsx` (`FocusRing`, `RAIL_CARD_VARIANTS`), `cues.ts`, `ui-sound-specs.ts`.

### 2.4 What needs deciding before any redesign

Do **not** start by changing radii or hue. These questions change the answer:

1. **Which style was judged?** Soft default or Studio? A screenshot of the screen they mean would settle it.
2. **Which references define "right"?** 2–3 products/screens, and 2–3 "never this". Candidates to react to: Nintendo Switch (Soft's stated model), PS5 (spatial, glass but dark), Steam Big Picture (dense, dark), Apple TV (typographic, sparse), or *Ludex's own desktop* extended to 10 feet. My recommendation is the last: it is the only reference the product already owns.
3. **Is Console a continuation of the app or a separate device persona?** If continuation: the console must follow the app theme by default and Soft must justify itself as an option, not the default. If persona: it needs its own art direction (type, palette, icon set), not a preset swap.
4. **Is the console allowed to become dark-first and typographic** (near-black, 7:1, 28px body, 48px+ titles), accepting fewer items per screen? The current 9.5px/6-tile density cannot coexist with the 10-foot rule.
5. **Is coral a brand colour?** Today the brand signal is safety yellow `#ffc61a`; Soft introduces a second, unrelated accent.
6. **Keep the six-tile home row?** Or 3–4 primary actions + a "More" sheet (Switch's actual model)? This is the biggest composition decision.
7. **Custom icons or a restricted Lucide subset?** The stock set is a large part of the generic read; a 12-glyph custom set at 2 weights would fix it cheaply.
8. **Confirm motion and cues are frozen.** They are the part the user likes; a visual redesign should not retune them.

What I would *not* do: a half-Soft/half-Studio hybrid (soft corners over archive hairlines). The two languages are coherent internally; a blend will read as indecision.

---

## 3. UX flows

### 3.1 First run / sign-in

- Login is strong: split statement panel, clear SSO + password, demo helper isolated (`01-login.png`). Error copy and focus styles are correct.
- Setup (`screens/setup.tsx`) is form-heavy but linear; min lengths are explained. No issue found in the first paint.
- **Onboarding wizard is the first-run obstacle.** `onboarding-wizard.tsx:203-280`: non-modal (`aria-modal="false"`), fixed panel, `max-h-[min(78vh,640px)]`, on desktop 430px bottom-end, on mobile `inset-x-3` full width. It covers the library, the game CTA, settings categories and the install dialog (`03-home.png`, `15-mobile-library.png`, `31-install-dialog.png`, `36-mobile-settings.png`). On a phone it is a full-screen blocker without a scrim, and it sits under toasts (`z-70` vs `z-100`), so a "Bodycam installed" toast covers its "Don't show again" link (`31-install-dialog.png`).
- Two dismiss links do nearly the same thing: "Skip for now" (session skip) and "Don't show again" (completes the account flag) (`:253-260`), both 18px-high, 12px text. Users cannot tell the difference.
- It auto-opens on every mount until completed (`:94-104`); there is no "later" affordance other than Skip, which is per-session.

### 3.2 Library

- Header, counts, filters, sticky toolbar, empty states (`LibraryEmpty`) and skeletons are solid. Cards have a 3px lift and a 620ms sheen — a nice, restrained hover.
- **Missing archives are hidden by default behind an ambiguous toggle.** `screens/library.tsx:455-465`: a ghost button with a warning triangle labelled "Show missing" + count. The server default excludes games whose archives are all missing (API `status=available`); the count line says "20 games" while 19 are hidden. A user whose drive went offline thinks games vanished. Better: keep them in the grid with the existing `Missing` badge (`game-card.tsx:57-64`) and a clear filter chip.
- **Duplicate link per card.** The cover is a `<Link aria-label="Open X">` overlay (`game-card.tsx:44-50`) and the title is a second `<Link>` (`:83-85`). Keyboard users stop twice per card; on `/library` there are **116 tabbables**, including 20 duplicate card targets (one extra stop per card).
- Heading order skips `h2`: `H1 Library` → 20 × `H3 <game>` (`components/page.tsx` supports level, but `GameCard` hardcodes `h3`).
- On mobile, sorting lives only inside the Filters dialog; the sticky bar exposes search, missing toggle, filters. Discoverable, but not obvious ("Sorted by Title" is static text above).
- The rail's `g h` / `g b` / `g s` hints render as loose letters in a mono cell (`nav-rail.tsx:100-110`, `lib/navigation.ts:92-302`). They look like debris, not shortcuts.

### 3.3 Game detail

- Strong hero (`60-library-dark.png` context, `05-game-detail.png`): backdrop, large display title, meta line, one primary action. Theme scope is correct on the stage section.
- **Mobile opens the page scrolled 796px down.** `components/game/game-nav.tsx:50-58` calls `activeChip.scrollIntoView({ block: "nearest", inline: "center" })`; `nearest` also scrolls the vertical ancestor, so the `main[data-scroll-root]` jumps to the chip row. Measured on `/game/1` at 390×844: `main.scrollTop = 796`, hero top `-740` (`35-mobile-game-top.png`, `37-mobile-game-scroll0.png`). The comment says "without moving the page vertically"; it does.
- On mobile, even at scroll 0 the hero is mostly empty: `mt-[16vh]` (135px) + 150px cover before the title (`game-detail.tsx:606`). The title is the product.
- `…` overflow and bookmark are icon-only but correctly labelled ("More actions", "Bookmark"); their menus still lack arrow-key navigation (see §4).
- Install/stream/update actions are correctly conditional, but all render as equal-weight `Button`s next to the primary; on a game with update + stream + bookmark + menu the row reads as a toolbar rather than a next action.

### 3.4 Install / downloads / queue

- Browser downloads use a ticket and an `<a>` click (`lib/download.ts`); errors are surfaced through toasts. Fine.
- Desktop install (`install-dialog.tsx`): the strongest logic in the app — location preselection, peak-space estimate, staging volume, blocked states with a reason. Two UX defects:
  - **Controller dead end.** Console's primary for an uninstalled game opens this desktop dialog (`screens/big-picture.tsx:2359`). It is `aria-modal="true"` (`components/ui/dialog.tsx:115`), and the gamepad hook ignores all input while a blocking modal is open except B/Escape (`lib/gamepad.ts:53`, `lib/gamepad-core.ts:337-343`). A controller-only player can open Install and then only close it. It is also visually a desktop modal: 3px radius, 13px labels, mouse-sized selects, in the middle of a 10-foot UI.
  - Its `Select` fields open a portal listbox outside the dialog (§4), so keyboard focus can escape the modal.
- Downloads screen: the per-row status line is an ungrouped mono dump — bytes, "of", percent, files, speed, ETA, phase elapsed, verify label — at 10.5px (`screens/downloads.tsx:571-595`). The eye has no primary metric.
- Per-row actions reach 8–9 controls with the same visual weight: reorder top/up/down, Check again, Cancel, Pause, Retry now, Resume, Retry as download, Open folder, Remove from history (`:626-760`). Cancel is underlined red text; Pause is a ghost button; reorder arrows are 3.5px icons in 32px buttons. On mobile this wraps into several lines.
- No bulk controls (pause all / cancel all) on a queue screen; "Clear finished" is an 18px-high text button.
- The aggregate chart sits in the section heading row, 160–240×40px, with a 3-line 10px caption (`:74-155`). It is compact but competes with the section title and is the first thing the onboarding panel covers.
- State copy conflicts: a row can show `100.0%` and "Waiting for a slot" simultaneously (`06-downloads.png`), because the percentage is transport progress while the phase is the extraction queue. The user reads it as a contradiction.

### 3.5 Launch handoff

- Excellent: FLIP from the selected cover, 0.35 dim, filament, minimum hold, 8s ceiling with honest copy, failure hold, dismiss only when given up (`launch-overlay.tsx`, `launch-plate.ts`). Keep.
- Two messaging channels can fire for one failure: `useLaunchGame` shows a toast and the plate shows `detail` (`big-picture.tsx:995-1020`). Pick the plate; a toast behind a full-bleed plate is noise.
- The plate is `role="group"` + `aria-busy` but has no live region, so a screen reader gets no "Launching X" / "Launch failed" announcement.

### 3.6 Console / Big Picture shell

- Boot focus lands on the selected game; zones, wrap and direction exits are coherent. Layouts share card shapes and the same ring (`parts.tsx`), so the four layouts do feel like one product.
- **Small-width layout is broken.** At 390px the system bar's left block is 114.5px wide while its contents (wordmark 76.8 + "Console" label + gap) need ≈130px, so the label paints under the avatar/name; the screenshot shows "LUDEX. CONSO…" colliding with "MA @mara" (`26-console-mobile.png`). The home row becomes 3 rows of tiles (≈290px), the hero is squeezed to ~200px, the title/description clip and the primary button is cut at the fold.
- **Six equal tiles dominate the top third.** The tiles are the chrome, not the content; the selected game is below them. The Spotlight placement (floating row) is a better composition and is only used by one layout.
- **Status bar affordances that are not interactive.** The right cluster shows a download count/progress and a power glyph, both in non-interactive `<div>` slots (`system-bar.tsx:150-190`). At 10 feet they read as buttons; pressing them does nothing. Either make the download slot open the queue and the power slot open Power, or remove the glyph.
- **Two hint strips at once.** `BigPictureFooter` is always rendered (`screens/big-picture.tsx:2563`), and `OverlayShell` renders its own `HintStrip` (`overlays.tsx:105`). With the menu open, the same "choose/open/close" strip appears twice (`23-console-menu.png`).
- **Double focus ring on overlay rows.** Active rows get `data-overlay-active` (signal border) *and* `data-gamepad-focus` (global 2px outline, offset 3px) (`overlays.tsx:132-145`, `styles/index.css:203`). Cards suppress the global outline for this reason (`index.css:590-594`); overlay rows do not. The result is a heavy coral double ring that reads as an error state (`23-console-menu.png`, `25-console-settings-app.png`).
- **Console settings is mostly empty.** At 1920 the Language tab shows two rows and ~70% empty canvas (`14-console-settings.png`, `25-console-settings-app.png`); Appearance has six rows. The section tabs are `tabIndex={-1}` (`settings-app.tsx:119`) and keyboard section switching is `[` / `]` (`big-picture.tsx:2250`), but the footer only advertises LB/RB (`settings-app.tsx:157-160`). A keyboard player has no visible way to change tabs.
- **Theme row is disabled and explains the architecture.** `big-picture.tsx:1748-1760` disables Theme in Soft and tells the user to switch style to follow the app theme. That is an admission that the two presets are two products.
- **Controller dead ends beyond Install:** "Game page" (`overlays.tsx:184`) navigates to `/game/:id`, and "Remote play" (`big-picture.tsx:1947`) to `/remote`; neither route mounts `useGamepad` (only `big-picture.tsx`, `library.tsx`, `overlay-screen.tsx` do). A controller-only player has no input on those screens. The menu should mark them as mouse destinations, or those screens need a back-only pad handler.

### 3.7 Settings

- Information architecture is good: URL-driven section, category search, deep links, focus moves to the heading, contrast badges, theme previews.
- **Two competing navigation columns** on desktop: the app rail (Settings, Help…) and the settings category column, both vertical lists with the same row style. It is readable but costs a beat; the settings nav could be quieter than the app rail.
- Theme card labels truncate: "Follow the sys…" in the selected card (`components/settings/appearance-panel.tsx:181-186`, `08-settings-appearance.png`). The most important label in the section is the one cut. The mono uppercase `NOW: FOLLOW THE SYSTEM` far right compounds the noise.
- Accent swatches are 24px circles labelled "Use accent 1…5" (`appearance-panel.tsx:404-418`); no colour names, no `aria-pressed`, 24px targets, and the selected state is a border colour only.
- Console appearance panel is chips only, no preview of Soft vs Studio; the user must apply to see. The reset copy names three internals ("Soft, artwork, theme accent, soft corners").
- Copy includes a roadmap line (`console-appearance-panel.tsx:176`). Remove from production.

### 3.8 Overlay

- Architecture is right: one component for the floating window and in-app, focus trap via `trapFocusIndex`, gamepad-first, game never paused, downloads unaffected (`overlay-screen.tsx:25-39`).
- **The scale setting does not scale type.** Container `style={{ maxWidth: 1120*scale, fontSize: 16*scale }}` (`overlay-screen.tsx:282`), but every child is fixed px (`text-[13.5px]`, `text-[10.5px]`, `text-[12px]` at `:296-330`, `:562-610`). Only the panel width changes; text stays put, so "Scale" is a misnomer and large-text users get no benefit.
- Tabs are `aria-pressed` buttons (`:320-334`), not a `tablist`; acceptable but inconsistent with the app's own "tabs are links" model.
- Opacity applies to the panel background only (`:286-288`); text contrast against the game remains constant, which is good.

### 3.9 Errors, empty and loading states

- Good inventory: `BootError` with retry/reset, `ErrorState`, `LibraryEmpty` variants, console error/empty with "Try again"/"Exit Console mode", overlay empty messages, `Skeleton`/`LayoutSkeleton`.
- Errors generally state the cause and the next action. The console error state prints the raw message in 11px mono after a 15px sentence — fine.
- Toasts are the fallback for too many failures (download, launch, settings save, language save). On console, toasts are desktop-styled cards that cover the footer (`25-console-settings-app.png`) and the app's own hint bar. Console needs a native notice surface or a footer message slot.

---

## 4. Accessibility

### 4.1 Focus and keyboard

- **Modal focus trap is incomplete for portalled widgets.** `Dialog` collects focusables inside `panelRef` (`dialog.tsx:71-85`); `Select`/`DropdownMenu` render their list into `document.body` (`popover.tsx:166-184`). Inside a dialog, focus moves to the portal option (outside the trap), then Tab is neither "first" nor "last", so the trap does nothing and focus leaves the modal. Reproduced by code path in the install dialog; needs a trap that understands portals (or popover rendered inside the dialog).
- **Menus/listboxes have no arrow-key model.** `Popover` focuses the first item but has no `ArrowUp/Down/Home/End` or typeahead (`popover.tsx:145-158`). `DropdownMenu` claims `role="menu"` with `menuitem` buttons that use `aria-checked` (`:195-225`) — `aria-checked` is not valid on plain `menuitem`. `Select` claims `listbox`/`option` (`select.tsx:111-126`) without roving focus. Keyboard users get Tab-only, which contradicts the `aria-haspopup` promise.
- **Popover does not close on focus-out.** Tab can move into the page behind an open notification/menu; only outside pointerdown and Escape close it (`popover.tsx:128-146`). Escape restores focus, outside click does not.
- **No route-change focus or document title.** `AppShell` never focuses `main` or sets `document.title` on navigation (`app-shell.tsx:339-377`); the tab and screen reader announce "Ludex" on every screen. Only `collection-share.tsx:51` sets a title.
- **Duplicate tab stops** per game card (§3.2) and two hint-strip presentations; `/library` has 116 tabbables with no roving index or grid semantics.
- **Positive:** skip link, `main tabIndex={-1}`, Escape handling, focus restoration in `Dialog`, `aria-current` on nav, gamepad focus ring, non-modal console overlays, focus restore after power confirm (`big-picture.tsx:2130-2148`).

### 4.2 Screen-reader semantics

- 198 `aria-label`s for ~40 screens; the sampled routes had **0 unnamed interactive elements**, and 0 images without `alt`. Good baseline.
- `role="dialog"` is used for the command palette, the notification popover, the onboarding panel and the overlay. Only the palette and `Dialog` are modal. The notification popover is labelled but not trapped (correct for non-modal), yet it does not close on focus-out (above).
- Console launch plate is `aria-busy` but not `aria-live`; failure/slow transitions are silent.
- `Progress` exposes `aria-valuenow`, label, min/max (`controls.tsx:243-266`) — good. The indeterminate branch renders a moving bar but no `aria-valuetext`; acceptable.
- `HintStrip` reads key glyphs and words in DOM order without a group label (`parts.tsx:388-460`); low priority on a TV, noisy if a screen reader is attached.
- Heading hierarchy skips levels on Library (`h1` → `h3`).

### 4.3 Contrast

- Token pairs are AA-checked and pass. The gap is **composited** surfaces:
  - Console Soft, sampled from `13-console-1920-soft.png` (TV-safe tokens active, text `#534c41`): tile micro-labels **4.65–5.45:1**, footer hint **4.53:1** — against the console's own **7:1** target. Over a brighter or darker area of artwork this moves in both directions; nothing guarantees 7:1 over art.
  - Pairs used in components but not in `CONTRAST_PAIRS`: `ink-2` on `surface-2`, `danger`/`ok` on `surface-2`. Nord computes `danger/surface-2 = 4.28` and `ok/surface-2 = 4.23` (< 4.5); gruvbox `danger/surface-2 = 4.39`. Whether they ship below AA depends on the exact composition, but the test cannot catch it.
- Focus colour is the same yellow as the primary button in dark themes; the outline offset saves it, but a 2px offset is the entire separation.

### 4.4 Reduced motion, transparency, contrast preferences

- Strong: global reduced-motion rule (`index.css:1526-1556`), JS `useReducedMotion` in every animated surface, `REDUCED_FADE`, no travel under reduce, `.cover-sheen` removed, `prefers-reduced-transparency` and `prefers-contrast: more` for the console (`:1325-1390`). `console-material.test.ts` enforces the fallbacks.
- Gap: `prefers-contrast: more` only rewrites the console; the desktop UI has no high-contrast branch beyond theme choice. `forced-colors` is not handled (borders/background images may vanish).

### 4.5 Touch and overflow

- Many mobile targets are below 44×44: switches 20×36 (`controls.tsx:153-176`), checkboxes 20×20 (`:185-200`), segmented "sm" 28px tall (`:279-303`), filter chips 24px, toolbar buttons 32–36px, theme swatches 24px. Measured on `/library` at 390px: 20+ controls under 44px.
- Long text: `truncate` is used almost everywhere, which protects layout but destroys information (download names, game titles, server names). The console's rail captions truncate even at 1920 (`Marvel's Spider-…` in `50-console-720p.png`).
- Console has no minimum-width message and no small-screen layout; `26-console-mobile.png` is the worst screen in the product.
- The 16px input rule at `max-width: 640px` prevents iOS zoom, but compact fields (search 34px) grow text without growing height.

---

## 5. Code organization for UI

### 5.1 Giant components

| File | Lines | Note |
|---|---:|---|
| `screens/big-picture.tsx` | 2,728 | ~1,400 lines of state/handlers before the render; data, input, motion, power, settings rows and JSX in one component. |
| `components/game/cloud-saves.tsx` | 1,020 | several dialogs and flows. |
| `components/settings/remote-play-panel.tsx` | 1,078 | one panel. |
| `components/game/game-settings.tsx` | 1,012 | one panel. |
| `components/game/make-it-run.tsx` | 1,000 | one panel. |
| `components/settings/sections-device.tsx` | 813 | 7 section bodies in one file. |
| `screens/library.tsx` | ~1,000 | filters, grid, gamepad, empty states, imports. |

`big-picture.tsx` is the critical one. It already extracts layouts, parts, zones and motion, so the seam exists: extract `use-console-shell` (level/zone/overlay/power), `use-console-launch`, `console-settings-rows.ts`, `use-console-input` and keep a render file. That would also make the dead-end/parity rules testable in one place.

### 5.2 Duplication and primitives

- **Two settings implementations** for one model: desktop sections and console rows. Theme is a 6-card picker on desktop and a 3-value cycle in the console (system/dark/light), so the Nord/Gruvbox/Amber themes are unreachable from the couch. Copy and behaviour will drift.
- **Two hint strips** (page footer and overlay footer), **two card systems** (desktop `GameCard`, console `RailCard`/`GridCard`/`CompactRow`), **two empty-state approaches** (`EmptyState` and hand-rolled console empty). Some divergence is justified (10-foot vs desktop), but the shell/status patterns can be shared.
- **No surface/spacing primitives**; 207 raw bordered containers and 1,915 arbitrary text sizes are the symptom.
- **Copy:** 3,864 messages and only 20 hardcoded English strings (advisory lint), which is very good. Remaining user-facing ones: `artwork-dialog.tsx:190`, `cloud-saves.tsx:588-991`, `users.tsx:476` (`No limit`), plus debug panels. Concatenated labels (`t("Quit") + " Ludex"`, `power.ts:75`) are untranslatable for languages that put the product first.
- **Naming:** `Console mode` (UI), `Big Picture` (comments/tests), `/bigpicture` (route), `bp-*` (tokens/classes). Pick one user-facing name and alias the route (`/console`). The `bp` prefix is fine internally.
- **Tests are colocated** with motion, layout, cues, controller, tokens, contrast, material budget, page scroll. That is a strength; the missing one is visual/responsive regression (no Playwright screenshot assertions for Console at 390/720p/4K, no axe pass).

---

## 6. Top 20 fixes (impact / effort)

Impact 1–10 (user-visible), effort S (<half day), M (1–3 days), L (>3 days). Ranked by impact ÷ effort.

| # | Fix | Impact | Effort | Evidence |
|---|---|---:|---|---|
| 1 | **Stop defaulting Console to Soft.** Default to Studio (or a new "archive at 10 feet" preset), or make the preset an explicit first-run choice. One-line change plus test/default expectation updates. | 9 | S | `theme/console-appearance.ts:62-70`; `console-material.test.ts` |
| 2 | **Give Console a 10-foot type scale.** Minimum 16px for metadata, 20px body, 48px+ hero; scale sizes with `--bp-u`/`rem`, not fixed px. | 9 | M | `home-row.tsx:112`; `parts.tsx:80,489`; `system-bar.tsx:94`; skill requires 28px |
| 3 | **Commit Soft's value structure.** Opaque or near-opaque tiles, or a dark material; remove cream+coral as the default. Do not keep translucent light cards over dark art. | 9 | M | `index.css:987-1005`; sampled 4.5–5.4:1 |
| 4 | **Make install controller-usable.** Console-native install sheet with pad navigation, or a pad-navigable dialog; mark "Game page"/"Remote play" as mouse destinations or add back-only pad handling. | 9 | M | `big-picture.tsx:2359,1947`; `gamepad.ts:53`; `dialog.tsx` |
| 5 | **Remove dashboard ornament and recompose the home row.** Drop 01–06 indexes and corner-icon/tile template; 3–4 primary actions + More, or one launcher band. | 8 | M | `home-row.tsx:100-140`; `power-app.tsx:57-95` |
| 6 | **Onboarding becomes a real surface.** Modal with scrim under 640px, one dismiss action, close on scrim, raise above toasts, never cover a CTA. | 8 | S–M | `onboarding-wizard.tsx:203-280`; `15/31/36-*.png` |
| 7 | **Fix the game-detail scroll jump.** Replace `block: "nearest"` with a horizontal-only scroll (`list.scrollLeft`) or `inline` on a horizontal scroller. | 8 | S | `game-nav.tsx:50-58`; measured `scrollTop 796` |
| 8 | **Introduce type-scale tokens (desktop).** 8 sizes + 3 trackings; codemod the 1,915 occurrences. | 8 | L | 27 sizes, half-pixel values |
| 9 | **Fix modal/listbox keyboard model.** Arrow/Home/End/typeahead for menus/selects; make the dialog trap portal-aware; close popovers on focus-out. | 7 | M | `popover.tsx:145-158`; `dialog.tsx:71-85`; `select.tsx:111-126` |
| 10 | **Console small-screen behaviour.** Either a minimum-size message with a clean exit, or a real 1-column layout; fix SystemBar collision first. | 7 | M | `26-console-mobile.png`; measured left block overflow |
| 11 | **Stop hiding missing archives.** Show them with the existing `Missing` badge; replace "Show missing" with a proper filter chip. | 7 | S–M | `library.tsx:455-465`; server default |
| 12 | **Fix composited contrast.** Add material-over-art checks and the pairs actually used (`ink-2/surface-2`, `danger|ok/surface-2`); fix Nord/Gruvbox if confirmed. | 7 | M | `registry.ts:83-109`; sampled 4.53:1 footer |
| 13 | **44px touch targets on mobile.** Switch, checkbox, chips, segmented, swatches, toolbar. | 6 | M | `controls.tsx:153-206,279-303`; 20+ under 44px |
| 14 | **One primary action per download row.** Move reorder/retry/open/remove into a menu; collapse the status line to phase + percent + one secondary metric. | 6 | M | `downloads.tsx:571-760` |
| 15 | **Remove duplicate hint strip / double focus ring.** Hide the page footer under overlays; drop `data-gamepad-focus` outline where a row has its own signal border. | 6 | S | `big-picture.tsx:2563`; `overlays.tsx:105,132` |
| 16 | **Console settings density and language.** Fill the empty screen (appearance preview), share copy with desktop settings, remove roadmap text and internal preset names. | 6 | M | `settings-app.tsx`; `labels.ts:56-59`; `:176` |
| 17 | **Route-change focus and titles.** Focus `main` heading and set `document.title` per screen. | 6 | S | `app-shell.tsx:339-377`; only `collection-share` sets title |
| 18 | **One link per game card, correct heading depth.** Cover and title should not both be tab stops; cards under an `h2` grid. | 5 | S | `game-card.tsx:44-85`; 116 tabbables |
| 19 | **Fix overlay scale.** Convert overlay type to `em`/`rem` so `scale` actually scales, or remove the setting. | 4 | S | `overlay-screen.tsx:282,296-330` |
| 20 | **Clean the console chrome affordances.** Dead power glyph, non-clickable download status, `SHELF · SOFT` tile subtitle, "Quit Ludex" concatenation. | 4 | S | `system-bar.tsx:150-190`; `big-picture.tsx:2049`; `power.ts:75` |

Also queued but below the cut: duplicate nav icons (`nav-icons.tsx:35-49`), theme-card truncation (`appearance-panel.tsx:181`), 20 hardcoded strings, `g h` shortcut debris (`nav-rail.tsx:100-110`), two navigation columns in Settings.

---

## 7. Five quick wins (independent of the Console direction)

1. **Hide `BigPictureFooter` while an overlay or the power confirm is open.** Removes a duplicated hint strip and a chunk of visual noise on every console interaction. `screens/big-picture.tsx:2563`.
2. **Fix `GameNav`'s scroll call.** `inline: "center"` only, or set `scrollLeft` on the list; game detail stops opening 796px down on phones. `components/game/game-nav.tsx:57`.
3. **Fix the SystemBar collision under ~480px.** Hide the `Console` eyebrow and the online/download/profile slots, keep wordmark + clock + avatar. `components/big-picture/system-bar.tsx:106-190`.
4. **Remove the second focus ring on overlay rows.** Let `data-overlay-active` own the ring, as cards already do for `data-gamepad-focus`. `components/big-picture/overlays.tsx:132-145`, `styles/index.css:590-594`.
5. **Let theme-card labels breathe.** Two-line label or shorter "System" copy; the selected theme name currently truncates to "Follow the sys…". `components/settings/appearance-panel.tsx:181-186`.

---

## 8. Questions to answer before any Console redesign

1. Which screen was judged: the default Soft preset, or Studio? (Screenshot if possible.)
2. Three references for the intended feel, and three "never this". Is the target Switch, PS5, Steam Big Picture, Apple TV, or **Ludex's own desktop at 10 feet**?
3. Is Console a continuation of the app brand (follow the app theme) or a separate device persona with its own art direction?
4. Can Console go dark-first and typographic at the skill's 28px body / 48px+ titles, accepting fewer items per screen?
5. Is the six-tile home row a requirement, or can it become 3–4 primary actions + More?
6. Is coral a brand colour, or is safety yellow the only signal?
7. Custom icon set, or a curated/optically-adjusted Lucide subset?
8. Confirm `move`/`launch` cues and the motion table are frozen while the visuals change.

Until 1–4 are answered, changing radii, blur amounts or hues will only produce a third style, not a better one.

---

## Appendix — evidence index

**Screenshots** (`/tmp/opencode/audit/shots/`): `01-login`, `03-home`, `04-library`, `05-game-detail`, `06-downloads`, `07/08-settings`, `13/20-console-soft-1920`, `23-console-menu`, `25-console-settings-app`, `26-console-mobile`, `31-install-dialog`, `33/64-console-studio`, `35/37-mobile-game`, `36-mobile-settings`, `40/60-library-dark`, `42/63-console-soft-dark-app`, `50-console-720p`, `51-console-4k`.

**Measurements:** `main.scrollTop = 796` on `/game/1` at 390×844; console type 15/9.5/10.5px identical at 1080p and 4K while `--bp-u` doubles; sampled contrast 4.53:1 (footer) and 4.65–5.45:1 (tiles) with TV-safe tokens; 116 tabbables on `/library`; 1,915 arbitrary font sizes / 27 values; 207 raw bordered containers; 109 `rounded-[2px]`.

**Limitations:** mock data; browser build only (no Tauri install dialog, overlay window or controller observed); no automated axe or screen-reader run; contrast sampled from screenshots (antialiasing makes ±0.1–0.2 possible). Findings that depend on the Tauri shell are marked as code-path assessments.
