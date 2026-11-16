/* --------------------------------------------------------------------------
   Console compositions (#141, #197, #268).

   Three ways to read the same shelf at ten feet, down from six: **Stage** (the
   selected game and one captioned rail), **Wall** (a cover grid) and **List**
   (dense rows, the accessible toggle). The ids stay `shelf`, `grid` and
   `compact` because they are the persisted values and the code paths; the
   player only ever reads Stage, Wall and List.

   Everything that can be decided without the DOM lives here — the registry, the
   persisted choice, the grid geometry and the direction-to-step mapping — so
   every view shares one navigation model. The six-way registry asked the player
   to be a designer before they could play; this one is a decision.
   -------------------------------------------------------------------------- */

import { atLibraryBottom, atLibraryTop, zoneAfterMove } from "./system-zones";

export type BigPictureLayout = "shelf" | "grid" | "compact";

/** Registry order is the order of the settings row and the toggle. */
export const BIG_PICTURE_LAYOUTS: readonly BigPictureLayout[] = ["shelf", "grid", "compact"];

/** Stage is the Cabinet home: one object, one rail. */
export const DEFAULT_BIG_PICTURE_LAYOUT: BigPictureLayout = "shelf";

/** Ids the six-layout build wrote; each maps to the composition that replaces it. */
const RETIRED_LAYOUTS: Readonly<Record<string, BigPictureLayout>> = {
  dock: "shelf",
  spotlight: "shelf",
  hero: "shelf",
};

/**
 * Browser-local preference, scoped per server by `scopedStorageKey` like the
 * library view. It is a device choice (a TV wants a different layout than a
 * laptop), so it never travels to the account.
 */
export const BIG_PICTURE_LAYOUT_KEY = "ludex.bigPicture.layout";

export function isBigPictureLayout(value: unknown): value is BigPictureLayout {
  return typeof value === "string" && (BIG_PICTURE_LAYOUTS as readonly string[]).includes(value);
}

/**
 * Anything unknown falls back to Stage. A value from the retired six-layout
 * registry resolves to the composition that replaces it (Dock, Spotlight and
 * Hero all became Stage), so an upgrade never resets a player's choice.
 */
export function sanitizeLayout(value: unknown): BigPictureLayout {
  if (isBigPictureLayout(value)) return value;
  if (typeof value === "string" && value in RETIRED_LAYOUTS) {
    return RETIRED_LAYOUTS[value] ?? DEFAULT_BIG_PICTURE_LAYOUT;
  }
  return DEFAULT_BIG_PICTURE_LAYOUT;
}

/* ------------------------------- persistence ------------------------------- */

export type LayoutStorage = Pick<Storage, "getItem" | "setItem">;

export function readLayout(
  storage: LayoutStorage | null | undefined,
  key: string = BIG_PICTURE_LAYOUT_KEY,
): BigPictureLayout {
  if (!storage) return DEFAULT_BIG_PICTURE_LAYOUT;
  try {
    return sanitizeLayout(storage.getItem(key));
  } catch {
    // Private mode, a blocked origin or a quota error: the session default.
    return DEFAULT_BIG_PICTURE_LAYOUT;
  }
}

/** Stores the sanitized value and returns it, so callers keep one source. */
export function writeLayout(
  storage: LayoutStorage | null | undefined,
  layout: BigPictureLayout,
  key: string = BIG_PICTURE_LAYOUT_KEY,
): BigPictureLayout {
  const next = sanitizeLayout(layout);
  try {
    storage?.setItem(key, next);
  } catch {
    /* the preference is session-only when storage is unavailable */
  }
  return next;
}

/* --------------------------------- geometry -------------------------------- */

export type MoveDirection = "left" | "right" | "up" | "down";

/** What a direction does in the active layout. */
export type LayoutStep =
  | { type: "move"; index: number }
  | { type: "shelf"; delta: -1 | 1 }
  /** The game row has run out: the ring belongs to another zone. */
  | { type: "zone"; target: "actions" | "home" }
  | { type: "none" };

export interface StepInput {
  layout: BigPictureLayout;
  direction: MoveDirection;
  index: number;
  total: number;
  /** Grid columns, from `gridColumns(width)`. Ignored by the other layouts. */
  columns: number;
}

/**
 * Grid columns for a full-screen cover grid. Breakpoints are matched by width
 * in CSS pixels; `layout-grid.tsx` writes the same number into
 * `grid-template-columns`, so the DOM and the keyboard can never disagree.
 *
 * 4K gets eight covers across, 1080p six, a small laptop four: the grid is the
 * composition for "show me a lot at once", and every step down keeps tiles wide
 * enough to read a title at three metres.
 */
const GRID_COLUMNS: ReadonlyArray<readonly [number, number]> = [
  [3000, 8],
  [1680, 6],
  [1360, 5],
  [1024, 4],
  [760, 3],
  [520, 2],
  [0, 2],
];

