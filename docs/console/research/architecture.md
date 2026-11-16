# Ludex Console 2.0 + rebrand — architecture review

**Baseline:** HEAD `1a32205` (`chore(release): v0.4.17`, 2026-11-16) **plus an uncommitted working tree observed during the review**. The tree is dirty and moving: a downloads-selector lane is staged, route-level lazy loading is in progress, the mock/ESLint work is staged, and `.opencode/skills/console-mode-ui/SKILL.md` now defines the Console 2.0 contract. Line numbers are from the current files; the selector work is treated as **landing**, not as a recommendation.

**Scope:** `apps/web` (React shell, Console, providers, data), `apps/desktop/src-tauri` (console mode, window/power, Sunshine, overlay), console-relevant server surfaces.
**Method:** code reading, the existing 2026-11-16 audit, the current tests, and the new `console-mode-ui` skill. No hardware, Tauri build, or screen reader was available; native/streaming claims are from code and the repo's own tests.

> **Tree state (important for the orchestrator).**
> - `state/downloads.tsx` now exposes a selector store (`useAllJobs`, `useActiveJobs`, `useJob`, `useInstallingPercent`, `useQueueSummary`, `useTitleFor`) and a compatibility `useDownloads()`; `big-picture`, `library`, `home`, `game-detail`, `app-shell`, `game-card`, `install-dialog` are being migrated.
> - `app.tsx` now lazy-loads `BigPictureScreen` and the admin screens behind one `Suspense`.
> - `parts.tsx` memoises `RailCard`/`GridCard`/`CompactRow`; `game-card.tsx` memoises `GameCard`.
> - These are exactly two Phase 0 items in this plan. Verify them, finish the remaining consumers, and do not re-implement them.
> - The skill references `docs/console/` as the blueprint source; that directory is **not present** in this working tree. The report follows the skill contract and flags the missing blueprint.

**Question map:** 1 shell → §1 · 2 boot/profile/power → §2 · 3 input → §3 · 4 surfaces → §4 · 5 data/runtime → §5 · 6 native/streaming → §6 · 7 rebrand → §7 · 8 migration/decisions → §8–§10.

---

## Verdict

The Console's **interaction architecture is an asset**. The motion vocabulary, the cue/haptics table, the controller fingerprint/profile resolver, the pure layout/zone models, the 191 colocated console tests, the static-blur budget, and the `--console` Sunshine promotion are better than the average desktop app. They are also the parts the user explicitly likes.

The problems are structural:

1. **Input has no owner.** Four components run their own `useGamepad` loop; with the in-app overlay open, the Console underneath still receives the same presses.
2. **The shell has no boundary.** `/bigpicture` is already outside `AppShell`, but it shares every provider and every global (toasts, PWA prompts, onboarding, update dialog, overlay host), and boot is a late redirect that can paint the desktop shell first.
3. **One 2,748-line component owns a shell, seven surfaces, the input router, data fetching, power, and settings.** The pure seam exists but was never pulled through.
4. **The native layer is still missing the two commands the Power screen calls** (`quit_app`, `system_sleep`), and window/fullscreen policy is inferred independently in Rust and in the web.
5. **The visual composition is still a dashboard**, not a console: six equal tiles with `01`–`06`, duplicate chrome, micro-mono labels. The new skill names the target: **Cabinet** — warm charcoal, matte anodised panels, one lit object, rounded 6/14/20/24u, one blurred wallpaper, signal yellow only, the dot as the only ornament.

The right move is not a rewrite. It is a **new shell built around the existing kit**, with the structural fixes done first. The new Console should be a first-class `/console/*` route tree in the same SPA, lazy-loaded, with its own providers and globals, running in the **same single Tauri window**. The rebrand is **one identity, two densities**: Cabinet for the couch, the dense archive for the desktop. Not a second palette, not a return to cream/coral Soft, not a new logo in the same release as the layout rewrite.

---

## 0. Baseline: what is already true

### 0.1 What #237 already fixed (do not re-litigate)

- The Console default is **Studio**, not Soft (`apps/web/src/theme/console-appearance.ts:69-77`), and the preset no longer owns a palette (`console-appearance.ts:15-24`).
- Console type scales with `--bp-u` to 28 px body / 48 px hero at 1080 p (`apps/web/src/styles/index.css:125-129`, enforced by `styles/type-scale.test.ts:28-52`).
- Soft's plates are opaque; only the wallpaper is blurred (`styles/index.css:1044-1070`, `theme/console-contrast.test.ts:55-72`).
- Pads traverse real dialogs and close them with B (`lib/gamepad.ts:390-425`), and route changes set the title/focus.
- Route-level lazy loading for the Console and admin screens is landing (`apps/web/src/app.tsx` working tree).
- Download progress now has selector hooks and memoised cards in flight (`state/downloads.tsx`, `parts.tsx`, `game-card.tsx` working tree).

### 0.2 What still reads as slop

The old audit's "default Soft is the slop" finding is stale. What remains is composition:

- six equal home tiles with decorative `01`–`06` (`components/big-picture/home-row.tsx:112-127`);
- the same KPI-grid recipe on Power and search;
- uppercase tracked mono labels used for words, not just numerals (`system-bar.tsx:94-190`, `home-row.tsx:112-127`, `parts.tsx:475-520`);
- a layout picker exposing six variants (`layout.ts`, `layout-switcher.tsx`), multiplying focus graphs, tests and visual states;
- desktop escapes from a couch surface: `InstallDialog`, `/game/:id`, `/downloads`, `/collections/:id` (`big-picture.tsx:1200, 1331, 1375, 1474, 2729`);
- two hint strips and a double focus ring (audit §3.6, `overlays.tsx:105,132-145`, `big-picture.tsx:2583`);
- the `--bp-text-micro` token is 20u, below the skill's new 24u floor (`styles/index.css:129`), and it is used for words on several surfaces.

### 0.3 What must survive any redesign

| Asset | Files | Why it matters |
|---|---|---|
| Motion vocabulary | `components/big-picture/motion.ts`, `entrance.ts`, `launch-overlay.tsx`, `launch-plate.ts` | 120/180/240 ms, springs only on surfaces, interrupt/instant rules, reduced-motion mapping. The user likes this. |
| Sound + haptics | `lib/ui-sound-specs.ts`, `lib/ui-sound.ts`, `lib/gamepad-core.ts:403-460` | Documented cue table (11 cues), polyphony/dedupe, frozen move/launch hashes, pad-only rumble. Skill: add only press, detent, boot-mark. |
| Controller identity | `lib/controller-identity.ts`, `controller-devices.ts`, `controller-resolve.ts` | Fingerprint survives reconnect; per-device profiles; A/B swap and glyph family move together. |
| Pure navigation | `layout.ts`, `navigation.ts`, `system-zones.ts`, `input.ts`, `search.ts`, `power.ts` | Testable without a DOM; keyboard and pad cannot drift. |
| Tokens/contrast/type | `theme/themes.json`, `theme/registry.ts`, `styles/tokens.test.ts`, `console-material.test.ts`, `console-contrast.test.ts`, `type-scale.test.ts` | Contrast is enforced; literal colours fail the build. |
| Tests | 14 console test files, ~191 `it`/`test` cases | Move them with the code; they are the regression net. |
| Native console mode | `console.rs:39-160,232-330`; `streaming.rs:32-36,147,836` | `--console`, promotion, Sunshine publishing, embedded art. |

**Rule:** Console 2.0 is a new composition and a new shell, not a new interaction model. Reuse the kit; retarget the tests.

---

## 1. Shell architecture

### 1.1 What exists today

The Console is already **outside** the desktop `AppShell`: `/bigpicture` is a sibling route in the same `<Routes>` (`apps/web/src/app.tsx:324-366`), wrapped in the same `Gate`. It shares every provider and every global:

```
QueryClientProvider
 └ SettingsProvider
   └ ToastProvider
     └ AuthProvider
       └ PreferencesProvider
         └ DownloadsProvider
           └ UploadProvider
             └ VirusTotalQueueProvider
               └ I18nProvider
                 └ UpdateProvider
                   └ HashRouter
                     ├ Routes  (/bigpicture = BigPictureScreen; /* = AppShell)
                     ├ OverlayHost        (app.tsx:482)
                     ├ ToastViewport      (app.tsx:484)
                     └ PwaPrompts         (app.tsx:485)
```

Consequences:

- `ToastViewport` renders desktop toast cards over the Console footer; the skill forbids toasts in Console. `OnboardingWizard` can cover a 10-foot screen. `PwaPrompts` and `UpdateNotifier` have desktop presentation; `UpdateNotifier` has a special `/bigpicture` branch (`components/updates/update-notifier.tsx:80-86,145-160`). `OverlayHost` can draw a desktop-styled overlay inside the Console (`components/overlay/overlay-host.tsx:96`).
- Console boot is a **late redirect**. `ConsoleEntry` probes `console_mode` in an effect and navigates (`app.tsx:266-284`). If auth is already warm, the desktop shell can paint for a frame before the redirect. `Gate` sends signed-out users to the desktop `/login` (`app.tsx:171-211`).
- Rust hardcodes the route in three places: `console.rs:323,326` and `overlay.rs:1523` (`#/bigpicture`, `/bigpicture`).
- `HashRouter` is deliberate: Tauri's asset protocol and the Rust deep links use hash routes. Keep it.

### 1.2 Options

