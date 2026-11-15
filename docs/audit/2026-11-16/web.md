# Ludex web client — quality & cleanliness audit

Repo: `/root/ludex` @ `7199a2c` (`chore(release): v0.4.14`)
Scope: `apps/web/src/**`, `apps/web/scripts/**`, `apps/web/package.json`, Vite/TS config, `e2e/tests/**`
Mode: read-only. No repository file was modified. Build artefacts under `apps/web/dist` were only read (pre-existing, dated 2026-11-15).

---

## 0. Executive summary

The web client is **well above the average React codebase** in the areas that usually rot: i18n is tooled and gated, colour tokens are machine-enforced, the theme system is registry-driven with WCAG tests, Tauri command coverage is nearly exact, and there is no `any`, no `@ts-ignore`, no `dangerouslySetInnerHTML`, no TODO litter. The problems are concentrated elsewhere:

1. **No bundler-level code splitting.** Every one of the 35 screens, the 2 728-line Console screen and all 12 settings sections load in the entry chunk: **2 133 144 B raw / 575 780 B gzip** for `index-*.js`, plus a 308 KB `en.json` statically imported into it. No `lazy()`, no `manualChunks`.
2. **Two runtime-broken console actions shipped as enabled.** `quit_app` and `system_sleep` are exposed by two bridge methods (`native.ts:535,545`), wired to the console Power menu, and registered in **zero** `generate_handler!` entries (see §5.1). "Sleep this PC" always fails.
3. **A 1 Hz context that re-renders the whole product.** `DownloadsProvider` publishes a new context value every second while any download runs; 8 consumers include `LibraryScreen` and `BigPictureScreen`, and there is **not a single `React.memo` in the codebase** — 200 game cards re-render and repaint inline SVG covers once per second.
4. **`lib/api.ts` and `lib/native.ts` are god modules**: 168 API methods and 193 native methods with inline mock/real branches, in 1 433 + 1 564 lines, with no request timeout/abort anywhere.
5. **Zero component-level tests.** 72 unit test files (11 028 lines) cover pure helpers; 0 render tests; 5 Playwright specs (196 lines + an 83-line helper) for 35 screens. Regression safety on UI is effectively E2E-only, and the E2E suite has one test per file.

Confirmed defects with evidence are in §5; the top 25 refactors with impact/effort/risk are in §8.

---

## 1. Layer map

### 1.1 Boot & composition

| Layer | File | Notes |
|---|---|---|
| HTML shell + first-paint theme/locale replay | `apps/web/index.html:1-150` | Synchronous inline script reads `ludex.theme` / `ludex.locale`, sets CSS vars, `lang`, `dir`, `theme-color` before paint. Well documented; duplicated locale-resolution order with `lib/i18n.tsx` is guarded by a build check (`RTL_BASES` parity). |
| Entry | `apps/web/src/main.tsx:1-25` | `bootstrapI18n()` awaited, then one root `ErrorBoundary`, `initPwa()`. No route-level boundary. |
| App root / providers | `apps/web/src/app.tsx:250-454` | `QueryClientProvider → Settings → Toast → Auth → Preferences → Downloads → Upload → I18n → Update → Router`. Note the asymmetric JSX indentation at `app.tsx:260-263` / `443-449` (providers drift right then close in mirror order) — cosmetic but makes the tree hard to read. |
| Query defaults | `app.tsx:63-71` | `retry: 1`, `staleTime: 20s`, `refetchOnWindowFocus: false`. No `gcTime`, no `networkMode`, no error handler, no global `onError`. |
| Routing | `app.tsx:269-439` | `HashRouter`. Public routes (`/share/:token`, `/collection-share/:token`, `/download`, `/register`) are **outside** the auth gate by design; everything else is under `Gate` + `AppShell` `Outlet`. |
| Auth/setup gate | `app.tsx:134-168` | Single `Gate` handles `!ready`, `bootError`, `needsSetup`, unauthenticated, and the `#157` desktop-handoff hold. Dense but correct and well commented. |
| Role gates | `app.tsx:170-180` | `RequireEditor` / `RequireAdmin` silently redirect to `/` (no "not allowed" state). |
| Console entry | `app.tsx:229-248` | `ConsoleEntry` probes `native.consoleMode()` on `/` and redirects to `/bigpicture`. |
| Overlay entry | `app.tsx:253`, `components/overlay/overlay-window.tsx:18-80` | Same bundle, `?overlay=1` → tiny provider tree (Query + I18n only), transparent document. |
| Global side-effects | `app.tsx:189-221` (`ConfigSyncDefaults`), `lib/hooks.ts:225-341` (`useNativeEvents`) | Both mount once under the shell; 5 Tauri listeners + 2 timers in `useNativeEvents`. |

### 1.2 Screens (35)

Authenticated shell screens: `home`, `library` (4 modes: library/recent/bookmarks/index), `downloads`, `remote`, `game-detail`, `collections`, `collection`, `community`, `friends`, `profile`, `news`, `settings`, `updates`, `not-found`.
Auth/public: `login`, `register`, `setup`, `share`, `collection-share`, `download`.
Admin (13): `admin/ingest`, `admin/scan`, `admin/games`, `admin/users`, `admin/news`, `admin/server`, `admin/observability`, `admin/authentication`, `admin/metadata`, `admin/integrations`, `admin/backups`, `admin/upload`, `admin/health`, `admin/invitations`.
Console: `big-picture`.

### 1.3 Feature components (161 files, 39 522 LOC)

`shell/` (9) app frame, nav rail/mobile, command palette, server switcher, window controls, shortcuts, uploads.
`game/` (35) detail panels, install pipeline UI, saves, settings, emulators, streaming, VirusTotal.
`settings/` (30) one panel per concern + the `sections-*.tsx` composition layer + `use-desktop-settings.ts` controller.
`library/` (3) filters, search, external imports.
`collections/` (5), `community/` (5), `updates/` (5), `overlay/` (3), `onboarding/` (2), `admin/` (1), `gamepad/` (1), `art/` (1), `theme/` (1), `ui/` (7 primitives).
`big-picture/` (49 files, 3 954 LOC tsx + 3 251 LOC ts logic): 6 layouts, overlays, navigation, motion hub, controller prefs, sound, power. This is the one area with real unit coverage (12 test files).

### 1.4 State (6 providers, 2 159 LOC)

`auth` (335), `downloads` (662), `preferences` (640, account sync + conflict/offline state machine), `settings` (350, theme/accent/view), `toasts` (54), `upload` (118), plus `updates/update-provider.tsx` (327) and `lib/i18n.tsx:626` (`I18nProvider`).

### 1.5 Lib / clients (78 non-test files, 24 932 LOC)

`types.ts` (4 169, hand-written mirror of `docs/CONTRACT.md`), `mock.ts` (4 155) + `mock-install.ts` (905) + `data/demo.ts` (853), `native.ts` (1 564), `api.ts` (1 433), `i18n.tsx` (701), `gamepad.ts` + `gamepad-core.ts` (1 111), `preferences.ts` (416), `navigation.ts` (403), `updates.ts` (400), `ui-sound*` (622), `hooks.ts` (342), theme helpers, formatting, install pipeline helpers (`eta`, `install-phase`, `install-queue`, `install-retry`, `install-history`, `install-transport`).

### 1.6 Theme / styles

`theme/themes.json` (registry, source of truth) → `scripts/theme.mjs` → `styles/themes.generated.css`; `theme/storage.ts` writes the pre-paint payload; `theme/derive.ts` + `contrast.ts` derive accent/tint and check WCAG; `state/settings.tsx` paints tokens on `documentElement`; `components/theme/theme-scope.tsx` scopes a dark stage to artwork screens. `styles/index.css` (1 548) holds `@theme inline` mappings, console materials, motion tokens.

### 1.7 i18n

English message ids are the keys (3 864 ids × 13 shipped locales). `lib/i18n.tsx` + `scripts/i18n.mjs` (gate) + `scripts/i18n-lint.mjs` (advisory hardcoded scan) + `scripts/i18n-pseudo.mjs` (`en-XA`). Catalogs are `import.meta.glob` lazy chunks except `en.json`, which is static.

### 1.8 Tauri bridge

`lib/native.ts` — 193 thin wrappers over 166 distinct `invoke` commands, one automatic `diagnostics_record_error` on failure (`native.ts:120-140`), browser fallbacks that either no-op or reject with `nativeUnavailable()`. `lib/install.ts` is a second adapter layer (mock vs native) for the install queue.

### 1.9 Tests

69 unit files (`lib/` 52, `components/big-picture/` 12, `styles/` 4, `theme/` 3, `settings/` 1), 5 Playwright specs, 3 test files for mocks/avatars/etc. No component renderer, no `@testing-library/*` dependency at all.

---

## 2. Per-file inventory

Legend — **Q**: quality 1–5. **P**: priority (P0 broken/security, P1 user-visible debt, P2 maintainability, P3 polish).
Issue codes: `GIANT` oversized unit, `STATE` state sprawl, `DUP` duplicated logic, `I18N` hardcoded copy, `PERF` render/perf, `A11Y` accessibility, `EFFECT` effect/timer race, `TYPES` type drift/weakness, `DEAD` dead/stale code, `COUPLE` excessive coupling, `NOMO` no memo/derivation, `QKEY` query-key literal.

