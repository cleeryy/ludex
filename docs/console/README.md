# Ludex Console 2.0 + rebrand — blueprint

**Baseline:** v0.4.17 + #237 (`5406405`), 2026-11-16. Read-only research; no code was changed by the audit.

This is the source of truth for the Console 2.0 / rebrand work. The short contract lives in [`.opencode/skills/console-mode-ui/SKILL.md`](../../.opencode/skills/console-mode-ui/SKILL.md).

| Research | What it covers |
|---|---|
| [external.md](research/external.md) | 2024–2026 console/TV/web patterns, Steam/Xbox/PS/Switch/Apple TV/Plex/Kodi, spatial navigation, Gamepad API, TV budgets, sources |
| [current.md](research/current.md) | Current `/bigpicture` and `components/big-picture/*` inventory, coupling, keep/rewrite/delete, 191 tests |
| [design.md](research/design.md) | Cabinet brand/UX direction, tokens, IA, screen-by-screen blueprint, customization, motion/sound, anti-slop |
| [architecture.md](research/architecture.md) | Shell/provider/input architecture, native power/window, data boundaries, migration phases, gates |

Evidence for the Phase 0.6 gates: [evidence.md](evidence.md) (fake-pad suite and
the 720p/1080p/4K + overscan matrix), [E2E.md](E2E.md) (the harness, the native
bridge, how to add a spec), [PERFORMANCE.md](PERFORMANCE.md) (TV boot, rail
frames, long tasks, heap, rAF loops), [WEBKITGTK.md](WEBKITGTK.md) (engine spike,
not verified on hardware) and [NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md)
(Windows + Sunshine/Moonlight, with evidence rules) — #270, #302, #303, #304,
#308. The boot target itself — the Wayland session entry, autologin for each
display manager, backups and rollback — is in
[CONSOLE-SESSION.md](../CONSOLE-SESSION.md) (#286).

## Verdict

The Console's **interaction kit is an asset**: motion vocabulary, sound/haptics, controller fingerprint/profile resolver, pure input/layout/zone models, 191 colocated tests, static-blur budget, and the `--console` Sunshine promotion. The user likes these; do not rewrite them.

The problems are structural:

1. **Input has no owner.** Four components run independent `useGamepad` loops; with the in-app overlay open, the Console underneath receives the same presses.
2. **The shell has no boundary.** `/bigpicture` is outside `AppShell` but shares every provider/global: toasts, onboarding, PWA prompts, update dialog, overlay host. Boot is a late redirect and can paint the desktop shell first.
3. **One 2,748-line component** owns shell, routes, input, data, power and settings.
4. **The native layer is missing** `quit_app`/`system_sleep`, and fullscreen/close/tray/overlay policy is inferred in both Rust and web.
5. **The composition is a dashboard**, not a console: six equal tiles with `01–06`, duplicate chrome, micro-mono labels, six layouts, layout picker.

**Target:** a real `/console/*` shell in the same SPA, lazy, with console-scoped providers/globals, one input owner, and the **Cabinet** visual system. One identity, two densities — not a second brand, not a return to cream/coral Soft.

## Decisions (confirmed 2026-11-16)

The user confirmed: keep **Ludex** + one shared identity, desktop density unchanged; Console dark-first; delete Soft + pastels; two layouts (+ List); sign-in v1 password / v1.1 device code; offline-first promise; Daylight in v1.1.

| # | Decision | Confirmed |
|---|---|---|
| 1 | Keep the name “Ludex”? | **Yes.** Wordmark/dot kept; Console is the device room, not a sub-brand. |
| 2 | Console dark-first even when the desktop is light? | **Yes.** Device-owned room; Daylight later. |
| 3 | Delete Soft + five pastel accents? | **Yes.** They are the strongest slop signal left. |
| 4 | Six layouts? | **Two (+ List):** Stage and Wall; delete the layout picker. |
| 5 | Rebrand scope | **One identity, two densities.** Desktop density unchanged; shared wordmark/icon/dot/mono rules. |
| 6 | Sign-in | Password in v1; device-code pairing v1.1. |
| 7 | Offline-first brand promise | **Yes.** "42 installed games still play" is the differentiator. |
| 8 | Daylight room | v1.1; Night + Projector in v1. |

## Architecture

```
AppProviders (QueryClient, Settings, Toast, Auth, Preferences, I18n, Update)
└ HashRouter
  ├ /console/*  → ConsoleApp (lazy) → ConsoleProviders → ConsoleShell → routes
  └ /*          → DesktopApp → AppShell + desktop globals
```

- **Same SPA, same Tauri window.** Do not add a second webview/HTML entry in v1.
- `console_v2` device flag (`ludex.console.shell`), default off through Phase 1; `/bigpicture` redirects for one release.
- Console providers/globals are scoped; desktop globals move behind `DesktopShell`. No toast/onboarding/PWA/update/overlay host inside Console.
- **One `InputProvider`** + focus store + scope stack. One polling loop for pad/keyboard/mouse/remote; scope stack makes the top surface own input and restores the exact previous target.
- Finish the in-flight selector lane: `updateOne(job)` on `install://progress`, remove `useDownloads()` from cards/shells, paginate the library beyond 200.
- Native: `quit_app`, `system_sleep`, one Rust `WindowPolicy` emitted as `window://state`; keep `--console` promotion, Sunshine publishing, focus/overlay.

## IA

- **Home:** stage (selected game + primary/secondary action) + rail (5.5 covers) + band (Search · Library · Downloads · Settings · Power).
- **Library:** shelf/grid + shelf chips; Collections is a chip inside Library.
- **Game:** one primary action + tabs (Overview · Saves · Artwork · Controller).
- **Settings:** 4 rooms × 8 knobs, live preview, no Apply.
- **Power:** Sleep · Restart · Quit · Sign out; confirm defaults to Cancel.
- **Search/OSK, notifications, downloads, offline** are console surfaces, not desktop routes.
- Exactly **two modals**: install and power. Everything else is a non-modal sheet; B restores focus.

## Visual System (#333 × Console 2.0)

> **Supersedes the earlier "Cabinet warm amber cast" note:** GitHub issue #333 ("ARC × Swiss/Bauhaus × shadcn/ui") is the single app-wide token source for both Desktop and Console.

- **Exact #333 tokens, no warm brown/amber cast:** `theme/console-room.ts` uses the active #333 preset (`speranza`, `bauhaus`, `braun`, `telemetry`, `blueprint`, `olive`, `kyoto`, `studio`) plus `info` (C2 network/stream telemetry) and `stripe-1..4` (3px `.arc-stripe`, toggled via `data-stripe="on|off"`). `warmTokens()` is removed; Console achieves 7:1 contrast on every foreground (`ink`, `ink-2`, `ink-3`, `signal-text`, `info`, `danger`, `ok`, `warn`) via `cabinetSafe()` without altering desktop base tokens.
- **Restrained radii scaled by `--bp-u`:** `8 / 10 / 12 / 14u` default (`2 / 4 / 6 / 8u` sharp, `10 / 14 / 18 / 20u` round; never above `24u`).
- **One baked wallpaper layer on Night** (solid on Projector/Daylight/Arcade and `prefers-reduced-transparency`); zero `backdrop-filter` on panels. Depth = luminance step + 1px rim, never a drop shadow.
- Type: 24u floor, 28u body/actions, 34u surface title, 48u+ hero; Inter/Instrument Sans for all words, JetBrains Mono strictly for numerals/units/shortcuts.
- No six-tile grids, `01–06`, uppercase mono words, layout picker, Soft/pastels, toasts in Console, desktop dialogs.
- Customization: 4 rooms × 8 knobs + all 8 #333 themes, live preview, "from artwork" accent validated at 7:1, device-local; no CSS injection.

## Shipped in #268

The design/token lane landed the Cabinet system on top of the #267 shell without
moving the input owner or the route table.

- **Tokens** — `theme/console-room.ts`: four rooms (Night, Projector, Daylight,
  Arcade) × eight knobs (room, accent, wall, corners, density, motion, sound,
  haptics), a v3 record (`ludex.console.room`), the v1/v2 migration (Soft and the
  five pastels become Night + Theme), `cabinetTokens` (app theme → warm cast →
  room polarity → accent → 7:1 pass) and the CSS attributes the stylesheet reads.
- **Accent** — `theme/artwork-accent.ts`: dominant chromatic cluster from the
  selected cover, OKLCH chroma clamped to 0.16, lifted to 7:1, cached per game,
  and a visible line when a cover has no colour to give.
- **Compositions** — Stage (`shelf`), Wall (`grid`) and List (`compact`). The
  six-layout registry, the picker and its four views are deleted; the toggle is
  a Settings row, and a stored retired layout migrates to Stage.
- **Shell** — `components/console/shell/console-shell.tsx` renders the band
  (Search · Library · Downloads · Settings · Power), the monogram bar, one hint
  strip and the Cabinet materials. `use-console-settings.ts` owns the eight
  knobs; `console-dialogs.tsx` keeps exactly two modals (launch, power confirm).
- **Gates and notice** — `components/console/shell/console-gate.tsx` and
  `surfaces/console-notice.tsx` wear the Cabinet room and the 24u floor: boot is
  a stage/rail silhouette, and a notice is one card above the band, never a
  desktop toast.
- **Type** — 24/28/34/56u; the 20u step is gone, every console heading comes
  from `text-bp-*`, and a test fails on an ad-hoc `clamp()` or pixel size.
- **Materials** — one blurred wallpaper layer, no `backdrop-filter` on panels,
  depth by luminance, one bracket, radii ≤ 24u.
- **Brand** — the wordmark keeps its drawing and gains a Console cut (heavier
  stroke, larger dot); the bar carries the monogram; web/Tauri icons and the
  Sunshine poster are regenerated in the Cabinet palette
  (`e2e/cabinet-assets.mjs`).
- **Tests** — `theme/console-room.test.ts`, `theme/artwork-accent.test.ts`,
  `styles/console-cabinet.test.ts` (the kill list over `components/console/**`
  and `components/big-picture/**`), plus the rewritten `styles/type-scale.test.ts`,
  `styles/console-material.test.ts` and `theme/console-contrast.test.ts` (7:1 for
  every room × theme).
- **Screenshots** — `e2e/console-screenshots.mjs` captures Stage, Wall, List,
  Settings and Power at 720p, 1080p and 4K on `/console*`, plus a 5 % overscan
  crop of each, and measures every control against the safe band. Output and
  measurements live in `docs/assets/screenshots/console/`.

## Screenshots

| View | 720p | 1080p | 4K |
|---|---|---|---|
| Stage | [720](../assets/screenshots/console/stage-720.png) | [1080](../assets/screenshots/console/stage-1080.png) | [4K](../assets/screenshots/console/stage-2160.png) |
| Wall | [720](../assets/screenshots/console/wall-720.png) | [1080](../assets/screenshots/console/wall-1080.png) | [4K](../assets/screenshots/console/wall-2160.png) |
| List | [720](../assets/screenshots/console/list-720.png) | [1080](../assets/screenshots/console/list-1080.png) | [4K](../assets/screenshots/console/list-2160.png) |
| Settings | [720](../assets/screenshots/console/settings-720.png) | [1080](../assets/screenshots/console/settings-1080.png) | [4K](../assets/screenshots/console/settings-2160.png) |
| Power | [720](../assets/screenshots/console/power-720.png) | [1080](../assets/screenshots/console/power-1080.png) | [4K](../assets/screenshots/console/power-2160.png) |

The `-overscan` variants are a 5 % crop of each side: what a TV with overscan
shows. `docs/assets/screenshots/console/manifest.json` records the measured
boxes and proves the safe area is empty.

Stage and Wall are also the pixel gate (#303): `node
e2e/console-screenshots.mjs --gate --skip-build` compares them against these
files at the three resolutions plus overscan, with the tolerance and the
re-approval procedure in [evidence.md](evidence.md#visual-gate-303).

## Issues

- [#267](https://github.com/cleeryy/ludex/issues/267) — `/console` shell, single input owner, data boundaries.
- [#268](https://github.com/cleeryy/ludex/issues/268) — Cabinet rebrand and design system.
- [#269](https://github.com/cleeryy/ludex/issues/269) — console surfaces (install/downloads/settings/power/search/offline/native policy).
- [#270](https://github.com/cleeryy/ludex/issues/270) — fake-pad E2E, TV performance, hardware validation and rollout.

## Rollout state (2026-11-16)

Console 2.0 is **shipped behind a device flag, default off**. Nothing on the
desktop changes until the machine in front of the television asks for it.

| Piece | State |
|---|---|
| `/console/*` shell | On main (#267). Lazy sibling of the desktop shell in the same SPA. |
| Cabinet rooms and compositions | On main (#268). Three compositions (Stage, Wall, List); the layout picker is gone. |
| Console surfaces (install, downloads, settings, power, search, offline, native power) | On main (#269). |
| Device flag | `ludex.console.shell` in `localStorage`, read by `lib/console-flag.ts`. **Default off**: the desktop home stays the desktop home. A `/console` deep link always boots the Console, and native Console mode (`ludex --console`) enters it regardless of the flag. |
| Old route | `/bigpicture` is a one-release redirect to `/console` (`LegacyConsoleRedirect` in `app.tsx`). Old links, the Sunshine shortcut and `overlay_show_big_picture` keep working through it. |
| Desktop settings | Settings → Console exposes the shell flag and the console appearance panel, so a TV device can opt in without a devtools console. |

**Deletion gate for the old screen.** `BigPictureScreen` and the components it
alone owned can only be deleted after the new shell is the default on dogfood
hardware for one release. The gate is: the flag flipped on, the native checklist
passed (`NATIVE-CHECKLIST.md`), no `/bigpicture`-only surface left, and the
redirect removed in the same PR that deletes the screen. Until then the old
route stays as a redirect, never as a second shell.

**Promotion of the default.** Flipping `console_v2` to on by default is a
product decision made after dogfooding, not a code cleanup. When it happens: the
flag reader returns `true`, the old screen and its layouts are deleted, the
redirect is dropped, README screenshots and the CHANGELOG are updated together.

## Regression net (#308)

The console's unit net is counted from the runner, not estimated. At `1cbf01c`
plus this lane's two new files (2026-11-19) it is **38 files / 400 tests**:

| Area | Files | Tests |
|---|---|---|
| `components/console/**` — shell, input owner, focus store, routes | 6 | 24 |
| `components/big-picture/**` — motion, sound, layout, input, power, wall | 15 | 190 |
| `theme/console-*`, `theme/artwork-accent`, `styles/console-*`, `styles/type-scale` | 6 | 56 |
| `lib/console-*`, `lib/gamepad-*`, `lib/controller-*` | 9 | 123 |
| `state/downloads*` — the Console's queue and the selector split | 2 | 7 |
| **Total** | **38** | **400** |

Run it with `pnpm --filter @ludex/web test`; the console files are all under
`apps/web/src`, so a red run is a console regression or a shared-kit regression,
never a separate suite to remember.

**Rule: no test is deleted without a replacement assertion in the same PR.**
Retargeting a test to the new shell is allowed and expected; deleting one
because its internal store disappeared is not. The replacement must assert
public behaviour (rendered DOM, command output, a pure model's return value),
not the private state the old test reached into. The old screen's 191 tests are
a floor, not a ceiling: this net is larger after the migration, and the two
files added here are part of it.

### The net catches a break

On 2026-11-19 the focus/controller path was deliberately broken, then reverted,
to check that the net is a net:

| Step | Command | Result |
|---|---|---|
| Mutation | `use-pad-back.ts`: `enabled: options.enabled ?? true` → `enabled: true` | not committed |
| Run | `pnpm --filter @ludex/web exec vitest run src/components/console/input/use-pad-back.test.tsx` | **1 failed** — "lets a disabled scope fall through instead of swallowing B" |
| Revert | `git checkout -- apps/web/src/components/console/input/use-pad-back.ts` | tree clean |

A second, independent check lives in `input-provider.test.tsx` ("restores the
previous scope and its exact focus target when the overlay closes"): breaking
the scope snapshot restore fails it. `focus-store.test.ts` pins the ring's two
writers. There is no path where a focus or controller break passes silently —
that is the acceptance this document records.

## Phases and gates

| Phase | Goal | Gate |
|---|---|---|
| 0 | De-risk: flag, `/console/*`, input owner, selector lane, power commands, WindowPolicy, fake-pad harness | Deep links, one press, no 1 Hz, no test rewrite |
| 1 | New shell + Home/Library/Game/Settings/Power/Search/install behind the flag | 6 Playwright specs, 720p/1080p/4K screenshots |
| 2 | Downloads console-native, offline, rebrand assets, flag default, delete old screen | Hardware checklist, memory/fps, `/bigpicture` redirect |
| 3 | Profiles, achievements, social, ambient, store research | Optional |

**Do not** rewrite the motion/sound/controller/pipeline kit, unify the two local databases, or start with a visual reskin. The structural input/shell fixes come first.

## Notes for agents

- #237 already landed Studio default, 24/28/48u scale, downloads selector, memoised cards and lazy routes; #268 replaced the Studio/Soft presets with rooms and cut the registry to three compositions. Do not reintroduce a preset picker.
- The skill `console-mode-ui` is the short contract; update it and this directory together when a rule changes.
- Old `docs/big-picture/` specs are legacy; this directory supersedes them.