| Option | Auth/boot | Providers / query cache | Window / streaming | Overlays / globals | Deep links / PWA | Cost / risk |
|---|---|---|---|---|---|---|
| **A. Mode inside `AppShell`** | Shared | Shared | Same window | Desktop chrome around the Console | Hash routes inside shell | **Reject.** The rail, topbar, onboarding and desktop providers would need conditional logic everywhere; the Console would inherit the shell it was meant to escape. |
| **B. Top-level `/console/*` sibling shell, same SPA, lazy subtree** | Shared `AuthProvider`; console gate | Shared `QueryClient` (cache warm from desktop) | Same window; Rust policy | Console-scoped globals; desktop globals move into `DesktopShell` | Nested hash routes; `/bigpicture` redirect | **Recommend.** Reuses the hard parts, one cache, one token, one window. Main work is the provider/global split. |
| **C. Separate Tauri window (`console` webview)** | Second JS context; token read from native settings | Second React tree + second QueryClient (cold cache, duplicate fetches) | Independent fullscreen/policy | Separate overlay host | Capability changes and route plumbing | **Defer.** High risk for little gain; Sunshine streams the desktop regardless. Revisit for multi-display or console-alongside-desktop. |
| **D. Separate HTML entry (`console.html`, own bundle/CSS)** | Shared auth module, second bootstrap | Shared cache only if same JS context (it is not) | Same window | Own globals | Server/Tauri fallback for `/console` | **Optional later** if browser-TV bundle size/memory becomes first-class. Today, a lazy chunk gets most of the benefit. |

### 1.3 Recommendation: a real Console shell in the same SPA

```
AppProviders (QueryClient, Settings, Toast state, Auth, Preferences, I18n, Update)
└ HashRouter
  ├ /console/*  → ConsoleApp (lazy)   → ConsoleProviders → ConsoleShell → routes
  └ /*          → DesktopApp          → AppShell + desktop globals
```

**Provider scoping**

| Global (must stay above both) | Desktop-only | Console-only |
|---|---|---|
| `QueryClientProvider` | `OverlayHost` (in-app overlay) | `ConsoleNoticeProvider` + `ConsoleNoticeSlot` |
| `SettingsProvider` (theme/tokens) | `ToastViewport` | `ConsoleUpdateNotice` |
| `ToastProvider` (state only) | `PwaPrompts` | `ConsoleInputProvider` (focus store) |
| `AuthProvider` | `OnboardingWizard` | `ConsoleRoomProvider` (appearance) |
| `PreferencesProvider` | `FloatingWindowControls` (boot) | `ConsoleRunningGameProvider` |
| `I18nProvider`, `UpdateProvider` | `ConfigSyncDefaults` remains global (native only) | — |

- `ToastProvider` stays global because install/launch/error code already calls `push`; only the **viewport** is shell-specific. The Console renders notices in a footer/status slot, not desktop cards.
- `OverlayHost` is desktop-only. In native Console mode the backend should prefer the **floating overlay window** (`overlay::open`) over the in-app panel; an in-app desktop overlay over the Console is redundant and creates the double-input problem (§3).
- `UpdateNotifier` is desktop-only; the Console gets a quiet row in Settings/System plus the existing OS notification path, never a launch dialog.
- `OnboardingWizard` never mounts in Console; first-run states are explicit (§2).

**Routes**

| Route | Surface | URL state |
|---|---|---|
| `/console` | Home (stage + rail + band) | `?game=` optional for a deep-linked selection |
| `/console/library` | Library (shelf/grid + chips) | `?shelf=`, `?genre=`, `?sort=` |
| `/console/game/:id` | Game (one primary + tabs) | path id |
| `/console/downloads` | Downloads | `?tab=active|history` |
| `/console/settings/:section?` | Settings | path section |
| `/console/power` | Power | — |
| `/console/search` | Search | `?q=` |
| `/console/settings/controllers` | Controller profiles (Settings section; also a Game tab) | — |
| `/bigpicture` | Redirect to `/console` (preserve query) | one release |

**Boot/auth**

- Native Console mode normally has a token: `AuthProvider` reads `native.getSettings().token` before `/api/me` (`state/auth.tsx:76-143`). Keep that; it is the reason a TV works with no typing.
- Console needs its own **gate**: `ConsoleGate` renders a console boot screen, then either `ConsoleApp`, a console sign-in, or a "finish setup on a computer" screen. Do not render `LoginScreen`/`SetupScreen` unchanged at 10 feet.
- Resolve `console_mode` **before the first route render** (or render the console boot screen while probing). The current effect redirect is the source of the desktop-shell flash.
- Boot → Home in ~1.2 s, no spinner. First run is four honest states: signed out / no server / empty library / offline (skill §4).
- Browser Console: existing token/localStorage login is fine for v1; a TV pairing/PIN flow is v2 (there is no device-pairing endpoint today; `desktop-handoff` is desktop-bound, `routes/oidc.rs:203-231`).
- First-run browser Console: show the server address and a "set up on the desktop" instruction; do not build a 10-foot server-URL form in v1.

**PWA / browser TV**

- Keep `HashRouter`; a PWA shortcut to `./#/console` works with the existing standalone manifest (`apps/web/public/manifest.webmanifest`). Add the shortcut in the rebrand pass.
- Browser Console is a **companion**: browse, search, download, remote install; no launch/install/sleep/quit. Encode this in a capability matrix, not scattered `isNative()` checks.
- Fullscreen on browser TV requires a user gesture; enter fullscreen on the first A/Enter press, or let the TV browser handle F11.
- The service worker precaches the whole build (`apps/web/vite.config.ts:15-40`, `public/sw.js`); a lazy console chunk must be in the precache or network-first with the shell fallback.

### 1.4 Capability matrix: native vs browser

One matrix drives every surface; do not scatter `isNative()` branches.

| Capability | Native (Tauri) | Browser / PWA |
|---|---|---|
| Boot into Console | `--console` argv, second-instance promotion | `#/console` route |
| Auth | Native token from `LocalSettings` (`auth.tsx:82-85`) | localStorage token + login |
| Launch / resume game | `launch_game`, `resume_game` | No; browse/download only |
| Install | `install_game` queue + native progress events | Browser download (`lib/download.ts`) |
| Queue control | Pause/resume/cancel/retry/reorder | No |
| Sleep / quit / restart app | Native commands (after §2.4) | No |
| Fullscreen | Rust `WindowPolicy` | Browser Fullscreen API on a user gesture |
| Gamepad | Gamepad API; native listener; gilrs fallback if needed | Gamepad API (browser-dependent) |
| Overlay | Floating window / in-app panel | In-app only, or absent |
| Updates | Tauri updater | PWA service worker |
| Offline | Cached library + native queue | Cached shell + library snapshot |

A Console surface asks the matrix, not the platform: `caps.launch`, `caps.install`, `caps.sleep`, etc. This is what keeps a browser TV from showing dead Play buttons and a native TV from hiding a working one.

### 1.5 Why not a separate Tauri window now

A second `console` webview would:

- duplicate the QueryClient and auth boot (cold cache on every entry, two `/api/me`, two install subscriptions);
- require a new capability entry and every `get_webview_window("main")` assumption revisited (`capabilities/default.json` lists `main`/`overlay`; `overlay.rs:1278` builds the overlay; `console.rs:314-330` navigates `main`);
- complicate Sunshine's close semantics: today a console session closes for real so Moonlight sees it end (`lib.rs:137-163`, `overlay.rs:1423-1439`);
- double the hardware/QA matrix.

Revisit only if the product wants the Console on a second display while the desktop stays usable, or a true "console app" that survives closing its window. Then add a `console` window + capability + a shared native/event store deliberately, not by splitting React state.

---

## 2. Boot, profile, power

### 2.1 Boot state machine

| State | Native Console | Browser Console | Current defect |
|---|---|---|---|
| Cold boot | process → window → auth boot → console route | page → auth boot → console route | Late `console_mode` probe can paint desktop shell first (`app.tsx:266-284`). |
| No token | console boot → sign-in (rare; native token usually present) | console sign-in | `Gate` uses desktop `/login` (`app.tsx:171-211`). |
| Needs setup | "Finish setup on a computer" + retry | same, plus server URL | `SetupScreen` is a desktop form. |
| Ready | Home | Home (capability-degraded) | — |
| In game | Console hidden behind the game; `game://exited` returns Home | not applicable | Console keeps its own `runningGame` state (`big-picture.tsx:257`), overlay has another (`overlay.rs:1767`). |
| Power confirm | modal surface, default Cancel | same | `power.ts` models it well; commands missing. |
| Quit / end session | `exit_console` quits (`commands.rs:4452-4463`) | leave route | `quit_app` missing (`native.ts:535`); `system_sleep` missing (`native.ts:545`). |

### 2.2 First run, sign-in, profile picker

- **Single-user household is the v1 assumption.** Native Console inherits the desktop token; browser Console uses the existing login. No new server work.
- **Profile picker is v2, not a UI-only feature.** There is no public profile list endpoint; `api.listUsers()` is admin-only (`lib/api.ts:692`, `routes/users.rs`). A couch profile picker needs a server contract (opt-in profiles, a PIN or a short-lived switch token) and a security review. Do not fake it with local avatars.
- **No local profiles.** Playtime, achievements, saves and preferences are account-scoped; a local "profile" would fork ownership semantics. If multi-user is wanted, do it server-side.

### 2.3 Resume / suspend

- `resume_game` already exists (`resume.rs:133-180`) and never launches a second instance. Keep it.
- There is no cross-platform "suspend process" primitive worth building; Console "Resume" means **focus the running game**, not SIGSTOP. Use `overlay_game_state` (`overlay.rs:1767`) or a thin `running_game` command as the single source of truth; add `onGameStarted`/`onGameExited` events to a `ConsoleRunningGameProvider`.
- Sleep is an OS action, not a game action. It must be gated and honest.

### 2.4 Power: actions and missing commands

`power.ts` already models the actions, consequences and a confirm that starts on Cancel (`power.ts:13-130`). What is missing is the native half:

