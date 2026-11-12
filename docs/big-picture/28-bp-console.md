<!-- Suggested title: [v0.5] Big Picture as a complete console shell: system bar, home row, settings, power -->

## Goal

A player holding only a controller can sit down in front of Ludex and never need the desktop again: install, launch, stop, update, change the picture and the sound, switch account or server, read a notification, and end the session — all from the couch, in the same hairline / ink / signal language Big Picture already speaks.

This is not a skin. Today Big Picture is a full-screen launcher with five overlays. A console shell is a persistent system layer with real system apps and one back stack that cannot dead-end.

## The bar for “complete”

Without a keyboard, a mouse or a desktop window, the player can:

1. see who is signed in, on which server, online or not, the time, and the pad's battery;
2. reach every system function from a home row instead of a desktop route;
3. open a settings app and change layout, display scale, sound, controller, downloads, storage, account/server and updates;
4. manage installs: install, cancel, pause, retry, update, verify, repair, uninstall;
5. read and clear notifications;
6. switch account or server;
7. end the session, quit, restart or sleep with a confirmation that defaults to Cancel;
8. always know what B does, and never get trapped in a dialog.

## Current state

### What already works

| Capability | Where |
| --- | --- |
| One input → command router for keyboard and pad | `components/big-picture/input.ts:48-206` |
| Gamepad polling, repeat, hot-plug, optional haptics | `lib/gamepad.ts:104-300`, `lib/gamepad-core.ts:220-243` |
| Launch an installed game, playtime recorded on exit | `screens/big-picture.tsx:432-447`, `lib/hooks.ts:139-155` |
| Install / download start, live progress | `screens/big-picture.tsx:423-430`, `state/downloads.tsx:120-131` |
| Couch search with an on-screen keyboard | `components/big-picture/search-overlay.tsx`, `search.ts:20-56` |
| Controller profile choice and haptics toggle | `components/big-picture/profile-overlay.tsx:21-111` |
| Layout picker and persistence | `components/big-picture/layout-switcher.tsx:59-138`, `use-layout.ts:38-90` |
| Exit confirmation | `components/big-picture/overlays.tsx:242-282` |
| Console mode as a single Sunshine app | `apps/desktop/src-tauri/src/console.rs:18-24, 140-169`, `#172` |

### What is missing, and where the ceiling is

| Capability | Today | Evidence |
| --- | --- | --- |
| Install | Opens a modal dialog the pad cannot operate | `screens/big-picture.tsx:443`, `components/game/install-dialog.tsx:202`, `components/ui/dialog.tsx:114-115`, `lib/gamepad.ts:264-303` |
| Cancel / pause an install | Navigates out of Big Picture | `screens/big-picture.tsx:440-441, 567-572` |
| Updates | Only the desktop Update center | `screens/updates.tsx:11-38` |
| Uninstall / verify / repair | Only on the game page | `screens/game-detail.tsx:453`, `components/game/install-files.tsx:35-110` |
| Settings | Navigates to the desktop screen | `screens/big-picture.tsx:556-558`; Big Picture is outside `AppShell` (`app.tsx:277-283`); `screens/settings.tsx:104-133` |
| Sound | Nothing exists | no `AudioContext` / `new Audio` in `apps/web/src` |
| Account | Invisible: no `useAuth` in Big Picture | `screens/big-picture.tsx` (absent) |
| Server | Switcher only in the desktop shell | `components/shell/server-switcher.tsx:55-90`, `components/shell/app-shell.tsx:213` |
| Notifications | Bell only in the desktop shell | `components/community/notification-bell.tsx:63-70`, `app-shell.tsx:210` |
| Friends / community / achievements | Desktop routes only | `lib/navigation.ts:164-180`, `lib/api.ts:873-997` |
| Power | Exit confirmation only; no quit command outside console | `overlays.tsx:242-282`, `commands.rs:3946-3951` |
| Clock | Exists, hardcoded `en-GB` | `screens/big-picture.tsx:745` |
| Battery / network / storage | No readout | `lib/types.ts:1434-1441` (battery), `lib/types.ts:467-472` (disk) |
| Controller setup | Pick a profile, no test, no deadzone preset | `profile-overlay.tsx:21-111` |
| Media / apps | Nothing | — |

