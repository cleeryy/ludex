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

/**
 * The focus graph around the game grid (#196, extended by #197): the system
 * row, the games themselves, and — in the Spotlight layout — the quick-action
 * row under the title. A layout only exposes the zones it actually draws.
 */
export type FocusZone = "home" | "library" | "actions";

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
  /** The layout whose zone graph applies. */
  layout: BigPictureLayout;
  /** True when Up would otherwise run out of game rows. */
  atLibraryTop: boolean;
}

/**
 * Up from the first game row enters the home row; Down from the home row
 * returns to the game grid. Left/right and every other direction stay in the
 * zone they started in, so the ring never jumps between zones sideways.
 *
 * #197 makes the graph per layout: the Dock puts the system row *under* the
 * games, the Spotlight adds an action row under the title, and a rail with a
 * single row always hands Up to the home row instead of cycling shelves.
 */
export function zoneAfterMove(
  zone: FocusZone,
  direction: MoveDirection,
  { layout, atLibraryTop }: ZoneMoveOptions,
): FocusZone {
  if (zone === "library") {
    switch (layout) {
      case "dock":
        return direction === "down" ? "home" : "library";
      case "spotlight":
        if (direction === "up") return "home";
        if (direction === "down") return "actions";
        return "library";
      case "shelf":
      case "hero":
        return direction === "up" ? "home" : "library";
      case "grid":
      case "compact":
        return direction === "up" && atLibraryTop ? "home" : "library";
    }
  }
  return direction === zoneExit(layout, zone) ? "library" : zone;
}

/** Zones a layout exposes, top to bottom. */
export function zonesForLayout(layout: BigPictureLayout): readonly FocusZone[] {
  switch (layout) {
    case "dock":
      return ["library", "home"];
    case "spotlight":
      return ["home", "library", "actions"];
    case "shelf":
    case "hero":
    case "grid":
    case "compact":
      return ["home", "library"];
  }
}

/**
 * The direction that leaves a secondary zone and lands back on the selected
 * game. The Dock row sits at the bottom, so Up goes back to the games; every
 * other placement is above the row.
 */
export function zoneExit(layout: BigPictureLayout, zone: Exclude<FocusZone, "library">): MoveDirection {
  if (zone === "actions") return "up";
  return layout === "dock" ? "up" : "down";
}

/**
 * After a layout switch, a zone the new layout does not draw cannot keep the
 * ring: it falls back to the game row. A zone that survives keeps its place.
 */
export function restoreZone(layout: BigPictureLayout, zone: FocusZone): FocusZone {
  return zonesForLayout(layout).includes(zone) ? zone : "library";
}

/** Where the shell renders the system row for each layout (#197). */
export type HomeRowPlacement = "row" | "dock" | "overlay";

export function homeRowPlacement(layout: BigPictureLayout): HomeRowPlacement {
  switch (layout) {
    case "dock":
      return "dock";
    case "spotlight":
      return "overlay";
    case "shelf":
    case "hero":
    case "grid":
    case "compact":
      return "row";
  }
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
  columns: number;
}

/**
 * Whether Up at this slot should leave the grid. The rails are a single row, so
 * Up always reaches the home row (shelf, hero and spotlight, whose system row
 * floats over the art); the Dock's row is below the games, so Up is a dead end
 * there; the grid and the list hand Up over on their first row. This keeps
 * every layout usable without changing what Up means inside it.
 */
export function atLibraryTop({ layout, index, columns }: LibraryEdgeInput): boolean {
  switch (layout) {
    case "shelf":
    case "hero":
    case "spotlight":
      return true;
    case "dock":
      return false;
    case "grid":
      return index < Math.max(1, columns);
    case "compact":
      return index === 0;
  }
}