| Action | Web call | Native command | Status |
|---|---|---|---|
| End session | `native.exitConsole()` | `exit_console` | Exists; quits the process (`commands.rs:4452`). |
| Sleep this PC | `native.systemSleep()` | `system_sleep` | **Missing** from `generate_handler!`; web call fails (`native.ts:545`, `docs/CONTRACT.md:3078`). |
| Restart Ludex | `native.restartApp()` | `restart_app` | Exists (`commands.rs:658`, handler `lib.rs:312`). |
| Quit Ludex | `native.quitApp()` | `quit_app` | **Missing**; Console falls back to `exit_console` (`big-picture.tsx:1115-1127`). |
| Quit game | `native.stopGame(sessionId)` | `stop_game` | Exists (`lib.rs:292`). |
| Exit Console | route back | — | Only meaningful outside a Sunshine session. |

**Power copy bug:** `power.ts:58` promises "Press a button on the controller to wake the PC." USB/gamepad wake is not guaranteed by any OS and is not implemented. Remove it or implement/document per-platform USB wake; do not ship a false promise.

**`system_sleep` design**

- Platform implementation: Windows `SetSuspendState(false, false, false)`; macOS `pmset sleepnow`; Linux `systemctl suspend` via logind.
- Add a `sleep_capability` probe (`can_sleep`, reason) so the row is disabled honestly instead of failing on press.
- Refuse or warn while a game is running (stream drop) and while an install is active (downloads stop); the row detail should say which.
- Confirm always starts on Cancel (already modelled).

**`quit_app` design**

- `app.exit(0)` after a best-effort flush; installs are resumable, so no need to block on them.
- Keep `exit_console` as the Sunshine path; `quit_app` is for the non-Sunshine desktop app opened in Console mode.

### 2.5 Multi-user

Defer to v2 with a written security design. The server already has users, roles, sessions and avatars, but no "switch profile" contract. A proper version needs:

1. an opt-in, account-scoped `console_profiles` list (display name + avatar);
2. a switch endpoint that requires a PIN or a device-bound token;
3. a rule for running games/downloads during a switch;
4. a visible "who is playing" state in the system bar.

Until then: one signed-in account per Console, `Sign out` in Settings → full login.

---

## 3. Input

### 3.1 The structural defect: there is no single input owner

`useGamepad` is called by four components, each running its own `requestAnimationFrame` loop:

- Console: `screens/big-picture.tsx:1623`
- Desktop library: `screens/library.tsx:356`
- In-game overlay: `components/overlay/overlay-screen.tsx:158`
- Onboarding: `components/onboarding/onboarding-wizard.tsx:151` (`owner: true`)

The overlay root is `role="dialog"` **without** `aria-modal` or `data-pad-layer` (`overlay-screen.tsx:270-278`); the blocking selector only knows those two markers (`lib/gamepad-core.ts:333-355`). Result: with the in-app overlay open over the Console, both loops dispatch the same press. B can close the overlay **and** open Power/return to the library; A can click an overlay row **and** launch the selected game. The overlay comment says this is deliberate ("never a modal to the gamepad hook", `overlay-screen.tsx:25-36`), but it only makes sense if exactly one loop is active. Today four can be.

This is the highest-severity Console correctness bug and the first thing to fix.

### 3.2 Target: one focus store, one input owner

The new skill is explicit: **one focus store; DOM focus follows it in one place; never querySelector sweeps.** The current code does the opposite (`focusSurface` removes `[data-gamepad-focus]` across the document, `big-picture.tsx:853-870`).

```
InputProvider (one rAF loop, one pad state, repeat tracker, profile resolver)
  └ FocusStore (focused node id + scope + last input device)
      └ scope stack: Home → Library → Game → Downloads → Settings → Power …
          ├ DialogScope (registered by the two allowed modals)
          ├ OverlayScope (registered by OverlayScreen)
          └ OnboardingScope
```

- Only the **top scope** receives actions. A scope declares `{ id, priority, onAction, onBack, active }`; registration is a layout effect, so mount/unmount is the stack.
- **Never move focus without user input.** If the focused item disappears, hide the ring; move to a neighbour only on the next discrete directional press (skill §5). The current code calls `focusSelected()` from effects/timeouts after data changes (`big-picture.tsx:908, 928, 946, 2090, 2160`), which violates this rule.
- **B/Escape returns to the exact previous target** (rail index, scroll, tab), not "the library" generically.
- Keep `focusGamepadTarget` as the DOM writer, but call it from the focus store in one place; delete document-wide ring sweeps.
- Keep the `walkModal` fallback for real `aria-modal` dialogs (`gamepad.ts:390-425`); migrate the two allowed modals to registered scopes.
- Haptics/cues move to the provider's `InputEvent` (source included) so a press has one cue and one rumble, regardless of which scope handles it.

### 3.3 Focus graphs, not DOM geometry

The Console is already model-driven: `navigation.ts` owns shelf/card selection, `system-zones.ts` owns the home row/settings/power zones, `input.ts` owns the command table. That is the right foundation.

Do **not** introduce a geometric spatial-navigation library (nor `document.elementFromPoint`-style search) as the Console's source of truth. It is fragile under transforms, scroll, animation, RTL and virtualized rails, and it makes "what does Left mean here" untestable.

Add a small pure scope layer:

| Scope | Used by | Move rule |
|---|---|---|
| `LinearScope` | Settings rows, queue rows, search results | up/down ±1, wrap optional |
| `RowScope` | Band action row, shelves | left/right ±1, wrap |
| `GridScope` | Library grid, Power | `(row,col)` with `columns` and edge behavior |
| `StageScope` | Home stage primary + rail + band | vertical between stage/rail/band, horizontal within rail |

Each scope returns a stable **node id**; the route maps ids to DOM targets (refs or a selector held by the focus store). Tests cover the scope without a DOM. Keep `layout.ts` geometry for shelf/grid math.

### 3.4 Keyboard, remote, mouse parity

- `routeKey` (`input.ts:126-209`) already maps arrows, Enter, Escape, Backspace, `[`/`]`, `PageUp/Down`, `/`, `m`, `p`, `d`, `l`, `g`. Keep it, but move the per-surface contexts into the route modules instead of one 2.7k screen.
- Add TV remote keys: `GoBack`/`BrowserBack` (treat as B), `MediaPlayPause` (A/confirm where safe), `MediaTrackNext/Previous` (shelf/page if desired). Do not claim support for buttons the browser does not deliver.
- Hints must follow the **last input device**, not merely "pad connected". Today the footer always shows pad hints when a pad is connected (`parts.tsx:475-520`); a keyboard player sees the wrong keys. Track `lastInput: "pad" | "keyboard" | "pointer"` in the focus store.
- **One hint strip, max 3 hints.** Today `hintsFor` returns 6–8 hints for the library (`controller.ts:256-266`), `BigPictureFooter` and `OverlayShell` both render one (`parts.tsx:505`, `overlays.tsx:105`). Hide the page footer when a sheet is open; keep the sheet's strip.
- Mouse: hover highlights, click activates, and moving the pointer back must not leave two rings. One `useFocusRing` owns the marker.
- **B must always back out one level**, never quit the app from the library. Today B opens Power (`input.ts:110-116`); that is defensible, but the hint must say "Power", and the Power default is Cancel.
- **No route a controller can reach but not operate.** The audit's install-dialog dead end (`big-picture.tsx:1200`, `install-dialog.tsx:238`) is the canonical failure; the new install confirmation is one of exactly **two allowed modals** (install, power), and it must be console-native. Everything else is a non-modal sheet: the library stays mounted, and input is never silently disabled.

### 3.5 Text input / on-screen keyboard

- Search already has a real input + a focus-grid keyboard (`search.ts`, `search-overlay.tsx`). Extract it as `OnScreenKeyboard` with 28u keys, delete/clear, recents, and locale layouts (AZERTY for French, QWERTY for English). Never trap focus in a portalled widget.
- v1 Console has almost no text fields: search, pairing code (v2), server address only in the setup fallback. Do not build a 10-foot password form; native token or desktop setup.
- Keep physical typing native: when a real keyboard is present, printable keys go to the `<input>`, arrows/Enter are routed (`input.ts:126-150`).

### 3.6 Controller identity and glyphs

The hard part is already solved:

- fingerprint from vendor/product or normalized id, never `gamepad.index` (`controller-identity.ts:1-30`);
- per-device profile store with source (`auto`/`account`/`game`/`preset`), glyph theme and A/B swap (`controller-devices.ts:1-60`);
- resolver applies fallback, swap and gaps (`controller-resolve.ts:60-120`);
- hints and labels follow the resolved family and swap (`controller.ts:245-254`).

Console 2.0 work: a single `PadGlyph`/`HintBar` primitive, used by the Console, overlay and onboarding; one visual test per family (Xbox, DualSense, Switch, Steam Deck, generic/numbers). Do not rewrite the resolver.

### 3.7 WebKitGTK/TV reality: spike before locking

The skill requires a spike before locking the visuals: **Gamepad API `mapping === "standard"`, hot-plug and `timestamp` on the weakest target (WebKitGTK/Tauri Linux TV)**. If it is broken or unreliable, read pads **in Rust (gilrs)** and emit `input://pad` events to the one input provider.

Architecturally, keep this door open from day one:

- the input provider consumes a `PadSource` interface (`web-gamepad` | `native-gilrs`);
- the native listener emits the same logical actions (`left/right/up/down/confirm/back/menu/downloads/prevTab/nextTab`) and source kind;
- the controller identity/profile layer is shared, so a native event carries `vendor/product` or `fingerprint` instead of a `Gamepad.id` string;
- one owner, one scope stack, either source.

### 3.8 Input acceptance matrix

| Input | Must work | Current risk |
|---|---|---|
| D-pad / left stick | Move focus in every scope; held direction lands instantly | Good, but four loops |
| A / Cross | Activate focused node; Install/Play/Resume | Double dispatch with overlay |
| B / Circle | Back one level; close sheet; cancel power | Double dispatch; B on library opens Power |
| X / Square | Context action (search, details) | Per-surface definitions drift |
| Y / Triangle | Downloads/context action | Same |
| LB/RB, LT/RT | Tabs/shelves, page | Layout-dependent |
| Start/Options/Guide | Menu/search | Overlay listener vs web loop |
| Keyboard | Full parity, last-input hints | Pad hints shown to keyboard users |
| Mouse | Hover + click, one ring | Pointer/focus interplay |
| TV remote | Arrows/Enter/Back | Back not mapped |
| Hot-plug | Reconnect keeps profile | Polling + fingerprint good |
| Text | Search OSK + physical typing | No shared OSK component |
| Native fallback | Same actions from Rust/gilrs | Not built; spike required |

