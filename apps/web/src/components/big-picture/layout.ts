/* --------------------------------------------------------------------------
   Big Picture layouts (#141, #197).

   Six ways to read the same shelf at ten feet: the shelf Big Picture always
   had, a Dock that puts the system row under one large rail, a Spotlight that
   lets the selected game take over the screen, a PlayStation-style hero, a
   Steam-style cover grid and a dense list. Everything that can be decided
   without the DOM lives here — the registry, the persisted choice, the grid
   geometry and the direction-to-step mapping — so every view shares one
   navigation model instead of six copies of it.
   -------------------------------------------------------------------------- */

import { atLibraryTop, zoneAfterMove } from "./system-zones";

export type BigPictureLayout = "shelf" | "dock" | "spotlight" | "hero" | "grid" | "compact";

/** Registry order is the order of the settings cards and of the switcher. */
export const BIG_PICTURE_LAYOUTS: readonly BigPictureLayout[] = [
  "shelf",
  "dock",
  "spotlight",
  "hero",
  "grid",
  "compact",
];

/** The shelf is the layout Big Picture has always had, so it stays the default. */
export const DEFAULT_BIG_PICTURE_LAYOUT: BigPictureLayout = "shelf";

/**
 * Browser-local preference, scoped per server by `scopedStorageKey` like the
 * library view. It is a device choice (a TV wants a different layout than a
 * laptop), so it never travels to the account.
 */
export const BIG_PICTURE_LAYOUT_KEY = "ludex.bigPicture.layout";

export function isBigPictureLayout(value: unknown): value is BigPictureLayout {
  return typeof value === "string" && (BIG_PICTURE_LAYOUTS as readonly string[]).includes(value);
}

/** Anything unknown (old value, hand-edited storage, `null`) falls back to shelf. */
export function sanitizeLayout(value: unknown): BigPictureLayout {
  return isBigPictureLayout(value) ? value : DEFAULT_BIG_PICTURE_LAYOUT;
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
  /** The game row has run out: the ring belongs to the shell's system surfaces. */
  | { type: "zone"; target: "home" | "actions" }
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
 * 4K gets eight covers across, 1080p seven, a small laptop five: the grid is
 * the layout for "show me a lot at once", and every step down keeps tiles
 * wide enough to read a title at three metres.
 */
const GRID_COLUMNS: ReadonlyArray<readonly [number, number]> = [
  [2560, 8],
  [1680, 7],
  [1360, 6],
  [1024, 5],
  [760, 4],
  [520, 3],
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
 * The one direction model every layout obeys. Moving left/right on a rail
 * walks the rail; in the grid, up/down walk the rows; in the list, up/down walk
 * the rows and left/right change shelf, because the tabs sit along the top.
 * Shoulders change shelf everywhere, triggers jump a page (#138).
 *
 * Vertical travel that runs out of games is a zone step (#196, #197): the
 * layout decides whether that means the system row above (shelf, hero, grid,
 * compact), the dock underneath (dock) or the quick actions (spotlight).
 * `zoneAfterMove` owns the graph; this function only knows when the game row
 * can no longer move.
 */
export function stepForLayout({ layout, direction, index, total, columns }: StepInput): LayoutStep {
  if (total <= 0) return { type: "none" };
  const horizontal = direction === "left" || direction === "right";
  const delta: -1 | 1 = direction === "left" || direction === "up" ? -1 : 1;
  const atTop = atLibraryTop({ layout, index, columns });
  const zoneStep = (): LayoutStep => {
    const target = zoneAfterMove("library", direction, { layout, atLibraryTop: atTop });
    return target === "library" ? { type: "none" } : { type: "zone", target };
  };

  switch (layout) {
    case "shelf":
    case "hero":
    case "dock":
    case "spotlight":
      if (horizontal) return { type: "move", index: stepIndex(index, delta, total) };
      return zoneStep();
    case "grid":
      if (horizontal) return { type: "move", index: stepIndex(index, delta, total) };
      if (delta < 0 && atTop) return { type: "zone", target: "home" };
      return { type: "move", index: gridStepIndex(index, delta, columns, total) };
    case "compact":
      if (horizontal) return { type: "shelf", delta };
      if (delta < 0 && atTop) return { type: "zone", target: "home" };
      return { type: "move", index: stepIndex(index, delta, total) };
  }
}

/* -------------------------------- page jumps ------------------------------- */

/** A rail page skips five covers; the dense list skips ten rows. */
export const RAIL_PAGE_STEP = 5;
export const LIST_PAGE_STEP = 10;

/**
 * LT/RT (and PageUp/PageDown) jump a page instead of one step (#138). Every
 * rail — shelf, hero, dock, spotlight — moves along the shelf by
 * {@link RAIL_PAGE_STEP} covers, the list by {@link LIST_PAGE_STEP} rows, and
 * the grid by two full rows — one screenful at ten feet. Every jump clamps, so
 * the ring never wraps behind the player's back.
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
    case "hero":
    case "dock":
    case "spotlight":
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