### Four things that are actively broken at ten feet

1. **The install path dead-ends.** `confirm()` opens `InstallDialog` on native (`big-picture.tsx:443`), which renders an `aria-modal` dialog (`ui/dialog.tsx:114-115`). The pad loop detects that modal and stops delivering every action except a synthesized Escape on B (`gamepad.ts:264-303`). A and the d-pad never reach the dialog, so a controller-only player can open Install and dismiss it, but cannot choose a version or press Install.
2. **The exit copy lies in console mode.** The overlay promises “Downloads and installs continue in the background” (`overlays.tsx:255`), but `exit()` calls `exitConsole()` (`big-picture.tsx:409-417`), which is `app.exit(0)` (`commands.rs:3946-3951`). Downloads die with the process. The same overlay says “Exit Big Picture” while the header button says “Quit” (`big-picture.tsx:879`).
3. **Settings is a desktop trip.** `/settings` mounts the rail, the 1240 px layout and 15 px UI (`settings.tsx:104-133`). Only Big Picture calls `setTv(true)` (`big-picture.tsx:168-171`), so the TV palette — higher text contrast, a stronger focus colour, no grain (`theme/derive.ts:158-172`) — is gone, and the selection is lost on the way back.
4. **Escape and B disagree.** Escape leaves the whole screen (`input.ts:171-174`), B opens a confirmation (`input.ts:104-106`). At ten feet, the two “back” gestures do different things.

## Design

### The shell, in one picture

```text
┌ system bar — status only, not focusable ─────────────────────────────────┐
│ ◼ LUDEX  BIG PICTURE   @cleeryy · home   ● online   2 ↓   4 ◔   21:48  ⏻ │
├ home row — focus zone A ─────────────────────────────────────────────────┤
│  [Games] [Search] [Downloads] [Updates] [Controller] [Settings] [Power]  │
├ library — focus zone B ──────────────────────────────────────────────────┤
│  the active layout (shelf / hero / grid / compact / dock / spotlight)    │
├ hint strip — not focusable ──────────────────────────────────────────────┤
│  Ⓐ play   Ⓑ back   ✛ move   LB/RB shelf   MENU quick actions            │
└──────────────────────────────────────────────────────────────────────────┘
```

Five rules make it a shell rather than a chrome bar:

1. **The system bar is a camera, not a dashboard.** Nothing in it joins the focus graph. Each pointer affordance has a pad path in the home row or the quick menu. Six extra focus stops above the shelf would make Up ambiguous and buy nothing at three metres.
2. **Zones, not shelves, own Up/Down.** Left/right moves inside a row. Up/down crosses zones. LB/RB changes shelf. The shelf tabs stay a passive strip with no focus stop (`parts.tsx:92-97`), so Up from the first row lands on the home row, not on a tab. This is the one model that works for both console layouts in `29-bp-layouts.md`; `stepForLayout` (`layout.ts:146-163`) keeps owning intra-row geometry.
3. **No `aria-modal` inside Big Picture.** The pad loop treats `[role="dialog"][aria-modal="true"]` as a dead zone (`gamepad-core.ts:336-343`, `gamepad.ts:264-303`). Every confirm is a non-modal `OverlayShell` panel with a highlighted row and a Cancel default, like today's exit overlay (`overlays.tsx:23-31`).
4. **B is always one step, and never zero.** Overlay → app → library. B never unmounts the console.
5. **Nothing is pointer-only.** Hover may preview a highlight; it may never be the only way to reach an action.

### 1. System bar

Replace the 64 px header (`big-picture.tsx:837-882`) with one strip at `bg-bg-deep`, `border-b border-line`, height `calc(var(--bp-u) * 64)`.

