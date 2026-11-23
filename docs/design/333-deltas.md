# Design System #333 — Documented Deltas

This file records every intentional delta between the literal table in GitHub issue `#333`
(*Generalized Design System — ARC × Swiss/Bauhaus × shadcn/ui*) and the shipped implementation
in `apps/web/src/theme/`.

## 1. WCAG AA (4.5:1) Contrast Adjustments

All surface fills (`bg-deep`, `bg`, `surface`, `surface-2`, `elevated`), border tokens
(`line`, `line-2`), primary/secondary inks (`ink`, `ink-2`), primary action fills (`signal`),
and 4-band hardware stripe colors (`stripe-1..4`) use the **exact** hex values from `#333`.

To satisfy the repository's WCAG AA 4.5:1 contract (`apps/web/src/theme/registry.ts` and
`apps/web/src/styles/tokens.test.ts`), minimal lightness-only lifts were applied to muted
text (`ink-3`), `signal-text` (when `signal` is painted as text on light surfaces), and
light-theme status text (`info`, `warn`, `ok`). Console 7:1 contrast continues to be
computed dynamically by `cabinetSafe()` without mutating base tokens.

### Dark `on-signal` ink on bright red/orange/rose presets

White `#FFFFFF` text on `signal` fell below 4.5:1 on three dark presets (`speranza` 3.44:1,
`braun` 3.56:1, `kyoto` 3.67:1). Rather than darkening the signature `signal` fills, each
uses its own `bg-deep` dark ink for `on-signal`:

| Preset | `signal` (unchanged) | `#333` `on-signal` | Shipped `on-signal` | Ratio |
|---|---|---|---|---|
| `speranza` | `#ff4422` | `#ffffff` (3.44:1) | `#0b0c0f` | 5.68:1 |
| `braun` | `#ea580c` | `#ffffff` (3.56:1) | `#141210` | 5.25:1 |
| `kyoto` | `#f43f5e` | `#ffffff` (3.67:1) | `#110d12` | 5.25:1 |

### Minimal lightness lifts on `ink-3` and light-theme text roles

| Preset | Token | `#333` hex | Shipped hex | Worst painted pair ratio |
|---|---|---|---|---|
| `speranza` | `ink-3` | `#6c7382` | `#8e939f` | 4.61:1 on `elevated` |
| `bauhaus` | `ink-3` | `#7c776e` | `#6b665f` | 4.57:1 on `surface-2` |
| `bauhaus` | `signal-text` | `#e0331e` | `#cc2e1b` | 4.58:1 on `bg` |
| `bauhaus` | `warn` | `#d9820b` | `#9c5e08` | 4.58:1 on `bg` |
| `bauhaus` | `ok` | `#15803d` | `#147c3b` | 4.55:1 on `bg` |
| `braun` | `ink-3` | `#82786d` | `#a09890` | 4.58:1 on `elevated` |
| `braun` | `signal-text` | `#ea580c` | `#ea5a0e` | 4.56:1 on `surface` |
| `telemetry` | `ink-3` | `#61828b` | `#8fa6ad` | 4.59:1 on `elevated` |
| `blueprint` | `ink-3` | `#6e85ba` | `#94a5cc` | 4.60:1 on `elevated` |
| `olive` | `ink-3` | `#707a6c` | `#979e94` | 4.58:1 on `elevated` |
| `kyoto` | `ink-3` | `#827382` | `#9e929e` | 4.64:1 on `elevated` |
| `studio` | `ink-3` | `#71717a` | `#6e6e76` | 4.60:1 on `surface-2` |
| `studio` | `info` | `#0891b2` | `#077d99` | 4.56:1 on `surface` |
| `studio` | `warn` | `#d97706` | `#ab5e05` | 4.55:1 on `surface` |
| `studio` | `ok` | `#16a34a` | `#12843c` | 4.57:1 on `surface` |

## 2. Fonts

- Replaced `@fontsource-variable/bricolage-grotesque` with `@fontsource-variable/inter` (bundled locally; no runtime Google Fonts).
- `--font-display` and `--font-ui` resolve to `"Inter Variable", "Inter", "Instrument Sans Variable", "Instrument Sans", "Helvetica Neue", system-ui, sans-serif`.
- `--font-code` stays `"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, SFMono-Regular, monospace`, strictly for numerical and technical data.

## 3. Preference Persistence (No Backend Changes)

- `crates/ludex-server/src/routes/preferences.rs` allowlists `theme.{selection, accent, tint}`.
- All 8 new preset ids (`speranza`, `bauhaus`, `braun`, `telemetry`, `blueprint`, `olive`, `kyoto`, `studio`) pass existing server validation unchanged.
- `info` (custom C2 picker), `radius` (`data-radius`: `shadcn | swiss | brutalist`), and `stripe` (`data-stripe`: `on | off`) are persisted device-locally inside `localStorage["ludex.theme"]` and replayed synchronously by `apps/web/index.html`.
- Legacy preset ids stored in `localStorage` or synced from older sessions migrate automatically: `midnight`/`dark` → `speranza`, `light`/`soft` → `bauhaus`, `nord` → `telemetry`, `gruvbox`/`amber` → `braun`.