### 2.1 App shell / routing / providers

| Path | LOC | Role | Props / state surface | Coupling | Q | Issues | P |
|---|---|---|---|---|---|---|---|
| `app.tsx` | 454 | Root, router, gates, boot screens | 6 small components; `Gate` reads `useAuth`+`useLocation`; `ConfigSyncDefaults` has module-global `lastSyncedDefaults:187` | imports 40+ modules, all screens eagerly | 3 | GIANT-ish, no lazy routes, global mutable `lastSyncedDefaults`, `BootError` prints `api: configured` (`:128`) | P1 |
| `main.tsx` | 25 | Entry | — | pulls all 3 fonts + `index.css` | 4 | single root boundary only | P2 |
| `state/auth.tsx` | 335 | Session, setup, 2FA, invitation, OIDC, locale | 8 `useState`, `useMemo` value of 18 members | `api`, `native`, `servers`, `i18n` | 4 | boot effect does 4 sequential network calls (`status`→`config`→`me`) with no abort on unmount; token in `localStorage` | P1 |
| `state/downloads.tsx` | 662 | Install queue + ETA sampling → context | 5 `useState`, 3 refs, 1 query; publishes 21 members | `installBridge`, `native`, `toasts`, `eta`, `install-queue`, `install-history` | 3 | **`jobsRef.current = jobs` during render (`:181`)** — React anti-pattern; **1 Hz `setTransfer` → whole context subtree re-renders** (`:178`, `SAMPLE_INTERVAL_MS` in `lib/eta.ts:28`); `invalidateQueries()` without key on local-server status (`lib/hooks.ts:268`) nukes the entire cache | P0 |
| `state/preferences.tsx` | 640 | Account preference document sync (rev/409/offline) | 9 `useState`+refs, status machine | `api`, `settings`, `auth`, `server-storage` | 4 | debounce/visibility/online listeners dense but well documented; largest non-visual provider | P2 |
| `state/settings.tsx` | 350 | Theme selection, accent/tint derivation, library view | 1 storage read + derived tokens | `theme/*`, `localStorage` | 4 | writes `ludex.libraryView` outside the storage helpers used elsewhere | P3 |
| `state/toasts.tsx` | 54 | Toast queue | array + timeout ids | none | 4 | no cap on simultaneous toasts | P3 |
| `state/upload.tsx` | 118 | Chunked upload orchestration | 2 states + AbortController | `api` | 4 | — | P3 |

### 2.2 Screens

| Path | LOC | Role | State / props | Coupling | Q | Issues | P |
|---|---|---|---|---|---|---|---|
| `screens/big-picture.tsx` | 2728 | Console shell: 6 layouts, 5 overlays, gamepad router, power, launch plate, sound | **23 `useState`, 31 `useEffect`, 22 `useRef`**; no props | ~40 imports; every big-picture module + 4 providers + gamepad | 2 | GIANT; STATE; the `level/zone/homeIndex/appIndex/settingsSection/overlay` cluster is a hand-rolled reducer; `systemSleep()` dead action (`:1092`); raw English toast `"Fullscreen unavailable"` (`:767`); 3 `window.setTimeout(...,0)` focus hacks (`:884,904,922`) | P0 |
| `screens/setup.tsx` | 981 | First-run wizard (desktop local/remote + browser) | 27 `useState` across 6 stages | `api`, `native`, `auth`, `setup-auth` | 3 | GIANT; STATE; stage machine implicit in booleans; 2 inert `eslint-disable` (`:601`); duplicate URL normalisation vs `lib/servers.ts` | P1 |
| `screens/library.tsx` | 954 | Library (4 modes), filters, imports, grid/list/index | 5 `useState`, 5 `useEffect`, 5 queries | filters, library-entries, downloads, gamepad | 3 | query key embeds the whole query object (`:177`); filter-restore effect with inert `eslint-disable` (`:160-171`); 4 parallel list requests on open (games + missing-count + genres + platforms); **200 cards in one `.stagger-in` grid (`:638`) with no virtualization**; consumes `useDownloads()` (`:119`) → 1 Hz rerender | P0 |
| `screens/admin/games.tsx` | 915 | Admin bulk manager + drawer | 7 states, inline `Drawer` (`:110`) | `admin-games`, `api` | 3 | GIANT; drawer/bulk/confirm logic in one file; `eslint-disable` `:366` | P2 |
| `screens/game-detail.tsx` | 890 | Game page: hero, tabs/panels, state picker, menus | 14 `useState` | 20+ game components, `useDownloads()` (`:132`) | 3 | GIANT; section/panel switching + drawer state in one component; 1 Hz rerender during downloads | P1 |
| `screens/downloads.tsx` | 779 | Install queue UI + 2 SVG graphs | 8 states | `useDownloads`, `eta` | 3 | hand-rolled `TransferGraph` (`:79-140`) — fine, but a second chart exists in `admin/observability.tsx:80` with no shared primitive | P2 |
| `screens/admin/metadata.tsx` | 698 | Provider list + candidates + apply | 11 `useState` | `api`, `metadata-match` | 3 | candidate list keyboard handling duplicated from `match-metadata.tsx` | P2 |
| `screens/admin/authentication.tsx` | 691 | OIDC/TOTP/password policy admin | 8 states | `api` | 3 | form validation copy repeated from `security-panel.tsx` | P2 |
| `screens/admin/observability.tsx` | 689 | Metrics, diagnostics, anonymous reports | 8 states | `api`, `observability` | 3 | second inline SVG chart (`:80`) | P2 |
| `screens/admin/ingest.tsx` | 608 | Upload/ingest jobs | 11 states, 3 polls (2 s/5 s/10 s) | `api`, `upload` | 3 | 3 overlapping `refetchInterval`s | P2 |
| `screens/admin/users.tsx` | 580 | Users CRUD, reset, invite | 5 states + `EditUserDialog` | `api` | 4 | `roleOptions()` duplicated in `invitations.tsx:15`; `"No limit"` placeholder untranslated (`:476`) | P2 |
| `screens/admin/backups.tsx` | 571 | Backup list/create/restore/verify | 6 states | `api` | 4 | — | P3 |
| `screens/admin/health.tsx` | 554 | Library health actions | 6 states | `api` | 4 | — | P3 |
| `screens/login.tsx` | 505 | Login, 2FA, OIDC, handoff | 9 states | `auth`, `setup-auth` | 4 | 2 inert `eslint-disable` (`:102,130`); duplicated field validation with `register.tsx` | P2 |
| `screens/collection.tsx` | 474 | Collection detail/editor | 7 states | `collections`, `shares` | 4 | — | P3 |
| `screens/download.tsx` | 453 | Public desktop download page | 5 states | `desktop-download` | 4 | — | P3 |
| `screens/home.tsx` | 442 | Hero, recent, shelves, onboarding card | 3 states, 7 queries | `useAllGames`, `useRecentGames`, `collections` | 3 | `sourceLabel()` (`:28-52`) duplicates `lib/launchers.ts` labels; 1 Hz rerender via `useDownloads` (`:65`) | P2 |
| `screens/remote.tsx` | 421 | Mobile companion pairing | 6 states + polls | `remote-play`, `api` | 4 | `deviceLabel(navigator.userAgent)` in `useMemo` (`:79`) | P3 |
| `screens/admin/integrations.tsx` | 418 | Webhooks/integrations | 6 states | `api` | 4 | `localStorage` used directly (`:?`) instead of a helper | P3 |
| `screens/share.tsx` | 372 | Public share page | 4 states | `shares` | 3 | **raw English titles** (`:318 "This link is not available"` …) | P1 |
| `screens/friends.tsx` | 338 | Friends/presence/activity | 5 states, 2 polls | `api` | 4 | — | P3 |
| `screens/collection-share.tsx` | 322 | Public collection page | 3 states | `collection-shares` | 4 | — | P3 |
| `screens/admin/upload.tsx` | 311 | Chunked upload admin | 6 states | `api`, `upload` | 4 | — | P3 |
| `screens/profile.tsx` | 254 | Public profile + feed | 3 queries | `api` | 4 | untranslated fallback strings (`:198,245`) | P3 |
| `screens/register.tsx` | 219 | Invitation landing | 5 states | `auth` | 4 | validation duplicated with `login.tsx` | P2 |
| `screens/admin/news.tsx` | 201 | News CRUD | 2 states + `NewsEditor` | `api` | 3 | raw English toasts (`:154`) | P2 |
| `screens/admin/server.tsx` | 195 | Server config/folders | 2 queries | `api` | 4 | health poll 30 s even when tab hidden (`:25`) | P3 |
| `screens/community.tsx` | 173 | Feed | 2 queries | `api` | 4 | — | P3 |
| `screens/admin/scan.tsx` | 143 | Scan status | 1 poll | `api` | 4 | — | P3 |
| `screens/settings.tsx` | 134 | Settings shell (URL-driven section) | 1 state + `useDesktopSettings` | 12 panels | 5 | exemplary: taxonomy in `lib/settings-sections.ts`, normalises `?section=`, no prop drilling beyond one controller | — |
| `screens/news.tsx` | 93 | News list | 1 query | `api` | 4 | — | P3 |
| `screens/collections.tsx` | 92 | Collections index | 1 query | `api` | 4 | — | P3 |
| `screens/updates.tsx` | 42 | Update centre host | — | `updates` | 4 | — | P3 |
| `screens/not-found.tsx` | 25 | 404 | — | router | 5 | — | — |

