# Ludex Console / Big Picture — current-state audit (read-only)

Repo: `/root/ludex` — `v0.4.17-5-g1a32205` (`git describe`), version field `0.4.17`.
Tree clean. No files modified by this audit.

Method: full read of the route/boot layer, the 2,744-line console screen, every
module in `apps/web/src/components/big-picture/` (41 non-test files), the console
CSS block, the console appearance/theme pipeline, the sound and controller
subsystems, and the Tauri side (`console.rs`, `commands.rs`, `settings.rs`,
`lib.rs`, `overlay.rs`, `streaming.rs`).

Sizes below are `wc -l` at this commit.

---

## 1. Entry points and modes

### 1.1 The route

| Surface | Path | File:line |
| --- | --- | --- |
| Console route | `/bigpicture` | `apps/web/src/app.tsx:288-295` |
| Desktop shell (everything else) | `/` … `/admin/*` | `apps/web/src/app.tsx:304-440` |
| Public, outside the Gate | `/share/:token`, `/collection-share/:token`, `/download`, `/register` | `app.tsx:273-279` |
| Overlay window (separate webview) | `?overlay=1` → `OverlayWindowApp` | `app.tsx:252-254`, `components/overlay/overlay-window.tsx` |

The console is **a route inside the same SPA, the same provider tree and the same
`Gate`** as the desktop app. It is not a shell.

```tsx
<Route path="/bigpicture" element={<Gate><BigPictureScreen /></Gate>} />
```

`Gate` (`app.tsx:135-169`) is the only auth gate: it renders `BootScreen` until
`useAuth().ready`, `BootError` on `bootError`, redirects to `/setup` when the
server needs setup, and to `/login` when there is no user. `/bigpicture` sits
behind exactly the same rules as `/`.

### 1.2 How the shell is selected

Three different mechanisms, none of which is "the console shell":

1. **Navigation, always available** — a nav item `id: "bigpicture"`,
   `label: "Console mode"`, group `play`, icon `gamepad`
   (`lib/navigation.ts:149-155`), rendered by the rail and the mobile bar, plus a
   `Gamepad2` icon button in the desktop top bar (`components/shell/app-shell.tsx:207`).
2. **Sunshine / `--console`** — `ConsoleEntry` (`app.tsx:230-249`) probes
   `native.consoleMode()` and, *only when the path is exactly `/`*, navigates to
   `/bigpicture` with `replace`. In console mode the console screen itself then
   queries the same status (`screens/big-picture.tsx:494-508`) and sets
   `consoleActive`.
3. **In-game overlay** — "Open Big Picture" calls
   `native.overlayShowBigPicture()` → Rust `overlay::show_big_picture`
   (`apps/desktop/src-tauri/src/overlay.rs:1519-1526`): closes the overlay,
   shows the main window, emits `overlay://navigate` `/bigpicture`.

There is no route guard for console mode. `AppShell` has zero knowledge of
`consoleMode` (grep: the only web consumers are `app.tsx:237` and
`screens/big-picture.tsx`). A console session that leaves `/bigpicture` therefore
renders the **desktop shell** — top bar, sidebar, mobile bar.

### 1.3 Boot sequence (in order)

`main.tsx` → `App` (`app.tsx:251`) →

`QueryClientProvider` → `SettingsProvider` → `ToastProvider` → `AuthProvider` →
`PreferencesProvider` → `DownloadsProvider` → `UploadProvider` →
`VirusTotalQueueProvider` → `I18nProvider` → `UpdateProvider` → `HashRouter`.

Notes that matter for a rewrite:

- `I18nProvider` remounts the whole child tree on a locale change:
  `<Fragment key={state.locale}>{children}</Fragment>`
  (`lib/i18n.tsx:682-685`). This is the documented reason the console's
  shell level lives in the URL (`system-zones.ts:186-202`, `?app=settings&section=`).
- `PreferencesProvider` hydrates the **account** preference document, which
  already carries `bigPicture.layout`, `bigPicture.sound`, `bigPicture.haptics`
  (`state/preferences.tsx:113-118, 181-188, 590-608`), and writes them into the
  same device stores the console reads. So console state is already
  server-synced *and* device-local; two writers.
- `SettingsProvider` exposes `tv`, `setTv`, `tokens`, `accent`, `tint`,
  `resolved` (`state/settings.tsx:71-90, 299-332`) — the console calls
  `setTv(true)` for its lifetime (`screens/big-picture.tsx:329-332`), which swaps
  the rendered token map for `tvTokens()` (7:1 text, wider focus ring, no grain,
  `theme/derive.ts:158-172`).

### 1.4 Browser / PWA / mobile

`/bigpicture` is reachable on every device (nav item `visibility: "everyone"`).
Consequences:

- `h-dvh`, `--bp-u` scale and safe-area padding let it render on a phone, but the
  grid is the only mobile-viable layout and the on-screen keyboard is the only
  text input.
- No install/download path exists for a browser: `confirm()` falls through to
  `downloadGame()` (`screens/big-picture.tsx:2358-2380`), `Power` hides
  desktop-only actions with a reason (`power.ts:39-104`).
- No mobile-specific entry: no PWA shortcut, no "add to home screen → console",
  no orientation/fullscreen request on Android.
- `PwaPrompts` and `UpdateNotifier` render outside the route table
  (`app.tsx:269, 442-445`) so console mode inherits desktop toasts and the
  update banner.

---

## 2. Architecture inventory

### 2.1 Top level

| File | LOC | Role |
| --- | --- | --- |
| `apps/web/src/screens/big-picture.tsx` | **2,744** | The whole shell: state, input wiring, zones, apps, overlays, render tree |
| `apps/web/src/components/big-picture/` (41 non-test files) | **7,267** | Layout views, parts, overlays, motion, controller model, sound panel, geometry |
| `apps/web/src/components/big-picture/` (13 test files) | 2,055 | 182 `it()` blocks, all DOM-free |
| `apps/web/src/styles/index.css` (console block) | 1,604 total, ~440 console | Materials, `--bp-*` tokens, focus move, rails |
| `apps/web/src/theme/console-appearance.ts` | 258 | Presets/accents/backgrounds/radii + versioned storage |
| `apps/desktop/src-tauri/src/console.rs` | 606 | `--console` parsing, live promote, Sunshine command line, receipt |
| `apps/web/src/components/overlay/overlay-screen.tsx` | 692 | In-game overlay (separate surface, not the console) |

Console-specific UI is therefore **~10,000 lines** (2,744 + 7,267) excluding tests
and CSS.

### 2.2 The screen (`screens/big-picture.tsx`)

Composition, in source order:

