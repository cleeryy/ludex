# Design System #333 — Verification & Visual Evidence

This document records the automated checks and real Chromium visual captures for the **ARC × Swiss/Bauhaus × shadcn/ui** app-wide design system (#333).

## Automated Validation Matrix

| Check | Scope | Result |
| --- | --- | --- |
| `node apps/web/scripts/theme.mjs` | 8 built-in presets × 31 tokens (`themes.generated.css` freshness) | PASS |
| `node apps/web/scripts/i18n.mjs` | 4,501 message IDs, 13 shipped locales (`en` and `fr` 100%) | PASS |
| `pnpm --filter @ludex/web test` | 117 test files, 1,168 tests (AA contrast, token completeness, hardcoded color guard, app-wide semantic type scale, monospace hygiene, core UI primitives) | PASS |
| `pnpm --filter @ludex/web exec tsc --noEmit` | Full TypeScript strict check | PASS |
| `pnpm --filter @ludex/web build` | Production + demo (`VITE_USE_MOCKS=true`) bundle + SW precache | PASS |

## Key Invariants Verified

1. **8 Built-in Presets & Contrast**: `speranza` (default dark), `bauhaus` (default light), `braun`, `telemetry`, `blueprint`, `olive`, `kyoto`, `studio` all pass WCAG AA (`≥ 4.5:1` for text & `info`, `≥ 3:1` for UI accents) across `bg` and `surface`. Minimal lifts are documented in [333-deltas.md](./333-deltas.md).
2. **Type Scale & Monospace Hygiene**: Zero arbitrary `text-[Npx]` or `text-[clamp(...)]` remain across `apps/web/src/{screens,components,app.tsx}`. JetBrains Mono is restricted to numeric/technical telemetry (`.num`, `<code>`, `<pre>`, URLs, digests, paths); headings, table headers, kickers, pills, and buttons use Inter / Instrument Sans Variable.
3. **ARC × Swiss × shadcn/ui Chrome**:
   - 3px 4-band `.arc-stripe` at the top of `AppShell` and `PublicPage` (toggled via `data-stripe="on|off"`).
   - 236px `NavRail` on `bg-deep` with square 7px active LEDs and a mini 16-segment dual-color (`--signal` + `--info`) active-download widget.
   - 3:4 game covers with `--r-md`, `border-line` → `border-ink` + `-2px` lift on hover, and zero stacked badges over artwork.
   - Unified Swiss tables (`bg-bg-deep` header, 1px `line` dividers, `hover:bg-surface-2`, tabular numerals), 4-column `TelemetryStrip`, and 16-segment `SegmentedGauge`.

## Captured Screenshots (Playwright Chromium, Demo Mode)

All captures were rendered from the `VITE_USE_MOCKS=true` production build in headless Chromium (`data-radius="shadcn"`, `data-stripe="on"`), verified with zero horizontal overflow (`overflowX: false`) and visible `.arc-stripe`.

| Frame | Route | Theme | Viewport | Size |
| --- | --- | --- | --- | --- |
| [home-speranza-desktop.png](../assets/screenshots/design-333/home-speranza-desktop.png) | `/` | `speranza` (dark) | 1920×1080 | 256.2 KB |
| [library-speranza-desktop.png](../assets/screenshots/design-333/library-speranza-desktop.png) | `/library` | `speranza` (dark) | 1920×1080 | 302.5 KB |
| [library-bauhaus-desktop.png](../assets/screenshots/design-333/library-bauhaus-desktop.png) | `/library` | `bauhaus` (light) | 1920×1080 | 160.0 KB |
| [downloads-speranza-desktop.png](../assets/screenshots/design-333/downloads-speranza-desktop.png) | `/downloads` | `speranza` (dark) | 1920×1080 | 158.0 KB |
| [admin-health-speranza-desktop.png](../assets/screenshots/design-333/admin-health-speranza-desktop.png) | `/admin/health` | `speranza` (dark) | 1920×1080 | 52.1 KB |
| [admin-users-speranza-desktop.png](../assets/screenshots/design-333/admin-users-speranza-desktop.png) | `/admin/users` | `speranza` (dark) | 1920×1080 | 110.5 KB |
| [public-download-speranza-desktop.png](../assets/screenshots/design-333/public-download-speranza-desktop.png) | `/download` | `speranza` (dark) | 1920×1080 | 49.5 KB |
| [library-speranza-mobile.png](../assets/screenshots/design-333/library-speranza-mobile.png) | `/library` | `speranza` (dark) | 390×844 @2x | 57.3 KB |
| [library-bauhaus-mobile.png](../assets/screenshots/design-333/library-bauhaus-mobile.png) | `/library` | `bauhaus` (light) | 390×844 @2x | 62.6 KB |