export function gridColumns(width: number): number {
  const safe = Number.isFinite(width) && width > 0 ? width : 1920;
  for (const [min, columns] of GRID_COLUMNS) {
    if (safe >= min) return columns;
  }
  return 2;
}

export function clampIndex(index: number, total: number): number {
  if (!Number.isFinite(index) || total <= 0) return 0;
  return Math.min(Math.max(Math.trunc(index), 0), total - 1);
}

/** Moves by `delta` and clamps; moving off either end stays put. */
export function stepIndex(index: number, delta: number, total: number): number {
  return clampIndex(clampIndex(index, total) + delta, total);
}

/**
 * Flat index for up/down in the grid. Rows are full except possibly the last
 * one, so up always lands on the same column of the row above and down into a
 * short last row lands on its last card. Up from the first row and down from
 * the last row stay put: the ring never jumps sideways on its own.
 */
export function gridStepIndex(index: number, delta: number, columns: number, total: number): number {
  if (total <= 0) return 0;
  const current = clampIndex(index, total);
  const width = Math.max(1, Math.trunc(columns));
  const row = Math.floor(current / width);
  const lastRow = Math.floor((total - 1) / width);
  if (delta < 0) return row === 0 ? current : current - width;
  if (delta > 0) return row >= lastRow ? current : Math.min(current + width, total - 1);
  return current;
}

/**
 * The one direction model every layout obeys (#268). Home is three zones on one
 * axis: the stage actions on top, the games, then the band at the bottom.
 *
 *   - Stage: Left/Right walk the rail; Up is the stage actions; Down is the band.
 *   - Wall: Left/Right walk the grid, Up/Down walk the rows; leaving the first
 *     row upwards is the actions, leaving the last row downwards is the band.
 *   - List: Up/Down walk the rows, Left/Right change shelf; the same two exits.
 *
 * Shoulders change shelf everywhere, triggers jump a page.
 */
export function stepForLayout({ layout, direction, index, total, columns }: StepInput): LayoutStep {
  if (total <= 0) return { type: "none" };
  const horizontal = direction === "left" || direction === "right";
  const delta: -1 | 1 = direction === "left" || direction === "up" ? -1 : 1;

  // The list changes shelf sideways; every composition walks its games
  // horizontally otherwise.
  if (layout === "compact" && horizontal) return { type: "shelf", delta };
  if (horizontal) return { type: "move", index: stepIndex(index, delta, total) };

  // Vertical travel that runs out of games is a zone step. `zoneAfterMove` owns
  // the graph; this function only knows when the games can no longer move.
  const target = zoneAfterMove("library", direction, {
    layout,
    atLibraryTop: atLibraryTop({ layout, index, columns }),
    atLibraryBottom: atLibraryBottom({ layout, index, columns, total }),
  });
  if (target !== "library") return { type: "zone", target };
  if (layout === "grid") {
    return { type: "move", index: gridStepIndex(index, delta, columns, total) };
  }
  return { type: "move", index: stepIndex(index, delta, total) };
}

/* -------------------------------- page jumps ------------------------------- */

/** A rail page skips five covers; the dense list skips ten rows. */
export const RAIL_PAGE_STEP = 5;
export const LIST_PAGE_STEP = 10;

/**
 * LT/RT (and PageUp/PageDown) jump a page instead of one step (#138). The rail
 * moves by {@link RAIL_PAGE_STEP} covers, the list by {@link LIST_PAGE_STEP}
 * rows, and the wall by two full rows — one screenful at ten feet. Every jump
 * clamps, so the ring never wraps behind the player's back.
 */
export function pageStepForLayout({ layout, direction, index, total, columns }: StepInput): LayoutStep {
  if (total <= 0) return { type: "none" };
  const delta: -1 | 1 = direction === "left" || direction === "up" ? -1 : 1;
  switch (layout) {
    case "grid":
      return {
        type: "move",
        index: clampIndex(index + delta * Math.max(1, Math.trunc(columns)) * 2, total),
      };
    case "compact":
      return { type: "move", index: stepIndex(index, delta * LIST_PAGE_STEP, total) };
    case "shelf":
      return { type: "move", index: stepIndex(index, delta * RAIL_PAGE_STEP, total) };
  }
}

/* -------------------------------- selection -------------------------------- */

/**
 * Where the selected game went after a refresh. Big Picture refetches on open
 * and on focus, so a shelf can reorder or drop an entry under the ring: keep
 * the game if it is still in the list, otherwise clamp the old slot.
 */
export function resolveIndex(
  ids: readonly number[],
  selectedId: number | null,
  current: number,
): number {
  if (ids.length === 0) return 0;
  if (selectedId !== null) {
    const at = ids.indexOf(selectedId);
    if (at !== -1) return at;
  }
  return clampIndex(current, ids.length);
}

/** Clamped shelf slot, used when a refresh removes "Recently played". */
export function resolveShelfIndex(index: number, count: number): number {
  if (count <= 0) return 0;
  return clampIndex(index, count);
}
