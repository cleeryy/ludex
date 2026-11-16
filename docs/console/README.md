# Ludex Console 2.0 + rebrand — blueprint

**Baseline:** v0.4.17 + #237 (`5406405`), 2026-11-16. Read-only research; no code was changed by the audit.

This is the source of truth for the Console 2.0 / rebrand work. The short contract lives in [`.opencode/skills/console-mode-ui/SKILL.md`](../../.opencode/skills/console-mode-ui/SKILL.md).

| Research | What it covers |
|---|---|
| [external.md](research/external.md) | 2024–2026 console/TV/web patterns, Steam/Xbox/PS/Switch/Apple TV/Plex/Kodi, spatial navigation, Gamepad API, TV budgets, sources |
| [current.md](research/current.md) | Current `/bigpicture` and `components/big-picture/*` inventory, coupling, keep/rewrite/delete, 191 tests |
| [design.md](research/design.md) | Cabinet brand/UX direction, tokens, IA, screen-by-screen blueprint, customization, motion/sound, anti-slop |
| [architecture.md](research/architecture.md) | Shell/provider/input architecture, native power/window, data boundaries, migration phases, gates |

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
- **Library:** shelf/grid + shelf chips; Collections is a shelf.
- **Game:** one primary action + tabs (Overview · Saves · Artwork · Controller).
- **Settings:** 4 rooms × 8 knobs, live preview, no Apply.
- **Power:** Sleep · Restart · Quit · Sign out; confirm defaults to Cancel.
- **Search/OSK, notifications, downloads, offline** are console surfaces, not desktop routes.
- Exactly **two modals**: install and power. Everything else is a non-modal sheet; B restores focus.

## Cabinet

- Warm charcoal, matte anodised panels, one lit object, 6/14/20/24u radii, one blurred wallpaper layer, signal yellow only.
- The **dot** is the only ornament: cursor, progress head, unread badge, wordmark full stop.
- Type: 24u floor, 28u body/actions, 48u+ hero; mono only for numerals/units/shortcuts. 7:1 contrast.
- No six-tile grids, `01–06`, uppercase mono words, layout picker, Soft/pastels, toasts in Console, desktop dialogs.
- Customization: 4 rooms × 8 knobs, live preview, “from artwork” accent validated at 7:1, device-local; no CSS injection.

## Issues

- [#267](https://github.com/cleeryy/ludex/issues/267) — `/console` shell, single input owner, data boundaries.
- [#268](https://github.com/cleeryy/ludex/issues/268) — Cabinet rebrand and design system.
- [#269](https://github.com/cleeryy/ludex/issues/269) — console surfaces (install/downloads/settings/power/search/offline/native policy).
- [#270](https://github.com/cleeryy/ludex/issues/270) — fake-pad E2E, TV performance, hardware validation and rollout.

## Phases and gates

| Phase | Goal | Gate |
|---|---|---|
| 0 | De-risk: flag, `/console/*`, input owner, selector lane, power commands, WindowPolicy, fake-pad harness | Deep links, one press, no 1 Hz, no test rewrite |
| 1 | New shell + Home/Library/Game/Settings/Power/Search/install behind the flag | 6 Playwright specs, 720p/1080p/4K screenshots |
| 2 | Downloads console-native, offline, rebrand assets, flag default, delete old screen | Hardware checklist, memory/fps, `/bigpicture` redirect |
| 3 | Profiles, achievements, social, ambient, store research | Optional |

**Do not** rewrite the motion/sound/controller/pipeline kit, unify the two local databases, or start with a visual reskin. The structural input/shell fixes come first.

## Notes for agents

- #237 already landed Studio default, 24/28/48u scale, downloads selector, memoised cards and lazy routes. Verify/finish; do not reimplement.
- The skill `console-mode-ui` is the short contract; update it and this directory together when a rule changes.
- Old `docs/big-picture/` specs are legacy; this directory supersedes them.