### 2.3 Components — shell

| Path | LOC | Role | Q | Issues | P |
|---|---|---|---|---|---|
| `shell/app-shell.tsx` | 384 | Frame, topbar, nav, downloads strip, shortcuts listener | 4 | `DownloadsStrip` (`:224`) is the only `useDownloads` consumer here (good), but `OnboardingWizard`/`DesktopAppPrompt`/`CommandPalette` mounted on every route (`:374-380`); global keydown effect depends on `paletteOpen` (`:333`) → re-binds on every toggle | P2 |
| `shell/server-switcher.tsx` | 673 | Server menu + 2 dialogs + panel | 2 | GIANT; **19 `useState`**; three components (switcher/panel/form/manager) in one file (`:48,138,335,514`); `urlProblemMessage` duplicated with `lib/server-storage.ts` semantics | P1 |
| `shell/command-palette.tsx` | 448 | ⌘K palette | 4 | loads its own `["palette-games"]` copy of the library (`:75`); item construction ~200 lines; hand-rolled listbox keyboard handling (correct, but duplicated with `search-field`/`settings-nav`) | P2 |
| `shell/window-controls.tsx` | 197 | Custom title bar for Win/Linux | 3 | 3 `console.warn` in IPC failure paths (`:93,99,105`) — acceptable but inconsistent with the diagnostics recorder used elsewhere | P3 |
| `shell/nav-rail.tsx` | 195 | Desktop rail | 4 | `localStorage` read inline (`:~?`), badge via `useDownloads` | P3 |
| `shell/nav-mobile.tsx` | 187 | Phone tabs + sheet | 4 | — | P3 |
| `shell/nav-copy.ts` | 74 | Nav labels | 5 | clean key→`t()` table | — |
| `shell/nav-icons.tsx` | 57 | Icon map | 5 | — | — |
| `shell/shortcuts-dialog.tsx` | 82 | Help modal | 4 | shortcut list duplicates `app-shell.tsx:281-330` logic in prose | P3 |
| `shell/upload-indicator.tsx` | 71 | Upload progress chip | 4 | — | — |

### 2.4 Components — game

| Path | LOC | Role | Q | Issues | P |
|---|---|---|---|---|---|
| `game/cloud-saves.tsx` | 1020 | Cloud saves panel: status, snapshots, conflict, restore | 2 | GIANT; **21 `useState`** in one component; **~15 raw-English toast titles/descriptions** (`:345-427`) that the i18n lint cannot see; `console.debug` in a user path (`:209`); 5 inert `eslint-disable` (`:168,192,217,230`) masking dep arrays | P0 |
| `game/game-settings.tsx` | 1012 | Per-game config: adapters, files, common fields, backups | 2 | GIANT; 6 sub-components + 14 states; `fieldLabel`/`confidenceLabel` hardcode English labels for config fields | P1 |
| `game/make-it-run.tsx` | 1000 | Runtime diagnosis/apply wizard | 2 | GIANT; 8 `useState`; 7 pure label/detail mappers (`:54-287`) should be `lib/make-it-run.ts` (only 120 LOC there today) | P1 |
| `game/virustotal.tsx` | 638 | VT status/scan/upload + queue hook | 3 | local `tones` map duplicated (`:102`) with `admin/virustotal-scan.tsx:173` and `ui/controls.tsx:213`; 2 hooks exported from a component file (`:43`) | P2 |
| `game/launch-options.tsx` | 517 | Wine/proton/runtime launch config | 3 | raw English toasts (`:200-219`) | P1 |
| `game/match-metadata.tsx` | 513 | Manual metadata match + candidates | 3 | candidate keyboard navigation reimplemented (also in `admin/metadata.tsx`) | P2 |
| `game/installer-panel.tsx` | 496 | Installer choice/run | 3 | callback prop `onEvent?: (e) => void \| Promise<void>` shows up in i18n lint as text `"void \| Promise"` (`:21`) — type annotation leaking into scanned source | P3 |
| `game/install-dialog.tsx` | 486 | Install options dialog | 3 | consumes `useDownloads()`; install-location + transport + stream options crammed | P2 |
| `game/media-gallery.tsx` | 469 | Screenshots/trailers lightbox | 3 | falls back to deprecated `game.trailers` (`:327`); focus trap hand-rolled | P2 |
| `game/version-manager.tsx` | 362 | Versions, rollback, pin | 4 | — | P3 |
| `game/adopt-install.tsx` | 350 | Adopt existing folder | 4 | — | P3 |
| `game/executable-picker.tsx` | 319 | Executable scan/pick | 4 | exports `useGameExecutables` (`:29`) from a component file | P3 |
| `game/install-files.tsx` | 309 | Manifest/verify/repair rows | 3 | raw English strings (`:77,102`) | P1 |
| `game/game-overview.tsx` | 211 | Detail hero copy/facts | 4 | — | P3 |
| `game/achievements.tsx` | 205 | Achievements panel | 3 | raw English empty copy (`:46`) | P2 |
| `game/artwork-dialog.tsx` | 197 | Artwork override/reset | 3 | raw English confirm title/desc (`:190-192`) | P2 |
| `game/delete-adopted.tsx` | 195 | Unlink/delete adopted install | 4 | — | P3 |
| `game/stream-dialog.tsx` | 193 | Moonlight stream launch | 4 | — | P3 |
| `game/move-install.tsx` | 175 | Move installation | 4 | — | P3 |
| `game/startup-command.tsx` | 173 | Args/env editor | 4 | — | P3 |
| `game/game-card.tsx` | 158 | Library card | 3 | **no `memo`**, 3 inline closures per card, called 200×/grid; badge/format logic inline | P1 |
| `game/missing-archive.tsx` | 134 | Missing-archive explainer | 4 | — | P3 |
| `game/rom-panel.tsx` | 123 | ROM/emulator per game | 4 | — | P3 |
| `game/remote-install.tsx` | 123 | Remote queue instructions | 4 | — | P3 |
| `game/game-nav.tsx` | 120 | Section tabs | 4 | — | P3 |
| `game/device-picker.tsx` | 101 | Cloud-save device selector | 3 | direct `localStorage`; exports hook only | P3 |
| `game/protondb-badge.tsx` | 81 | ProtonDB tier | 4 | — | P3 |
| `game/remote-queue-strip.tsx` | 69 | Remote queue chip | 4 | — | P3 |
| `game/state.tsx` | 5 | `STATE_TONE` table | 5 | — | — |

### 2.5 Components — settings

