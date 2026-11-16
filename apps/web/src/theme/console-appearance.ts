import { notifyPreferencesChanged } from "@/lib/preferences-events";
import { applyAccent, tvTokens } from "./derive";
import { cssVarsFor } from "./registry";
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
 * Colour comes from the app theme, always (#237). A preset only answers the
 * material question — corners, elevation, wallpaper — so entering Console can
 * no longer flip the product to a second palette. The curated console accents
 * still refine the rendered tokens through the same accent/contrast pipeline as
 * every other theme. Components never see a hex value: this module returns CSS
 * custom properties and data attributes, and the material classes in
 * `styles/index.css` read them.
 */

export type ConsolePreset = "soft" | "studio";
/** `artwork` = the selected game's art behind the chrome, blurred and veiled. */
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

/**
 * Studio is the default (#237): the archive look the desktop app already owns,
 * crisp hairlines, flat panels and the app theme. Soft stays selectable as an
 * opt-in rounding of the same palette, never as a second palette.
 */
export const DEFAULT_CONSOLE_APPEARANCE: ConsoleAppearance = {
  preset: "studio",
  accent: "theme",
  background: "artwork",
  radius: "sharp",
};

/**
 * The v1 default. It is the record every player who never opened the
 * appearance panel carries implicitly, so a stored value equal to it is read
 * as "untouched" and migrated to Studio. Anything else is a real choice and is
 * kept: Soft remains one cycle away.
 */
const LEGACY_DEFAULT_APPEARANCE: ConsoleAppearance = {
  preset: "soft",
  accent: "theme",
  background: "artwork",
  radius: "soft",
};

export const CONSOLE_APPEARANCE_KEY = "ludex.console.appearance";
/** v2: Studio default; the preset no longer owns a palette (#237). */
export const CONSOLE_APPEARANCE_VERSION = 2;
/** Records written before the version wrapper existed. */
const LEGACY_CONSOLE_APPEARANCE_VERSION = 1;

export interface StoredConsoleAppearance {
  v: typeof CONSOLE_APPEARANCE_VERSION;
  appearance: ConsoleAppearance;
}

function sameAppearance(a: ConsoleAppearance, b: ConsoleAppearance): boolean {
  return (
    a.preset === b.preset &&
    a.accent === b.accent &&
    a.background === b.background &&
    a.radius === b.radius
  );
}

/**
 * Reads a stored record through the version ladder. Only the untouched v1
 * default moves; an explicit Soft choice is preserved so a player who picked
 * the rounded look keeps it after the upgrade.
 */
export function migrateConsoleAppearance(
  appearance: ConsoleAppearance,
  version: number,
): ConsoleAppearance {
  if (version >= CONSOLE_APPEARANCE_VERSION) return appearance;
  return sameAppearance(appearance, LEGACY_DEFAULT_APPEARANCE)
    ? { ...DEFAULT_CONSOLE_APPEARANCE }
    : appearance;
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
      const record = parsed as Record<string, unknown>;
      const version = typeof record.v === "number" ? record.v : LEGACY_CONSOLE_APPEARANCE_VERSION;
      return migrateConsoleAppearance(sanitizeConsoleAppearance(record.appearance), version);
    }
    return migrateConsoleAppearance(
      sanitizeConsoleAppearance(parsed),
      LEGACY_CONSOLE_APPEARANCE_VERSION,
    );
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
 * Custom properties for the console root. Only a curated accent changes
 * colour: both presets inherit the app's rendered tokens, including the
 * TV-safe variant, so Console keeps the product's palette (#237). The accent
 * still runs through the derived pipeline, so a curated tone that cannot clear
 * 4.5:1 falls back instead of shipping a bad pair.
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
  void app.tint;
  if (appearance.accent === "theme") return null;
  const accent = consoleAccentHex(appearance, app.mode);
  if (!accent) return null;
  const tokens = applyAccent(app.tokens, accent).tokens;
  // `app.tokens` is already the rendered (TV-safe) map, so this only lifts the
  // accent's text pairs; the line tokens are mixed again, which is a slightly
  // stronger hairline, never a weaker one.
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
