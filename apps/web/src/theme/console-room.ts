import { notifyPreferencesChanged } from "@/lib/preferences-events";
import { bestForeground, contrastRatio, ensureContrast, mixHex, roundRatio } from "./contrast";
import { tvTokens } from "./derive";
import type { StorageLike } from "./storage";
import type { ThemeMode, ThemeTokens } from "./types";

/**
 * Cabinet, the Console's material (#268).
 *
 * The Console is a device room, not a second brand: colour still comes from the
 * app theme, and this module never ships a palette. A room answers the *material*
 * question — polarity, warmth, wall, corners, density, motion, sound, haptics —
 * and the eight knobs let a player override each answer. Switching a room
 * restores that room's bundle; every knob is then independent, applied live and
 * reset per section, with no Apply step anywhere.
 *
 * Everything here is pure so the Settings preview, the screen and the tests
 * resolve exactly the same values. Components read CSS attributes and custom
 * properties; no component ever sees a hex value.
 *
 * Cabinet rules encoded here and in `styles/index.css`:
 *   - warm charcoal: a small warm cast over the theme's own surfaces, never a
 *     second colour map;
 *   - depth by luminance: sheets are a lighter step, not a drop shadow;
 *   - one blurred layer: the wallpaper. Never a blurred moving element;
 *   - signal yellow as the only chromatic event. The "from artwork" accent is
 *     clamped and lifted to 7:1, or it falls back to the theme's signal.
 */

/** Four rooms. Each is a bundle of the eight knobs, not a colour picker. */
export type ConsoleRoom = "night" | "projector" | "daylight" | "arcade";
/** Theme follows the app accent; Signal keeps the theme's yellow; artwork derives one. */
export type ConsoleAccentSource = "theme" | "signal" | "artwork";
export type ConsoleWall = "artwork" | "solid" | "gradient";
export type ConsoleCorners = "sharp" | "soft" | "round";
export type ConsoleDensity = "comfortable" | "compact" | "dense";
export type ConsoleMotion = "full" | "calm" | "off";
export type ConsoleSound = "full" | "reduced" | "off";
/** Mirrors the pad engine's own strength scale (`gamepad-core.ts`). */
export type ConsoleHaptics = "off" | "standard" | "strong";

export interface ConsoleRoomSettings {
  room: ConsoleRoom;
  accent: ConsoleAccentSource;
  wall: ConsoleWall;
  corners: ConsoleCorners;
  density: ConsoleDensity;
  motion: ConsoleMotion;
  sound: ConsoleSound;
  haptics: ConsoleHaptics;
}

export const CONSOLE_ROOMS: readonly ConsoleRoom[] = ["night", "projector", "daylight", "arcade"];
export const CONSOLE_ACCENT_SOURCES: readonly ConsoleAccentSource[] = ["theme", "signal", "artwork"];
export const CONSOLE_WALLS: readonly ConsoleWall[] = ["artwork", "solid", "gradient"];
export const CONSOLE_CORNERS: readonly ConsoleCorners[] = ["sharp", "soft", "round"];
export const CONSOLE_DENSITIES: readonly ConsoleDensity[] = ["comfortable", "compact", "dense"];
export const CONSOLE_MOTIONS: readonly ConsoleMotion[] = ["full", "calm", "off"];
export const CONSOLE_SOUNDS: readonly ConsoleSound[] = ["full", "reduced", "off"];
export const CONSOLE_HAPTICS: readonly ConsoleHaptics[] = ["off", "standard", "strong"];

/** Night is the Cabinet: warm charcoal, one lit object, art wall, springs. */
export const DEFAULT_CONSOLE_ROOM: ConsoleRoomSettings = {
  room: "night",
  accent: "theme",
  wall: "artwork",
  corners: "soft",
  density: "comfortable",
  motion: "full",
  sound: "full",
  haptics: "standard",
};

/**
 * The bundles. A room changes all eight knobs at once because that is what a
 * room is: the answer to "where does this TV live". A player who then changes
 * one knob keeps the rest of the room, and `Reset section` restores the bundle.
 */
