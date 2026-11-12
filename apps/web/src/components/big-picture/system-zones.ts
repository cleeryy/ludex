import type { BigPictureLayout, MoveDirection } from "./layout";

/* --------------------------------------------------------------------------
   The console shell's navigation model (#196).

   Big Picture already owns the game grid. A console shell adds surfaces that
   are not games — the home row, Settings and Power — so "what does Up mean"
   can no longer live in the shelf geometry alone. This module is the pure part:
   which zone owns a direction, how a short ring of system tiles wraps, how the
   level stack opens and closes, and which settings section a shoulder lands on.

   The screen only applies these decisions. Keeping them here means the state
   machine is covered without a DOM, and the keyboard and the pad cannot drift
   apart.
   -------------------------------------------------------------------------- */

/** The focus graph above the game grid: one row of system tiles, then the shelf. */
export type FocusZone = "home" | "library";

/** Console surfaces that replace the grid but keep the system bar. */
export type SystemAppId = "settings" | "power";

export type ShellLevel =
  | { kind: "library" }
  | { kind: "app"; id: SystemAppId };

export const LIBRARY_LEVEL: ShellLevel = { kind: "library" };

/** Left/right on a short ring wraps; a dead end at Power would be a trap. */
export function wrapIndex(index: number, delta: number, count: number): number {
  if (!Number.isFinite(index) || count <= 0) return 0;
  const current = Math.trunc(index);
  const step = Number.isFinite(delta) ? Math.trunc(delta) : 0;
  return (((current + step) % count) + count) % count;
}

export interface ZoneMoveOptions {
  /** True when Up would otherwise run out of game rows. */
  atLibraryTop: boolean;
}

/**
 * Up from the first game row enters the home row; Down from the home row
 * returns to the game grid. Left/right and every other direction stay in the
 * zone they started in, so the ring never jumps between zones sideways.
 */
export function zoneAfterMove(
  zone: FocusZone,
  direction: MoveDirection,
  { atLibraryTop }: ZoneMoveOptions,
): FocusZone {
  if (zone === "library" && direction === "up" && atLibraryTop) return "home";
  if (zone === "home" && direction === "down") return "library";
  return zone;
}

/**
 * The home row is ordered left to right. The ids are stable because the screen
 * builds live details per slot; the component owns the labels and icons.
 */
export const HOME_ITEM_IDS = [
  "continue",
  "library",
  "downloads",
  "collections",
  "settings",
  "power",
] as const;

export type HomeItemId = (typeof HOME_ITEM_IDS)[number];

/** The shoulder order of the in-shell Settings app. */
export const SETTINGS_SECTIONS = [
  "language",
  "appearance",
  "controllers",
  "streaming",
] as const;

export type SettingsSectionId = (typeof SETTINGS_SECTIONS)[number];

export function isSettingsSection(value: unknown): value is SettingsSectionId {
  return typeof value === "string" && (SETTINGS_SECTIONS as readonly string[]).includes(value);
}

/** LB/RB move one category and wrap, so the last tab is not a dead end. */
export function stepSettingsSection(section: SettingsSectionId, delta: number): SettingsSectionId {
  const index = SETTINGS_SECTIONS.indexOf(section);
  return SETTINGS_SECTIONS[wrapIndex(index, delta, SETTINGS_SECTIONS.length)];
}

export function isSystemApp(value: unknown): value is SystemAppId {
  return value === "settings" || value === "power";
}

export function openSystemApp(id: SystemAppId): ShellLevel {
  return { kind: "app", id };
}

/** B always lands on the grid, never on another app and never outside Big Picture. */
export function closeSystemApp(): ShellLevel {
  return { kind: "library" };
}

export function sameLevel(a: ShellLevel, b: ShellLevel): boolean {
  if (a.kind !== b.kind) return false;
  return a.kind === "library" || a.id === (b as { id: SystemAppId }).id;
}

/* ------------------------------ URL state -------------------------------- */

/**
 * The shell level is URL state (`?app=settings&section=language`) for one
 * concrete reason: changing the language remounts the provider tree, and the
 * app has to come back where it was instead of dropping the player on the
 * grid. It also makes a console surface deep-linkable.
 */
export function levelFromSearch(search: string): ShellLevel {
  const params = new URLSearchParams(search.startsWith("?") ? search : `?${search}`);
  const app = params.get("app");
  return isSystemApp(app) ? { kind: "app", id: app } : LIBRARY_LEVEL;
}

export function settingsSectionFromSearch(search: string): SettingsSectionId {
  const params = new URLSearchParams(search.startsWith("?") ? search : `?${search}`);
  const section = params.get("section");
  return isSettingsSection(section) ? section : SETTINGS_SECTIONS[0];
}

/* ---------------------------- library geometry --------------------------- */

export interface LibraryEdgeInput {
  layout: BigPictureLayout;
  index: number;
  shelfIndex: number;
  columns: number;
}

/**
 * Whether Up at this slot should leave the grid. The rails switch shelf on
 * Up/Down, so only the first shelf hands the press to the home row; the grid
 * and the list hand it over on their first row. This keeps every layout usable
 * without changing what Up means inside it.
 */
export function atLibraryTop({ layout, index, shelfIndex, columns }: LibraryEdgeInput): boolean {
  if (layout === "shelf" || layout === "hero") return shelfIndex === 0;
  if (layout === "grid") return index < Math.max(1, columns);
  return index === 0;
}
