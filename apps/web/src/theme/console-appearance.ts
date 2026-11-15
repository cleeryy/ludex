import { notifyPreferencesChanged } from "@/lib/preferences-events";
import { applyAccent, deriveTheme, tvTokens } from "./derive";
import { builtinTheme, cssVarsFor } from "./registry";
import type { StorageLike } from "./storage";
import type { ThemeMode, ThemeTokens } from "./types";

/**
 * Console appearance (#229).
 *
 * The console is a device, not a browser window: it owns its surface treatment
 * (a preset), its wallpaper and its corner scale, and those choices belong to
 * the screen in front of the player. They are stored per server like the layout
 * preference, never in the account document — a phone must not inherit a TV's
 * materials.
 *
 * Colour still comes from the theme registry. The Soft preset is the `soft`
 * built-in palette (cream paper, warm ink) run through the same
 * accent/tint/contrast pipeline as every other theme; the Studio preset keeps
 * the app's rendered tokens and only lets the console accent refine them.
 * Components never see a hex value: this module returns CSS custom properties
 * and data attributes, and the material classes in `styles/index.css` read them.
 */

export type ConsolePreset = "soft" | "studio";
/** `artwork` = the selected game's art, blurred by the Soft material. */
export type ConsoleBackground = "artwork" | "solid" | "gradient";
export type ConsoleRadius = "soft" | "round" | "sharp";
export type ConsoleAccentId = "theme" | "coral" | "mint" | "sky" | "lilac" | "sun";

export interface ConsoleAppearance {
  preset: ConsolePreset;
  accent: ConsoleAccentId;
  background: ConsoleBackground;
  radius: ConsoleRadius;
}

/**
 * Curated accents, one tone per surface polarity. A single hex cannot clear
 * 4.5:1 on both cream and near-black, so each entry carries the two tones the
 * composition needs. `applyAccent` still validates every pair at runtime and
 * keeps the base palette when a theme makes the tone impossible.
 */
const CONSOLE_ACCENTS: Record<Exclude<ConsoleAccentId, "theme">, Record<ThemeMode, string>> = {
  coral: { dark: "#ffc2b8", light: "#b4382e" },
  mint: { dark: "#93e3b6", light: "#2c6f4e" },
  sky: { dark: "#a6d2ff", light: "#1f5f9e" },
  lilac: { dark: "#dccbff", light: "#5b3f9e" },
  sun: { dark: "#ffd166", light: "#7d5400" },
};

export const CONSOLE_PRESETS: readonly ConsolePreset[] = ["soft", "studio"];
export const CONSOLE_BACKGROUNDS: readonly ConsoleBackground[] = ["artwork", "solid", "gradient"];
export const CONSOLE_RADII: readonly ConsoleRadius[] = ["soft", "round", "sharp"];
export const CONSOLE_ACCENT_IDS: readonly ConsoleAccentId[] = [
  "theme",
  "coral",
  "mint",
  "sky",
  "lilac",
  "sun",
];

/** Soft is the console default: the Switch-like, rounded, translucent look. */
export const DEFAULT_CONSOLE_APPEARANCE: ConsoleAppearance = {
  preset: "soft",
  accent: "theme",
  background: "artwork",
  radius: "soft",
};

export const CONSOLE_APPEARANCE_KEY = "ludex.console.appearance";
export const CONSOLE_APPEARANCE_VERSION = 1;

export interface StoredConsoleAppearance {
  v: typeof CONSOLE_APPEARANCE_VERSION;
  appearance: ConsoleAppearance;
}

function isOneOf<T extends string>(values: readonly T[], value: unknown): value is T {
  return typeof value === "string" && (values as readonly string[]).includes(value);
}

/** Unknown values (old record, hand-edited storage, `null`) fall back per field. */
export function sanitizeConsoleAppearance(value: unknown): ConsoleAppearance {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { ...DEFAULT_CONSOLE_APPEARANCE };
  }
  const record = value as Record<string, unknown>;
  return {
    preset: isOneOf(CONSOLE_PRESETS, record.preset) ? record.preset : DEFAULT_CONSOLE_APPEARANCE.preset,
    accent: isOneOf(CONSOLE_ACCENT_IDS, record.accent) ? record.accent : DEFAULT_CONSOLE_APPEARANCE.accent,
    background: isOneOf(CONSOLE_BACKGROUNDS, record.background)
      ? record.background
      : DEFAULT_CONSOLE_APPEARANCE.background,
    radius: isOneOf(CONSOLE_RADII, record.radius) ? record.radius : DEFAULT_CONSOLE_APPEARANCE.radius,
  };
}