| Lines | Block |
| --- | --- |
| 164-202 | Constants (`REFRESH_ON_OPEN_MS` 10 s, `REFRESH_ON_FOCUS_MS` 30 s), `LAYOUT_VIEWS` (6 entries, exhaustive `Record`), `matchesExitShortcut` |
| 211-290 | Component open, ~25 hooks: auth, i18n, settings, downloads, layout, appearance, 13 `useState`, refs, motion hub, reduced motion, `gridColumns(useViewportWidth())` |
| 296-313 | `emit` / `beginMotion` — the single cue + motion entry point |
| 318-325 | Overlay state (`overlay`, `overlayIndex`, `overlayReturnRef`, query, search index), prefs |
| 329-347 | `setTv(true)`, pointer idle, held-input instant flag |
| 358-392 | Layout witness, sound preload/unlock, `setPrefs`, reduced motion, `setGameRunning` |
| 399-508 | URL level sync, online/offline, pad battery poll (60 s), `onGameExited`, download phase cue priming, `consoleMode()` |
| 510-554 | Shelves (Recent/All), `useBigPictureNavigation`, per-selection game detail query, `consoleTheme` |
| 557-600 | Servers query, collections query, controller profiles query |
| 603-660 | Pad resolution chain (`pickActivePad` → `resolveProfileForPad` → `withBigPictureDefaults`), haptics gate |
| 663-812 | Search debounce, overlay counts, library refresh policy, entrance timing, **fullscreen effect** |
| 814-826 | `exit()` — `native.exitConsole()` in console mode, else `history.back()` / `/` |
| 830-1090 | `syncLevelUrl`, zone focus helpers, `openApp`/`closeApp`, home/action zone movement, cover rect, `startGame` (launch plate) |
| 1096-1180 | Power execution (`end-session`, `sleep`, `restart`, `quit`, `quit-game`, `exit`) |
| 1186-1250 | Layout picker, overlay open/close |
| 1256-1400 | Overlay movement/activation, command router `runCommand` |
| 1409-1610 | Gamepad dispatch: `handleGamepad` (plate → picker → confirm → overlay → app → zone → library) |
| 1617-1700 | `useGamepad` with per-pad profile resolver, haptics, pulse ref |
| 1706-1900 | **Settings rows model** (language / appearance / controllers / streaming), `activateSetting`, `adjustSetting` |
| 1930-2060 | Home-row details model, `activateHome` |
| 2068-2160 | Focus-sync effects (zone → DOM focus, ring clearing, app clamp, confirm restore) |
| 2168-2359 | **Keyboard handler** (one 190-line `useEffect`, 16 deps) |
| 2361-2468 | Primary action node, `viewProps`, `body` (skeleton/error/empty/layout), overlay shell props, footer context, home row |
| 2470-2743 | Render tree: backdrop, `SystemBar`, `HomeRow`, stacks, `SettingsApp`/`PowerApp`, footer, 5 overlay `AnimatePresence` blocks, `LaunchPlate`, `ConfirmPanel`, `InstallDialog`, debug panels, `LayoutPicker` |

Hook density: **30 `useEffect`, 13 `useState`, 11 `useMemo`, 8 `useCallback`,
13 `useRef`, 2 `useLayoutEffect`, 4 `useQuery`** in one component.

### 2.3 Modules (`components/big-picture/`), by role

**Model / pure logic (portable, tested)**