---

## 4. Product surfaces

The skill fixes the IA: **stage + rail + band**; download/settings/power/search/notifications are console surfaces; collections is a shelf, not a system app; game has one primary action + tabs. Use that, not the current six-tile dashboard.

### 4.1 Scope

| Surface | v1 | Reuse | New work | Notes |
|---|---|---|---|---|
| **Home** | Yes | `useRecentGames`, `useAllGames`, `useInstalledMap`, `useLaunchGame` | Stage, rail, band; Resume | Default route. No six tiles, no indexes. |
| **Library** | Yes | `useAllGames`, facets, `navigation.ts`, `layout.ts` | Shelf/grid + shelf chips, filters, infinite pagination | Current `useAllGames` is capped at 200 (`hooks.ts:24-29`, server clamp `routes/games.rs:315`). |
| **Game** | Yes | `api.getGame`, `useGameSession`, `useInstalledMap` | One primary + tabs (Overview · Saves · Artwork · Controller) | Stop navigating to desktop `/game/:id`. |
| **Install confirmation** | Yes | `downloads.startInstall`, versions/locations | Console-native modal (one of two) | Replace `InstallDialog` (`big-picture.tsx:2729`). |
| **Downloads** | Yes | install store/selectors, `eta.ts`, `install-queue.ts` | Console rows, aggregate, pause/resume/cancel/retry, history | Stop navigating to desktop `/downloads`. |
| **Settings** | Yes | settings model, theme/controller logic | 4 rooms × 8 knobs, live preview, sections | Today Console has 4 sections vs 13 desktop (`lib/settings-sections.ts:64-175`). |
| **Power** | Yes | `power.ts` | Commands + confirm + capability reasons | Default Cancel; honest sleep. |
| **Search** | Yes | `search.ts`, `filterGames` | Global results, OSK, recents | Games + settings actions. |
| **Controller profiles** | Yes | `controller-resolve.ts`, device store | Settings section + Game tab + quick-switch sheet | Account profile picker v2. |
| **Notifications** | Yes | `ToastProvider` state | Console notice surface, one at a time | No toasts. |
| **Collections** | v1.5 | `api.listCollections` | A shelf, not a system app | Create/edit on desktop. |
| **Achievements** | v1.5 | `routes/achievements.rs` | Read-only tab + unlock notice | Server exists; no console UI today. |
| **Friends / social** | v2 | `routes/social.rs`, `community.rs` | Not a couch v1 surface | Presence on the system bar could be v1.5. |
| **Store** | Out | none | — | No backend; do not invent one in v1. |
| **Admin** | Out | desktop only | — | Never in Console. |
| **Setup / sign-in** | Minimal v1 | auth actions | Console gate; pairing v2 | Native token is the primary path. |

### 4.2 Home = stage + rail + band

- **Stage**: the selected game (continue/recent) with background art, 48u+ title, one-line meta, one primary `Resume`/`Play` and a secondary `Details`.
- **Rail**: one horizontal shelf, 5.5 covers visible (skill §4), recents/shelf chips. No second row, no dashboard.
- **Band**: `Search · Library · Downloads · Settings · Power` — a horizontal action band, not six equal cards.
- **Focus**: stage primary → rail → band; B opens Power. No `01`–`06`, no corner-icon template, no uppercase tracked mono captions.
- **One lit object per screen** (skill §2). The wallpaper is the only blurred layer; the shell is matte.

### 4.3 Library

- Shelf/grid + **shelf chips** (All, Installed, Recent, Genres, Collections). Collections is a shelf, not a system app.
- No layout picker. The skill explicitly says "no layout picker"; pick shelf or grid per viewport or a single Library setting, not six exposed layouts.
- Filters in a sheet (genre, players, year, state, platform) — not a desktop sidebar.
- **Pagination is mandatory.** The server caps `limit` at 200 (`routes/games.rs:315`); `useAllGames` requests 200 and silently truncates larger libraries (`hooks.ts:24-29`). Use `useInfiniteQuery` with `page`/`limit` and the existing `GameList.total` (`types.ts:928-933`).
- Card: cover, title, installed/update badge, progress; no index, no corner-icon template.
- Render a window of cards (selection ± 20) rather than all 200 DOM nodes with motion wrappers; `content-visibility` for off-screen rails; one static blur.

### 4.4 Game

A console-native page is required; the current "Game page" opens the desktop `/game/:id` (`big-picture.tsx:1331, 1474`). v1 content:

- stage art + title + year/developer/genres/players/rating;
- one primary `Play` / `Resume` / `Install` / `Installing` with progress;
- tabs: **Overview** (description, meta), **Saves** (cloud/local, v1.5), **Artwork** (screenshots, v1.5), **Controller** (profile for this game);
- version selector, install size/location, playtime, last played, update badge;
- achievements (v1.5), collections membership (v1.5).

### 4.5 Install and downloads

- **Install confirmation** is one of exactly two allowed modals (the other is Power). Console-native: version, size, target location (default; "Change on desktop" for advanced), confirm; progress with pause/cancel. No `Dialog` with mouse-sized selects (`components/game/install-dialog.tsx:238`).
- **Downloads** is a console surface, not a navigated-away desktop page (`big-picture.tsx:1375` opens `/downloads`): active rows (title, phase, percent, speed, ETA, pause/resume/cancel, reorder), history (done/failed, retry, remove, open folder on native).
- Aggregate header: speed graph, ETA, active count; completion notices in the Console notice surface.
- Browser: `Download` starts the browser download (`lib/download.ts`); no install/launch.

### 4.6 Settings: 4 rooms × 8 knobs

- Use the skill's customization model, not a general settings maze: **Rooms/preset, accent (theme/from artwork/curated), wall (art/solid/gradient), corners, density, motion, sound, haptics.**
- Live preview, no Apply button, reset per section. No per-element colour pickers, no CSS injection.
- "From artwork" accent is extracted and runtime-validated at 7:1.
- Device-local persistence; account sync optional, never required.
- Settings sections at 10 feet: **Room** (the 8 knobs), **Library** (view/sort/shelf chips), **Controllers** (profile, glyphs, swap, haptics, test), **Network** (server, connection, offline), **Account** (name/avatar/locale/sign out), **System** (version, updates, storage, Sunshine, logs, quit).
- Reuse the *logic* of desktop panels (theme validation, controller editor, server switch) but not their layout. A `ConsoleSetting` record declares `{ id, section, label, detail, control, value, apply, available(capabilities) }`.
- Expose all themes, not a 3-value cycle; the current Console cannot reach Nord/Gruvbox/Amber (`big-picture.tsx:1040-1060`).

### 4.7 Power and notifications

- Power: End session, Sleep, Restart Ludex, Quit Ludex, Quit game, Exit Console. Confirmation on destructive actions; default Cancel; unavailable rows explain why inline.
- Notifications: install complete, update available, server unreachable, launch failed. **One notice surface**, max 2 lines + one action; no toasts, no modal over a game launch.

---

## 5. Data and runtime

### 5.1 Reuse the query cache, fix the keys

- Keep the single `QueryClient` (`app.tsx:100-108`). The Console entering from the desktop should paint instantly from the cache; a separate window/entry would lose that.
- Add `lib/query-keys.ts` (audit E5) and use it in Console. Today keys are ad-hoc (`["games","all"]`, `["installed"]`, `["game-session", id]`), and invalidation is by literal string.
- Reuse `useInstalledMap` (`hooks.ts:92`), `useGameUpdates`, `useLaunchGame` (`hooks.ts:139`), `useGameSession` (`hooks.ts:166`), `api.getGame`, collections/achievements routes.
- **Do not fork cache keys per shell.** `["games","all"]` is shared; the Console derives shelves with `select`/`useMemo`.
- Prefetch on Console idle: `games`, `installed`, `game-updates`, `collections` (only if the v1.5 surface is enabled).
- **Pagination**: replace `useAllGames` (limit 200) with an infinite query for the library; Home can use the first page.

### 5.2 The 1 Hz problem: mostly landing, finish it

The working tree already contains the right shape (`state/downloads.tsx` staged):

- a `DownloadsStore` + `useSyncExternalStore` selectors (`useAllJobs`, `useActiveJobs`, `useJob`, `useInstallingPercent`, `useQueueSummary`, `useTitleFor`);
- `reconcileJobs` preserves unchanged job objects;
- the list poll is a **2 s safety net**, paused when the window is hidden;
- `RailCard`/`GridCard`/`CompactRow`/`GameCard` are memoised;
- `big-picture`, `library`, `home`, `game-detail`, `app-shell`, `game-card`, `install-dialog` use selectors.

Remaining work:

1. **`install://progress` still calls `refresh()`** (`downloads.tsx` staged listeners: `onInstallProgress(() => void refresh())`). That is a full-list IPC round trip on every progress event. Update **one job** from the event payload (`updateOne(job)`), and use `install://jobs` only for structural changes. Keep `reconcileJobs`.
2. **`screens/downloads.tsx` still uses the whole `useDownloads()` snapshot** (line 159). That is acceptable for the queue screen, but split its header/aggregate from the rows so a tick does not repaint the history list.
3. **`useDownloads()` compatibility hook** subscribes to the whole snapshot. Keep it only for the queue screen; remove it from cards/shells. The Console 2.0 shell must not use it.
4. **ETA sampling** stays at 1 Hz only while an active job exists; subscribe only the status strip and queue header. Do not put `meters`/`speedHistory` in the shell context.
5. **Add a regression test** that a progress tick does not render the Console shell or the library grid. The existing `state/downloads.test.tsx` is a good start; extend it with a render-count assertion.

