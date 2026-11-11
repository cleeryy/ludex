import {
  DEFAULT_THEME_ID,
  builtinTheme,
  cssVarsFor,
  isBuiltinThemeId,
  isCustomThemeId,
  themeColorFor,
} from "./registry";
import { deriveTheme } from "./derive";
import { normalizeHex } from "./contrast";
import { validateThemeFile } from "./validate";
import {
  type BuiltinTheme,
  type ThemeConfig,
  type ThemeFile,
  type ThemeMode,
  type ThemeSelection,
  type ThemeTokens,
} from "./types";

/**
 * Persistence for the appearance settings (#140).
 *
 * The boot script in `index.html` is deliberately dumb: it reads one record and
 * replays its ready-made variable maps. Resolving a selection, deriving an
 * accent and validating contrast all happen here, once, at save time, so the
 * first paint is a single `style.setProperty` loop with no branching.
 */

export const THEME_STORAGE_KEY = "ludex.theme";
export const CUSTOM_THEMES_KEY = "ludex.theme.custom.v1";
export const APPEARANCE_VERSION = 2;

const LEGACY_THEMES: Record<string, ThemeSelection> = {
  dark: "midnight",
  light: "light",
  system: "system",
};

export interface StoredAppearance {
  v: typeof APPEARANCE_VERSION;
  config: ThemeConfig;
  /** Resolved mode for fixed themes; `system` ignores it and follows the OS. */
  mode: ThemeMode;
  bases: Record<ThemeMode, string>;
  vars: Record<ThemeMode, Record<string, string>>;
  themeColor: Record<ThemeMode, string>;
}

export type StorageLike = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export function browserStorage(): StorageLike | null {
  if (typeof localStorage === "undefined") return null;
  try {
    return localStorage;
  } catch {
    return null;
  }
}

function stringOrNull(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

export function sanitizeSelection(value: unknown): ThemeSelection {
  if (typeof value !== "string") return "system";
  if (value === "system" || isBuiltinThemeId(value) || isCustomThemeId(value)) return value;
  return LEGACY_THEMES[value] ?? "system";
}

export function defaultConfig(): ThemeConfig {
  return { selection: "system", accent: null, tint: null };
}

export function readConfig(storage: StorageLike | null = browserStorage()): ThemeConfig {
  if (!storage) return defaultConfig();
  let raw: string | null = null;
  try {
    raw = storage.getItem(THEME_STORAGE_KEY);
  } catch {
    return defaultConfig();
  }
  if (!raw) return defaultConfig();

  try {
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === "object") {
      const record = parsed as Record<string, unknown>;
      const config = (record.config ?? record) as Record<string, unknown>;
      return {
        selection: sanitizeSelection(config.selection),
        accent: normalizeHex(stringOrNull(config.accent) ?? ""),
        tint: normalizeHex(stringOrNull(config.tint) ?? ""),
      };
    }
  } catch {
    // Legacy plain string: "dark" | "light" | "system".
  }

  return { selection: LEGACY_THEMES[raw] ?? "system", accent: null, tint: null };
}

export function readCustomThemes(storage: StorageLike | null = browserStorage()): ThemeFile[] {
  if (!storage) return [];
  let raw: string | null = null;
  try {
    raw = storage.getItem(CUSTOM_THEMES_KEY);
  } catch {
    return [];
  }
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    const themes: ThemeFile[] = [];
    for (const entry of parsed) {
      const result = validateThemeFile(entry);
      if (result.ok) themes.push(result.theme);
    }
    return themes;
  } catch {
    return [];
  }
}

export function writeCustomThemes(themes: readonly ThemeFile[], storage: StorageLike | null = browserStorage()): void {
  if (!storage) return;
  try {
    if (themes.length === 0) storage.removeItem(CUSTOM_THEMES_KEY);
    else storage.setItem(CUSTOM_THEMES_KEY, JSON.stringify(themes));
  } catch {
    // Private mode or a full quota: the picker still works for this session.
  }
}

export function customThemeId(name: string, tokens: ThemeTokens): `custom:${string}` {
  const slug = name
    .toLowerCase()
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 24) || "theme";
  let hash = 0x811c9dc5;
  const source = `${name}:${JSON.stringify(tokens)}`;
  for (const character of source) {
    hash ^= character.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `custom:${slug}-${hash.toString(16).padStart(8, "0")}`;
}

export function customThemeById(id: string, themes: readonly ThemeFile[]): ThemeFile | null {
  if (!isCustomThemeId(id)) return null;
  return themes.find((theme) => customThemeId(theme.name, theme.tokens) === id) ?? null;
}