export const CONSOLE_ROOM_BUNDLES: Record<ConsoleRoom, Omit<ConsoleRoomSettings, "room" | "accent">> = {
  night: {
    wall: "artwork",
    corners: "soft",
    density: "comfortable",
    motion: "full",
    sound: "full",
    haptics: "standard",
  },
  projector: {
    // Long throw, low contrast, no blur to spend and no tint to wash out.
    wall: "solid",
    corners: "sharp",
    density: "compact",
    motion: "calm",
    sound: "reduced",
    haptics: "standard",
  },
  daylight: {
    // Paper and ink: the app's own light theme, flat panels, no blur.
    wall: "solid",
    corners: "soft",
    density: "comfortable",
    motion: "full",
    sound: "full",
    haptics: "standard",
  },
  arcade: {
    // Hard edges, poster wall, dense rail; the palette stays Cabinet.
    wall: "gradient",
    corners: "sharp",
    density: "dense",
    motion: "full",
    sound: "full",
    haptics: "strong",
  },
};

/** Per-room material, consumed by the stylesheet through data attributes. */
export interface ConsoleRoomDesign {
  /** `inherit` keeps the app polarity; `light` renders the app's light map. */
  polarity: "inherit" | "light";
  /** Warm cast applied to the theme's surfaces, 0–1. */
  warm: number;
  /** How far the backgrounds step to pure black, 0–1. */
  toBlack: number;
  /** Wallpaper blur in u; 0 for the rooms that do not spend it. */
  wallBlur: number;
  /** Art under the wall wash, 0–1. */
  artOpacity: number;
}

export const CONSOLE_ROOM_DESIGN: Record<ConsoleRoom, ConsoleRoomDesign> = {
  night: { polarity: "inherit", warm: 0.035, toBlack: 0, wallBlur: 26, artOpacity: 0.4 },
  projector: { polarity: "inherit", warm: 0.01, toBlack: 0.55, wallBlur: 0, artOpacity: 0 },
  daylight: { polarity: "light", warm: 0, toBlack: 0, wallBlur: 0, artOpacity: 0 },
  arcade: { polarity: "inherit", warm: 0.015, toBlack: 0.25, wallBlur: 0, artOpacity: 0 },
};

/** Warm anchor and paper ink: the two directions Cabinet moves a token. */
const WARM_ANCHOR = "#b8892f";
const PAPER_INK = "#f6eeda";

const SURFACE_KEYS = [
  "bg",
  "bg-deep",
  "surface",
  "surface-2",
  "elevated",
  "line",
  "line-2",
  "stage",
] as const;
const INK_KEYS = ["ink", "ink-2", "ink-3", "stage-ink", "stage-ink-2"] as const;

/**
 * Warm charcoal: the theme's own surfaces nudged towards a warm grey, its inks
 * towards paper. Small on purpose — this is a cast, not a palette — and every
 * pair is re-validated by `tvTokens` after the move.
 */
export function warmTokens(tokens: ThemeTokens, amount: number): ThemeTokens {
  const strength = Math.min(0.12, Math.max(0, amount));
  if (strength === 0) return { ...tokens };
  const next: ThemeTokens = { ...tokens };
  for (const key of SURFACE_KEYS) next[key] = mixHex(tokens[key], WARM_ANCHOR, strength);
  for (const key of INK_KEYS) next[key] = mixHex(tokens[key], PAPER_INK, strength / 2);
  return next;
}

/** Projector steps the backgrounds to black so the panel is the only light. */
export function blackTokens(tokens: ThemeTokens, amount: number): ThemeTokens {
  const strength = Math.min(1, Math.max(0, amount));
  if (strength === 0) return { ...tokens };
  const next: ThemeTokens = { ...tokens };
  for (const key of SURFACE_KEYS) next[key] = mixHex(tokens[key], "#000000", strength);
  return next;
}

/** The six surfaces an accent must survive, in the order the tests report them. */
const ACCENT_BACKGROUNDS: readonly (keyof ThemeTokens)[] = [
  "bg",
  "bg-deep",
  "surface",
  "surface-2",
  "elevated",
  "stage",
];

export interface ConsoleAccentResult {
  /** The colour the console paints as its signal, safe on every surface. */
  color: string;
  /** Worst pair after the lift; the settings row reports it. */
  ratio: number;
  /** True when the input could not keep enough colour and signal was kept. */
  fellBack: boolean;
}

/**
 * The console's 7:1 accent contract. An artwork colour is clamped upstream
 * (chroma ≤ 0.16) and then lifted towards white or black until it clears 7:1 on
 * every Cabinet surface. When the lift keeps less than 4 % chroma the colour has
 * stopped being a colour, and the theme's own signal is the honest answer.
 */
