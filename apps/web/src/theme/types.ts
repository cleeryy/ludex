import registryData from "./themes.json";

/**
 * Theme data model (#140, #333).
 *
 * Every colour the interface paints comes from this registry. Components use
 * semantic utility classes (`bg-surface`, `text-ink-2`, `border-line`, `text-info`)
 * and the values below arrive as CSS custom properties at runtime, so a theme is
 * data, never a per-component colour. `scripts/theme.mjs` turns the same JSON
 * into the static fallback stylesheet, and the inline boot script in `index.html`
 * replays the last applied variable map before the first paint.
 */

export const THEME_TOKEN_KEYS = [
  "bg",
  "bg-deep",
  "surface",
  "surface-2",
  "elevated",
  "line",
  "line-2",
  "ink",
  "ink-2",
  "ink-3",
  "signal",
  "on-signal",
  "signal-text",
  "signal-line",
  "info",
  "stripe-1",
  "stripe-2",
  "stripe-3",
  "stripe-4",
  "danger",
  "ok",
  "warn",
  "focus",
  "stage",
  "stage-ink",
  "stage-ink-2",
  "stage-ok",
  "scrim",
  "shadow",
  "sheen",
  "grain-opacity",
] as const;

export type ThemeTokenKey = (typeof THEME_TOKEN_KEYS)[number];
export type ThemeTokens = Record<ThemeTokenKey, string>;

/** `grain-opacity` is a number, every other token is a colour. */
export const NON_COLOR_TOKEN_KEYS: readonly ThemeTokenKey[] = ["grain-opacity"];
export const COLOR_TOKEN_KEYS: readonly ThemeTokenKey[] = THEME_TOKEN_KEYS.filter(
  (key) => !NON_COLOR_TOKEN_KEYS.includes(key),
);

export type ThemeMode = "dark" | "light";
export type BuiltinThemeId =
  | "speranza"
  | "bauhaus"
  | "braun"
  | "telemetry"
  | "blueprint"
  | "olive"
  | "kyoto"
  | "studio";
export type CustomThemeId = `custom:${string}`;
export type ThemeSelection = "system" | BuiltinThemeId | CustomThemeId;

export type ThemeRadius = "shadcn" | "swiss" | "brutalist";
export type ThemeStripe = "on" | "off";

export const THEME_RADII: readonly ThemeRadius[] = ["shadcn", "swiss", "brutalist"];
export const THEME_STRIPES: readonly ThemeStripe[] = ["on", "off"];
export const DEFAULT_THEME_RADIUS: ThemeRadius = "shadcn";
export const DEFAULT_THEME_STRIPE: ThemeStripe = "on";

export interface BuiltinTheme {
  id: BuiltinThemeId;
  label: string;
  mode: ThemeMode;
  tokens: ThemeTokens;
}

export interface ThemeRegistry {
  version: number;
  default: BuiltinThemeId;
  themes: BuiltinTheme[];
}

/** Portable theme file: what export writes and import accepts. */
export interface ThemeFile {
  format: "ludex-theme";
  version: 1;
  name: string;
  base: BuiltinThemeId;
  mode: ThemeMode;
  tokens: ThemeTokens;
  accent: string | null;
  info?: string | null;
}

export interface ThemeConfig {
  selection: ThemeSelection;
  accent: string | null;
  tint: string | null;
  /** Custom C2 (`--info`) override, device-local (#333). */
  info?: string | null;
  /** Geometry preset applied to `data-radius` (#333). */
  radius?: ThemeRadius;
  /** 3px 4-band hardware stripe toggle applied to `data-stripe` (#333). */
  stripe?: ThemeStripe;
}

export type ThemeIssueCode =
  | "bad-format"
  | "bad-field"
  | "bad-name"
  | "bad-base"
  | "bad-mode"
  | "bad-color"
  | "bad-grain"
  | "missing-token"
  | "unknown-token"
  | "low-contrast";

/** Structured so the UI can translate every message it shows. */
export interface ThemeIssue {
  code: ThemeIssueCode;
  field?: string;
  token?: string;
  base?: string;
  /** Token names for import issues; "accent"/"info"/"tint" for picker issues. */
  foreground?: string;
  background?: string;
  ratio?: number;
  min?: number;
}

export const registry = registryData as unknown as ThemeRegistry;