### 5.3 Running game store

- One source: `overlay_game_state` (`overlay.rs:1767`) + `game://exited` + the launch result. Put it in `ConsoleRunningGameProvider` / `useRunningGame`.
- Remove the Console's local `runningGame` (`big-picture.tsx:257`) and the overlay's separate query; they will disagree after a tray launch or a crash.
- `useGameSession(id)` remains for the game page; the global store answers "is anything running?" for Home/Status bar/Power.

### 5.4 Console preferences

- **4 rooms × 8 knobs**, device-local, per the skill. The app theme selection remains the existing account preference; the Console knobs (including the accent source) are device-local, with `theme` following the app theme.
- Today `console-appearance.ts` has `{ preset, accent, background, radius }`, and `withConsolePreset` overwrites radius (`:198-203`) — the axes are not orthogonal. Replace with the skill's 8 knobs; presets set defaults, user overrides survive.
- **Delete the Soft preset and the five pastel accents (`coral`, `mint`, `sky`, `lilac`, `sun`)** (`console-appearance.ts:44-62`). Accent sources become `theme`, `from-artwork`, `curated` (signal). This is a deliberate contract change; update `console-appearance.test.ts` and `console-contrast.test.ts`.
- **Type floor 24u**: `--bp-text-micro` is 20u (`styles/index.css:129`); raise it to 24u and update `type-scale.test.ts` (currently checks micro ≥ 20). Layouts that relied on 20u captions will need more air; that is the point.
- **Mono only for numerals/units/shortcuts**: audit `system-bar.tsx`, `home-row.tsx`, `parts.tsx`, `settings-app.tsx`; words move to the UI face.

### 5.5 Route and state ownership

| State | Owner | Why |
|---|---|---|
| Surface, selected game id, settings section, downloads tab, search query | URL (nested route/params) | Deep links; survives the locale remount. |
| Focused node id, scope, last input | Focus store (external) | One owner; no sweeps; survives data refresh. |
| Install jobs, speed/ETA | External store + selectors | Cross-shell; high frequency; scoped re-renders. |
| Running game | External store + native events | One truth for Console/overlay/status bar. |
| Room/wall/corners/density/motion/sound/haptics | External device store | Cross-window `storage` event; no reload. |
| Server data | TanStack Query | Cache, retries, stale-while-revalidate. |

**Locale remount constraint:** `I18nProvider` remounts its entire subtree on a locale change (`lib/i18n.tsx:680-686`), because `t` is module-level. Any state that must survive a language switch **cannot live in component state**. The current Console discovered this and put `?app=settings&section=` in the URL (`system-zones.ts:180-220`). Make that the rule: URL or external store for durable state.

**Selection by id, not index.** On refresh, re-resolve the selected game by id; never keep a raw index across data changes (the current `resolveIndex` does this in places, but `runningGame` and some overlays still use indices).

### 5.6 Splitting the 2,748-line screen

| Current responsibility | New module | Target |
|---|---|---|
| Data: games, installed, updates, collections, profiles | `console/data/useConsoleLibrary.ts`, `useConsoleGame.ts`, `useRunningGame.ts` | < 200 lines each |
| Input: pad/key, focus store, command routing, scope stack | `console/input/useConsoleInput.ts`, `focus-store.ts`, `scopes.ts`, `commands.ts` | pure + < 300 |
| Shell: backdrop, status bar, rail, band, notices | `console/shell/Frame.tsx`, `StatusBar.tsx`, `Rail.tsx`, `Band.tsx`, `Notice.tsx` | < 250 each |
| Surfaces | `console/routes/Home.tsx`, `Library.tsx`, `Game.tsx`, `Downloads.tsx`, `Settings.tsx`, `Power.tsx`, `Search.tsx` | < 400 each; screen < 150 |
| Sheets/modals | `console/overlays/Menu.tsx`, `ConfirmPower.tsx`, `ConfirmInstall.tsx`, `Choice.tsx` | < 200 each |
| Model (keep) | `layout.ts`, `navigation.ts`, `system-zones.ts`, `input.ts`, `search.ts`, `power.ts` + new `focus-graph.ts` | unchanged / pure |
| Visual primitives | `console/ui/Stage.tsx`, `RailCard.tsx`, `BandItem.tsx`, `Sheet.tsx`, `Progress.tsx`, `PadGlyph.tsx` | < 150 each |
| Room/theme | `console/theme/rooms.ts` (8 knobs), `material.ts` | pure + tests |

Keep `components/big-picture/{motion,entrance,cues,controller,launch-*}*` as the **console kit**; move them under `console/kit/` only if the churn is worth it. Do not rewrite their tests.

**Test globs move with the code:** `styles/type-scale.test.ts:20-23` reads `components/big-picture` and `screens/big-picture.tsx`; point it at `console/**` in the same PR, or the no-arbitrary-text guard silently stops covering the new shell. Same for `console-material.test.ts` if the CSS file moves.

### 5.7 Error, offline, performance

- **Boot error**: a console-specific screen (server name/URL, Retry, Switch server, Exit Console). The desktop `BootError` (`app.tsx:129-168`) is not 10-foot.
- **Route error**: one error boundary per console route; errors render inline with the failing action and a retry, not a toast.
- **Offline**: distinguish `navigator.onLine` from "server unreachable" (query error). Keep a last-library snapshot (localStorage, per server) and hydrate Home/Library with `placeholderData`; disable launch/install with a reason; downloads continue native.
- **Performance budget**:
  - no Console-wide re-render at >1 Hz while idle or downloading;
  - no context value that changes per second; selectors + memo for cards/rows;
  - one static wallpaper blur; no animated `backdrop-filter` (enforced by `console-material.test.ts:35-51`);
  - max 3 composited layers; artwork decode off the focus path; virtualize long grids;
  - 60 fps rail scroll on WebKitGTK/Tauri Linux TV;
  - TV memory: **< 380 MB UHD / < 300 MB FHD**;
  - lazy Console chunk; no mock/demo data in the production chunk (audit E2);
  - measure with React Profiler and `performance.mark`.

---

## 6. Native and streaming

### 6.1 `--console` promotion: keep it

`ConsoleMode::from_args` / `from_forwarded_argv`, `second_instance_action`, `ConsoleSession::activate`, `promote`, `remove_tray`, `apply_window` (`console.rs:39-160,232-330`) already solve the hard part: Sunshine starts `Ludex --console`; a second instance promotes the running one; a console session has no tray and closes for real. Do not rebuild this.

### 6.2 One window policy in Rust

Current split:

- Rust decides decorations/fullscreen from argv (`console.rs:232-260`), close behavior from tray/console (`lib.rs:137-163`, `overlay.rs:1423-1439`), and window state is restored by the Tauri window-state plugin.
- Web independently calls `setFullscreen` on mount/unmount (`big-picture.tsx:767-819`) and polls window state in `window-controls.tsx`.
- The audit already found the consequence: #233 fullscreen trap; #227/#228 needed bespoke fixes (`docs/audit/2026-11-16/architecture.md:268-274`).

Recommendation: a Rust `WindowPolicy` resolved once from `{argv, settings, session, platform}`:

```rust
struct WindowPolicy {
  decorations: bool,
  fullscreen: bool,
  resizable: bool,
  close_action: CloseAction, // HideToTray | Close | CloseOverlay
  tray: bool,
  overlay_host: OverlayHost, // InApp | Window | Disabled
}
```

Apply it in one place; emit `window://state` on every transition; the web **reads** it and stops inferring. Add a truth-table test (console × windowed × close × overlay) next to the existing `console.rs` tests. In console mode, F11/Alt+Enter is either handled by Rust or disabled; Escape/F12 follows the configured exit shortcut. In desktop mode, F11/Alt+Enter remains a user escape.

### 6.3 Sunshine / Moonlight

- Sunshine publishing already has `publishing(console_mode, per_game_apps)` (`console.rs:147-152`), `generated_console_app` (`streaming.rs:836`), managed markers and app-name/exit-shortcut settings (`settings.rs:373-380`). Keep.
- Rebrand touchpoints: `DEFAULT_APP_NAME` (`console.rs:26`), embedded 600×900 console art (`streaming.rs:32-36`), Sunshine app icon/cover, README screenshots. Regenerate art as part of the brand rollout.
- Moonlight: no code change. Document recommended host settings (1080p60, HEVC, 20–50 Mbps) and test the Console at 1280×720, 1920×1080 and 3840×2160 with 5 % overscan margins.
- **UI scale**: `--bp-u` is viewport-derived (`styles/index.css:104`); the skill's density knob covers this, not a separate free-form UI scale. Do not rely on OS DPI alone.

### 6.4 Overlay

- The overlay is a separate window (`?overlay=1`, `OverlayWindowApp`) and an in-app panel (`OverlayHost`). Keep the floating window for games; in Console mode, prefer it or disable the in-app overlay. The in-app panel is desktop-styled (692 lines, 1 s clock, 2.5 s polling) and is not a Console surface.
- Update `overlay_show_big_picture` to navigate `/console` (`overlay.rs:1523`), and `console::show_console` (`console.rs:323-326`).
- Register the overlay as an input scope (§3) even if it is only used over the desktop; it removes the last duplicate loop.
- If the overlay stays in-app over Console, it must pause Console input and use Console tokens; otherwise it will read as a desktop popup.

### 6.5 Focus, performance, hardware