| Slot | Content | Source | Pad path |
| --- | --- | --- | --- |
| Start | wordmark + `BIG PICTURE` mono label | `components/brand`, `big-picture.tsx:838-839` | — |
| Identity | avatar + display name + server name, each `max-w-[18ch] truncate` | `api.me()` (`api.ts:290`), `api.config()` (`api.ts:252`) | Quick menu → Account |
| Network | dot + `Online` / `Offline` | `navigator.onLine` + `online`/`offline` events | Quick menu → Network |
| Activity | downloads chip: `2 ↓ · 61%` | `useDownloads` (`state/downloads.tsx:107-134`) | Home row → Downloads |
| Social | unread count | `api.notifications(10)` (`api.ts:954`) | Quick menu → Notifications |
| Input | pad family + battery | `useGamepad` + `OverlayController.battery` (`types.ts:1434-1441`) | Quick menu → Controller |
| Clock | `HH:MM`, locale-aware | `Intl.DateTimeFormat(getIntlLocale())` (`i18n.tsx:262`) | — |
| Power | power glyph | — | Home row → Power |

Details that must not be skipped:

- Every figure uses `.num` (`styles/index.css:360-364`) and a fixed `min-width` so the bar never reflows when a percentage changes.
- The clock is `<time dateTime=…>` with `aria-live="off"`; it updates on the next minute boundary, not every 30 s (`big-picture.tsx:289-292` currently drifts by up to 30 s). Replace `"en-GB"` with the active locale.
- Battery is hidden when the listener reports none; never fake a value in the browser.
- Offline is a state, not an error: the shelf keeps its cached data (the refresh already tolerates a failed round trip, `big-picture.tsx:294-320`), and actions that need the server are disabled with the reason in `detail`, not a toast.

### 2. Home row

One component, three placements, one item list.

| `placement` | Layout | Shape |
| --- | --- | --- |
| `strip` | shelf, hero, grid, compact | under the system bar, 96 px, icon 28 + label 10.5 |
| `dock` | dock | bottom, above the hint strip, 112 px, icon 32 + label 11.5 |
| `overlay` | spotlight | top-right, no plate, icon 22 + label 10 |

Items (P1): `games`, `search`, `downloads`, `updates`, `controller`, `settings`, `power`. P2 adds `library-filters`, `storage`, `account`. P3 adds `notifications`, `friends`, `achievements`, `community`.

Rules:

- Each item is a `<button data-gamepad-target="true">` inside `<nav aria-label={t("System")}>`; the accessible name includes live status (`t("Downloads, {count} active", …)`).
- The row wraps at both ends. `clampIndex` (`layout.ts:113-121`) is right for a shelf position and wrong for a short ring of system functions; add `wrapIndex` in `system-zones.ts`. Wrapping removes the dead end at the last icon, and the row is short enough that wrap is predictable.
- A tile that has no backing feature is not rendered. Follow `availableSettingsSections` (`lib/settings-sections.ts:236-252`): availability filters the list, so no dead entries ever ship.
- No layout animation on focus. Scale and opacity only, 150 ms, matching `RailCard` (`parts.tsx:190`).

### 3. Quick menu (Home / Guide)

Replaces `CommandMenu` (`overlays.tsx:199-238`), keeps `COMMAND_ITEMS` as the seed and `OverlayRow` as the row.

- Anchored to the inline start, full height minus the system bar and hint strip, width `calc(var(--bp-u) * 420)`, `border-e border-line-2`, `bg-bg-deep`, scrim `bg-scrim/60` over the rest of the screen (click closes). A centred 560 px box reads as a dialog at ten feet; a quick menu is a persistent surface.
- Rows are 56 px, 11 maximum, grouped by mono labels `Go` / `Library` / `System`.
- Each row carries a live status line, never a static description: `Play · installed`, `Downloads · 2 active, 61%`, `Updates · 3 available`, `Notifications · 4 unread`, `Controller · DualSense 72%`, `Account · @cleeryy on home`, `Power · end session`.
- `HintContext` gains `"quick"` (`controller.ts:101-109`, `hintsFor` at `194-264`) so the strip advertises only what the menu honours.
- A second press of Home/Guide, B or Start closes it. `L` still opens the layout picker (`input.ts:197-199`).