/** Base palette for a selection in a given mode. Imported themes keep their own tokens. */
export function baseThemeFor(
  selection: ThemeSelection,
  customs: readonly ThemeFile[],
  mode: ThemeMode,
): BuiltinTheme {
  if (selection === "system") return builtinTheme(mode === "light" ? "light" : "midnight");
  if (isCustomThemeId(selection)) {
    const custom = customThemeById(selection, customs);
    if (custom) {
      return { id: custom.base, label: custom.name, mode: custom.mode, tokens: custom.tokens };
    }
    return builtinTheme(mode === "light" ? "light" : DEFAULT_THEME_ID);
  }
  if (isBuiltinThemeId(selection)) return builtinTheme(selection);
  return builtinTheme(mode === "light" ? "light" : DEFAULT_THEME_ID);
}

function selectionMode(selection: ThemeSelection, customs: readonly ThemeFile[]): ThemeMode {
  if (isCustomThemeId(selection)) return customThemeById(selection, customs)?.mode ?? "dark";
  if (isBuiltinThemeId(selection)) return builtinTheme(selection).mode;
  return "dark";
}

/** Resolved mode for a fixed theme; system keeps the caller's OS answer. */
export function modeForSelection(
  selection: ThemeSelection,
  customs: readonly ThemeFile[],
  systemMode: ThemeMode,
): ThemeMode {
  return selection === "system" ? systemMode : selectionMode(selection, customs);
}

function varsFor(config: ThemeConfig, customs: readonly ThemeFile[], mode: ThemeMode): ThemeTokens {
  const base = baseThemeFor(config.selection, customs, mode);
  return deriveTheme(base, config.accent, config.tint).tokens;
}

export function buildAppearance(config: ThemeConfig, customs: readonly ThemeFile[] = []): StoredAppearance {
  const sanitized: ThemeConfig = {
    selection: sanitizeSelection(config.selection),
    accent: normalizeHex(config.accent ?? ""),
    tint: normalizeHex(config.tint ?? ""),
  };
  const dark = varsFor(sanitized, customs, "dark");
  const light = varsFor(sanitized, customs, "light");
  return {
    v: APPEARANCE_VERSION,
    config: sanitized,
    mode: sanitized.selection === "system" ? "dark" : modeForSelection(sanitized.selection, customs, "dark"),
    bases: {
      dark: baseThemeFor(sanitized.selection, customs, "dark").id,
      light: baseThemeFor(sanitized.selection, customs, "light").id,
    },
    vars: { dark: cssVarsFor(dark), light: cssVarsFor(light) },
    themeColor: { dark: themeColorFor(dark), light: themeColorFor(light) },
  };
}

export function writeAppearance(
  appearance: StoredAppearance,
  storage: StorageLike | null = browserStorage(),
): void {
  if (!storage) return;
  try {
    storage.setItem(THEME_STORAGE_KEY, JSON.stringify(appearance));
  } catch {
    // The theme applies for this session even when persistence is unavailable.
  }
}

export type ThemeRoot = Pick<HTMLElement, "style" | "dataset">;

export function applyAppearance(
  appearance: StoredAppearance,
  mode: ThemeMode,
  tv: boolean,
  root: ThemeRoot | null = typeof document === "undefined" ? null : document.documentElement,
): void {
  if (!root) return;
  const vars = appearance.vars[mode] ?? appearance.vars.dark;
  for (const [property, value] of Object.entries(vars)) root.style.setProperty(property, value);
  root.style.setProperty("--focus-width", tv ? "3px" : "2px");
  root.style.setProperty("--focus-offset", tv ? "4px" : "2px");
  root.style.colorScheme = mode;
  root.dataset.theme = appearance.bases[mode] ?? DEFAULT_THEME_ID;
  root.dataset.themeMode = mode;
  root.dataset.themeSelection = appearance.config.selection;
  root.dataset.tv = tv ? "true" : "false";

  const meta = typeof document === "undefined" ? null : document.querySelector('meta[name="theme-color"]');
  meta?.setAttribute("content", appearance.themeColor[mode]);
}

/** Used by tests and by the settings panel after a rejected import. */
export function clearAppearance(root: ThemeRoot): void {
  for (const key of [
    "bg", "bg-deep", "surface", "surface-2", "elevated", "line", "line-2", "ink", "ink-2", "ink-3",
    "signal", "on-signal", "signal-text", "signal-line", "danger", "ok", "warn", "focus", "stage",
    "stage-ink", "stage-ink-2", "stage-ok", "scrim", "shadow", "sheen", "grain-opacity",
    "focus-width", "focus-offset",
  ]) {
    root.style.removeProperty(`--${key}`);
  }
  delete root.dataset.theme;
  delete root.dataset.themeMode;
  delete root.dataset.themeSelection;
  delete root.dataset.tv;
}