- `window_focus.rs` (620 lines) and the game-window focus/restore logic are existing assets; do not rewrite in this program. Console promotion already calls `show_console` and removes the tray.
- The Console's full-screen blur is one static layer (`console-backdrop.tsx:1-20`, `styles/index.css:1119-1141`); keep it that way. Do not add per-card `backdrop-filter`, parallax on scroll, or animated filters.
- Spike Gamepad API on WebKitGTK/Tauri Linux TV; if unreliable, implement the gilrs fallback (§3.7) before locking visuals.
- Measure on the host GPU (the streamed client only decodes). Test Intel iGPU at 4K and a low-end Windows laptop; the blur pass is the expensive part.

---

## 7. Rebrand: Cabinet

### 7.1 One identity, two densities

Keep the name **Ludex** and the drawn wordmark. Console is a *device room*, not a second brand.

- **Desktop** stays dense: archive, hairlines, small meta.
- **Console** is dark-first, one object at a time, ten times the air.
- Shared: name, wordmark, dot, signal yellow, type families, semantic tokens, motion/sound vocabulary.
- **Console status bar carries the monogram** (chamfered D + dot) at 28u, never a 17 px wordmark. Wordmark only at boot, About, and empty/error walls (≥40u). Current bar uses `Wordmark height={17}` (`system-bar.tsx:126`); change it.

### 7.2 Visual direction: Cabinet

- Warm charcoal, matte anodised panels, **one lit object per screen**, radii **6/14/20/24u**, soft shadows only for depth.
- **One blurred layer**: the wallpaper behind the shell. No blur on every panel; never animate `backdrop-filter`.
- **Signal yellow only** chromatic event. Delete the Soft preset and the five pastel accents (coral/mint/lilac). No second palette.
- **The dot is the only ornament**: cursor, progress head, unread badge, wordmark full stop.
- No six equal KPI tiles, no `01`–`06` indexes, no uppercase tracked mono labels, no gradient SaaS surfaces, no glass everywhere.
- This is how the user's "cute, minimalist, Switch-like, blur/rounded" ask is satisfied without returning to cream/coral: rounded matte objects, generous air, one soft wallpaper blur, a small dot, warm charcoal. If the user wants a light/cream Console, that is a change to the skill contract, not an architecture decision.

### 7.3 Foundation (source of truth)

| Layer | Current | Recommendation |
|---|---|---|
| Logo | Drawn wordmark + dot, app icon (`components/brand.tsx:1-45`) | Keep the skeleton; monogram in Console bar; wordmark at boot/About/empty. Do not change logo and layout in the same release. |
| Palette | 6 themes; signal yellow `#ffc61a`; Soft cream/coral (`theme/themes.json`) | Keep signal yellow as the single brand accent; re-author or retire Soft; no coral/mint/lilac in Console. |
| Type | Bricolage Grotesque / Instrument Sans / JetBrains Mono (`main.tsx:2-4`) | Keep. Console display = Bricolage, body = Instrument, mono only numerals/units/shortcuts. 24u floor. |
| Shape | Console radii 2/12/18/24/34; desktop 2 px | Console Cabinet 6/14/20/24 + one pill; desktop archive 2 px. |
| Material | Opaque plates; one wallpaper blur; chrome `backdrop-filter` none for Soft | Matte panels, one wallpaper blur, no glass cards. Chrome may be a dark tinted sheet, never a text-bearing translucent tile. |
| Icons | Lucide, duplicate glyphs (`nav-icons.tsx:35-49`; console menu duplicates `Gamepad2`) | A 12-glyph custom Console set at 2 weights; dedupe the desktop registry. Icons are most of the label at 10 feet. |
| Motion | `motion.ts` vocabulary | Keep. Add only press, detent, boot-mark; do not retune cues. |
| Sound | 11 cues + haptics | Keep. Add per-cue preview/volume; no second pack. |
| Voice | Explicit, technical, sometimes internal ("SHELF · SOFT", "A full theme editor… not built yet") | Short, warm, outcome-first; remove preset names and roadmap notes from the UI. |

### 7.4 Customization: 4 rooms × 8 knobs

| Knob | Values | Default | Scope |
|---|---|---|---|
| Room / preset | 4 rooms, **Cabinet** default; Studio retained as the archive room | Cabinet | Device |
| Accent | theme / from artwork / curated (signal) | theme | Device |
| Wall | art / solid / gradient | art | Device |
| Corners | 6 / 14 / 20 / 24u | 14u | Device |
| Density | cozy / compact | cozy | Device |
| Motion | full / reduced / off | full (system override wins) | Device |
| Sound | on/off, volume, cue previews | on | Device |
| Haptics | on/off, strength | on | Device |

The app theme selection is the existing account preference; the eight Console knobs are device-local. Account sync of the knobs is optional and never required.

- **Live preview, no Apply button, reset per section** (skill §6).
- No per-element colour pickers, no CSS injection.
- "From artwork" accent is extracted and runtime-validated at 7:1.
- Persist device-local; account sync optional, never required.
- Delete the current `CONSOLE_ACCENT_IDS` pastel set (`console-appearance.ts:44-62`) and the `soft` **preset**. Update `console-appearance.test.ts` (currently pins Studio default and the five accents) and `console-contrast.test.ts`.
- `soft` is two things today: the Console **preset** (deleted) and the built-in **theme** in `theme/themes.json` (cream/coral; re-author or retire). Do not confuse them in the migration.
- Appearance v3 migration: keep explicit choices where they map to a knob; move untouched defaults to Cabinet; update the pinned default test.

### 7.5 Asset rollout

A rebrand touches more than components:

- `apps/web/index.html` boot script hardcodes `#e7e2d4` / `#0a0a09` for the legacy theme path (`index.html:105-130`); update if the palette changes.
- `apps/web/public/manifest.webmanifest` theme/background colours and icons; add a `#/console` shortcut.
- `apps/web/public/offline.html` has its own hardcoded palette.
- `apps/desktop/src-tauri/tauri.conf.json` `backgroundColor` (`#0b0e0d`), bundle icons, installer art.
- `apps/desktop/src-tauri/assets/ludex-console.png` (Sunshine art) and `DEFAULT_APP_NAME`.
- `README`, screenshots manifest, release notes/OG images.
- `theme/themes.json` + generated `styles/themes.generated.css` (generated, do not hand-edit).

Sequence: **brand foundation → Console 2.0 → desktop shell accent/logo/type adoption → marketing assets**. Do not block Console 2.0 on the full desktop type-scale migration (1,915 arbitrary sizes, audit item 8); the desktop can adopt the new accent/logo first and the scale later.

### 7.6 Rebrand acceptance

- Every text pair ≥ 7:1 on Console surfaces; **24u floor**, 28u body, 48u hero at 1080 p; type scales with `--bp-u`.
- No translucent text plates over art; no animated blur; one blurred layer.
- No internal names ("Soft", "Studio", "SHELF · SOFT") in user copy; no uppercase mono labels for words.
- Screenshot set at 1280×720, 1920×1080, 3840×2160 with overscan approved by the user before building all surfaces (3 reference screens: Home, Game, Downloads).
- `no-hardcoded-colors`, `tokens`, `console-material`, `type-scale` tests green; add the anti-slop tests (§8.3).

---

## 8. Migration plan

Effort is in focused engineer-days for one senior dev with agent assistance, excluding review latency. Each phase lands on `main` behind green CI; no phase mixes a move with a behavior change.

**Keep vs rewrite vs delete**

| Keep (retarget tests) | Rewrite / compose | Delete |
|---|---|---|
| `motion.ts`, `entrance.ts`, `launch-*`, `cues.ts`, `ui-sound*` | `big-picture.tsx` → `console/routes/*` + `shell/*` | Six-layout picker and its exposed variants |
| `controller-identity/devices/resolve`, glyph/hint logic | `console-appearance.ts` → 4 rooms × 8 knobs | Soft preset, five pastel accents |
| `layout.ts`, `navigation.ts`, `system-zones.ts`, `input.ts`, `search.ts`, `power.ts` | Settings rows → one settings model + console renderer | Home KPI grid and `01`–`06` indexes |
| Theme tokens, contrast/type/material tests | `DownloadsProvider` → selector store + facade (landing) | Desktop escapes (`InstallDialog`, `/game/:id`, `/downloads`, collections) |
| `--console` promotion, Sunshine publishing, window focus | `InstallDialog` → one console install modal | Double hint strip, toasts in Console, double focus ring |
| 191 console unit tests | Boot/gate → console-specific screens | `/bigpicture` shell after parity (redirect stays one release) |

### Phase 0 — De-risk and foundations (12–16 engineer-days, some already landing)

Goal: no visible Console 2.0 yet; remove the structural risks and make the new shell possible behind a flag.

| # | Work | Files | Gate |
|---|---|---|---|
| 0.1 | `console_v2` device flag; `/console/*` route tree + lazy boundary; `/bigpicture` redirect; update Rust deep links | `apps/web/src/app.tsx`, `lib/navigation.ts`, `components/shell/app-shell.tsx`, `components/updates/update-notifier.tsx`, `console.rs:323-326`, `overlay.rs:1523` | Deep links work; no desktop-shell flash; old route redirects |
| 0.2 | Single input owner + focus store + scope stack; migrate library, overlay, onboarding, Console | `lib/gamepad.ts`, new `console/input/*`, `screens/big-picture.tsx`, `screens/library.tsx`, `components/overlay/overlay-screen.tsx`, `components/onboarding/onboarding-wizard.tsx` | One press = one action with overlay open; existing input tests green |
| 0.3 | **Finish the in-flight install selector lane**: `updateOne(job)` on `install://progress` instead of full `refresh()`; split the Downloads screen; remove `useDownloads()` from cards/shells; render-count test | `state/downloads.tsx`, `screens/downloads.tsx`, `state/downloads.test.tsx` | No Console-wide 1 Hz rerender; queue accurate; desktop screens unchanged |
| 0.4 | `quit_app` + `system_sleep` + capabilities + honest power copy | `commands.rs`, `lib.rs` handler, `lib/native.ts:535-548`, `components/big-picture/power.ts:57-60` | Windows/Linux manual; unavailable rows explain why |
| 0.5 | Rust `WindowPolicy` + `window://state`; remove web fullscreen inference | `console.rs:232-260`, `lib.rs:137-163`, `chrome.rs`, new `window_policy.rs`, `lib/window-chrome.ts`, `big-picture.tsx:767-819` | Truth-table test; console/windowed/close matrix |
| 0.6 | Playwright fake-pad harness + boot/input/screenshot specs; Gamepad API spike on WebKitGTK | `e2e/tests/helpers.ts`, new `e2e/pad.ts`, `e2e/tests/06-console.spec.ts` | CI job green; screenshots at 3 sizes; gilrs go/no-go |