export function consoleAccent(
  candidate: string | null,
  tokens: ThemeTokens,
): ConsoleAccentResult {
  const signal = tokens.signal;
  const signalRatio = worstRatio(signal, tokens);
  if (!candidate) return { color: signal, ratio: signalRatio, fellBack: false };
  const lifted = ensureContrast(
    candidate,
    ACCENT_BACKGROUNDS.map((key) => tokens[key]),
    7,
  );
  const ratio = worstRatio(lifted, tokens);
  if (ratio < 7 || chromaOf(lifted) < 0.04) {
    return { color: signal, ratio: signalRatio, fellBack: true };
  }
  return { color: lifted, ratio, fellBack: false };
}

function worstRatio(color: string, tokens: ThemeTokens): number {
  return roundRatio(
    Math.min(...ACCENT_BACKGROUNDS.map((key) => contrastRatio(color, tokens[key]))),
  );
}

/** Rough sRGB chroma (max − min channel, 0–1); enough to reject a grey lift. */
function chromaOf(hex: string): number {
  const value = hex.replace("#", "");
  const [r, g, b] = [0, 2, 4].map((offset) => Number.parseInt(value.slice(offset, offset + 2), 16) / 255);
  return Math.max(r ?? 0, g ?? 0, b ?? 0) - Math.min(r ?? 0, g ?? 0, b ?? 0);
}

/** Signal tokens from an accent that already cleared {@link consoleAccent}. */
function withSignal(base: ThemeTokens, color: string): ThemeTokens {
  const onSignal = bestForeground(color, [base["bg-deep"], base.ink, "#14120a", "#ffffff", "#000000"]);
  return {
    ...base,
    signal: color,
    "on-signal": onSignal,
    "signal-text": color,
    "signal-line": mixHex(color, base["line-2"], 0.55),
    focus: color,
  };
}

export interface CabinetInput {
  settings: ConsoleRoomSettings;
  /** The rendered app tokens (TV-safe while the console is mounted). */
  tokens: ThemeTokens;
  /** The app's light map, for the Daylight room. */
  lightTokens: ThemeTokens;
  /** The app's dark map: the three dark rooms keep a dark room when the app is light. */
  darkTokens: ThemeTokens;
  /** A validation-ready artwork colour, or null when none could be read. */
  artworkAccent?: string | null;
  /** App polarity, for the rooms that inherit it. */
  mode: ThemeMode;
}

/**
 * The TV-safe pass, widened to the console's own chrome. `tvTokens` validates
 * text against `bg` and the raised surfaces; the console also paints on
 * `bg-deep` (bar, band, full-screen surfaces), which is the darkest step in a
 * light theme and therefore the pair that would have failed at 6.37:1.
 */
function cabinetSafe(base: ThemeTokens): ThemeTokens {
  const tokens = tvTokens(base);
  const backgrounds = [
    tokens["bg-deep"],
    tokens.bg,
    tokens.surface,
    tokens["surface-2"],
    tokens.elevated,
  ];
  for (const foreground of ["ink", "ink-2", "ink-3", "signal-text", "danger", "ok", "warn"] as const) {
    tokens[foreground] = ensureContrast(tokens[foreground], backgrounds, 7);
  }
  tokens["stage-ink"] = ensureContrast(tokens["stage-ink"], [tokens.stage], 7);
  tokens["stage-ink-2"] = ensureContrast(tokens["stage-ink-2"], [tokens.stage], 6.2);
  return tokens;
}

/**
 * The console's full token map for one room. Order matters: polarity, then the
 * room's cast, then the accent, then the TV-safe pass that guarantees 7:1 on
 * every pair the console paints.
 */
export function cabinetTokens({
  settings,
  tokens,
  lightTokens,
  darkTokens,
  artworkAccent = null,
  mode,
}: CabinetInput): ThemeTokens {
  const design = CONSOLE_ROOM_DESIGN[settings.room];
  // Night, Projector and Arcade are dark rooms even when the desktop is light:
  // the Console is a device, and a paper-white TV at night is the worst default.
  const dark = mode === "dark" ? tokens : darkTokens;
  const base = design.polarity === "light" ? lightTokens : dark;
  const cast = blackTokens(warmTokens(base, design.warm), design.toBlack);
  const accent =
    settings.accent === "artwork" && artworkAccent
      ? consoleAccent(artworkAccent, cast).color
      : null;
  return cabinetSafe(accent ? withSignal(cast, accent) : cast);
}

/* ------------------------------- persistence ------------------------------ */

