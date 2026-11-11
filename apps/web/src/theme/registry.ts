import {
  AA_FOCUS,
  AA_TEXT,
  contrastRatio,
  roundRatio,
} from "./contrast";
import {
  registry,
  type BuiltinTheme,
  type BuiltinThemeId,
  type ThemeMode,
  type ThemeSelection,
  type ThemeTokenKey,
  type ThemeTokens,
} from "./types";

export const BUILTIN_THEMES: readonly BuiltinTheme[] = registry.themes;
export const DEFAULT_THEME_ID: BuiltinThemeId = registry.default;
export const CUSTOM_THEME_PREFIX = "custom:";

const THEME_BY_ID = new Map<string, BuiltinTheme>(BUILTIN_THEMES.map((theme) => [theme.id, theme]));

export function themeById(id: string): BuiltinTheme | null {
  return THEME_BY_ID.get(id) ?? null;
}

export function builtinTheme(id: BuiltinThemeId): BuiltinTheme {
  return THEME_BY_ID.get(id) ?? BUILTIN_THEMES[0];
}

export function isBuiltinThemeId(value: string): value is BuiltinThemeId {
  return THEME_BY_ID.has(value);
}

export function isCustomThemeId(value: string): value is `custom:${string}` {
  return value.startsWith(CUSTOM_THEME_PREFIX) && value.length > CUSTOM_THEME_PREFIX.length;
}

/** Built-in id for a fixed selection; null for `system` and imported themes. */
export function fixedBaseId(selection: ThemeSelection): BuiltinThemeId | null {
  return isBuiltinThemeId(selection) ? selection : null;
}

export function modeForTheme(theme: BuiltinTheme): ThemeMode {
  return theme.mode;
}

/** `true` when the browser/OS asks for a light interface. */
export function prefersLight(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return window.matchMedia("(prefers-color-scheme: light)").matches;
}

export function resolveMode(selection: ThemeSelection, light = prefersLight()): ThemeMode {
  if (selection === "system") return light ? "light" : "dark";
  if (isBuiltinThemeId(selection)) return builtinTheme(selection).mode;
  return light ? "light" : "dark";
}

export function cssVarsFor(tokens: ThemeTokens): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const [key, value] of Object.entries(tokens)) vars[`--${key}`] = value;
  return vars;
}

/** The browser chrome colour: the page background of the resolved theme. */
export function themeColorFor(tokens: ThemeTokens): string {
  return tokens.bg;
}

export interface ContrastPair {
  foreground: ThemeTokenKey;
  background: ThemeTokenKey;
  min: number;
  /** Short label used by the Appearance panel. */
  id: "body" | "secondary" | "muted" | "accent" | "danger" | "ok" | "warn" | "focus" | "stage" | "on-signal";
}

/**
 * Every pair the UI actually paints. Text pairs use WCAG AA 4.5:1; the focus
 * ring only has to clear the 3:1 non-text threshold (1.4.11).
 */
export const CONTRAST_PAIRS: readonly ContrastPair[] = [
  { id: "body", foreground: "ink", background: "bg", min: AA_TEXT },
  { id: "body", foreground: "ink", background: "surface", min: AA_TEXT },
  { id: "body", foreground: "ink", background: "surface-2", min: AA_TEXT },
  { id: "body", foreground: "ink", background: "elevated", min: AA_TEXT },
  { id: "secondary", foreground: "ink-2", background: "bg", min: AA_TEXT },
  { id: "secondary", foreground: "ink-2", background: "surface", min: AA_TEXT },
  { id: "secondary", foreground: "ink-2", background: "elevated", min: AA_TEXT },
  { id: "muted", foreground: "ink-3", background: "bg", min: AA_TEXT },
  { id: "muted", foreground: "ink-3", background: "surface", min: AA_TEXT },
  { id: "muted", foreground: "ink-3", background: "surface-2", min: AA_TEXT },
  { id: "muted", foreground: "ink-3", background: "elevated", min: AA_TEXT },
  { id: "accent", foreground: "signal-text", background: "bg", min: AA_TEXT },
  { id: "accent", foreground: "signal-text", background: "surface", min: AA_TEXT },
  { id: "danger", foreground: "danger", background: "bg", min: AA_TEXT },
  { id: "danger", foreground: "danger", background: "surface", min: AA_TEXT },
  { id: "ok", foreground: "ok", background: "bg", min: AA_TEXT },
  { id: "ok", foreground: "ok", background: "surface", min: AA_TEXT },
  { id: "warn", foreground: "warn", background: "bg", min: AA_TEXT },
  { id: "warn", foreground: "warn", background: "surface", min: AA_TEXT },
  { id: "on-signal", foreground: "on-signal", background: "signal", min: AA_TEXT },
  { id: "stage", foreground: "stage-ink", background: "stage", min: AA_TEXT },
  { id: "stage", foreground: "stage-ink-2", background: "stage", min: AA_TEXT },
  { id: "stage", foreground: "stage-ok", background: "stage", min: AA_TEXT },
  { id: "focus", foreground: "focus", background: "bg", min: AA_FOCUS },
  { id: "focus", foreground: "focus", background: "surface", min: AA_FOCUS },
];

export function checkContrast(foreground: string, background: string, min: number): boolean {
  return contrastRatio(foreground, background) >= min;
}

export interface ContrastResult {
  pair: ContrastPair;
  ratio: number;
  ok: boolean;
}

export function contrastResults(tokens: ThemeTokens): ContrastResult[] {
  return CONTRAST_PAIRS.map((pair) => {
    const ratio = roundRatio(contrastRatio(tokens[pair.foreground], tokens[pair.background]));
    return { pair, ratio, ok: ratio >= pair.min };
  });
}

export interface ContrastReport {
  body: number;
  secondary: number;
  muted: number;
  accent: number;
  focus: number;
  checks: ContrastResult[];
  ok: boolean;
}

export function contrastReport(tokens: ThemeTokens): ContrastReport {
  const checks = contrastResults(tokens);
  const ratioFor = (id: ContrastPair["id"]) => checks.find((check) => check.pair.id === id)?.ratio ?? 0;
  return {
    body: ratioFor("body"),
    secondary: ratioFor("secondary"),
    muted: ratioFor("muted"),
    accent: ratioFor("accent"),
    focus: ratioFor("focus"),
    checks,
    ok: checks.every((check) => check.ok),
  };
}