**Note:** 0.1 (lazy routes) and 0.3 (selector store) are already in flight. Keep them, review the remaining gaps, and do not duplicate. `console_v2` is a **device-local** flag (`ludex.console.shell` in localStorage, exposed under Settings → System → About), default off through Phase 1 and on in Phase 2; do not put it in the account preference document.

### Phase 1 — New shell and core surfaces (20–26 engineer-days)

Goal: a complete Console 2.0 for Home, Library, Game, Settings, Power, Search, controller profiles, install confirmation, behind the flag; `/bigpicture` still the default.

| # | Work | Gate |
|---|---|---|
| 1.1 | Shell frame, status bar (monogram), rail, band, notice surface, boot/gate, Cabinet tokens | 3 reference screens approved; no desktop globals |
| 1.2 | Home (stage/rail/band), Library (shelf/grid + chips, infinite pagination), Game (one primary + tabs) | Routes deep-link; selection by id; 200+ library complete |
| 1.3 | Settings 4 rooms × 8 knobs; live preview; controller/account/network/system sections | No Apply button; reset per section; all themes reachable |
| 1.4 | Power, Search + OSK, controller profiles, Install confirmation | Exactly two modals; every flow exits with B; no `InstallDialog`; no `/game/:id` |
| 1.5 | Cabinet visual system: matte panels, 6/14/20/24, one blur, dot, 24u floor, no uppercase mono words | Contrast/type/material tests; 720p/1080p/4K screenshots |
| 1.6 | Console e2e: boot, home→launch, install→downloads, settings persistence, power confirm, overlay single-dispatch | 6 specs green in CI |

### Phase 2 — Downloads, streaming, rollout (14–18 engineer-days)

Goal: parity, rebrand assets, default flag, old screen deleted.

| # | Work | Gate |
|---|---|---|
| 2.1 | Downloads console-native; aggregate speed/ETA; history actions | Pause/resume/cancel/retry/reorder work with pad |
| 2.2 | Offline/last-library snapshot; console error surfaces; update notice | Airplane-mode boot keeps Home; errors are actionable |
| 2.3 | Rebrand assets: logo/icon/palette/type adoption, manifest/offline/Tauri icons/Sunshine art/README screenshots | Brand tests; screenshot set approved |
| 2.4 | Default `console_v2` on; delete `BigPictureScreen`; keep `/bigpicture` redirect; docs/CHANGELOG | Old route redirects; no dead imports; release check |
| 2.5 | Hardware/TV validation: Windows + Sunshine/Moonlight 1080p60/4K, Linux, controller hot-plug, sleep/quit | Manual checklist signed; memory budget met |

### Phase 3 — Optional (15–25 engineer-days)

- Multi-user profile picker (server contract + PIN/switch token).
- Achievements read-only + unlock notice; collections shelf.
- Social/friends presence on the status bar.
- Ambient/screensaver; store research (only with a backend).

### 8.1 Testing strategy

| Layer | What | How |
|---|---|---|
| Unit | Focus store/scopes, command routing, power capability, room migration, install selectors, running-game reducer | Vitest; retarget existing 191 console tests |
| Browser E2E | Boot, deep links, home→launch, install→downloads, settings, power, overlay single-dispatch, offline | Playwright + `page.addInitScript` stubbing `navigator.getGamepads`; fake pad frames |
| Visual | 1280×720, 1920×1080, 3840×2160, Cabinet + one custom room, overscan | Existing screenshot script + diffs; user approval gate |
| A11y | Roles/labels, focus ring, reduced motion/transparency/contrast, axe | Playwright + axe; keyboard-only pass |
| Native | WindowPolicy truth table, `quit_app`/`system_sleep` capability, close/quit, second-instance promotion | Rust unit tests + Windows/Linux manual |
| Streaming | Sunshine publish, Moonlight 1080p60/4K, overlay over game, controller | Manual checklist on hardware; memory measurement |

**Fake-pad harness sketch**

```ts
// e2e/pad.ts
await page.addInitScript(() => {
  const pad = {
    index: 0, id: "Xbox Wireless Controller (STANDARD GAMEPAD)",
    mapping: "standard", connected: true,
    buttons: Array.from({ length: 17 }, () => ({ pressed: false, value: 0, touched: false })),
    axes: [0, 0, 0, 0],
    timestamp: 0,
  };
  (window as any).__setPad = (index: number, value: number) => {
    pad.buttons[index].pressed = value > 0.5;
    pad.buttons[index].value = value;
  };
  Object.defineProperty(navigator, "getGamepads", { value: () => [pad] });
});
await page.evaluate(() => (window as any).__setPad(0, 1)); // A down
await page.waitForTimeout(80);
await page.evaluate(() => (window as any).__setPad(0, 0));
```

This tests the real loop, repeat tracker, focus ring and routing — not a mocked React hook.

### 8.2 Acceptance gates

1. `tsc --noEmit`, web build, unit tests green; no new arbitrary text sizes in `console/**`.
2. Console E2E suite (6–7 specs) green in CI, including overlay single-dispatch and fake-pad navigation.
3. Input matrix: pad (Xbox/PS/Switch), keyboard, mouse, TV remote; no dead ends; B always backs out.
4. Performance: no Console-wide >1 Hz idle/download re-render; 60 fps rail at 1080 p; no animated `backdrop-filter`; < 380 MB UHD / < 300 MB FHD.
5. 10-foot: 7:1 text, 24u floor/28u body/48u hero at 1080 p, 4K + overscan, reduced motion/transparency/contrast.
6. Native: `quit_app` + `system_sleep` implemented and honest; WindowPolicy truth table; console close quits; tray absent; Sunshine publish still works.
7. No desktop leaks: no toast viewport, PWA prompt, onboarding wizard, desktop dialog or AppShell inside `/console`.
8. Regression: desktop routes/screens unaffected; existing e2e green; `/bigpicture` redirects; no duplicate `/bigpicture` and `/console` implementations.

### 8.3 Anti-slop tests (skill §9)

Every rule that matters becomes a test:

- no text below 24u; no uppercase mono labels outside numerals;
- no six equal tiles/indexes; no layout picker; no Soft/pastel tokens;
- no `aria-modal` desktop dialog inside Console; no desktop route from Console;
- no toast in Console; one hint strip; one focus ring;
- 7:1 contrast; 1080p/720p/4K + overscan screenshots; keyboard/controller E2E.

A rule without a test will be gone in three releases.

### 8.4 Risk register

| Risk | Probability | Impact | Mitigation |
|---|---|---|---|
| Input owner refactor regresses desktop library/overlay | Medium | High | Migrate one consumer per PR; keep `useGamepad` adapter; fake-pad tests; manual hardware pass |
| Visual redesign bikeshedding | High | Medium | 3 reference screens approved before building all surfaces; freeze motion/sound |
| Route rename breaks Rust deep links | Medium | Medium | Grep all `/bigpicture`; keep redirect; update Rust in Phase 0 |
| Install selector lane regressions | Medium | High | Finish the in-flight tests; render-count assertion; provider facade for desktop |
| Locale remount loses shell state | Medium | Medium | URL + external stores; no durable component state |
| Scope creep (store/social/multi-user) | High | High | Locked v1 surface list; Phase 3 only |
| Blur vs contrast regressions | Medium | Medium | Matte text plates; worst-case art tests; no animated filters |
| 4K/iGPU performance | Medium | Medium | One static blur; density knob; hardware test early |
| Tauri multi-window temptation | Low | High | Explicit "not in v1" decision; revisit trigger documented |

---

## 9. Decision table