export const CONSOLE_ROOM_KEY = "ludex.console.room";
/** v3: rooms replaced presets, and the eight knobs replaced four. */
export const CONSOLE_ROOM_VERSION = 3;
/* v2 was `console-appearance` (preset, curated accent, background, radius); v1
   was the same record without the version wrapper. Both are read below. */

export interface StoredConsoleRoom {
  v: typeof CONSOLE_ROOM_VERSION;
  room: ConsoleRoomSettings;
}

function isOneOf<T extends string>(values: readonly T[], value: unknown): value is T {
  return typeof value === "string" && (values as readonly string[]).includes(value);
}

/** Unknown values (old record, hand-edited storage, `null`) fall back per field. */
export function sanitizeConsoleRoom(value: unknown): ConsoleRoomSettings {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { ...DEFAULT_CONSOLE_ROOM };
  }
  const record = value as Record<string, unknown>;
  return {
    room: isOneOf(CONSOLE_ROOMS, record.room) ? record.room : DEFAULT_CONSOLE_ROOM.room,
    accent: isOneOf(CONSOLE_ACCENT_SOURCES, record.accent)
      ? record.accent
      : DEFAULT_CONSOLE_ROOM.accent,
    wall: isOneOf(CONSOLE_WALLS, record.wall) ? record.wall : DEFAULT_CONSOLE_ROOM.wall,
    corners: isOneOf(CONSOLE_CORNERS, record.corners)
      ? record.corners
      : DEFAULT_CONSOLE_ROOM.corners,
    density: isOneOf(CONSOLE_DENSITIES, record.density)
      ? record.density
      : DEFAULT_CONSOLE_ROOM.density,
    motion: isOneOf(CONSOLE_MOTIONS, record.motion) ? record.motion : DEFAULT_CONSOLE_ROOM.motion,
    sound: isOneOf(CONSOLE_SOUNDS, record.sound) ? record.sound : DEFAULT_CONSOLE_ROOM.sound,
    haptics: isOneOf(CONSOLE_HAPTICS, record.haptics)
      ? record.haptics
      : DEFAULT_CONSOLE_ROOM.haptics,
  };
}

/** The eight knobs, in Settings order. `room` first because it resets them. */
export const CONSOLE_KNOBS = [
  "room",
  "accent",
  "wall",
  "corners",
  "density",
  "motion",
  "sound",
  "haptics",
] as const;

export type ConsoleKnob = (typeof CONSOLE_KNOBS)[number];

/** A v2 appearance record, the shape `console-appearance.ts` used to write. */
export interface LegacyConsoleAppearance {
  preset?: unknown;
  accent?: unknown;
  background?: unknown;
  radius?: unknown;
}

const LEGACY_PASTEL_ACCENTS = new Set(["coral", "mint", "sky", "lilac", "sun"]);

/**
 * v2 → v3. Studio becomes Night with crisp corners, Soft becomes Night with the
 * soft corner scale, and the five pastel accents move to Theme: they were the
 * strongest "generated" signal and the Console no longer has a colour picker.
 * An explicit radius/background choice survives.
 */
export function migrateConsoleAppearance(appearance: LegacyConsoleAppearance): ConsoleRoomSettings {
  const preset = appearance.preset === "studio" ? "studio" : "soft";
  const background =
    appearance.background === "solid" || appearance.background === "gradient"
      ? appearance.background
      : "artwork";
  const radius =
    appearance.radius === "round" || appearance.radius === "sharp" ? appearance.radius : "soft";
  return {
    ...DEFAULT_CONSOLE_ROOM,
    room: "night",
    corners: radius === "round" ? "round" : preset === "studio" ? "sharp" : "soft",
    wall: background,
    accent:
      typeof appearance.accent === "string" && !LEGACY_PASTEL_ACCENTS.has(appearance.accent)
        ? isOneOf(CONSOLE_ACCENT_SOURCES, appearance.accent)
          ? appearance.accent
          : "theme"
        : "theme",
  };
}

