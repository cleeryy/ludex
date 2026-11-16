import type { BigPictureLayout, MoveDirection } from "./layout";

/* --------------------------------------------------------------------------
   The console shell's navigation model (#196, #268).

   Console home is one axis with three zones: the **stage actions** on top, the
   games, then the **band** at the bottom. Settings and Power replace the games
   but keep the bar and the hints.

   This module is the pure part: which zone owns a direction, how the five band
   destinations wrap, how the level stack opens and closes, and which settings
   section a shoulder lands on. The screen only applies these decisions, so the
   keyboard and the pad cannot drift apart.
   -------------------------------------------------------------------------- */

/** The focus graph: stage actions, the games, the band. */
export type FocusZone = "actions" | "library" | "home";

/** Console surfaces that replace the games but keep the shell. */
export type SystemAppId = "settings" | "power";

export type ShellLevel =
  | { kind: "library" }
  | { kind: "app"; id: SystemAppId };

export const LIBRARY_LEVEL: ShellLevel = { kind: "library" };

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
  atLibraryTop?: boolean;
  /** True when Down would otherwise run out of game rows. */
  atLibraryBottom?: boolean;
}

/**
 * Up from the first game row reaches the stage actions; Down from the last row
 * reaches the band. Inside the row every direction stays where it started, so
 * the ring never jumps zones sideways. The Stage rail is a single row, so both
 * exits are always available there.
 */
export function zoneAfterMove(
  zone: FocusZone,
  direction: MoveDirection,
  { layout, atLibraryTop, atLibraryBottom = false }: ZoneMoveOptions,
): FocusZone {
  if (zone === "library") {
    if (direction === "up") {
      return layout === "shelf" || atLibraryTop === true ? "actions" : "library";
    }
    if (direction === "down") {
      return layout === "shelf" || atLibraryBottom ? "home" : "library";
    }
    return "library";
  }
  if (zone === "actions") return direction === "down" ? "library" : "actions";
  return direction === "up" ? "library" : "home";
}

/** Every composition draws the same three zones, top to bottom. */
export function zonesForLayout(layout: BigPictureLayout): readonly FocusZone[] {
  void layout;
  return ["actions", "library", "home"];
}

/** The direction that leaves a secondary zone and lands back on the games. */
export function zoneExit(_layout: BigPictureLayout, zone: Exclude<FocusZone, "library">): MoveDirection {
  return zone === "actions" ? "down" : "up";
}

/** After a layout switch the zone graph is identical; the ring keeps its zone. */
export function restoreZone(layout: BigPictureLayout, zone: FocusZone): FocusZone {
  return zonesForLayout(layout).includes(zone) ? zone : "library";
}

/**
 * The band is Search · Library · Downloads · Settings · Power (#268): five
 * destinations, one row, no Collections tile and no "continue" duplicate of the
 * stage's own Play button. Collections is a shelf inside Library.
 */
export const HOME_ITEM_IDS = ["search", "library", "downloads", "settings", "power"] as const;

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
  return SETTINGS_SECTIONS[wrapIndex(index, delta, SETTINGS_SECTIONS.length)] as SettingsSectionId;
}

export function isSystemApp(value: unknown): value is SystemAppId {
  return value === "settings" || value === "power";
}

export function openSystemApp(id: SystemAppId): ShellLevel {
  return { kind: "app", id };
}

/** B always lands on the games, never on another app and never outside Console. */
export function closeSystemApp(): ShellLevel {
  return { kind: "library" };
}

export function sameLevel(a: ShellLevel, b: ShellLevel): boolean {
  if (a.kind !== b.kind) return false;
  return a.kind === "library" || a.id === (b as { id: SystemAppId }).id;
}

/* ------------------------------ URL state -------------------------------- */

/**
 * The shell level is URL state (`?app=settings&section=language`): changing the
 * language remounts the provider tree, and the app has to come back where it
 * was. It also makes a console surface deep-linkable.
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

/** Whether Up at this slot leaves the games. The Stage rail is one row. */
export function atLibraryTop({ layout, index, columns }: LibraryEdgeInput): boolean {
  switch (layout) {
    case "shelf":
      return true;
    case "grid":
      return index < Math.max(1, columns);
    case "compact":
      return index === 0;
  }
}

/** Whether Down at this slot leaves the games for the band. */
export function atLibraryBottom({
  layout,
  index,
  total,
  columns,
}: LibraryEdgeInput & { total: number }): boolean {
  if (total <= 0) return true;
  switch (layout) {
    case "shelf":
      return true;
    case "grid": {
      const width = Math.max(1, columns);
      return Math.floor(index / width) >= Math.floor((total - 1) / width);
    }
    case "compact":
      return index >= total - 1;
  }
}