### 4. Settings app

A full-bleed surface inside Big Picture, not a route. Implement as `?app=settings&section=<id>` so it deep-links and the browser Back button works, while overlays stay local state.

**Shape: one axis.** A horizontal category strip under the title (same affordance as `ShelfTabs`, `parts.tsx:98-148`) and one vertical stack of rows. No left column: a two-axis settings screen is the classic console trap, because Left/Right must both change a value and change columns.

```text
Settings
Appearance   Display   Sound   Controllers   Downloads   Network   Account   System
──────────────────────────────────────────────────────────────────────────────────
Layout                          Shelf  Dock  Spotlight  Hero  Grid  Compact
Interface scale                 1.00×                    ◀ ─────●───── ▶
Safe-area preview               Off
Reduce motion                   Follow the system
```

Five controller-native row primitives, all one focus stop each:

| Primitive | Keys | Semantics |
| --- | --- | --- |
| `SettingToggle` | A | wraps the existing `Switch` (`ui/controls.tsx:139-171`) |
| `SettingChoice` | ←/→, A | radio group with roving tabindex; one stop, values announced |
| `SettingSlider` | ←/→, up/down leaves | `role="slider"`, `aria-valuemin/max/now`, `aria-valuetext` (`"45 percent"`) |
| `SettingAction` | A | runs; destructive opens a confirm |
| `SettingReadout` | — | not focusable; a fact, not a control |

Sections and rows for v1. Every row is backed by an existing API or store; nothing is a placeholder:

| Section | Rows | Data |
| --- | --- | --- |
| Appearance | Layout (6 cards, horizontal, previews), Interface scale (0.90–1.15), Safe-area preview, Reduce motion readout | `use-layout.ts:80-90`, `useSettings` |
| Display | Fullscreen, Overscan guide, TV contrast readout | `@tauri-apps/api/window`, `state/settings.tsx:123-136` |
| Sound | UI sounds, Volume (0–100, 5 % steps), Test move/confirm/error, Haptics, Strength | `30-bp-sound.md` |
| Controllers | Active pad + battery, Profile, Vibration test, Deadzone preset, Button test | `controller.ts:382-437`, `gamepad.ts:104-300` |
| Downloads & storage | Queue with pause/resume/cancel/retry, Bandwidth, Install root readout, Free space, Verify library | `state/downloads.tsx:120-131`, `lib/install.ts:76-78` |
| Network & server | Status, Switch server, Add server (text keyboard), Sign out | `lib/servers.ts:22-60` |
| Account | User, Avatar, Switch account, Sign out | `state/auth`, `api.ts:290` |
| System | Version, Check for updates, Channel, Restart, Licences, About | `components/updates/update-provider` |

Two hard constraints:

- **No OS dialogs.** The folder picker used by Settings → Storage is a native dialog (`lib/native.ts:185-188`, `locations-panel.tsx:156`); inside the console the install root is a readout with a list of known locations, and changing it stays on the desktop. Account and server text entry use the on-screen keyboard (a `text` layout with shift and symbols must be added to `search.ts:20-56`, which currently has neither and cannot type a password).
- **No dead ends.** A row that cannot work offline says so in `detail` and stays focusable; it does not disappear mid-focus.

### 5. Power

| Action | Console mode | Desktop | Implementation |
| --- | --- | --- | --- |
| End session | ends the stream, returns to the host desktop | — | `exit_console` (`commands.rs:3946-3951`, `native.ts:1026`) |
| Quit Ludex | same | exits the process, not the tray hide | new `quit_app` command; tray already quits with `app.exit(0)` (`tray.rs:127`) |
| Restart Ludex | restarts the app | restarts the app | `restart_app` (`commands.rs:559-561`, `native.ts:430`) |
| Sleep this PC | P2, best effort per OS | P2 | new `system_sleep` with a capability probe |
| Cancel | default | default | highlighted index starts on Cancel, like today (`big-picture.tsx:484-487`) |

