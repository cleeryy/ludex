import { clampIndex, type MoveDirection } from "./layout";
import { moveTargetIndex, type TargetRect } from "@/lib/gamepad-core";
import type { GameSummary } from "@/lib/types";

/* --------------------------------------------------------------------------
   Couch search (#138).

   A player on a sofa has no keyboard, so the search overlay is a gamepad-first
   on-screen keyboard: letters are cells, the result list is part of the same
   focus grid, and A presses the highlighted cell. Physical typing keeps
   working because the overlay owns a real input; these helpers only decide
   what a query matches and where the day-pad ring goes, which makes both
   testable without a DOM.
   -------------------------------------------------------------------------- */

/** Long enough that a fast typist is not filtered between letters. */
export const SEARCH_DEBOUNCE_MS = 140;
export const SEARCH_RESULT_LIMIT = 6;
export const SEARCH_QUERY_MAX = 40;

/** QWERTY rows for the on-screen keyboard; ids, not labels. */
export const SEARCH_KEY_ROWS: readonly (readonly string[])[] = [
  ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
  ["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"],
  ["a", "s", "d", "f", "g", "h", "j", "k", "l", "-"],
  ["z", "x", "c", "v", "b", "n", "m", "'"],
  ["backspace", "space", "clear"],
];

export type SearchCell =
  | { kind: "key"; key: string; label: string }
  | { kind: "result"; game: GameSummary };

/** Human label for a keyboard cell (`⌫` is a symbol, the rest are words). */
export function searchKeyLabel(key: string, translate: (key: "space" | "clear") => string): string {
  switch (key) {
    case "backspace":
      return "⌫";
    case "space":
      return translate("space");
    case "clear":
      return translate("clear");
    default:
      return key.toUpperCase();
  }
}

/**
 * The focus grid: five keyboard rows, then one row per result. Results are
 * single-cell rows so the whole list is reachable with down/up, exactly like
 * the library grid.
 */
export function searchRows(results: readonly GameSummary[]): SearchCell[][] {
  const rows: SearchCell[][] = SEARCH_KEY_ROWS.map((row) =>
    row.map((key) => ({ kind: "key" as const, key, label: key })),
  );
  for (const game of results) rows.push([{ kind: "result", game }]);
  return rows;
}

export function flattenSearchRows(rows: readonly SearchCell[][]): SearchCell[] {
  return rows.flat();
}

function rectsFor(rows: readonly SearchCell[][]): TargetRect[] {
  const rects: TargetRect[] = [];
  rows.forEach((row, rowIndex) => {
    row.forEach((_cell, column) => {
      rects.push({ top: rowIndex * 100, left: column * 100, width: 90, height: 90 });
    });
  });
  return rects;
}

/** One d-pad/stick move over the keyboard and the result list. */
export function moveSearchTarget(
  rows: readonly SearchCell[][],
  index: number,
  direction: MoveDirection,
): number {
  return moveTargetIndex(rectsFor(rows), index, direction);
}

/** Applies one on-screen key to the query; `submit` is handled by the screen. */
export function applySearchKey(query: string, key: string): string {
  switch (key) {
    case "backspace":
      return query.slice(0, -1);
    case "clear":
      return "";
    case "space":
      return `${query} `.slice(0, SEARCH_QUERY_MAX);
    default:
      return `${query}${key}`.slice(0, SEARCH_QUERY_MAX);
  }
}

/** Accent- and case-insensitive comparison key. */
export function normalizeSearch(value: string): string {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .trim();
}

function matchScore(game: GameSummary, query: string): number | null {
  const title = normalizeSearch(game.title);
  if (!title) return null;
  if (title === query) return 0;
  if (title.startsWith(query)) return 1;
  if (title.split(/[\s:'-]+/).some((word) => word.startsWith(query))) return 2;
  if (title.includes(query)) return 3;
  if (game.genres.some((genre) => normalizeSearch(genre).includes(query))) return 4;
  const year = game.releaseDate?.slice(0, 4);
  if (year && year.startsWith(query)) return 5;
  return null;
}

/**
 * Filtered results, best match first. An empty query suggests the recently
 * played titles (then the rest) so the list is useful before the first letter;
 * ties keep the library order, which is stable in JS.
 */
export function filterGames(
  games: readonly GameSummary[],
  query: string,
  limit: number = SEARCH_RESULT_LIMIT,
): GameSummary[] {
  const normalized = normalizeSearch(query);
  if (!normalized) {
    const recent = [...games].sort((left, right) => {
      const leftAt = left.lastPlayedAt ? Date.parse(left.lastPlayedAt) : 0;
      const rightAt = right.lastPlayedAt ? Date.parse(right.lastPlayedAt) : 0;
      return rightAt - leftAt;
    });
    return recent.slice(0, limit);
  }

  const scored: Array<{ game: GameSummary; score: number; order: number }> = [];
  games.forEach((game, order) => {
    const score = matchScore(game, normalized);
    if (score !== null) scored.push({ game, score, order });
  });
  scored.sort((left, right) => left.score - right.score || left.order - right.order);
  return scored.slice(0, limit).map((entry) => entry.game);
}

/**
 * Where a game lives in the shelf model, so a search result can put the ring
 * on it. The "all" shelf is preferred: it is the stable, complete list.
 */
export function findGameShelf(
  shelves: ReadonlyArray<{ id: string; games: ReadonlyArray<{ id: number }> }>,
  gameId: number,
): { shelf: number; index: number } | null {
  const all = shelves.findIndex((shelf) => shelf.id === "all" && shelf.games.some((game) => game.id === gameId));
  if (all !== -1) return { shelf: all, index: shelves[all].games.findIndex((game) => game.id === gameId) };
  for (let shelf = 0; shelf < shelves.length; shelf += 1) {
    const index = shelves[shelf].games.findIndex((game) => game.id === gameId);
    if (index !== -1) return { shelf, index };
  }
  return null;
}

/** Keeps the highlight inside the grid after the result list shrank. */
export function clampSearchIndex(index: number, count: number): number {
  return clampIndex(index, count);
}
