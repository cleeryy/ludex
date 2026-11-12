import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { focusGamepadTarget } from "@/lib/gamepad";
import type { GameSummary } from "@/lib/types";
import {
  clampIndex,
  pageStepForLayout,
  resolveIndex,
  resolveShelfIndex,
  stepForLayout,
  type BigPictureLayout,
  type MoveDirection,
} from "./layout";

/* --------------------------------------------------------------------------
   One navigation model for every Big Picture layout (#141).

   The screen used to own the arrows, the shelf switch and the card refs, and
   each layout would have needed its own copy. This hook owns the selection
   instead: which shelf, which card, where the ring sits after a refresh and
   what a direction means in the active layout. The keyboard handler, the
   gamepad handler and the four views all call the same actions, so they cannot
   drift apart.

   The registry of "what up means" is pure (`stepForLayout`); this file only
   applies it and keeps the DOM in step (scroll the selected card into view,
   move the gamepad ring to it).
   -------------------------------------------------------------------------- */

export interface BigPictureShelf {
  id: string;
  label: string;
  games: GameSummary[];
}

export interface BigPictureActions {
  left: () => void;
  right: () => void;
  up: () => void;
  down: () => void;
  /** Previous shelf: shoulder, arrow pair or `[`. */
  prevShelf: () => void;
  nextShelf: () => void;
  /** Previous/next page: trigger or PageUp/PageDown (#138). */
  page: (delta: -1 | 1) => void;
}

export interface BigPictureNavigation {
  shelfIndex: number;
  shelf: BigPictureShelf | undefined;
  shelves: readonly BigPictureShelf[];
  games: GameSummary[];
  index: number;
  selected: GameSummary | undefined;
  /** Registers a card in the active layout; positions are per shelf. */
  cardRef: (position: number) => (element: HTMLButtonElement | null) => void;
  setIndex: (index: number) => void;
  switchShelf: (delta: number) => void;
  /** Jumps to a shelf by slot, for the tabs a mouse can click. */
  selectShelf: (index: number) => void;
  /** Selects one card across shelves, for search results (#138). */
  selectGame: (shelfIndex: number, index: number) => void;
  move: (direction: MoveDirection) => void;
  /** Jumps a page in the active layout (trigger / PageUp / PageDown). */
  jump: (delta: -1 | 1) => void;
  /** Puts the gamepad ring back on the selected card (after a layout switch). */
  focusSelected: () => void;
  actions: BigPictureActions;
}

export interface BigPictureNavigationOptions {
  shelves: BigPictureShelf[];
  layout: BigPictureLayout;
  /** `gridColumns(viewport width)`: the grid's row step. */
  columns: number;
  /** Query identity for the game list; a refresh re-resolves the selected slot. */
  data: unknown;
  reduceMotion: boolean | null;
}

export function useBigPictureNavigation({
  shelves,
  layout,
  columns,
  data,
  reduceMotion,
}: BigPictureNavigationOptions): BigPictureNavigation {
  const [rawShelfIndex, setRawShelfIndex] = useState(0);
  const [rawIndex, setRawIndex] = useState(0);

  const shelfIndex = resolveShelfIndex(rawShelfIndex, shelves.length);
  const shelf = shelves[shelfIndex];
  const games = shelf?.games ?? [];
  const index = clampIndex(rawIndex, games.length);
  const selected = games[index];

  const cardRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const selectedIdRef = useRef<number | null>(null);
  const indexRef = useRef(index);
  indexRef.current = index;

  const setIndex = useCallback((next: number) => {
    setRawIndex(Number.isFinite(next) ? Math.trunc(next) : 0);
  }, []);

  const selectShelf = useCallback(
    (next: number) => {
      const clamped = clampIndex(next, shelves.length);
      if (clamped === shelfIndex) return;
      setRawShelfIndex(clamped);
      setRawIndex(0);
    },
    [shelves.length, shelfIndex],
  );

  /**
   * One atomic jump used by search and by the overlay shortcuts: pick the
   * shelf and the card in it without depending on the previous selection. A
   * shelf change used to reset the index to zero, which would have landed the
   * ring on the wrong game when the answer came from a query.
   */
  const selectGame = useCallback(
    (nextShelf: number, nextIndex: number) => {
      const target = clampIndex(nextShelf, shelves.length);
      setRawShelfIndex(target);
      setRawIndex(clampIndex(nextIndex, shelves[target]?.games.length ?? 0));
    },
    [shelves],
  );

  const switchShelf = useCallback(
    (delta: number) => {
      if (shelves.length < 2) return;
      selectShelf(shelfIndex + delta);
    },
    [shelves.length, shelfIndex, selectShelf],
  );

  const move = useCallback(
    (direction: MoveDirection) => {
      const step = stepForLayout({ layout, direction, index, total: games.length, columns });
      if (step.type === "move") setRawIndex(step.index);
      else if (step.type === "shelf") switchShelf(step.delta);
    },
    [layout, index, games.length, columns, switchShelf],
  );

  const jump = useCallback(
    (delta: -1 | 1) => {
      const step = pageStepForLayout({
        layout,
        direction: delta < 0 ? "left" : "right",
        index,
        total: games.length,
        columns,
      });
      if (step.type === "move") setRawIndex(step.index);
    },
    [layout, index, games.length, columns],
  );

  // A refresh can reorder "Recently played" or drop a title. Follow the
  // selected game to its new slot; fall back to the old slot, clamped, only
  // when it is gone. Layout effect so a stale slot never paints first.
  useLayoutEffect(() => {
    setRawIndex((current) => resolveIndex(games.map((game) => game.id), selectedIdRef.current, current));
    // `data` is the identity that matters: a new list, not a new closure.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data]);

  // Runs after the sync above: the ref always holds the game on screen.
  useLayoutEffect(() => {
    selectedIdRef.current = selected?.id ?? null;
  }, [selected?.id]);

  // Keep the selected card visible on a shelf change, a layout switch and a
  // move. `block: nearest` leaves the vertical scroll of the page alone and
  // only scrolls the container that actually clips the card.
  useEffect(() => {
    cardRefs.current[index]?.scrollIntoView({
      behavior: reduceMotion ? "auto" : "smooth",
      inline: "center",
      block: "nearest",
    });
  }, [index, shelfIndex, layout, reduceMotion]);

  const cardRef = useCallback(
    (position: number) => (element: HTMLButtonElement | null) => {
      cardRefs.current[position] = element;
    },
    [],
  );

  const focusSelected = useCallback(() => {
    const element = cardRefs.current[indexRef.current];
    if (element) focusGamepadTarget([element], 0, false, { scroll: false });
  }, []);

  const actions = useMemo<BigPictureActions>(
    () => ({
      left: () => move("left"),
      right: () => move("right"),
      up: () => move("up"),
      down: () => move("down"),
      prevShelf: () => switchShelf(-1),
      nextShelf: () => switchShelf(1),
      page: jump,
    }),
    [move, switchShelf, jump],
  );

  return {
    shelfIndex,
    shelf,
    shelves,
    games,
    index,
    selected,
    cardRef,
    setIndex,
    switchShelf,
    selectShelf,
    selectGame,
    move,
    jump,
    focusSelected,
    actions,
  };
}