Copy is mode-aware: console says “Quit to desktop. Downloads stop.”, desktop says “Close Big Picture. Downloads keep running.” Never claim downloads survive a process exit. This fixes `overlays.tsx:255`.

### 6. Notifications

- `useNotifications()` wraps `api.notifications`, `readNotification`, `readAllNotifications` (`api.ts:954-964`), polls at 60 s like the bell (`notification-bell.tsx:63-70`), and is shared by the bar, the quick menu and the app.
- Toasts already render outside the route tree (`app.tsx:417`), so they appear over Big Picture — but at desktop size and position (`ui/toast.tsx:14`). Set `data-bp="true"` on `<html>` in the same effect as `setTv` (`big-picture.tsx:168-171`), remove it on unmount, and add one CSS block that moves the viewport to the top end below the system bar at `--bp-u` scale. No new toast system.
- `role="status"` is already on each toast (`ui/toast.tsx:30`); keep it, and cap the stack at three (`state/toasts.tsx:38-44` already slices).
- Achievement and install-complete cues land in `30-bp-sound.md`.

### 7. Back stack

| Where you are | B / Backspace | Escape |
| --- | --- | --- |
| Library | opens Exit/Power confirm | browser build: leaves Big Picture (keeps `e2e/tests/02-navigation.spec.ts:26-29` green). Native console: opens the power confirm; the F12 chord still quits immediately. |
| Home row | returns to the library at the remembered index | leaves Big Picture |
| Quick menu | closes it | closes it |
| Settings / app | pops one level (`navigate(-1)` for URL apps) | same |
| Confirm | Cancel | Cancel |
| In-game overlay | closes the overlay | closes the overlay |

`system-zones.ts` owns this as a pure reducer: `Level = { kind: "library" } | { kind: "app"; id } | { kind: "overlay"; id }`, `pushLevel`, `popLevel`, `levelFromSearch`. Escape keeps its direct exit **only at base in the browser build**; everywhere else, and in a native console session, it is a back or a confirm. That is the smallest change that satisfies the console model without breaking the keyboard contract.

### 8. Focus and input rules

1. One ring: `FocusRing` (`parts.tsx:156-163`) and the global `[data-gamepad-focus]` outline (`styles/index.css:135-138`). Nothing new.
2. Every new focusable carries `data-gamepad-target="true"`; overlay rows keep `data-overlay-active` (`overlays.tsx:113-114`) so DOM focus follows the highlight (`big-picture.tsx:644-652`).
3. Entering an app focuses its first row; leaving it calls `nav.focusSelected()` (`big-picture.tsx:492-499`).
4. Refused actions are loud: `confirm()` currently returns silently when `!selected` (`big-picture.tsx:433`). Play the error cue, show a one-line reason, keep focus.
5. Pointer is additive: `onPointerEnter` may set the highlight, never the action.
6. No focus trap, anywhere.

### 9. Scale and TV-safe bounds

Big Picture typography is fixed px tuned for a 1080p window (`big-picture.tsx:839-882`, `parts.tsx:402`), and `setTv` only lifts contrast. At 4K/100 % the shell is unreadable from a couch.

Add to `:root` (`styles/index.css:45-62`):

```css
--bp-u: clamp(0.8px, 0.0521vw, 2.2px);                       /* 1px at 1920, 2px at 3840 */
--bp-gutter-x: max(calc(var(--bp-u) * 56), env(safe-area-inset-left));
--bp-gutter-y: max(calc(var(--bp-u) * 28), env(safe-area-inset-top));
```