| # | Decision | Recommendation | Alternatives / trade-offs | Why |
|---|---|---|---|---|
| 1 | **Shell topology** | Top-level `/console/*` route tree in the same SPA, lazy subtree, console-scoped globals | A: inside AppShell (reject: desktop chrome); C: separate Tauri window (defer: duplicate cache/auth, window plumbing); D: separate HTML entry (optional later: bundle isolation, cold cache) | One auth boot, one query cache, one window; formalizes the current sibling route; lowest risk. |
| 2 | **Window model** | One Tauri `main` window; Rust owns policy; Console is a route, not a window | Separate `console` webview: independent fullscreen but duplicate React state, capability changes, Sunshine close complexity | Sunshine streams the desktop; a second window buys little and costs a lot. Revisit for multi-display. |
| 3 | **Boot / auth** | Shared `AuthProvider`; console gate; resolve `console_mode` before first route; native token; browser login; no pairing v1 | Separate auth stack (reject); TV PIN pairing (v2) | Native already has the token; browser login is enough for a companion mode. |
| 4 | **Input ownership** | One `InputProvider` + focus store + scope stack; one rAF loop; DOM focus as output; gilrs fallback if WebKitGTK fails | Keep per-screen loops (reject: double dispatch); geometric spatial nav (reject: fragile, untestable) | The current four-loop design already causes double actions with the overlay; scopes make it testable. |
| 5 | **Navigation state** | Nested routes own surface/selection; focus store owns focus; external stores for high-frequency/cross-shell state | One 2.7k screen (reject); global Redux/Zustand (reject: no need) | Survives locale remount; deep links; scoped re-renders; one ring. |
| 6 | **Data / installs** | Shared QueryClient; finish the in-flight install store; incremental `updateOne`; selectors; no `useDownloads()` in cards | Keep 1 Hz context (reject: product-wide rerenders); per-shell cache (reject: duplicate fetches) | Events already exist (`install://progress/jobs/done`); selectors remove the churn. |
| 7 | **Layouts / IA** | Stage + rail + band; shelf/grid in Library; no layout picker; collections as a shelf | Six layouts (reject: combinatorial focus/visual/QA cost); dashboard home (reject: slop) | The skill's IA is the product rule; Switch-like consoles are stage+rail+band. |
| 8 | **Visual system** | One identity, two densities; Cabinet default (warm charcoal, matte, 6/14/20/24, one blur, signal yellow, dot); delete Soft/pastels | Revert to old Soft (reject: cream/coral, contrast); separate console palette (reject: different product) | Gives the user rounded/blurred/cute without breaking contrast or brand continuity. |
| 9 | **Surface scope** | v1: Home, Library, Game, Downloads, Settings, Power, Search, controller profiles, Install; v1.5: Collections, Achievements; v2: Social, multi-user; store out | Ship everything (reject: no backend for store; social is not couch core) | Focus on launch/play/manage; the product's core loop. |
| 10 | **Native power/window** | Implement `quit_app`/`system_sleep`; Rust `WindowPolicy` + `window://state`; honest capabilities | Leave as-is (reject: dead Power rows); web-owned fullscreen (reject: #233 trap) | The Power screen must not lie; window state needs one owner. |
| 11 | **Migration** | `console_v2` flag; Phase 0 independent fixes; `/bigpicture` fallback; delete after parity + dogfood | Big-bang rewrite (reject: loses the kit and the tests); permanent dual shell (reject: drift) | Low-risk, reversible, shippable in slices. |

---

## 10. What explicitly not to do

1. **Do not rewrite motion, sound, haptics, or the controller profile engine.** They are the liked, tested assets; retarget their tests, do not replace them.
2. **Do not create a second palette.** No cream/coral Soft, no pastel accents, no "cute" palette unrelated to signal yellow.
3. **Do not put the Console inside `AppShell`.** It will inherit the rail, topbar, onboarding and desktop providers.
4. **Do not add a second Tauri window/webview in v1.** Duplicate query cache/auth, capability changes, Sunshine close semantics, double QA.
5. **Do not use DOM geometry as the spatial-navigation source of truth.** Keep explicit, pure focus graphs; DOM focus is an output.
6. **Do not keep six layouts or a layout picker.** Stage+rail+band, shelf/grid, shelf chips.
7. **Do not add Redux/Zustand/another global store.** React Query + a focus store + small selector stores are enough.
8. **Do not let desktop toasts, PWA prompts, onboarding, `InstallDialog`, or `/game/:id` leak into the Console.** Every couch flow must be console-native.
9. **Do not ship a store, social feed, or multi-user picker in v1.** No store backend; social/multi-user need server contracts and security design.
10. **Do not animate blur/filters or put `backdrop-filter` on scrolling cards.** One static wallpaper blur; matte text plates.
11. **Do not make a TV user type a server URL or password.** Use the native token; browser pairing is v2.
12. **Do not scatter `isNative()` through surfaces.** One capability matrix (`launch`, `install`, `download`, `sleep`, `quit`, `stream`, `overlay`, `fullscreen`) drives the UI.
13. **Do not remove `/bigpicture` before parity + dogfood.** Keep the redirect, then delete the old screen.
14. **Do not treat "more settings" as customization.** The skill's 4 rooms × 8 knobs is the contract; no per-element pickers or CSS injection.

---

## Appendix A — Evidence index (line numbers at the current tree)

| Topic | Evidence |
|---|---|
| Provider stack / router | `apps/web/src/app.tsx:100-108, 293-304` |
| Console route + late redirect | `apps/web/src/app.tsx:266-284, 328-334` |
| Desktop globals | `apps/web/src/app.tsx:482-485` |
| Desktop gate/login | `apps/web/src/app.tsx:171-211` |
| 2.7k screen / state / effects | `apps/web/src/screens/big-picture.tsx:211-2748`; 23 `useState`, 33 effects |
| Install dialog dead end | `apps/web/src/screens/big-picture.tsx:1200, 2729`; `components/game/install-dialog.tsx:238` |
| Console footer always rendered | `apps/web/src/screens/big-picture.tsx:2583` |
| Selector store (in flight) | `apps/web/src/state/downloads.tsx` working tree: `DownloadsStore`, `useAllJobs`, `useActiveJobs`, `useJob`, `useInstallingPercent`, `useQueueSummary`, `useTitleFor` |
| 2 s safety poll (in flight) | `apps/web/src/state/downloads.tsx` working tree (`LIST_POLL_INTERVAL_MS`) |
| Full-list refresh on progress (remaining) | `apps/web/src/state/downloads.tsx` staged listeners (`onInstallProgress(() => void refresh())`) |
| Memoised cards (in flight) | `components/big-picture/parts.tsx` working tree; `components/game/game-card.tsx` working tree |
| Lazy console/admin (in flight) | `apps/web/src/app.tsx` working tree (`lazy`, `Suspense`) |
| Four `useGamepad` loops | `big-picture.tsx:1623`, `library.tsx:356`, `overlay-screen.tsx:158`, `onboarding-wizard.tsx:151` |
| Overlay not a blocking layer | `components/overlay/overlay-screen.tsx:25-36, 270-278`; `lib/gamepad-core.ts:333-355` |
| Input routing | `components/big-picture/input.ts:40-209` |
| Zones/shell model | `components/big-picture/system-zones.ts:20-220` |
| Navigation hook | `components/big-picture/navigation.ts:1-271` |
| Controller identity/resolve | `lib/controller-identity.ts:1-60`; `lib/controller-devices.ts:1-60`; `lib/controller-resolve.ts:60-120` |
| Hints/glyphs | `components/big-picture/controller.ts:123-266` |
| 01–06 ornament | `components/big-picture/home-row.tsx:112-127` |
| Status bar clock/power glyph | `components/big-picture/system-bar.tsx:47-64, 126, 195` |
| Power actions / false wake copy | `components/big-picture/power.ts:37-130` (sleep copy `:57-60`) |
| Missing commands | `apps/web/src/lib/native.ts:535-548`; `docs/CONTRACT.md:3078`; `lib.rs:266-396` handler list |
| `exit_console` quits | `apps/desktop/src-tauri/src/commands.rs:4452-4463` |
| `restart_app` exists | `commands.rs:658`; `lib.rs:312` |
| Console promotion | `apps/desktop/src-tauri/src/console.rs:39-160, 232-330` |
| Close policy | `apps/desktop/src-tauri/src/lib.rs:137-163`; `overlay.rs:1423-1439` |
| Rust hardcodes `/bigpicture` | `console.rs:323-326`; `overlay.rs:1523` |
| Window state/fullscreen | `tauri.conf.json:13-27`; `big-picture.tsx:767-819`; `lib/window-chrome.ts` |
| Capabilities windows | `apps/desktop/src-tauri/capabilities/default.json` (`main`, `overlay`) |
| Sunshine publishing | `console.rs:147-152`; `streaming.rs:32-36, 836` |
| Console appearance default | `theme/console-appearance.ts:69-77, 82-120, 190-200` |
| Pastel accents to delete | `theme/console-appearance.ts:44-62` |
| Material/contrast | `styles/index.css:1017-1070, 1119-1141, 1381-1400`; `theme/console-contrast.test.ts:20-72` |
| Type scale / 20u micro | `styles/index.css:104-129`; `styles/type-scale.test.ts:28-70` |
| 200-game cap | `lib/hooks.ts:24-29`; `crates/ludex-server/src/routes/games.rs:315` |
| Settings duplication | `lib/settings-sections.ts:64-175`; `screens/big-picture.tsx:1712-1856` |
| Locale remount | `lib/i18n.tsx:680-686` |
| Update notifier `/bigpicture` | `components/updates/update-notifier.tsx:80-86, 145-160` |
| PWA/desktop skip | `lib/pwa.ts:13-18`; `public/manifest.webmanifest`; `public/sw.js` |
| Boot script hardcoded colours | `apps/web/index.html:105-130` |
| Offline page palette | `apps/web/public/offline.html` |
| Tauri background/icons | `apps/desktop/src-tauri/tauri.conf.json` |
| Brand wordmark | `apps/web/src/components/brand.tsx:1-45` |
| Admin-only user list | `apps/web/src/lib/api.ts:692` |
| No public profiles/switch | `crates/ludex-server/src/routes/auth.rs:40-56`; `community.rs:63-67` |
| New skill contract | `.opencode/skills/console-mode-ui/SKILL.md` (current working tree) |
| Missing blueprint | `docs/console/` referenced by the skill, absent from the tree |

---

## Appendix B — Open questions

1. **Cabinet vs the user's "cute" wording.** The skill is explicit: dark-first, warm charcoal, signal yellow only, no Soft/pastels. The user asked for "minimalist/cute Switch-like with blur/rounded". Cabinet satisfies rounded/blur/tactile, but not light/cream. Confirm the direction before building all surfaces; if the user wants light/cream, change the skill/blueprint first, not the architecture.
2. **Is browser TV a first-class target?** If yes, a separate `console.html` entry and a PWA shortcut/manifest become worth it; if no, same-SPA lazy chunk is enough.
3. **Is multi-user a v1 requirement?** If yes, the server contract (opt-in profiles + PIN/switch token) must be designed before the shell; it is not a UI-only feature.
4. **Gamepad on WebKitGTK/Tauri Linux TV:** does the API report `mapping === "standard"`, hot-plug and `timestamp` reliably? If not, schedule the Rust/gilrs fallback before locking visuals.
5. **Logo: evolve or keep?** Recommendation: keep the wordmark skeleton for Console 2.0; if a full identity change is wanted, ship it as a separate brand release before the Console, not mixed with the layout rewrite.