/** Reads a stored record through the version ladder (v1, v2 appearance, v3). */
export function readConsoleRoom(
  storage: StorageLike | null | undefined,
  key: string = CONSOLE_ROOM_KEY,
): ConsoleRoomSettings {
  if (!storage) return { ...DEFAULT_CONSOLE_ROOM };
  try {
    const raw = storage.getItem(key);
    if (!raw) return { ...DEFAULT_CONSOLE_ROOM };
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return { ...DEFAULT_CONSOLE_ROOM };
    const record = parsed as Record<string, unknown>;
    if (typeof record.v === "number" && record.v >= CONSOLE_ROOM_VERSION) {
      return sanitizeConsoleRoom(record.room);
    }
    // A v2 record wrapped `appearance`; a bare object is either shape.
    if (record.appearance && typeof record.appearance === "object") {
      return migrateConsoleAppearance(record.appearance as LegacyConsoleAppearance);
    }
    if (record.room && typeof record.room === "object") {
      return sanitizeConsoleRoom(record.room);
    }
    if ("preset" in record || "background" in record || "radius" in record) {
      return migrateConsoleAppearance(record as LegacyConsoleAppearance);
    }
    return { ...DEFAULT_CONSOLE_ROOM };
  } catch {
    return { ...DEFAULT_CONSOLE_ROOM };
  }
}

export function writeConsoleRoom(
  storage: StorageLike | null | undefined,
  settings: ConsoleRoomSettings,
  key: string = CONSOLE_ROOM_KEY,
): ConsoleRoomSettings {
  const next = sanitizeConsoleRoom(settings);
  try {
    const record: StoredConsoleRoom = { v: CONSOLE_ROOM_VERSION, room: next };
    storage?.setItem(key, JSON.stringify(record));
    notifyPreferencesChanged();
  } catch {
    /* the choice applies for this session when storage is unavailable */
  }
  return next;
}

/* ------------------------------- knob helpers ----------------------------- */

/** Cycles a choice row; wraps so a held direction is never a dead end. */
export function cycleChoice<T>(values: readonly T[], current: T, delta: -1 | 1): T {
  if (values.length === 0) return current;
  const at = values.indexOf(current);
  const from = at === -1 ? 0 : at;
  return values[(from + delta + values.length) % values.length] as T;
}

export const KNOB_VALUES = {
  room: CONSOLE_ROOMS,
  accent: CONSOLE_ACCENT_SOURCES,
  wall: CONSOLE_WALLS,
  corners: CONSOLE_CORNERS,
  density: CONSOLE_DENSITIES,
  motion: CONSOLE_MOTIONS,
  sound: CONSOLE_SOUNDS,
  haptics: CONSOLE_HAPTICS,
} as const;

/** The value a reset restores: the room's own answer to this knob. */
export function resetKnob(settings: ConsoleRoomSettings, knob: ConsoleKnob): ConsoleRoomSettings {
  switch (knob) {
    case "room":
      return { ...DEFAULT_CONSOLE_ROOM };
    case "accent":
      return { ...settings, accent: DEFAULT_CONSOLE_ROOM.accent };
    default:
      return { ...settings, [knob]: CONSOLE_ROOM_BUNDLES[settings.room][knob] };
  }
}

/** Reset section: the current room's bundle, accent included. */
export function resetConsoleRoom(settings: ConsoleRoomSettings): ConsoleRoomSettings {
  return { ...DEFAULT_CONSOLE_ROOM, room: settings.room, ...CONSOLE_ROOM_BUNDLES[settings.room] };
}

/**
 * Switching rooms restores the room's bundle for the six material knobs and
 * keeps the two player-owned preferences that are not about materials: the
 * accent source and motion. A player who picked "from artwork" in Night keeps
 * it when the room changes; the room still decides the wall, corners, density,
 * sound and haptics.
 */
export function withConsoleRoom(
  settings: ConsoleRoomSettings,
  room: ConsoleRoom,
): ConsoleRoomSettings {
  return {
    ...settings,
    room,
    ...CONSOLE_ROOM_BUNDLES[room],
    accent: settings.accent,
    motion: settings.motion,
  };
}

/** Attributes the console root and the stylesheet read. */
export function consoleRoomAttributes(settings: ConsoleRoomSettings): Record<string, string> {
  return {
    "data-console-room": settings.room,
    "data-console-wall": settings.wall,
    "data-console-corners": settings.corners,
    "data-console-density": settings.density,
    "data-console-motion": settings.motion,
  };
}

/** Motion, sound and haptics knobs mapped onto the engines that own them. */
export function motionProfile(settings: ConsoleRoomSettings, prefersReduced: boolean | null) {
  const reduced = prefersReduced === true || settings.motion !== "full";
  return { reduced, instant: settings.motion === "off" };
}

export function soundEnabled(settings: ConsoleRoomSettings): boolean {
  return settings.sound !== "off";
}

export function soundReduced(settings: ConsoleRoomSettings): boolean {
  return settings.sound === "reduced";
}