export function readConsoleAppearance(
  storage: StorageLike | null | undefined,
  key: string = CONSOLE_APPEARANCE_KEY,
): ConsoleAppearance {
  if (!storage) return { ...DEFAULT_CONSOLE_APPEARANCE };
  try {
    const raw = storage.getItem(key);
    if (!raw) return { ...DEFAULT_CONSOLE_APPEARANCE };
    const parsed: unknown = JSON.parse(raw);
    // The version wrapper is the record; a bare appearance object is accepted
    // so a value written by an older build still resolves field by field.
    if (parsed && typeof parsed === "object" && "appearance" in parsed) {
      return sanitizeConsoleAppearance((parsed as Record<string, unknown>).appearance);
    }
    return sanitizeConsoleAppearance(parsed);
  } catch {
    return { ...DEFAULT_CONSOLE_APPEARANCE };
  }
}

export function writeConsoleAppearance(
  storage: StorageLike | null | undefined,
  appearance: ConsoleAppearance,
  key: string = CONSOLE_APPEARANCE_KEY,
): ConsoleAppearance {
  const next = sanitizeConsoleAppearance(appearance);
  try {
    const record: StoredConsoleAppearance = { v: CONSOLE_APPEARANCE_VERSION, appearance: next };
    storage?.setItem(key, JSON.stringify(record));
    notifyPreferencesChanged();
  } catch {
    /* the choice applies for this session when storage is unavailable */
  }
  return next;
}

/** Cycles a choice row; wraps so a held direction is never a dead end. */
export function cycleChoice<T>(values: readonly T[], current: T, delta: -1 | 1): T {
  if (values.length === 0) return current;
  const at = values.indexOf(current);
  const from = at === -1 ? 0 : at;
  return values[(from + delta + values.length) % values.length];
}

/**
 * A preset is a bundle: switching it restores the corner scale that belongs to
 * it (Soft rounds, Studio returns to the archive's 2 px edge). The player can
 * then re-pick the corners; the two choices stay independent afterwards.
 */
export function withConsolePreset(
  appearance: ConsoleAppearance,
  preset: ConsolePreset,
): ConsoleAppearance {
  return { ...appearance, preset, radius: preset === "soft" ? "soft" : "sharp" };
}

/** The accent tone for a preset, or `null` when the app theme owns the accent. */
export function consoleAccentHex(
  appearance: ConsoleAppearance,
  mode: ThemeMode,
): string | null {
  return consoleAccentColor(appearance.accent, mode);
}

/** Preview tone for the Settings swatches (null = follow the theme accent). */
export function consoleAccentColor(accent: ConsoleAccentId, mode: ThemeMode): string | null {
  if (accent === "theme") return null;
  return CONSOLE_ACCENTS[accent][mode];
}

/**
 * Custom properties for the console root. The return value is a complete map
 * for the Soft preset (it has its own palette) and an empty map for Studio
 * without an accent (it inherits the app's rendered tokens, including the
 * TV-safe variant). Both paths end in the same derived pipeline, so a curated
 * accent that cannot clear 4.5:1 falls back instead of shipping a bad pair.
 */
export function consoleTokens(
  appearance: ConsoleAppearance,
  app: {
    tokens: ThemeTokens;
    accent: string | null;
    tint: string | null;
    mode: ThemeMode;
    tv: boolean;
  },
): ThemeTokens | null {
  if (appearance.preset === "studio" && appearance.accent === "theme") return null;

  if (appearance.preset === "studio") {
    const accent = consoleAccentHex(appearance, app.mode);
    if (!accent) return null;
    const tokens = applyAccent(app.tokens, accent).tokens;
    // `app.tokens` is already the rendered (TV-safe) map, so this only lifts
    // the accent's text pairs; the line tokens are mixed again, which is a
    // slightly stronger hairline, never a weaker one.
    return app.tv ? tvTokens(tokens) : tokens;
  }

  // Soft is a light palette whatever the desktop theme is: picking the dark
  // tone of a curated accent here would fail `applyAccent` against the cream
  // surfaces and silently fall back.
  const mode: ThemeMode = "light";
  const accent = consoleAccentHex(appearance, mode) ?? app.accent;
  const tokens = deriveTheme(builtinTheme("soft"), accent, app.tint).tokens;
  return app.tv ? tvTokens(tokens) : tokens;
}

export function consoleVars(tokens: ThemeTokens | null): Record<string, string> {
  return tokens ? cssVarsFor(tokens) : {};
}

/** Attributes the console root and the CSS material classes read. */
export function consoleAttributes(appearance: ConsoleAppearance): Record<string, string> {
  return {
    "data-console-appearance": appearance.preset,
    "data-console-background": appearance.background,
    "data-console-radius": appearance.radius,
  };
}