| Path | LOC | Role | Q | Issues | P |
|---|---|---|---|---|---|
| `settings/remote-play-panel.tsx` | 1078 | Remote-play wizard (UPnP/VPN) | 2 | **largest non-screen component**; 15 `useState`; wizard machine spread over booleans; port-purpose table hardcoded (`:37-45`) | P1 |
| `settings/sections-device.tsx` | 813 | 7 settings bodies in one file | 3 | GIANT; `StorageBody`…`CompatibilityBody` all here (`:83…721`); one inert `eslint-disable` (`:745`) | P1 |
| `settings/installed-games.tsx` | 685 | Installed list + actions | 3 | 10 states; duplicate row layout vs `library` list view | P2 |
| `settings/streaming-panel.tsx` | 675 | Sunshine/Moonlight admin | 3 | 8 states; raw-English port labels | P2 |
| `settings/appearance-panel.tsx` | 648 | Theme/accent/palette editor | 3 | 8 states; contrast readouts inline | P2 |
| `settings/overlay-panel.tsx` | 624 | Overlay settings | 3 | polls status every 5 s (`:47`) | P2 |
| `settings/launcher-import.tsx` | 620 | Steam/Epic/… import | 3 | 10 states; i18n lint false positive `"new Set"` (`:67`) indicates generated code leaking into JSX | P2 |
| `settings/locations-panel.tsx` | 510 | Install/download locations | 3 | 10 s poll (`:31`) | P3 |
| `settings/sections-account.tsx` | 418 | Account/appearance/profiles/help bodies | 3 | GIANT-ish; `HelpBody` is the diagnostics entry point | P2 |
| `settings/security-panel.tsx` | 393 | Password/TOTP/sessions | 4 | validation duplicated with `admin/authentication.tsx` | P3 |
| `settings/controller-panel.tsx` | 379 | Controller profiles | 4 | — | P3 |
| `settings/diagnostics-panel.tsx` | 366 | Anonymous report preview/send | 4 | — | P3 |
| `settings/save-discovery-panel.tsx` | 365 | Save path detection | 4 | — | P3 |
| `settings/save-manifest-panel.tsx` | 256 | Ludusavi manifest | 4 | — | P3 |
| `settings/controller-manager.tsx` | 255 | Pad fingerprint/manager (#230) | 4 | 10 s poll when enabled | P3 |
| `settings/compatibility-panel.tsx` | 219 | Compat scan | 4 | — | P3 |
| `settings/rom-emulators.tsx` | 211 | Emulator list/commands | 3 | untranslated placeholder `"snes, gba"` (`:144`) | P3 |
| `settings/language-panel.tsx` | 208 | Locale picker + coverage | 5 | — | — |
| `settings/avatar-panel.tsx` | 203 | Avatar upload/crop | 4 | — | P3 |
| `settings/settings-section.tsx` | 197 | Section chrome | 4 | — | P3 |
| `settings/console-appearance-panel.tsx` | 185 | Studio/Soft material picker | 4 | — | P3 |
| `settings/use-desktop-settings.ts` | 163 | Shared desktop-settings controller | 5 | good: one controller, no context duplication | — |
| `settings/settings-nav.tsx` | 139 | Section nav + search | 4 | — | P3 |
| `settings/preferences-sync.tsx` | 132 | Sync status/conflict UI | 4 | — | P3 |
| `settings/desktop-update.tsx` | 86 | Desktop updater | 4 | — | P3 |
| `settings/settings-copy.ts` | 65 | Label/description helpers | 5 | — | — |
| `settings/presence-status.tsx` | 44 | Presence dot | 4 | 5 s poll while mounted | P3 |

### 2.6 Components — the rest

| Path | LOC | Role | Q | Issues | P |
|---|---|---|---|---|---|
| `art/cover.tsx` | 514 | Procedural SVG covers + 3 plate variants | 3 | `Motif` builds up to ~330 SVG nodes per cover (halftone 11×15); `<img onError>` hides itself leaving a blank box (`:378-380`), no motif fallback; `GameBackdrop` uses `loading="eager"` (`:473`) on every backdrop; three near-identical wrappers (`GameCover`/`ArtPlate`/`GameBackdrop`) repeat spec/seed/src | P1 |
| `updates/update-center.tsx` | 480 | Update centre | 3 | two `role="alert"` blocks; changelog rendering duplicated in `parts.tsx` | P2 |
| `updates/update-notifier.tsx` | 432 | Launch prompt/badge | 3 | 413-line component with its own dialog + toast; polls + version compare inline | P2 |
| `share/share-dialog.tsx` | 430 | Share link creation | 3 | 2× `role="alert"`; token list + expiry controls in one component | P2 |
| `collections/collection-dialog.tsx` | 394 | Create/edit collection | 3 | form state + validation duplicated with `collection-share-panel` | P2 |
| `updates/update-provider.tsx` | 327 | Updater context | 4 | — | P3 |
| `shell/../overlay/overlay-screen.tsx` | 692 | Overlay panel (4 tabs) | 3 | GIANT; 8 states; 2 s status poll + 2.5 s second poll (`:88,533`); tab content inline | P1 |
| `library/filter-panel.tsx` | 287 | Filter drawer | 4 | — | P3 |
| `library/external-games.tsx` | 273 | Launcher import cards/rows | 4 | — | P3 |
| `big-picture/*` (49 files) | 7 205 | Console layouts, overlays, motion, prefs | 3–4 | split is good; the **screen** is the problem, not the primitives | P2 |
| `onboarding/onboarding-wizard.tsx` | 590 | 20-step wizard | 3 | own poll 4 s; step content inline; mounted globally from `app-shell` | P2 |
| `admin/virustotal-scan.tsx` | 231 | VT queue hook + panel | 3 | third `tones` map | P3 |
| `ui/controls.tsx` | 312 | Button/Input/Field/Switch/Checkbox/Badge/Progress/Segmented | 4 | local `tones` map (`:213`); `Segmented` hides labels with `sr-only` at `sm` (`:307`) — icons-only segments rely on the group label | P3 |
| `ui/dialog.tsx` | 204 | Modal + `ConfirmDialog` | 4 | **`confirmLabel = "Confirm"` untranslated default (`:166`)**; focus trap is good; restores focus to trigger | P2 |
| `ui/popover.tsx` | 229 | Anchored popover/menu | 4 | own focus management; no `aria-activedescendant` strategy parity with palette | P3 |
| `ui/select.tsx` | 186 | Select + `useDropdown` | 4 | **`placeholder = "Select"` untranslated (`:103`)** | P2 |
| `ui/toast.tsx` | 61 | Toast viewport | 4 | `role="status"` per toast (correct) | — |
| `ui/tooltip.tsx` | 110 | Tooltip | 4 | — | — |
| `ui/elevation-confirm.tsx` | 88 | Elevation (sudo) confirm | 3 | password field + copy inline | P3 |
| `page.tsx` | 132 | `PageHeader`/`EmptyState`/`ErrorState` | 3 | **`title = "That didn't load"` untranslated default (`:73`)**; `ErrorState` unconditionally `role="alert"` | P2 |
| `auth-layout.tsx` | 91 | Auth shell | 4 | — | P3 |
| `public-page.tsx` | 40 | Public route scroll unlock | 5 | — | — |
| `brand.tsx` | 53 | Wordmark/logo | 5 | — | — |
| `pwa-prompts.tsx` | 76 | Install/update prompts | 4 | — | P3 |
| `error-boundary.tsx` | 92 | Root boundary + global capture | 4 | class component; single root boundary only; inline error copy is `t()` (good) | P2 |
| `desktop-app-prompt.tsx` | 126 | "Get the desktop app" banner | 4 | — | P3 |
| `theme/theme-scope.tsx` | 44 | Dark stage scoping | 5 | — | — |
| `gamepad/gamepad-debug.tsx` | 74 | Pad diagnostics overlay | 3 | untranslated `aria-label`/text (`:13,20,51`) | P3 |
| `community/*` (5 files) | 499 | Bell/activity/presence/avatar/showcase | 4 | — | P3 |
| `collections/*` others | 334 | Card/share panel/add | 4 | — | P3 |

### 2.7 Lib (non-test)

| Path | LOC | Role | Q | Issues | P |
|---|---|---|---|---|---|
| `types.ts` | 4169 | Hand-written API/Tauri mirror | 3 | no codegen; 147 importers; **dangling doc comment at `:2199-2200`** (a `/** steam entries… */` with no field); optional-vs-required drifts possible silently | P1 |
| `mock.ts` | 4155 | 155-method in-browser backend | 3 | DCE'd out of prod (verified); still 4 155 LOC to keep in sync by hand; several API methods bypass it with inline fake objects (`api.ts:288-300,735-741,769-805`) | P2 |
| `native.ts` | 1564 | 193 Tauri wrappers | 3 | GIANT; `quit_app`/`system_sleep` invoke unregistered commands (`:529-547`); every wrapper repeats `if (!isNative()) return …`; automatic `diagnostics_record_error` on **every** rejected command (`:120-140`) can amplify a failing loop | P0 |
| `api.ts` | 1433 | 168 methods, inline mock branch per method | 2 | GIANT; **no timeout/`AbortSignal` on `fetch` (`:204`)** → a hung server hangs boot forever; two request engines (`fetch` + 2 XHR helpers `:1337-1433`); `ApiError` lives in `http.ts` but is re-exported here, two import paths | P0 |
| `mock-install.ts` | 905 | In-browser install manager | 3 | imports `DEMO_GAMES` → **853-line demo library ships in prod** (verified `"Chrono Trigger"` in the built bundle) | P1 |
| `i18n.tsx` | 701 | Source-keyed i18n runtime + provider | 4 | `en.json` (308 KB) statically imported; unused exports `formatRelativeValue`, `formatListValue`, `getLocale` | P2 |
| `gamepad.ts` | 562 | Gamepad navigation hook | 4 | 100 ms debug frame + 1 s trace, both flag-gated; RAF loop well bounded | P2 |
| `gamepad-core.ts` | 549 | Pure pad logic | 5 | tested | — |
| `upload.ts` | 445 | Chunked upload state machine | 4 | — | P3 |
| `preferences.ts` | 416 | Preference schema/sanitise/cache | 5 | tested | — |
| `navigation.ts` | 403 | Nav model + shortcuts | 4 | inert `eslint-disable` (`:206`) | P3 |
| `updates.ts` | 400 | Version compare/changelog parse | 4 | tested | P3 |
| `ui-sound.ts` / `ui-sound-specs.ts` | 382 / 240 | Cue synthesis/spec table | 4 | `fetch(cueUrl)` in `:270` with no abort | P3 |
| `virustotal.ts` | 350 | VT helpers | 4 | — | P3 |
| `hooks.ts` | 342 | Shared queries/mutations | 3 | `useGenres` refetches the full library under a different key (`:97-106`); `useInstalledMap` builds a new `Map` every render (`:92-95`); `useNativeEvents` invalidates the entire cache on one event (`:268`) | P1 |
| `controller-devices.ts` / `controller-profiles.ts` / `controller-identity.ts` / `controller-resolve.ts` / `controller-fallback.ts` | 317/256/195/159/… | Controller stack | 4 | five modules for one domain, but each tested | P3 |
| `server-storage.ts` | 311 | Multi-server profiles/tokens | 4 | token scoping logic is the security-sensitive part; tested | P2 |
| `library-filters.ts` | 304 | Filter parse/serialise | 5 | tested | — |
| `settings-sections.ts` | 276 | Settings taxonomy | 5 | model of how the rest should be structured | — |
| `overlay.ts` | 266 | Overlay helpers | 4 | tested | — |
| `library-entries.ts` | 253 | Merge server+installs | 4 | tested | — |
| `eta.ts` | 239 | Speed/ETA sampling | 4 | tested; 1 Hz cap drives the provider rerender | P2 |
| `format.ts` | 212 | Byte/time/rating formatting | 5 | locale-aware via i18n | — |
| `desktop-download.ts` | 198 | Public download page model | 4 | tested | — |
| `servers.ts` | 160 | Server CRUD + health probe | 4 | only place with `AbortController` | P2 |
| `install.ts` | 173 | Install bridge (mock/native) | 3 | `openPath` not mock-guarded (throws "needs desktop" in demo) | P3 |
| `audio`/`pwa`/`search`/`shares`/etc. | 14–185 each | Helpers | 4 | mostly tested | P3 |

### 2.8 Theme / styles / scripts

| Path | LOC | Role | Q | Issues | P |
|---|---|---|---|---|---|
| `styles/index.css` | 1548 | Tokens, base, components, console materials, motion | 4 | `.stagger-in > *` (`:891-902`) animates **every** child of a 200-row grid; 7 keyframes; zero literal colours (issue refs only) | P1 |
| `styles/themes.generated.css` | 210 | Static fallback palettes | 5 | generated + checked in CI | — |
| `theme/storage.ts` | 268 | Pre-paint payload read/write | 4 | — | P3 |
| `theme/console-appearance.ts` | 218 | Studio/Soft materials | 4 | 1 `console.warn` | P3 |
| `theme/derive.ts` | 196 | Accent/tint derivation | 5 | tested | — |
| `theme/validate.ts` | 174 | Theme file import/export | 4 | tested | P3 |
| `theme/registry.ts` | 150 | Built-in palettes | 5 | WCAG-tested | — |
| `theme/contrast.ts` | 90 | WCAG math | 5 | — | — |
| `scripts/i18n.mjs` | 474 | Catalog gate (parity/coverage/RTL/plurals/pseudo) | 5 | excellent | — |
| `scripts/bp-sounds.mjs` | 364 | WAV synthesis for cues | 4 | — | P3 |
| `scripts/theme.mjs` | 120 | Registry → CSS | 5 | — | — |
| `scripts/i18n-pseudo.mjs` | 96 | `en-XA` generation | 5 | — | — |
| `scripts/i18n-lint.mjs` | 90 | Advisory hardcoded scan | 3 | heuristic misses props/locals: `confirmLabel`, `placeholder` defaults, toast object literals | P2 |

---

## 3. Cross-cutting analysis

### 3.1 API / Tauri type drift

* `types.ts` is a **hand-written 4 169-line mirror** of `docs/CONTRACT.md` (4 963 lines). No `ts-rs`/`specta` generation, no OpenAPI→TS step even though `docs/OPENAPI.md` exists. Drift is not detectable at build time.
* Positives: roles/enums match (`Role`, `ProgressState`), `User` matches `crates/ludex-server/src/models.rs:207-244` field-for-field, `LocalSettings` matches `apps/desktop/src-tauri/src/settings.rs:185+` including the `#[serde(default)]` additions (`downloadStaging`, `installTransport`).
* Stale doc comment: `types.ts:2199-2200` — two consecutive `/** */` blocks for one field; the "steam entries" sentence now describes nothing.
* Command coverage is **excellent**: 166 distinct `invoke` names vs 167 `generate_handler!` entries; only `quit_app` and `system_sleep` are unregistered (§5.1), so 164 of 167 commands are reachable from the web. Conversely `detect_save_paths`, `get_save_paths`, `set_save_paths` are registered but never called from the web bundle.
* TS strictness gaps: `noUncheckedIndexedAccess` and `exactOptionalPropertyTypes` are off, so `GameSummary.status?: …` is read without guards in places (`admin/games.tsx` uses `AdminGameRow` which is non-optional — consistent, but the optional variant in `GameSummary:51-55` invites it).

### 3.2 TanStack Query usage

* 328 literal `queryKey: [...]` occurrences, ~45 first-level namespaces, **no key factory**. Invalidation is prefix-based and mostly correct, but a typo (`["game"]` vs `["games"]`) only surfaces at runtime. `useBookmark` (`hooks.ts:108-137`) depends on prefix matching across `["games", *]`, `["game", id]`, `["palette-games"]`.
* **Cache fragmentation of the same resource**: the library list is fetched as
  * `["games","all"]` (`hooks.ts:26`, Home, BigPicture, DownloadsProvider),
  * `["games", mode, query]` (`library.tsx:177`, whole query object in the key),
  * `["palette-games"]` (`command-palette.tsx:75`),
  * `["facets","genres"]` (`hooks.ts:99`, **same `api.listGames({limit:200})` call, different key**).
  Net effect: opening `/library` can fire `/api/games?limit=200` twice plus `platforms` and `missing-count` — four requests for one screen.
* Optimistic update exists only for bookmarks (good pattern, `hooks.ts:114-127`) and queue reorder (`state/downloads.tsx:553-570`). Everything else invalidates and refetches.
* No global `onError`; every mutation repeats a `push({title, description: error.message, tone:"danger"})` block (≈60 copies).
* Polling is scattered: 2 s overlay, 2.5 s game session, 4 s onboarding, 5 s presence/overlay panel, 10 s locations/controller/ingest, 15 s devices, 30 s friends/observability/server health, 60 s notifications. Several keep polling when the tab is hidden (`refetchInterval` + `refetchOnWindowFocus:false`, no `refetchIntervalInBackground:false`).

### 3.3 State duplication

* Theme state lives in three places: `state/settings.tsx` (React), `theme/storage.ts` (pre-paint payload), and the inline script in `index.html`. That is deliberate and parity-checked; `styles/tokens.test.ts` and `theme/boot.test.ts` guard it. Fine.
* Locale state similarly triple-stored (`lib/i18n.tsx` module store, `ludex.locale.*` keys, `index.html` replay) with documented precedence and a build check for the RTL list. Fine.
* **Downloads state is the real duplication**: `jobs` in the provider, `jobsRef` mirror (`downloads.tsx:180-181`), `transfer`/`transferRef`, plus per-screen derivations (`library.tsx:119` + `installingPercent`, `home.tsx:65`, `game-detail.tsx:132`). The ref mirror is written during render.
* `useInstalledGames` list is cached under `["installed"]` but three screens rebuild their own `Map` (`library.tsx:112`, `hooks.ts:92`, `home.tsx` via hook) instead of using one selector.

### 3.4 Routing / auth gates

* Public routes are correctly outside `Gate` (`app.tsx:271-277`) and wrapped in `PublicPage` for scroll. `Gate` handles setup redirect, login redirect, desktop handoff hold, and keeps `?desktop_state=` across the `/setup` reroute — the trickiest logic in the app and it is documented.
* `RequireAdmin`/`RequireEditor` redirect silently; there is no "you need editor rights" screen, and no `403` distinction from `404`.
* HashRouter means no server-side deep links; consistent with the embedded/Tauri deployment.
* No route-level `lazy()` + no per-route error boundary: one thrown render in any panel replaces the entire app with the root boundary (`main.tsx:21`).

### 3.5 i18n discipline

Strong tooling, uneven application:

* `node scripts/i18n.mjs` gates parity + 95 % coverage for 13 locales; pseudo-locale; plural checks. `ar` is shipped at **6.8 %** (`coverage.json`) and hidden from the picker — 17 KB of chunk that no one can select.
* Advisory lint finds **20** hardcoded strings, mostly false-positive-ish (`"void | Promise"`, `"new Set"`), but it is blind to:
  * default props: `dialog.tsx:166 "Confirm"`, `select.tsx:103 "Select"`, `page.tsx:63 "That didn't load"`;
  * toast object literals: `cloud-saves.tsx:345-427` (~15 strings), `launch-options.tsx:200-219`, `install-files.tsx:77,102`, `add-to-collection.tsx:43`, `admin/news.tsx:154`, `share.tsx:318`, `big-picture.tsx:767`;
  * `aria-label`/`title` in `gamepad-debug.tsx:13,20,51` and `motion-probe.tsx:81,85`.
* Estimated real hardcoded user-visible strings: **~45**, concentrated in the cloud-saves and console paths.
* `t()` used **4 829 times** — discipline is real, not accidental.
* German/French/etc. are at 3 864/3 864 ids — plausible machine+review pipeline; no per-string provenance in the repo.

### 3.6 CSS tokens and colours

* **Zero literal colours in `src/components`, `src/screens`, `src/state`, `app.tsx`, `main.tsx`** — enforced by `styles/no-hardcoded-colors.test.ts:61-68` (regex over hex/rgb/hsl/Tailwind `white|black`). All 14 "hex" hits in `index.css` are issue references inside comments.
* Tokens are `@theme inline` mapped to CSS vars (`index.css:15-43`) so Tailwind utilities and arbitrary `var(--x)` stay in sync.
* Typography is tokenised (`--font-display|ui|code`) and loaded via `@fontsource-variable` (3 families, 10 woff2 files ≈ 330 KB, unsubset — Cyrillic/Greek/Vietnamese faces ship for every user).
* Sizes are hardcoded in Tailwind arbitrary values everywhere (`text-[10.5px]`, `text-[13.5px]`, `h-[3px]`, `w-[var(--rail-w)]` mixed). That is a deliberate "archive" look, but there is no type scale token, so 8 distinct px sizes for body copy coexist (`10.5/11/11.5/12/12.5/13/13.5/14`).

### 3.7 Bundle size and load

Measured from the committed `dist/` (v0.4.14 build):

| Artefact | Raw | Gzip |
|---|---|---|
| `index-*.js` (entry: all screens + all settings + console + en catalog) | 2 133 144 B | 575 780 B |
| `index-*.css` | 124 300 B | 23 771 B |
| 13 locale chunks (`de,es,fr,it,ja,ko,pl,pt-BR,ru,tr,zh-CN,ar` + en inline) | ~4.5 MB total | — |
| fonts (10 × woff2) | ~330 KB | — |

* **No code splitting**: `grep -r "lazy("` → 0 hits; `vite.config.ts` has no `manualChunks`; `app.tsx:27-61` statically imports all 35 screens.
* `en.json` (308 125 B) is a static import in `lib/i18n.tsx:11` → part of the entry chunk, even though 12 other catalogs are lazy.
* The service worker **precaches all 28 build assets**, including every locale chunk (`vite.config.ts:33-60` + `public/sw.js:76-92`). A French user installs ~4.5 MB of catalogs they will never read, on first load.
* Verified DCE: `lib/mock.ts` is tree-shaken out (`"This server is still in setup"` absent from the bundle), but `lib/mock-install.ts` + `data/demo.ts` are **not** (`"Chrono Trigger"`, `"chrono-trigger"` present) because `hooks.ts`/`install.ts` reference `mockInstalls` inside a ternary Rollup cannot fold across modules. ~50 KB of demo library ships to production.

### 3.8 Render hotspots

1. **`DownloadsProvider` at 1 Hz.** `sampleTransfers` (`state/downloads.tsx:236-268`) runs every `SAMPLE_INTERVAL_MS = 1000` (`lib/eta.ts:28`), `setTransfer` creates a new object, `useMemo` value (`:628-655`) changes, and all 8 `useDownloads()` consumers re-render — including `LibraryScreen` (200 `GameCard`s) and `BigPictureScreen` (2 728 lines, 6 layouts).
2. **No memoisation at all.** `grep "\bmemo("` → 0. `GameCard` (`game-card.tsx:12`) takes 9 props including three inline closures created per render (`library.tsx:641-660`), so every parent render rebuilds 200 cards.
3. **Inline SVG covers.** `Motif` (`art/cover.tsx:13-330`) emits 15–330 SVG nodes per card depending on motif (`halftone` = 165 circles). A 200-card grid is tens of thousands of DOM nodes, re-created when the cover re-renders.
4. **`.stagger-in` on the full grid** (`index.css:891-902`, used at `library.tsx:638,673`): 200 simultaneous `opacity`+`transform` animations with staggered delays; after `nth-child(9)` all share 176 ms, so it is one 200-element animation batch on mount.
5. **Console rails**: `big-picture.tsx` re-renders on every `useDownloads` tick and on the 2 s overlay poll; its layouts are unmemoised pure functions, so a 1 Hz tick in the living room repaints artwork rails.
6. **Two ad-hoc chart implementations** (`downloads.tsx:79-140`, `admin/observability.tsx:80-104`) recompute paths on every render with no memo on the point arrays (downloads does `useMemo` the visible window; observability does not).

### 3.9 Test coverage

| Area | Test files | Notes |
|---|---|---|
| `lib/` | 52 | Pure logic: eta, install-phase/queue/retry/transport/history, config-fields, controller-*, preferences, format, search, shares, cloud-saves, pwa, csp, elevation |
| `components/big-picture/` | 12 | Pure state machines (controller, cues, layout, motion, power, search, system-zones, launch-plate, pointer-idle, entrance, input, prefs) — **no rendering** |
| `styles/` | 4 | Token/no-hardcoded-colour/console-material/page-scroll guards |
| `theme/` | 3 | WCAG + boot parity + registry |
| `components/*` (other) | 1 | `settings/controller-manager.test.ts` |
| `screens/` | **0** | none |
| `hooks`/providers | 0 | none |
| E2E | 5 specs / 5 tests | library+search+filters+detail+download; navigation+console escape; ingest; setup; dialog focus |

Line ratio: 11 028 test lines / 84 644 production lines ≈ **13 %**, but almost all of it is pure-function coverage. The React layer (state providers, dialogs, forms, gamepad integration in the screen, overlay) is verified only by 5 Playwright tests and manual QA.

### 3.10 Mock / native parity

* `mockApi` implements 155 of the 168 API methods; the remainder are covered by inline `Promise.resolve(...)` / `Promise.reject(...)` stubs inside `api.ts` (`listDiagnostics:288`, `controllerProfiles:735`, `listEmulators:769`, `downloadTicket:645`, `launchCommand:801`, `loginMfa:1180`, …). Consequence: in demo mode the emulator list is **always empty**, diagnostics always empty, controller presets partially faked. Acceptable for a demo, but it means "demo mode" is not a uniform view of the product.
* `mockApi.loginMfa` is implemented as `mockApi.oidcExchange("demo-mfa")` (`api.ts:1180-1183`) — semantically wrong but harmless.
* `installBridge.openPath` (`lib/install.ts:150-152`) has no mock branch → demo mode rejects with "needs the desktop app" for a web-hosted path. The caller catches it and shows a friendly toast, so it degrades.
* Tauri side: 2 commands invoked but absent (`quit_app`, `system_sleep`), 3 registered but unused.

---

## 4. Quantification

### 4.1 Totals

| Metric | Value |
|---|---|
| Files in scope | 380 (`apps/web/src` 369, `apps/web/scripts` 5, `e2e/tests` 6) + root config (`package.json`, `vite.config.ts`, `tsconfig.json`, `index.html`) |
| TS/TSX lines in `src` | 95 672 (production ≈ 84 644, tests 11 028) |
| `components/` | 161 files (148 non-test), 39 522 lines incl. tests / 37 407 production (incl. 3 954 console tsx + 3 251 console ts) |
| `screens/` | 35 files, 17 600 lines |
| `lib/` | 130 files, 32 928 lines (78 non-test, 24 932 lines) |
| `state/` | 6 files, 2 159 lines |
| `theme/` | 10 files, 1 860 lines |
| `styles/` | 2 023 lines (incl. 4 test files) |
| Capitalised exports in `.tsx` (`export function|const Upper`) — components + tables | 257 |
| Exported hooks | 48 (`^export function use[A-Z]`) |
| Context providers | 8 |
| `useState` / `useEffect` / `useRef` occurrences | 674 / 267 / 146 |
| `useMemo` / `useCallback` | 133 / 125 |
| `React.memo` | **0** |
| Test files | 72 (52 `lib`, 13 `components`, 4 `styles`, 3 `theme`) |
| E2E specs / tests | 5 / 5 |

### 4.2 Top 30 largest files

| # | File | LOC |
|---|---|---|
| 1 | `lib/types.ts` | 4 169 |
| 2 | `lib/mock.ts` | 4 155 |
| 3 | `screens/big-picture.tsx` | 2 728 |
| 4 | `lib/native.ts` | 1 564 |
| 5 | `styles/index.css` | 1 548 |
| 6 | `lib/api.ts` | 1 433 |
| 7 | `components/settings/remote-play-panel.tsx` | 1 078 |
| 8 | `components/game/cloud-saves.tsx` | 1 020 |
| 9 | `components/game/game-settings.tsx` | 1 012 |
| 10 | `components/game/make-it-run.tsx` | 1 000 |
| 11 | `screens/setup.tsx` | 981 |
| 12 | `screens/library.tsx` | 954 |
| 13 | `screens/admin/games.tsx` | 915 |
| 14 | `lib/mock-install.ts` | 905 |
| 15 | `screens/game-detail.tsx` | 890 |
| 16 | `data/demo.ts` | 853 |
| 17 | `components/settings/sections-device.tsx` | 813 |
| 18 | `screens/downloads.tsx` | 779 |
| 19 | `lib/i18n.tsx` | 701 |
| 20 | `screens/admin/metadata.tsx` | 698 |
| 21 | `components/overlay/overlay-screen.tsx` | 692 |
| 22 | `screens/admin/authentication.tsx` | 691 |
| 23 | `screens/admin/observability.tsx` | 689 |
| 24 | `components/settings/installed-games.tsx` | 685 |
| 25 | `components/settings/streaming-panel.tsx` | 675 |
| 26 | `components/shell/server-switcher.tsx` | 673 |
| 27 | `state/downloads.tsx` | 662 |
| 28 | `components/settings/appearance-panel.tsx` | 648 |
| 29 | `state/preferences.tsx` | 640 |
| 30 | `components/game/virustotal.tsx` | 638 |
| — | `components/settings/overlay-panel.tsx` | 624 |

Files ≥ 600 LOC: **32**. Files ≥ 600 LOC with ≥ 8 `useState`: **17**.

### 4.3 Search counts (`grep` over `apps/web/src`)

| Pattern | Count | Comment |
|---|---|---|
| `: any` / `as any` / `<any>` | **0** | no explicit `any`; 76 hits of the word are prose |
| `as unknown as` | 9 | all justified (Tauri internals, experimental Gamepad fields) |
| `@ts-ignore` / `@ts-expect-error` | 0 | — |
| `as X` type assertions | 114 | mostly narrow unions |
| `console.log/warn/error/debug/info` | 13 (7 non-test) | `window-controls.tsx` ×3, `cloud-saves.tsx` ×1, `big-picture.tsx` ×3, gamepad debug-gated ×4 |
| `TODO` / `FIXME` / `XXX` / `HACK` | **0** | — |
| `eslint-disable` | **15** | inert: no ESLint config, no lint script, no CI job |
| `dangerouslySetInnerHTML` | 0 | news/changelog rendered as blocks |
| `t("…")` calls | 4 829 | — |
| Raw English `jsx-text`/`aria-label` (i18n lint) | 20 | advisory |
| Raw English in toast/confirm props | ~25 (est. 14 blocks) | invisible to the lint |
| `localStorage.` direct | 56 | helpers exist for the scoped ones only |
| `key={index|i|idx}` | 46 | mostly static art/motif lists — not a bug per se |
| `role="alert"` | 69 across 45 files | screen-reader noise risk on forms that re-render errors |
| `aria-label` | 199 | good baseline |
| `focus-visible` in TSX | 3 | global focus ring lives in `index.css` |
| `queryKey: [` | 328 | no factory |
| `useQuery|useMutation|useInfinite` call sites | 187 | |
| `refetchInterval` | 19 | 1.5 s–60 s |
| Hex literals in components | **0** | enforced by test |
| Hex literals in `index.css` | 0 (14 are `#issue` refs) | — |
| `lazy(` / `React.lazy` | **0** | no route splitting |
| `React.memo` / `memo(` | **0** | — |
| `AbortController` | 1 (`lib/servers.ts:125`) | none in `api.ts` |
| `setInterval` / `setTimeout` | 13 / 38 | — |

### 4.4 Bundle chunks

16 JS chunks: 1 entry (2.13 MB), 1 `core` (880 B), 1 `event` (1.4 KB), 13 locale chunks (17 KB – 448 KB). 1 CSS (124 KB). 10 woff2. Total dist 6.6 MB.

---

## 5. Confirmed defects (with evidence)

### 5.1 P0 — two console commands do not exist

`apps/web/src/lib/native.ts:529-547` calls `quit_app` and `system_sleep`. `apps/desktop/src-tauri/src/lib.rs:263-448` registers **167 commands**; neither name appears, nor anywhere else in `apps/desktop/src-tauri/src`. Cross-check of all 166 invoked names against the handler list:

```
in native.ts but not registered: quit_app, system_sleep
registered but never invoked by web: detect_save_paths, get_save_paths, set_save_paths
```

`powerActions` (`components/big-picture/power.ts:55-67`) marks "Sleep this PC" `enabled: context.native`. `runPower` (`screens/big-picture.tsx:1090-1094`) awaits `native.systemSleep()` and the surrounding `catch` shows a generic "Not available" toast. **Every native user who presses Sleep gets an error.**
`quit` has a `quit_app` → `exitConsole` fallback (`:1098-1106`) but only inside console mode; in the desktop shell the same "Not available" toast appears.

### 5.2 P0 — no request timeout

`lib/api.ts:196-214` calls `fetch` with no `signal` and no timeout. A server that accepts the TCP connection but never answers leaves `AuthProvider.boot` (`state/auth.tsx:98`) pending forever: `ready` stays `false` and the user stares at the boot screen with no error and no retry. Same for every query and every XHR helper (`api.ts:1360-1433`) used by cloud saves and uploads.

### 5.3 P1 — 1 Hz whole-app re-render

Chain: `lib/eta.ts:28 SAMPLE_INTERVAL_MS = 1000` → `state/downloads.tsx:295` interval → `setTransfer(next)` → `useMemo` value `:628-655` → 8 consumers (`install-dialog.tsx:49`, `nav-rail.tsx:123`, `app-shell.tsx:225`, `big-picture.tsx:230`, `home.tsx:65`, `downloads.tsx:159`, `game-detail.tsx:132`, `library.tsx:119`). There is a **second 1 Hz timer** (`downloads.tsx:299-303`) calling `installBridge.list()` — a full IPC round trip — then `setJobs(list)` with a fresh array identity, so the context re-renders every second even when no byte moved. With zero `React.memo`, `LibraryScreen` rebuilds up to 200 `GameCard`s (each with an inline SVG cover) every second while any install runs.

### 5.4 P1 — production bundle ships the demo library

Verified by string search in `dist/assets/index-Cl3uSvgc.js`: `"Chrono Trigger"` and `"chrono-trigger"` are present, `"This server is still in setup"` (a `mock.ts`-only string) is absent. Cause: `lib/hooks.ts:16,36` and `lib/install.ts:30-56` reference `mockInstalls` inside `USING_MOCKS ? … : …`, and `mock-install.ts:1` imports `DEMO_GAMES`. Rollup cannot constant-fold `USING_MOCKS` across modules, so `data/demo.ts` (853 lines) + `mock-install.ts` (905 lines) survive. Fix: make `USING_MOCKS` a `define` constant, or dynamic-import the mock adapter.

### 5.5 P1 — service worker precaches ~4.5 MB of unused locales

`vite.config.ts:39-53` walks `dist/` and precaches everything except `sw.js`; `public/sw.js:76-92` fetches every entry. All 13 locale chunks are in the list. A user who reads English downloads 4.5 MB of translations on first install (and again on every version bump, since the cache is content-versioned).

### 5.6 P2 — untranslated defaults and toast copy

| File:line | String |
|---|---|
| `components/ui/dialog.tsx:166` | `confirmLabel = "Confirm"` |
| `components/ui/select.tsx:103` | `placeholder = "Select"` |
| `components/page.tsx:63` | `title = "That didn't load"` (while the retry button one line down is `t("Try again")` at `:81`) — inconsistent, and the lint cannot see it |
| `screens/big-picture.tsx:767` | `title: "Fullscreen unavailable"` |
| `components/game/cloud-saves.tsx:345-346,350,359,368,372,387,399,408,424,427` | 11 toast titles/descriptions |
| `components/game/launch-options.tsx:200,204` | 2 toasts |
| `components/game/install-files.tsx:77,102` | 2 toasts |
| `components/collections/add-to-collection.tsx:43` | toast title |
| `screens/share.tsx:318` | 3 page titles |
| `screens/admin/news.tsx:154` | toast title |
| `components/gamepad/gamepad-debug.tsx:13,20,51` | `aria-label` + copy |
| `components/big-picture/motion-probe.tsx:81,85` | `aria-label` + copy |

Because English is the message id, these strings are simply missing from `en.json`/`fr.json` and will render English everywhere.

### 5.7 P2 — inert lint suppressions

15 `// eslint-disable…` comments (`cloud-saves.tsx` ×4, `login.tsx` ×2, `setup.tsx`, `library.tsx`, `admin/games.tsx`, `sections-device.tsx`, `navigation.ts`, `layout-spotlight.tsx`, `search-field.tsx`). There is **no ESLint dependency, config, script or CI job** (`apps/web/package.json`, `.github/workflows/ci.yml:105-121` runs i18n → build → tsc → vitest only). These comments are documentation at best and false assurance at worst: `react-hooks/exhaustive-deps` violations are unchecked everywhere.

### 5.8 P2 — accessibility nits

* 69 `role="alert"` regions across 45 files; a form that re-renders its error on every keystroke will re-announce (`Field` at `controls.tsx:127` is inside the per-field render path).
* `Segmented` (`controls.tsx:294-309`) hides option labels with `sr-only` at `sm`, so icon-only segments depend on the group's `aria-label` alone.
* `GameCard` uses an absolutely-positioned `<Link>` overlay (`game-card.tsx:47-52`) plus sibling `title` link (`:100`) — duplicate links to the same target in the accessibility tree.
* `dialog.tsx` traps Tab correctly but does not mark background content `aria-hidden`/`inert`; screen readers can still reach the page behind.
* `overlay-screen.tsx` explicitly refuses `aria-modal` (documented reason: the gamepad hook would swallow input) — a deliberate trade-off, but it means the overlay is not announced as a dialog.

### 5.9 P3 — dead/stale code

* `lib/i18n.tsx`: `formatRelativeValue` (566), `formatListValue` (577), `getLocale` (491) exported, zero importers.
* `types.ts:2199` dangling doc comment.
* `lib/native.ts` `systemSleep`/`quitApp` doc comments describe commands "not implemented yet" — accurate, but the UI still enables them.
* `screens/home.tsx:28-52` `sourceLabel()` re-implements `lib/launchers.ts` labels.
* `screens/admin/users.tsx:16` and `screens/admin/invitations.tsx:15` `roleOptions()` differ only in order.
* `components/game/virustotal.tsx:102`, `components/admin/virustotal-scan.tsx:173`, `components/ui/controls.tsx:213` — three verbatim `tones` maps.

---

## 6. What is genuinely good (do not regress)

1. `lib/settings-sections.ts` + `screens/settings.tsx` — a single taxonomy driving nav, URL, palette and a test that every panel appears once.
2. `scripts/i18n.mjs` + `scripts/i18n-pseudo.mjs` + `styles/no-hardcoded-colors.test.ts` + `theme/contrast.ts` + `styles/tokens.test.ts` — the only places in this codebase where the build *prevents* a class of regression. Extend this pattern to lint and bundle budget.
3. `components/big-picture/*` — 12 pure modules with tests; the screen above them is the problem, the primitives are not.
4. `docs/CONTRACT.md` discipline and the near-perfect command coverage (164/167 reachable).
5. `lib/eta.ts`, `lib/install-phase.ts`, `lib/install-queue.ts`, `lib/install-retry.ts` — the install pipeline is decomposed and tested, which is why the 662-line provider is survivable.
6. `state/preferences.tsx` — a real conflict/offline/revision state machine instead of "just PUT the JSON", documented at the top.

---

## 7. Split candidates (order matters)

| Order | File | Why first | Target modules |
|---|---|---|---|
| 1 | `screens/big-picture.tsx` | largest component, 23 states, everything flows through it | `console/state.ts` (reducer), `console/library-surface.tsx`, `console/home-surface.tsx`, `console/app-surface.tsx`, `console/power-controller.ts`, `console/launch-flow.ts` |
| 2 | `lib/api.ts` | 2 engines + 168 methods + mock branches | `api/client.ts` (fetch+xhr, timeout), `api/types.ts`, `api/resources/{games,admin,auth,uploads}.ts`, `api/mock-adapter.ts` (dynamic) |
| 3 | `components/game/cloud-saves.tsx` | 21 states + raw English + 5 suppressions | `cloud-saves/status.tsx`, `conflict.tsx`, `snapshots.tsx`, `use-cloud-saves.ts` |
| 4 | `components/settings/remote-play-panel.tsx` | 1 078 lines, wizard booleans | `remote-play/wizard.ts` (state machine), `plan-step.tsx`, `verify-step.tsx`, `ports.ts` |
| 5 | `components/shell/server-switcher.tsx` | 19 states, 4 components | `servers/switcher.tsx`, `servers/panel.tsx`, `servers/form-dialog.tsx` |
| 6 | `screens/setup.tsx` | 27 states, 6 implicit stages | `setup/stage.ts` (machine), `setup/desktop-wizard.tsx`, `setup/remote-form.tsx` |
| 7 | `components/game/game-settings.tsx` | 1 012 lines, 6 panels | `game-settings/{adapters,files,common-fields,backups}.tsx` |
| 8 | `components/game/make-it-run.tsx` | 1 000 lines, 7 pure mappers | move mappers to `lib/make-it-run.ts`, split steps |
| 9 | `lib/native.ts` | 1 564 lines, 193 wrappers | `native/{install,games,settings,system,overlay}.ts` behind one export |
| 10 | `lib/types.ts` | 4 169 lines | split by domain (`types/game.ts`, `types/admin.ts`, `types/desktop.ts`) or generate from `CONTRACT.md` |
| 11 | `screens/library.tsx` | 954 lines, 3 render modes inline | `library/{grid,list,index}-view.tsx`, `use-library-model.ts` |
| 12 | `components/settings/sections-device.tsx` | 7 bodies in 813 lines | one file per body |

---

## 8. Top 25 refactors (ranked by impact)

Impact = user-visible benefit × breadth; Effort in days (1 dev); Risk = chance of regression.

| # | Refactor | Impact | Effort | Risk | Notes |
|---|---|---|---|---|---|
| 1 | Register `quit_app` / `system_sleep` (or gate them to `enabled:false`) | 5 | 0.5 | 1 | `power.ts:55-67`, `native.ts:529-547`, `lib.rs:263` |
| 2 | Add timeout + `AbortSignal` to `api.ts` `request`, `putChunk`, `uploadWithProgress` | 5 | 0.5 | 1 | `api.ts:196-214,1360-1433`; surface as `ApiError(0,"timeout")` |
| 3 | Route-level `React.lazy` + Suspense per screen group | 5 | 1 | 1 | `app.tsx:27-61,269-439`; expect −40–55 % entry JS |
| 4 | Stop the 1 Hz context: split `meters`/`aggregate` into a separate lightweight context or a store | 5 | 2 | 3 | `state/downloads.tsx:178,236-300,628-655`; keeps `jobs` stable |
| 5 | Memoise `GameCard` + `useCallback` the three handlers in `library.tsx` | 4 | 0.5 | 1 | `game-card.tsx:12`, `library.tsx:641-660` |
| 6 | Runtime-cache locale chunks instead of precaching all 13 | 4 | 0.5 | 2 | `vite.config.ts:39-53`, `public/sw.js:76-92` |
| 7 | Make `USING_MOCKS` a Vite `define` constant (or dynamic mock import) | 4 | 0.5 | 1 | drops `mock-install.ts` + `data/demo.ts` from prod |
| 8 | Query-key factory + one library cache entry | 4 | 1 | 3 | kills the `["games","all"]` / `["games",mode,query]` / `["palette-games"]` / `["facets","genres"]` split |
| 9 | Fix ~45 hardcoded strings (toast/confirm/aria) and extend `i18n-lint` to object literals + default props | 4 | 1 | 2 | `cloud-saves.tsx:345-427` first |
| 10 | Split `big-picture.tsx` into a reducer + surfaces | 4 | 4 | 4 | preparation for any console feature work |
| 11 | Add ESLint (`react-hooks`, `jsx-a11y`, `import`) to CI and delete/repair the 15 suppressions | 4 | 1 | 2 | would have caught the `jobsRef` render write |
| 12 | Remove `jobsRef.current = jobs` during render; use `useEffect` or an event emitter | 3 | 0.5 | 2 | `downloads.tsx:181` |
| 13 | Virtualise the library grid (or paginate to 60/page) | 3 | 2 | 3 | `library.tsx:638-660`; also removes the 200-element stagger animation |
| 14 | Split `lib/api.ts` into client + resources + (lazy) mock adapter | 3 | 3 | 3 | `api.ts:263-1300` |
| 15 | Add per-route error boundary + `retry` UI | 3 | 0.5 | 1 | `main.tsx:21` |
| 16 | Lazy-load `en.json` (or precompile messages to a function) | 3 | 1 | 2 | 308 KB static import, `lib/i18n.tsx:11` |
| 17 | `useGenres` should derive from the games cache, not refetch | 3 | 0.5 | 1 | `hooks.ts:97-106` |
| 18 | Replace the 3 `tones` maps with one `Badge` tone API | 3 | 0.5 | 1 | `controls.tsx:213`, `virustotal.tsx:102`, `virustotal-scan.tsx:173` |
| 19 | Seeded RNG + memo for `Motif` (cache rendered node lists by `slug`) | 3 | 1 | 2 | `art/cover.tsx:13-330` |
| 20 | Turn `RequireAdmin/Editor` into an explicit "no access" state | 3 | 0.5 | 1 | `app.tsx:170-180` |
| 21 | Drop or finish `ar` (6.8 %) — either translate or remove from the shipped set | 2 | 0/2 | 1 | `coverage.json`, `manifest.json` |
| 22 | Consolidate the 19 `refetchInterval`s behind named hooks with `refetchIntervalInBackground:false` | 2 | 1 | 2 | battery/CPU on laptops |
| 23 | Retire `game.trailers` fallback once `videos` is universal | 2 | 0.25 | 1 | `media-gallery.tsx:327`, `types.ts:155` |
| 24 | Move `useGameExecutables`/`useGameVirusTotal`/`useDevices` out of component files | 2 | 0.5 | 2 | `executable-picker.tsx:29`, `virustotal.tsx:43`, `device-picker.tsx:14` |
| 25 | Add 10–15 render tests (`@testing-library/react`) for Dialog, Field errors, install dialog, settings nav | 2 | 2 | 2 | closes the biggest safety gap |

---

## 9. Quick wins (< 1 h each, low risk)

1. Register `quit_app` + `system_sleep` **or** set `enabled:false` with `reason` in `power.ts` (5 lines).
2. `req: { signal: AbortSignal.timeout(15000) }` in `lib/api.ts` `request()` (3 lines + one error branch).
3. Translate the three default props (`Confirm`, `Select`, `That didn't load`).
4. Translate `big-picture.tsx:767` "Fullscreen unavailable".
5. Add `loading="lazy" decoding="async"` to `GameBackdrop`'s `<img>` (currently `eager`, `cover.tsx:473`).
6. Replace the 3 duplicated `tones` maps with `Badge`.
7. Drop the three unused i18n exports.
8. Fix the dangling doc comment at `types.ts:2199`.
9. Merge `roleOptions()` into `lib/admin-users.ts`.
10. `React.memo(GameCard)` + wrap the three inline handlers in `useCallback` in `library.tsx`.
11. Add `refetchIntervalInBackground: false` to the 30/60 s polls.
12. Add `apps/web/.eslintrc` + `eslint`/`eslint-plugin-react-hooks` to `package.json` + one CI step.

---

## 10. Bundle budget proposal

| Budget | Current | Proposal |
|---|---|---|
| Entry JS (gzip) | 575 780 B | ≤ 300 000 B (lazy routes + lazy `en.json` + mock DCE) |
| Entry CSS (gzip) | 23 771 B | keep ≤ 30 000 B |
| Precache total | ~4.6 MB | ≤ 1.2 MB (entry + CSS + fonts; locales runtime-cached) |
| Per-locale chunk | 17–448 KB | ≤ 200 KB gzip, loaded on demand only |

Add a `size-limit`/`rollup-plugin-visualizer` check to `.github/workflows/ci.yml:105-121` next to `i18n:check`. The tooling already proves the team is willing to gate on invariants; bundle size is the missing invariant.