| Viewport | `--bp-u` | Bar | Home row | Body text |
| --- | --- | --- | --- | --- |
| 1280×800 | 0.8 | 51 | 77 | 11 |
| 1920×1080 | 1.0 | 64 | 96 | 14 |
| 2560×1440 | 1.33 | 85 | 128 | 19 |
| 3840×2160 | 2.0 | 128 | 192 | 28 |

- Align the existing layouts to `--bp-gutter-x` (one class each in `layout-*.tsx`); the header and the shelf must share an edge.
- Set `color-scheme: dark` on `<html>` when the resolved theme is dark, in `applyAppearance` (`state/settings.tsx:135-136`); native scrollbars and inputs currently ignore the theme.
- `overscroll-behavior: contain` on the settings scroll pane and every overlay body. `touch-action: manipulation` is already global (`styles/index.css:821-826`).

### 10. States

Every surface needs four states, with copy that says what to do next:

| Surface | Empty | Loading | Offline / error | Disabled |
| --- | --- | --- | --- | --- |
| Library | existing (`big-picture.tsx:807-814`) | existing skeleton | existing retry (`791-806`) | — |
| Downloads | “No downloads running.” (exists) | queued rows | per-job error + Retry | pause on a stream-failed job |
| Settings | — | row skeletons | readout with reason | row focusable, `detail` explains |
| Notifications | “Nothing new.” | 3 skeleton rows | “Notifications need a reachable server.” | — |
| Friends / achievements | “No friends yet” / “No achievements yet” | skeleton | same as notifications | — |

## Scope

### P1 — the shell (one release, 3 PRs: 1–2, 3–5, 6–8)

1. `system-zones.ts` + `system-zones.test.ts`: zone order per layout, wrap, level reducer, `levelFromSearch`. No UI.
2. Scale tokens and gutter alignment in `styles/index.css` and the four `layout-*.tsx`; `color-scheme`.
3. `system/SystemBar.tsx`: status only, locale clock, network, downloads chip, pad battery, unread count. Replaces the header in `big-picture.tsx:837-882`; keeps the exact `Big Picture` label for the e2e (`02-navigation.spec.ts:26-29`).
4. `system/HomeRow.tsx` with the seven P1 items, `strip` placement; `move` gains a zone return in `navigation.ts`; `hintsFor` gains `"quick"` and the library list drops `"page"` to stay at six.
5. `system/QuickMenu.tsx` replacing `CommandMenu`; same destinations, live status, left-anchored.
6. `system/PowerOverlay.tsx` + `quit_app` command + mode-aware copy; fix `overlays.tsx:255`.
7. Controller-native install: replace `InstallDialog` in Big Picture with a non-modal `InstallSheet` (version choice, size, free space, Start/Cancel) built on `OverlayShell` + `OverlayRow`. Keep `InstallDialog` for the game page.
8. Move `useNativeEvents()` from `components/shell/app-shell.tsx:270` to `app.tsx` inside the router, so Big Picture gets game-exit, install and playtime events. This also feeds the sound rule in `30-bp-sound.md`.

Tests: `system-zones.test.ts`, `layout.test.ts` (zone step per layout), `controller.test.ts` (quick hints), one Playwright spec that walks the shell with the keyboard and asserts the URL.

### P2 — the apps (4 PRs)

1. Settings app + five primitives; Appearance, Display, Controllers, System.
2. Downloads & updates app in-console; per-row actions; update all; verify library.
3. Library management panel for the selected game: install/update/verify/repair/uninstall with a confirm; reuse `installBridge.verifyInstall` (`install-files.tsx:69`) and `native.uninstallGame` (`game-detail.tsx:453`).
4. Account / server / storage: `listServers` + `activateServer` (`lib/servers.ts:22-60`), sign-out and a BP sign-in with the text keyboard; storage readouts via `installBridge.diskUsage` (`lib/install.ts:76-78`). Network and Account home-row items.

Tests: settings reducers, download row actions, one e2e that changes layout, bandwidth and server without leaving `/bigpicture`.

### P3 — the social layer (2 PRs)