| File | LOC | Role | Depends on |
| --- | --- | --- | --- |
| `motion.ts` | 574 | 15-kind transition table, springs, reduced-motion mapping, `transitionOf`/`exitOf`/`cueForMotion`, `createTransitionHub` (interruption counters), `motionDebugEnabled` | `motion/react` types, `lib/ui-sound-specs` |
| `controller.ts` | 584 | 7 pad families × 12 hint inputs, hint contexts, Big Picture extra mappings (`START_BUTTON`, `GUIDE_BUTTONS`, `TRIGGER_BUTTONS`), controller prefs sanitize/store, haptics strength cycle, `profileOptions` | `lib/controller-*`, `lib/gamepad-core`, `lib/ui-sound-specs`, `layout.ts` |
| `layout.ts` | 249 | 6 layout ids, persisted choice, `gridColumns` breakpoints, `stepForLayout` / `pageStepForLayout` (the one direction model), `resolveIndex` | `system-zones.ts` |
| `system-zones.ts` | 232 | Focus zones, `zoneAfterMove`, `zonesForLayout`, `homeRowPlacement`, `HOME_ITEM_IDS`, settings sections, `ShellLevel` + URL codec | `layout.ts` |
| `navigation.ts` | 271 | `useBigPictureNavigation`: shelf/index state, card refs, `scrollIntoView`, `move`/`willMove`/`jump`, refresh re-resolution | `layout.ts`, `lib/gamepad` |
| `input.ts` | 209 | Keyboard + pad → `BigPictureCommand` for 12 contexts; hints context mapping | `controller.ts`, `lib/gamepad-core` |
| `search.ts` | 169 | On-screen keyboard rows, filter, focus-grid movement, flatten rows | `layout.ts`, `lib/gamepad-core` |
| `power.ts` | 138 | Mode-aware action list, grid step, confirm defaults (`cancel` first) | `layout.ts`, i18n |
| `launch-plate.ts` | 114 | FLIP maths, 8 s ceiling, 420 ms min hold, 1.6 s fail hold, dismiss rules | i18n |
| `entrance.ts` | 76 | Boot cascade for bar/home/footer/cards, `ENTRANCE_MS` | `motion.ts` |
| `cues.ts` | 56 | Command → cue (movement only; surfaces own their cue) | `ui-sound-specs`, `input.ts` |
| `pointer-idle.ts` | 79 | Attribute-only cursor hiding after 1.6 s | — |
| `fullscreen.ts` | 59 | `shouldEnterFullscreen` / `fullscreenRestoreOnExit` (the #233 fix) | — |
| `model.ts` | 43 | `BigPictureLayoutProps` contract | — |
| `labels.ts` | 99 | Layout/preset/accent/background/radius copy | i18n, `theme/console-appearance` |
| `use-layout.ts` | 109 | Module store + `localStorage` mirror + `storage` event + `useViewportWidth` | `layout.ts` |

**Stores (module singletons, `useSyncExternalStore`)**

`use-layout.ts` 109 · `use-controller-prefs.ts` 135 · `use-console-appearance.ts` 94 ·
`use-change-witness.ts` 20. Three parallel store implementations with the same
shape (read/sanitize/write/notify + cross-window `storage`) — duplicated by design,
not factored.

**Shell surfaces (React)**

| File | LOC | Role |
| --- | --- | --- |
| `parts.tsx` | 510 | `SelectedEyebrow`, `MetaLine`, `ProgressLine`, `ActionRow`, `ShelfTabs`, `FocusRing`, `RailCard` (3 variants), `GridCard`, `CompactRow`, `HintStrip`, `BigPictureFooter` |
| `overlays.tsx` | 485 | `OverlayShell` (scrim + centred sheet + hint footer), `OverlayRow`, `COMMAND_ITEMS` + `CommandMenu`, `CollectionsOverlay`, `ConfirmPanel`, `DownloadsOverlay` |
| `system-bar.tsx` | 200 | Status bar: wordmark, "Console" eyebrow, refreshing, console badge, avatar/@name/server, online, downloads count+%, pad family+battery, clock (minute-aligned), power glyph. Status only, never a focus stop |
| `home-row.tsx` | 233 | 6 tiles (`continue, library, downloads, collections, settings, power`) in 3 placements (`row` grid 2/3/6, `dock` list, `overlay` icon strip) |
| `settings-app.tsx` | 251 | In-shell Settings: section tabs (LB/RB), row list (choice/toggle/action), facts dl (version/server/account/pads) |
| `power-app.tsx` | 105 | Power grid, 2 columns, danger tone |
| `profile-overlay.tsx` | 140 | Profile list + haptics toggle/strength rows |
| `search-overlay.tsx` | 148 | Real `<input>` + on-screen QWERTY grid + result rows |
| `layout-switcher.tsx` | 146 | Quick switcher (arrows/Enter/Esc, `l` closes) |
| `layout-preview.tsx` | 159 | Token-drawn thumbnails of the 6 layouts |
| `layout-skeleton.tsx` | 146 | Per-layout skeleton (exhaustive switch) |
| `layout-panel.tsx` | 76 | Settings → Appearance layout cards |
| `sound-panel.tsx` | 224 | Settings → sound/haptics (toggle, volume, test cues) |
| `motion-probe.tsx` | 110 | `?motionDebug=1` rAF frame-budget panel |
| `console-backdrop.tsx` | 37 | Static wallpaper: art plate or token canvas |
| `launch-overlay.tsx` | 162 | Launch plate view (FLIP, dim, filament, copy) |

**The six layouts** (all consume `BigPictureLayoutProps`, none fetch)

| File | LOC | Composition |
| --- | --- | --- |
| `layout-shelf.tsx` | 135 | Art backdrop, large cover + copy, dense tabs, horizontal rail |
| `layout-hero.tsx` | 122 | Full-bleed art as hero, centred title, rail under |
| `layout-grid.tsx` | 124 | Focused header + cover wall (`gridColumns`) |
| `layout-compact.tsx` | 124 | Dense list: index, thumb, title, state, progress |
| `layout-dock.tsx` | 105 | Bottom-anchored block, home row under the rail |
| `layout-spotlight.tsx` | 215 | Full-bleed art crossfade, activity line, quick-action zone, bottom strip |

**Overlays / surfaces currently in the screen** (state machine in the screen, view
in the module): `menu`, `search`, `downloads`, `profiles`, `collections`, `exit`,
plus `layout picker`, `launch plate`, `power confirm`, `settings app`, `power app`.

---

## 3. State and data the shell depends on

### 3.1 React context

| Provider | What the console reads | Why it matters |
| --- | --- | --- |
| `QueryClient` | `useQueryClient` for refresh-on-open/focus | Refresh policy is imperative here, not per-query |
| `SettingsProvider` | `theme, resolved, setTheme, tokens, accent, tint, tv, setTv` | `setTv(true)` on mount mutates the *global* token map |
| `Toaster` | `push` | Every refusal/error is a desktop toast |
| `AuthProvider` | `user`, `updateLocale` | Locale writes hit the account |
| `PreferencesProvider` | indirect (layout, sound, haptics sync) | Two writers to the same stores |
| `DownloadsProvider` | `jobs, active, activeCount, aggregate, jobForGame, titleFor` | **1 Hz updates** |
| `I18nProvider` | `locale`, `setLocale`, `deviceOverride`, `clearDeviceOverride`, `meta.intl` | Remounts the tree on change |
| `UploadProvider`, `VirusTotalQueueProvider`, `UpdateProvider` | none directly | Dead weight in the console tree; `UpdateNotifier` still renders |

### 3.2 Queries / hooks

`useAllGames()` (`lib/hooks.ts:24`, stale 60 s) · `useInstalledMap()` (`:92`,
stale 5 min) · `useLaunchGame()` · `api.getGame(selected.id)` **on every
selection change** (`screens/big-picture.tsx:530-535`, stale 60 s, no debounce) ·
`api.listCollections()` (enabled only for library level or its overlay) ·
`api.controllerProfiles()` (stale 5 min) · `listServers()` (stale 60 s) ·
`native.consoleMode()` · `native.overlayStatus()` every 60 s for battery ·
`native.onGameExited()`.

### 3.3 What breaks if the shell is rebuilt outside `AppShell`

Nothing structural — the shell never uses `Outlet`, `useOutletContext`, the rail,
`routeScreenKey` or `AppShell`'s scroll restoration. It has its own scroll
containers and its own focus model. But the rebuild must bring along:

1. `QueryClientProvider` (every list and detail).
2. `SettingsProvider` **or** an equivalent token source: the console reads
   `tokens/accent/tint/resolved` and calls `setTv`. Without `setTv`, the TV-safe
   map never applies and every `text-stage-*` pair drops from 7:1 to the base
   contract.
3. `AuthProvider` (`user` for the bar, `updateLocale` for the language row) and
   `Gate`, or an equivalent boot/401 path.
4. `I18nProvider` — module-level `t()` is not a hook; the console relies on the
   keyed remount plus URL state to survive a language change.
5. `DownloadsProvider` (queue, percent, `titleFor`) — the entire install/progress
   surface, plus the completion/error cues.
6. `ToastProvider` (refusals), `PreferencesProvider` (account sync of
   layout/sound/haptics).
7. `OverlayHost` is **not** needed; it is router-level and unrelated.

The console would *gain* by dropping `UploadProvider`,
`VirusTotalQueueProvider`, `UpdateProvider`/`UpdateNotifier`, `PwaPrompts`,
`DesktopAppPrompt`, `OnboardingWizard`, `CommandPalette`, `ShortcutsDialog`,
`DownloadsStrip` and `MobileBar`.

---

## 4. Native / desktop coupling

| Concern | Implementation | Notes |
| --- | --- | --- |
| Flag parsing | `console.rs:22-77` (`--console`, `--windowed`), pure + tested | `fullscreen = active && !windowed` |
| Live promote | `console.rs:79-133` (`ConsoleSession`, `second_instance_action`), `promote()` 263-294 | A second `--console` promotes the running desktop instance instead of being swallowed |
| Window policy | `console.rs:235-256` `apply_window`: `set_decorations(false)`, `set_fullscreen(true)` unless windowed | Best effort, logs on failure |
| Tray | `console.rs:306-310` removes `tray::TRAY_ID`; `lib.rs:142-155` bypasses close-to-tray in console mode | Console close = process close, so Moonlight sees the session end |
| Navigate to console | `console.rs:317-330`: `window.location.hash = '#/bigpicture'` **and** `ui://navigate` | Belt and braces because the webview may not be listening yet |
| In-game overlay → console | `overlay.rs:1519-1526` `show_big_picture`, `overlay://navigate` | |
| Sunshine app | `commands.rs:4512-4555` `console_plan`, `streaming.rs` `materialize_console_art`, publish 4600-4780 | One `Ludex` app + optional per-game apps (`console::publishing`) |
| Settings | `settings.rs:370-388` (`consoleMode`, `consoleAppName`, `consoleFullscreen`, `consoleExitShortcut`, `consolePerGameApps`), defaults 722-730, sanitize 950-956 | Exposed in Settings → Streaming (`components/settings/streaming-panel.tsx:54-129`) |
| Quit/exit | `commands.rs:4423-4460` `console_mode` / `exit_console` | `exit()` in the screen calls `exit_console` |
| Power verbs | `native.exitConsole/systemSleep/restartApp/quitApp/stopGame` (`lib/native.ts:521-563, 1208`) | `quit_app` is still "pending in the desktop lane" per the code comment at `big-picture.tsx:1122-1135`, with `exit_console` as fallback |
| Launch/stop | `useLaunchGame`, `native.onGameExited`, `native.stopGame` | Console tracks only the session it launched |
| Install/download | `InstallDialog` (desktop modal) from the console, `downloadGame()` in browser | See §7 "assumptions that must die" |
| Update | `UpdateProvider` + `UpdateNotifier` outside the router | Console inherits a desktop banner |
| Tray launch | `lib.rs` single-instance + `tray::show_window` | |

The native layer is genuinely small and well factored: `console.rs` is 606 lines
including 8 unit tests, with the policy decisions extracted as pure functions
(`ConsoleMode`, `second_instance_action`, `publishing`, `command_line`,
`desktop_executable`, `launch_receipt_line`).

---

## 5. Design system

### 5.1 Tokens

Four layers, all in `styles/index.css` + `theme/`:

1. **Theme tokens** — `--bg, --bg-deep, --surface, --surface-2, --elevated, --line,
   --line-2, --ink, --ink-2, --ink-3, --signal, --on-signal, --signal-text,
   --signal-line, --danger, --ok, --warn, --focus, --stage, --stage-ink,
   --stage-ink-2, --stage-ok, --scrim, --shadow, --sheen, --grain-opacity`
   (`styles/themes.generated.css`, generated from `theme/themes.json` by
   `scripts/theme.mjs`; 4 built-ins + custom themes).
2. **Desktop scale** — fixed px: `--text-micro 10` … `--text-hero 40`.
3. **Console scale** — `--bp-u: clamp(0.72px, 0.0521vw, 2.2px)` (1 px at 1920,
   ~2 px at 4K) and `--bp-text-{hero 48,title 34,body 28,meta 24,micro 20}`
   (`index.css:104-129`), exposed as Tailwind `text-bp-*` (`index.css:66-73`).
   Enforced by `styles/type-scale.test.ts` (exact multipliers, ordering, ≥20 px
   floor, no `text-[Npx]` in console files).
4. **Console material tokens** — radius xs/sm/md/lg + pill, `--bp-material-{filter,
   chrome,card,sheet,panel,hover,border,border-strong,shadow}`,
   `--bp-chrome-shadow`, `--bp-wall*`, `--bp-art-*` (`index.css:144-165`).

Geometry: `--bp-bar-h max(44px, 56u)`, `--bp-home-h max(84px, 104u)`,
`--bp-rail-card`, `--bp-pad-x max(44u, safe-area)`, `--bp-lift -2u`
(`index.css:104-107, 521-560, 92`).

### 5.2 Soft vs Studio

`theme/console-appearance.ts`:

- `ConsolePreset = "soft" | "studio"`, **default `studio`** (#237,
  `DEFAULT_CONSOLE_APPEARANCE` 69-74); v1's `soft` default is migrated only when
  the stored record equals the untouched v1 default (`82-122`).
- A preset **no longer owns a palette**: `consoleTokens()` returns `null` unless a
  curated accent is chosen, so both presets inherit the rendered (TV-safe) token
  map from `SettingsProvider` (`console-appearance.ts:219-245`).
- `withConsolePreset` couples a preset to a corner scale (soft→soft, studio→sharp)
  but the radius stays an independent choice afterwards (`198-203`).
- Storage: `ludex.console.appearance`, versioned wrapper `{v:2, appearance}`,
  per-field sanitize, `notifyPreferencesChanged()` on write (`89-183`).

CSS material branch (`index.css:1011-1060, 1157-1440`): Soft = raised, opaque
`elevated/surface-2/surface` plates, `line-2` borders, soft elevation shadow, a
`--bp-wall` of `--stage`; Studio = flat `bg-deep`, hairlines, 2 px corners, no
shadow. Radius choices: sharp 2 px, soft 6/12/18/24, round 10/18/26/34.

Backgrounds: `artwork` (blurred still, 0.45 opacity under an 80 % stage wash —
numbers mirrored in `theme/console-contrast.test.ts`), `solid`, `gradient`.

Accessibility fallbacks are real CSS: `@media (prefers-reduced-transparency: reduce)`
and `@media (prefers-contrast: more)` (`index.css:1382-1440`), asserted by
`styles/console-material.test.ts`.

### 5.3 Focus rings and focus movement

- One ring, drawn by the card (`parts.tsx:167-178`): `ring-2 ring-signal
  ring-offset-2`, `-inset-[4px]`, radius `--bp-radius-sm`, `ring-offset` switches
  between `bg-deep` and `stage` for over-art layouts.
- The global gamepad mark is actively suppressed on cards and `:focus-visible`
  (`index.css:632-640`), because a second outline "reads as a bug at ten feet".
- Every move is one CSS transition on transform/opacity at `--dur-1` (120 ms),
  `--ease-out` (`index.css:563-612`); `[data-bp-instant="true"] *` sets
  `transition: none` for a held/repeated press.
- Rail cards: `rail` scale 1.03 + caption always; `dock` scale 1.12 + lift 4 u,
  caption selected-only; `spotlight` scale 1.08 (`parts.tsx:188-208`).
- Zone items lift `-2u` and reveal a 2 px underline (`index.css:579-605`).

### 5.4 Typography

Bricolage Grotesque Variable (display), Instrument Sans Variable (UI), JetBrains
Mono Variable (code) (`index.css:77-79`). Console uses display for titles at
`clamp(1.15rem, 2-4.8vw, 32-96u)` (8 ad-hoc `text-[clamp(...)]` headings —
`layout-shelf.tsx:75`, `layout-hero.tsx:55`, `layout-dock.tsx:64`,
`layout-grid.tsx:71`, `layout-spotlight.tsx:154`, `layout-compact.tsx:101`,
`settings-app.tsx:109`, `power-app.tsx:40`), mono for every eyebrow/label/count,
and `text-bp-*` for body/metadata. `tabular-nums` via `.num`.

### 5.5 Motion table (`motion.ts`)

15 kinds, each with `duration, exit, ease, x/y/scale/opacity, reduced, cue,
cueAt`. Durations live in a 120–320 ms band; two eases only
(`EASE_OUT [0.16,1,0.3,1]`, `EASE_STANDARD [0.2,0.6,0.2,1]`). Six spring surfaces
(`MOTION_SPRING`): overlay `.30/0`, panel `.34/.12`, gameDetail `.40/.10`,
screenPush `.36/0`, queueRow `.26/0`, launch `.55/.08`. Held input → instant
final frame; reduced motion → 120 ms fade or nothing (`REDUCED_MOTION`). Cues are
declared once per kind and fired once by the surface, which removed the old
double-cue bug (`motion.ts:27-35, 474-488`).

`TransitionHub` (`519-550`) records `startedAt`, `instant`, `interrupted`,
`generation`, `interruptions()`, and hard-codes `accepts(): true` — a transition
never gates input.

### 5.6 Sound

`lib/ui-sound-specs.ts` (240) is the model: 11 cues (`move, confirm, click,
openGroup, closeGroup, error, launch, achievement, installComplete, power,
notification`), per-cue file, gain, `maxVoices`, `essential`, `minGapMs`,
duration, peak, partials. Budget: ≤ 512 KB, peak ≤ −10 dBFS, ≤ 3 partials.
`FROZEN_CUES = ["move","launch"]` pinned by SHA-256 in tests. 11 WAVs, **312 KB
total**, generated by `apps/web/scripts/bp-sounds.mjs`.

`lib/ui-sound.ts` (382) is the manager: guards in order `enabled → volume>0 → no
game running → page visible → reduced-motion allows → decoded → repeat gap →
polyphony (8)`, scheduled `currentTime + 4 ms`, gain `trim × volume²`, compressor
before destination, `GAMEPLAY_KEEPS = {launch}` so the launch cue rings out into
the game. Prefs are device-local (`SoundPrefs {enabled, volume}`, default 45 %).

Haptics: `gamepad-core.ts:481-604` — per-cue patterns, strength scale
(`off/standard/strong`), `error` is two 40 ms pulses, keyboard/mouse never rumble
(`padGesture` flag, `big-picture.tsx:294-300, 1593-1616`).

### 5.7 What is genuinely good and should survive

- The **motion table + interruption hub**: one vocabulary, no per-component
  durations, reduced-motion mapping, held-input semantics, dev probe.
- The **sound subsystem**: generated + hash-frozen assets, real guards, dedupe,
  silence-while-playing, volume² curve, haptics parity.
- The **input → command router** (`input.ts`) and the **hints registry**
  (`controller.ts:245-352`): a context can never advertise a key it does not
  honour; keyboard and pad share one table.
- **Pure geometry** (`layout.ts`, `system-zones.ts`, `power.ts`, `search.ts`,
  `launch-plate.ts`) — 182 DOM-free tests are possible because of this split.
- The **10-foot type scale in `--bp-u`** and its test; the TV-safe 7:1 token map.
- The **controller identity chain** (`controller-identity.ts`,
  `controller-devices.ts`, `controller-resolve.ts`, `controller-fallback.ts`) —
  fingerprint → device entry → account/game/preset → generic fallback, with A/B
  swap and per-family glyphs.
- The **console material CSS contract** (tokens only, static blur, no animated
  `backdrop-filter`, reduced-transparency fallback).

---

## 6. Tests, performance, known jank

### 6.1 Coverage

- **Unit**: 80 test files in `apps/web/src`; 13 in `components/big-picture/`
  with 182 `it()` blocks (`controller` 35, `layout` 30, `motion` 27, `search` 16,
  `system-zones` 16, `input` 13, `fullscreen` 10, plus `power`, `cues`,
  `entrance`, `launch-plate`, `pointer-idle`, `use-controller-prefs`). Plus
  `theme/console-appearance.test.ts` (242), `theme/console-contrast.test.ts`
  (116), `styles/type-scale.test.ts` (88), `styles/console-material.test.ts` (71).
  Everything is pure-logic; **no component render tests, no React Testing
  Library, no DOM tests of the screen**.
- **E2E**: 5 specs (`01-library`, `02-navigation`, `03-ingest`, `04-setup`,
  `05-forms`). Console coverage is a single smoke test:
  open `/`, click "Console mode", expect text "Console", press Escape, expect URL
  not to match `/bigpicture` (`e2e/tests/02-navigation.spec.ts:29-38`). **No test
  opens an overlay, a layout, settings, power, search, the launch plate, or
  console mode.**
- **Desktop**: `console.rs` has 8 unit tests (flag matrix, second-instance
  matrix, session promote, command line quoting, publishing matrix, AppImage
  resolution, receipt size).

### 6.2 Performance shape

- One 2,744-line component owns all console state; every console interaction
  re-renders it, and it re-creates `settingsRows` (a 190-line IIFE),
  `homeDetails`, `viewProps` and the whole render tree each time. No
  memoisation boundary between the bar, the row, the body and the overlays.
- **1 Hz re-render during downloads**: `DownloadsProvider` polls `refresh()`
  every 1,000 ms while jobs are active (`state/downloads.tsx:303-306`) and
  exposes a new `value` object (`:630-657`), which the screen consumes at the top
  (`big-picture.tsx:233`). A 4K console re-renders ~10k lines of JSX every second
  during an install, for a percent label.
- `useViewportWidth()` (`use-layout.ts:92-108`) subscribes to every `resize`
  event and re-renders on any width change (window drag = per-pixel renders).
- Per-selection detail query with no debounce (`:530-535`): holding right on a
  200-game shelf fires a `GET /api/games/:id` per card.
- Layout switch tears down the whole subtree (`<motion.div key={layout}>`,
  `:2520-2532`), re-decoding covers and re-running the entrance witness.
- Two full-screen image layers (`ArtPlate` backdrop + `ConsoleBackdrop`
  wallpaper) with a static `blur(18u)` and `blur(26u)`; the stylesheet is explicit
  that these never animate, and both are disabled under reduced transparency.
- `grain` overlay is one fixed layer with no blend mode (`index.css:319-325`).
- Debug tooling exists and is decent: `?motionDebug=1` frame-budget probe
  (`motion-probe.tsx`, 240-frame window, >17 ms / >33 ms counts, last kind,
  budget, cuts) and `?gamepadDebug=1` panel.

### 6.3 Known jank / risk points

| Risk | Where | Effect |
| --- | --- | --- |
| 1 Hz download re-render | `state/downloads.tsx:303`, `big-picture.tsx:233` | Whole console re-renders each second during installs |
| Detail query per selection | `big-picture.tsx:530` | d-pad walk = request storm, skeleton flicker in Spotlight |
| `useViewportWidth` | `use-layout.ts:92` | Resize storm |
| Single 190-line keyboard effect with 16 deps | `big-picture.tsx:2168-2359` | Re-subscribes on almost every state change |
| Manual DOM focus management (`querySelector` + `data-gamepad-focus`) | `:840-880`, `:1440-1470`, `:2100-2165` | Focus and React state can disagree; ring clearing is a manual `querySelectorAll` sweep |
| `aria-modal` bail-out | `:2169` (keyboard) vs `gamepad.ts:388-412` (pad walks the dialog) | Opening `InstallDialog` from the console swaps the console grammar for a desktop dialog; keyboard is swallowed while the pad is retargeted |
| Route escapes | `navigate('/game/:id')`, `/downloads`, `/collections/:id`, `/remote` | Drops into the desktop shell mid-session |
| Ad-hoc heading clamps | 8 layouts/apps | Titles not on the type scale, so a retune misses them |

---

## 7. Keep / rewrite / delete

Legend: **K** = keep (move, minimal change) · **R** = rewrite (same intent, new
implementation) · **D** = delete.

### 7.1 Web — model and stores

| File | LOC | Verdict | Reason / risk |
| --- | --- | --- | --- |
| `big-picture/motion.ts` | 574 | **K** | Best asset in the console. Only `motion/react` types + cue ids. Risk: none; move to `lib/console/motion.ts` |
| `big-picture/entrance.ts` | 76 | **K** | Reads the motion table |
| `big-picture/cues.ts` | 56 | **K** | Pure command→cue |
| `big-picture/input.ts` | 209 | **K** | The contract behind "B always does X"; needs new contexts for new surfaces |
| `big-picture/layout.ts` | 249 | **R** (partial) | Keep `gridColumns`, `clampIndex`, `resolveIndex`, persistence; **rewrite `stepForLayout`** — six layout-specific direction models are the thing "from zero" should shrink to one or two |
| `big-picture/system-zones.ts` | 232 | **R** (partial) | Keep `ShellLevel`/URL codec, `SETTINGS_SECTIONS`, `wrapIndex`. `zoneAfterMove`/`zonesForLayout`/`homeRowPlacement` encode the 6-layout × 3-zone matrix; rebuild around 2 layouts |
| `big-picture/power.ts` | 138 | **K** | Mode-aware actions, Cancel-first confirm |
| `big-picture/search.ts` | 169 | **R** | Keep the grid maths; the on-screen QWERTY (40 cells) is a 2010s pattern — replace with a 6×5 alphabet or a phone-style entry |
| `big-picture/launch-plate.ts` | 114 | **K** | Honest 8 s ceiling, min hold, dismiss rules |
| `big-picture/fullscreen.ts` | 59 | **K** | Fixes a real trap (#233) |
| `big-picture/pointer-idle.ts` | 79 | **K** | Attribute-only, zero re-render |
| `big-picture/model.ts` | 43 | **R** | Props contract will change with the new tree |
| `big-picture/labels.ts` | 99 | **K** (trim) | Drop copy for deleted presets/layouts |
| `big-picture/navigation.ts` | 271 | **R** | Good behaviour (scroll-into-view, refresh re-resolution) but tied to shelf/index; a new shell wants a flat virtualised index or a single row model |
| `big-picture/use-layout.ts` | 109 | **R** | Store shape is fine; rewrite as one generic device-preference store (see §8) |
| `big-picture/use-controller-prefs.ts` | 135 | **R** | Same |
| `big-picture/use-console-appearance.ts` | 94 | **R** | Same; and the appearance model itself is due a redesign (see §7.6) |
| `big-picture/use-change-witness.ts` | 20 | **K** | |

### 7.2 Web — surfaces

| File | LOC | Verdict | Reason / risk |
| --- | --- | --- | --- |
| `screens/big-picture.tsx` | **2,744** | **D → R** | Delete the file. Its responsibilities split into: shell router/level, input dispatcher, data hooks, and each surface. Rebuilding it in place would just re-grow it |
| `big-picture/parts.tsx` | 510 | **R** split | `MetaLine`/`ProgressLine`/`ShelfTabs`/`HintStrip` are reusable; `RailCard`/`GridCard`/`CompactRow` carry the old card language and the 3-variant focus machinery |
| `big-picture/overlays.tsx` | 485 | **D → R** | A centred 560/900 px sheet with 40 px rows is a desktop dialog, not a 10-foot surface. Rebuild as full-bleed surfaces |
| `big-picture/system-bar.tsx` | 200 | **R** | Right idea (status only, fixed slots, tabular numerals, minute-aligned clock) but it is a desktop status bar at 20u; a console bar needs fewer, bigger, glanceable items |
| `big-picture/home-row.tsx` | 233 | **D → R** | The "6 equal KPI tiles with 01…06 index numbers" grid is the single most desktop-shaped thing in the product. Replace with a hierarchical launcher (one hero + a short horizontal strip) |
| `big-picture/settings-app.tsx` | 251 | **R** | The tab + row-list + facts-dl pattern is a settings page; a console needs a 2-pane or a card carousel with one decision per screen |
| `big-picture/power-app.tsx` | 105 | **K** (restyle) | Big tiles + Cancel-first confirm is correct console grammar |
| `big-picture/profile-overlay.tsx` | 140 | **R** | Content is right; shell must change |
| `big-picture/search-overlay.tsx` | 148 | **R** | See `search.ts` |
| `big-picture/layout-switcher.tsx` | 146 | **D** | With 2 layouts there is nothing to switch; if kept, it is one segmented control |
| `big-picture/layout-preview.tsx` | 159 | **D** | Only exists to preview 6 layouts |
| `big-picture/layout-skeleton.tsx` | 146 | **R** | Keep the idea (per-layout skeleton), collapse to one |
| `big-picture/layout-panel.tsx` | 76 | **D** | Settings duplicate of the switcher |
| `big-picture/layout-shelf.tsx` | 135 | **K/R** | The strongest composition; keep as the default view, rebuilt on the new parts |
| `big-picture/layout-hero.tsx` | 122 | **D** or fold | 90 % shelf with a different crop |
| `big-picture/layout-grid.tsx` | 124 | **K** (as "All games") | The only many-items view; virtualise it |
| `big-picture/layout-compact.tsx` | 124 | **D** | 46 px rows at ten feet; listed as × rows |
| `big-picture/layout-dock.tsx` | 105 | **D** | Exists only to give the home row a second placement |
| `big-picture/layout-spotlight.tsx` | 215 | **D** or fold | Highest complexity, and its "quick actions zone" is a workaround for the home row |
| `big-picture/sound-panel.tsx` | 224 | **K** (restyle) | The controls and store are right |
| `big-picture/motion-probe.tsx` | 110 | **K** | Dev-only, invaluable |
| `big-picture/console-backdrop.tsx` | 37 | **K** | |
| `big-picture/launch-overlay.tsx` | 162 | **K** (restyle) | The FLIP handoff is genuinely console-grade |

### 7.3 Web — theme / CSS

| File | LOC | Verdict | Reason |
| --- | --- | --- | --- |
| `theme/console-appearance.ts` | 258 | **R** | Model is over-parameterised: preset × accent × background × radius = 2×6×3×3, and only the accent is a colour decision. New: 1 palette path + 2-3 materials, no user-facing accent list |
| `theme/derive.ts` (`tvTokens`) | 196 | **K** | 7:1 TV map is a real accessibility win |
| `theme/registry.ts` / `storage.ts` / `validate.ts` / `contrast.ts` | 150/268/174/90 | **K** | Product-wide, tested |
| `styles/index.css` console block | ~440 | **R** | Keep tokens and the "no animated blur" rule; rewrite the component classes (`bp-tile`, `bp-row`, `bp-sheet`, `bp-chrome`, `bp-glass-tile`) for the new tree. `bp-glass-tile` is currently only used by the home-row overlay variant |
| `styles/type-scale.test.ts`, `console-material.test.ts` | 88/71 | **K** | Keep as guardrails for the rewrite |
| `screens/big-picture.tsx` line 329-332 `setTv(true)` | — | **R** | A screen mutating a global token mode is a hack; the console shell should own its token root |

### 7.4 Native

| File | LOC | Verdict | Reason |
| --- | --- | --- | --- |
| `console.rs` | 606 | **K** | Pure, tested, small. Extend: `--console` should also select the shell, not just a route |
| `commands.rs` console block (4423-4460, 4512-4555, 4600-4780) | ~180 | **K** | |
| `settings.rs` console fields | ~40 | **K** | |
| `lib.rs` console wiring (82-215) | ~60 | **K** | Consider a dedicated window label for console |
| `overlay.rs::show_big_picture` | 8 | **K** | Rename target to `/console` when the route moves |
| `window_focus.rs`, `tray.rs` | 620 / — | **K** | |

### 7.5 Docs

`docs/big-picture/28-bp-console.md` (333), `29-bp-layouts.md` (252),
`30-bp-sound.md` (228), `31-bp-motion.md` (87). **K** for 30 and 31 (specs still
true); **R** 28 and 29 after the redesign, since they describe the 6-layout shell
and the home-row graph.

### 7.6 The "good bones" (explicit)

1. **Sound**: `ui-sound-specs.ts` + `ui-sound.ts` + 11 generated WAVs (312 KB) +
   hash-frozen `move`/`launch`.
2. **Motion hub**: `motion.ts` table, `TransitionHub`, `transitionOf`/`exitOf`,
   reduced-motion mapping, `MotionProbe`.
3. **Pipeline**: `state/downloads.tsx` (jobs, phases, aggregate, ETA, resume,
   history) and `lib/install-phase.ts` — the console only reads it.
4. **Controller manager**: `controller-identity/devices/resolve/fallback/profiles`,
   `gamepad-core` haptics, `useGamepad` repeat/hot-plug, `withBigPictureDefaults`.
5. **Ten-foot tokens**: `--bp-u`, `text-bp-*`, `tvTokens`, focus ring rules.
6. **Pure decision modules**: `input.ts`, `system-zones.ts`, `power.ts`,
   `launch-plate.ts`, `fullscreen.ts`, `pointer-idle.ts`.
7. **`gamepad.ts` modal walking** (`walkModal`, `gamepad.ts:388-412`) and
   `hasBlockingModal`/`hasModalDialog` (`gamepad-core.ts:330-360`) — a pad can
   drive a native/desktop dialog without a keyboard. Useful in the new shell too.

### 7.7 The "Big Picture assumptions" that must die (explicit)

| Assumption | Evidence | Why it must go |
| --- | --- | --- |
| **Console is a route, and routes may escape** | `/bigpicture` is a sibling of `/game/:id`; the screen navigates to `/game/:id` (`:2396`, also `:1325`, `:1468`), `/downloads` (`:1369`), `/collections/:id` (`:1362`), `/remote` (`:1963`) | In console mode, pressing "Game page" or "Open downloads" renders the **desktop app** on the TV. A console must own its whole navigation graph |
| **Installing is a desktop modal** | `confirm()` → `setInstallOpen(true)` (`:1194`; also the primary button at `:2375`) → `InstallDialog` (`:2725`, `dialog.tsx:114-115` `role="dialog" aria-modal="true"`). The console keyboard handler returns early on any `aria-modal` (`:2169`); the pad is retargeted into the dialog's focus stops by `gamepad.ts:388-412` (#237) | The flow is *technically* operable from a couch, which is worse than broken: an 11–13 px desktop dialog with no hint strip becomes the install UI at ten feet, and the console's own grammar disappears mid-task |
| **The home row is six equal KPI tiles** | `home-row.tsx:110-137`, `grid-cols-2 sm:grid-cols-3 lg:grid-cols-6`, each tile showing `01…06` + icon + label + micro detail | Six equal tiles with index numbers is a dashboard card grid. A console has one dominant action (Continue/Play) and a short strip |
| **Overlays are centred desktop sheets** | `overlays.tsx:79-108`: `max-w-[560px]`/`max-w-[900px]`, `max-h-[86dvh]`, `p-2` body, 40 px rows (`:144`) | At 3 m a 560 px sheet is a postage stamp; the console needs full-bleed or bottom-sheet surfaces with ≥ 64-72u rows |
| **Tiny type is acceptable if it is "meta"** | `--bp-text-micro` = 20u (20 px at 1080p) used for counts/eyebrows/tab glyphs; 8 headings still on ad-hoc `clamp()` | 20 px at 3 m is unreadable. The floor needs to be ~24u, and the ad-hoc clamps must go or the type test keeps missing them |
| **A forced "Soft" palette** | **Already fixed in #237** (`console-appearance.ts:64-74`, Studio default; `consoleTokens` returns the app tokens unless a curated accent is chosen) | Keep this: never let a console preset own a second palette. But the *material* branch (`data-console-appearance="soft"`) still doubles every component rule and should be reduced |
| **Six layouts are a feature** | `BIG_PICTURE_LAYOUTS` 6 ids, `layout.ts:174-191`, per-layout zone graphs, 6 skeletons, 6 previews, a switcher, a settings panel | Every layout multiplies the navigation matrix and the CSS. A console needs 2 (shelf + grid), each done properly |
| **URL state is the shell's memory** | `?app=settings&section=language` (`system-zones.ts:184-202`) | Needed only because `I18nProvider` remounts. A dedicated shell with a non-remounting locale context removes the need for URL-encoded UI state |
| **The bar is a status dashboard** | `system-bar.tsx:146-197`: 6 slots, 4 breakpoints, `hidden sm:flex` twice | A TV bar should show time, account, connection, and one activity indicator — not six desktop widgets that vanish by breakpoint |
| **Manual DOM focus management** | `querySelector('[data-gamepad-focus]')`, `setAttribute`, `focusSurface`, `[data-bp-card][aria-current]` sweeps (`:840-880`, `:2100-2165`, `cardRect` at 1000) | It works, but it is why the ring can ghost and why a rebuild is cheaper than a refactor |

---

## 8. What "from zero" should mean, technically

### 8.1 Three options, ranked

**A. Separate shell app (`apps/console`), recommended.**
A second Vite entry (or a second Tauri window label `console`) with its own
router, its own provider subset (`QueryClient`, `Auth`, `I18n`, `Settings/Tokens`,
`Downloads`, `Toast`) and **no `AppShell`**. `--console` opens that window; the
`/bigpicture` route is retired to a redirect for one release.

- Pros: removes every desktop coupling at once (top bar, rail, toasts, PWA,
  dialogs, route escapes), lets the console pick its own breakpoints and tokens,
  makes "console mode" a build target instead of a runtime flag, and makes the
  Tauri window policy trivial.
- Cons: two bundles to keep in sync on shared primitives (auth, api client,
  types); CI grows a target.
- Cost driver: shared primitives must be extracted, not copied.

**B. New route tree inside the SPA (`/console/*` with its own layout route).**
`<Route path="/console" element={<ConsoleShell/>}>` with child routes
`index, game/:id, downloads, settings, search, collections`, and a `<Navigate>`
guard that sends console-mode users to `/console` and desktop users away from it.

- Pros: one bundle, one build, server-rendered nothing to change; incremental
  migration possible; fixes the route-escape bug immediately (`/console/game/:id`
  renders a console game surface, not the desktop page).
- Cons: still shares the provider tree, still inherits `PwaPrompts`,
  `UpdateNotifier`, `OverlayHost` unless moved; the bundle keeps the desktop code
  in memory.

**C. Rewrite `screens/big-picture.tsx` in place.**
Explicitly **not** recommended: 2,744 lines with 30 effects will re-grow. Any
in-place rewrite reproduces the same coupling.

### 8.2 What "from zero" should preserve vs replace

Preserve (move, don't rewrite):

- `lib/ui-sound*`, `public/sounds/bp/*`, `scripts/bp-sounds.mjs`
- `components/big-picture/motion.ts` + `entrance.ts` + `cues.ts`
- `lib/gamepad*.ts`, `lib/controller-*.ts`, `lib/use-controller-devices.ts`
- `theme/derive.ts::tvTokens`, `theme/registry|storage|validate|contrast`
- `state/downloads.tsx`, `lib/install-phase.ts`, `lib/eta.ts`
- `components/big-picture/{input,power,launch-plate,fullscreen,pointer-idle,use-change-witness}.ts`
- `apps/desktop/src-tauri/src/console.rs` and the console commands
- The token contract + `type-scale.test.ts` + `console-material.test.ts`

Replace:

- The screen (split into: `ConsoleRouter` / `ConsoleShell` / `useConsoleInput` /
  per-surface route components)
- `home-row`, `overlays`, `system-bar`, `settings-app` (new grammar below)
- 6 layouts → 2 (`shelf`, `grid`), with the grid virtualised
- `navigation.ts` shelf/index model → a flat index + one "row" concept
- `system-zones.ts` zone matrix → 2 zones (`chrome`, `content`) or a simple stack
- Console appearance: preset × accent × background × radius → **1 palette path**
  (theme tokens + TV-safe map) × 2-3 materials × 3 radius steps, with **no
  user-facing accent picker**
- The three duplicated preference stores → one `createDeviceStore<T>` +
  `useDeviceStore`

New surfaces the console is missing (from the original design doc, still true):
install flow (location, size, progress, cancel) as a console surface; storage
management; account/server switch; notifications; a real back stack with a
guaranteed non-dead-end.

### 8.3 Target shape (option A/B, same insides)

```
console/
  shell.tsx            # tokens root, backdrop, <Outlet/>, level stack, TV mode
  router.tsx           # /console, /console/game/:id, /console/settings/*, /console/downloads
  input/
    dispatch.ts        # pad + keyboard → Command (from input.ts)
    hints.ts           # per-context hints (from controller.ts)
    focus.ts           # one focus store, no querySelector
  surfaces/
    home.tsx           # hero "Continue" + horizontal strip (not 6 KPI tiles)
    library.tsx        # shelf (default) + grid (virtualised)
    game.tsx           # detail + install/launch/stop/update in-shell
    settings/          # one decision per screen
    power.tsx
    search.tsx
    downloads.tsx
  data/                # query hooks specific to console
  styles/console.css   # tokens + materials, no desktop classes
```

Rules that should be enforced from day one:

1. **No `navigate()` to a desktop route.** The console router owns every path it
   can reach; anything else is a surface inside `/console`.
2. **No `aria-modal` desktop dialog inside the console.** Every install/confirm
   flow is a console surface; input is never silently disabled.
3. **One focus store.** Selection lives in React state; DOM focus follows it in
   one place (a `useFocusRing` hook), not via `querySelectorAll` sweeps.
4. **Per-surface re-render boundaries.** Download progress must not re-render the
   library: subscribe to `activeCount`/`aggregate.percent` in the bar only, or
   move progress to a `useSyncExternalStore` slice.
5. **Type floor 24u**, every heading from `text-bp-*`, test-enforced.
6. **Two layouts**, and the layout choice is a device preference with one store.
7. **Token root per shell**, not `setTv()` on a global provider.

### 8.4 Rough sequencing (if executed)

1. Extract shared primitives (`lib/console/{motion,sound,input,controller,power,launch}`)
   and the one device store; no UI change. Guard: existing 182 tests still pass.
2. Stand up `ConsoleShell` + router + tokens + backdrop behind `/console`
   (feature-flagged); port `home` + `library/shelf` + `power`. Delete nothing yet.
3. Add `game` + install/download surfaces inside the shell; remove the
   `InstallDialog` call from the console; add `downloads` as a console screen.
4. Port settings (one decision per screen), search (new keyboard), collections.
5. Flip `--console` and the nav item to the new shell; keep `/bigpicture` as a
   redirect for one release.
6. Delete `screens/big-picture.tsx`, `home-row.tsx`, `overlays.tsx`, 4 layouts,
   `layout-switcher/preview/panel`, `system-bar.tsx`, `settings-app.tsx` (old),
   the console CSS component classes and the 3 duplicated stores.

### 8.5 Risk register for the rewrite

| Risk | Mitigation |
| --- | --- |
| Losing the tested input semantics | Port `input.ts` + `controller.ts` hints unchanged; keep their tests as the spec |
| Losing motion feel | Port `motion.ts` verbatim; the table is the spec, the components are consumers |
| Regressing TV contrast | Keep `tvTokens` + `console-contrast.test.ts`; assert the new shell root uses the TV map |
| Sound regressions | Keep asset hashes; keep the guard order test |
| Two bundles drifting | Shared `lib/api`, `lib/types`, auth and downloads stay in the shared package; only surfaces fork |
| Console mode still reachable from desktop | Keep the desktop "Console mode" button pointing at the new shell; keep `--console` promote path |
| Losing the launch handoff | Port `launch-plate.ts` + `launch-overlay.tsx` (only the visual shell changes) |

---

## Appendix — file:line index of the most-load-bearing code

- Route + gate: `apps/web/src/app.tsx:135-169`, `:230-249`, `:288-295`
- Provider tree: `apps/web/src/app.tsx:256-455`
- Screen entry: `apps/web/src/screens/big-picture.tsx:211`
- Layout registry (exhaustive): `:174-181`
- TV mode: `:329-332`; pointer idle `:338`; sound preload `:367-392`
- Console status: `:494-508`; fullscreen policy `:747-812`; exit `:814-826`
- Refresh policy: `:660-740`
- Zones/focus: `:830-960`, `:2100-2165`
- Launch: `:991-1090`
- Command router: `:1409-1487`; pad dispatch `:1510-1616`
- Settings rows: `:1706-1900`; home row model `:2039-2090`
- Keyboard handler: `:2168-2359`
- Render: `:2470-2743`
- Motion table: `components/big-picture/motion.ts:207-372`; springs `:78-91`;
  reduced motion `:155-171`; hub `:519-550`
- Input router: `components/big-picture/input.ts:20-209`
- Hints: `components/big-picture/controller.ts:123-352`
- Layout geometry: `components/big-picture/layout.ts:108-223`
- Zone graph: `components/big-picture/system-zones.ts:57-129`
- Sound model: `lib/ui-sound-specs.ts:88-201`; manager guards `lib/ui-sound.ts:1-40`
- Haptics: `lib/gamepad-core.ts:481-604`
- Console appearance: `theme/console-appearance.ts:25-258`
- TV tokens: `theme/derive.ts:158-172`
- Materials/CSS: `styles/index.css:1000-1440`
- Native: `apps/desktop/src-tauri/src/console.rs:22-330`;
  commands `src/commands.rs:4423-4460, 4512-4555`;
  settings `src/settings.rs:370-388`;
  wiring `src/lib.rs:82-215`;
  overlay → console `src/overlay.rs:1519-1526`