1. Notifications app + TV toast placement + unread→read flow.
2. Friends (presence/activity), achievements (`api.meAchievements`, `api.friendsPresence`), and Sleep behind a capability probe.

## Acceptance

- [ ] From `ludex --console`, the first screen is Big Picture with the system bar showing account, server, network, clock and pad battery; no desktop window appears.
- [ ] With only a controller: Up enters the home row, Left/Right wraps, Down returns to the same game, A opens each app, B returns one level and never closes the app.
- [ ] Settings opens inside Big Picture; changing the layout or the interface scale applies immediately and keeps the selected game.
- [ ] An uninstalled game installs end to end with the pad: version choice, progress in the bar, and the job visible in the downloads app.
- [ ] A running install can be paused, resumed, retried or cancelled from inside Big Picture.
- [ ] An installed game can be verified and uninstalled from inside Big Picture, with a confirmation that defaults to Cancel.
- [ ] Account and server can be switched from the couch; the library reloads and Big Picture stays mounted.
- [ ] Notifications show an unread count, open in-console, and clear.
- [ ] Power offers End session / Quit / Restart / Cancel (Sleep in P3); the copy matches what actually happens in console mode.
- [ ] In the browser build, Escape leaves Big Picture at base and goes back inside an app; in a native console session it opens the power confirm. The existing navigation e2e passes.
- [ ] 1080p, 4K/100 % and 16:10 show no clipped bar, no overlapping toast, no content under the hint strip.
- [ ] `pnpm --filter @ludex/web check`, `test`, `build`; `cargo test`; i18n gate (`ci.yml:101-112`) green.

## Interface rules this design depends on

From the Web Interface Guidelines (fetched 2026-11-12), applied rather than restated:

- **Focus states:** every new control uses `:focus-visible` / `[data-gamepad-focus]`, never `outline-none` without a replacement (`styles/index.css:128-138`).
- **Async updates:** toasts are `role="status"` (`ui/toast.tsx:30`); counts are text, never colour-only.
- **Animation:** `transform` / `opacity` only, explicit property lists, reduced motion honoured at the token level (`styles/index.css:836-849`).
- **Typography and locale:** `…` for loading, curly quotes, `.num` for figures, `Intl.DateTimeFormat` with `getIntlLocale` instead of `en-GB`.
- **Safe areas:** `env(safe-area-inset-*)` on the bar and overlays; `overscroll-behavior: contain` on every scroll pane.
- **Navigation and state:** system apps are URL state (`?app=`, `?section=`); destructive actions confirm, with Cancel as the default.
- **Anti-patterns refused:** no `transition: all`, no clickable `<div>`, no unlabelled icon button, no `user-scalable=no`.

## Out of scope

- Media and third-party app launching: there is no app registry or media source; the tile ships only when a real one exists.
- Full controller remapping inside the console; the desktop editor stays the place for per-button maps.
- OS shutdown, hibernate or lid behaviour; only Sleep, best effort.
- Native folder pickers inside Big Picture; the console never opens an OS dialog.
- Per-account layouts (the preference stays per device, `layout.ts:19-24`).
- Any theme, palette or typeface change.
- Removing the desktop Settings screen, or making the console the default on a desktop.

## Risks

| Risk | Mitigation |
| --- | --- |
| Up/Down stops switching shelves in `shelf`/`hero` (`layout.ts:151-155`) | LB/RB already is the advertised shelf control (`parts.tsx:139-145`); the hint strip keeps it, the changelog names the change |
| `activateServer` calls `retryBoot()` and clears the query cache (`server-switcher.tsx:56-70`) | verify the Gate re-renders without unmounting `/bigpicture`; if it does, re-enter the same `?app=` level after boot |
| A settings write races a download | reuse the per-control save pattern from `overlay-panel.tsx:51-66` (disable the row while saving, toast on failure) |
| New strings inflate the catalog | run `pnpm --filter @ludex/web i18n:write`; the gate is mandatory |
