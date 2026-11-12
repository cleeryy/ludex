import { describe, expect, it } from "vitest";
import type { GameSummary } from "@/lib/types";
import {
  SEARCH_KEY_ROWS,
  SEARCH_RESULT_LIMIT,
  applySearchKey,
  clampSearchIndex,
  filterGames,
  findGameShelf,
  flattenSearchRows,
  moveSearchTarget,
  normalizeSearch,
  searchKeyLabel,
  searchRows,
} from "./search";

/* --------------------------------------------------------------------------
   #138: couch search. Matching, the on-screen keyboard model and the shared
   focus grid are pure, so the overlay can be trusted without rendering it.
   -------------------------------------------------------------------------- */

function game(overrides: Partial<GameSummary> & { id: number; title: string }): GameSummary {
  return {
    id: overrides.id,
    title: overrides.title,
    sortTitle: overrides.title,
    releaseDate: overrides.releaseDate ?? "2020-01-01",
    coverUrl: null,
    backgroundUrl: null,
    rating: null,
    genres: overrides.genres ?? [],
    earlyAccess: false,
    versionCount: 1,
    totalSize: 0,
    state: overrides.state ?? "unplayed",
    minutesPlayed: 0,
    bookmarked: false,
    lastPlayedAt: overrides.lastPlayedAt ?? null,
    maxPlayers: null,
  };
}

const LIBRARY: GameSummary[] = [
  game({ id: 1, title: "Hades", genres: ["Roguelike"], lastPlayedAt: "2026-11-01T12:00:00Z" }),
  game({ id: 2, title: "Hollow Knight", genres: ["Metroidvania"] }),
  game({ id: 3, title: "Pokémon Écarlate", genres: ["RPG"], lastPlayedAt: "2026-11-10T12:00:00Z" }),
  game({ id: 4, title: "The Legend of Zelda", genres: ["Adventure"] }),
  game({ id: 5, title: "Celeste", genres: ["Platformer"] }),
  game({ id: 6, title: "Hades II", genres: ["Roguelike"] }),
  game({ id: 7, title: "Stardew Valley", genres: ["Simulation"] }),
  game({ id: 8, title: "Hollow Knight: Silksong", genres: ["Metroidvania"] }),
];

describe("search matching", () => {
  it("normalises case and accents", () => {
    expect(normalizeSearch("  Pokémon Écarlate ")).toBe("pokemon ecarlate");
    expect(normalizeSearch("HADES")).toBe("hades");
  });

  it("ranks exact and prefix matches first", () => {
    const results = filterGames(LIBRARY, "hades");
    expect(results.map((entry) => entry.id)).toEqual([1, 6]);
  });

  it("matches accents both ways and ignores case", () => {
    expect(filterGames(LIBRARY, "pokemon")[0].id).toBe(3);
    expect(filterGames(LIBRARY, "ÉCARLATE")[0].id).toBe(3);
  });

  it("matches genres and years", () => {
    expect(filterGames(LIBRARY, "metroidvania").map((entry) => entry.id)).toEqual([2, 8]);
    expect(filterGames(LIBRARY, "2020").length).toBe(SEARCH_RESULT_LIMIT);
  });

  it("suggests recently played games for an empty query", () => {
    expect(filterGames(LIBRARY, "").map((entry) => entry.id)).toEqual([3, 1, 2, 4, 5, 6]);
    expect(filterGames(LIBRARY, "   ")).toHaveLength(SEARCH_RESULT_LIMIT);
  });

  it("honours the result limit and reports no matches", () => {
    expect(filterGames(LIBRARY, "", 2)).toHaveLength(2);
    expect(filterGames(LIBRARY, "zzzz")).toEqual([]);
    expect(filterGames([], "hades")).toEqual([]);
  });

  it("keeps library order on equal scores", () => {
    const results = filterGames(LIBRARY, "hollow knight");
    expect(results.map((entry) => entry.id)).toEqual([2, 8]);
  });
});

describe("on-screen keyboard", () => {
  it("appends, spaces, deletes and clears", () => {
    expect(applySearchKey("had", "e")).toBe("hade");
    expect(applySearchKey("hades", "space")).toBe("hades ");
    expect(applySearchKey("hades", "backspace")).toBe("hade");
    expect(applySearchKey("hades", "clear")).toBe("");
    expect(applySearchKey("", "backspace")).toBe("");
  });

  it("caps the query length", () => {
    expect(applySearchKey("x".repeat(60), "y")).toHaveLength(40);
  });

  it("labels letters, symbols and the two translated words", () => {
    const words = (key: "space" | "clear") => (key === "space" ? "Espace" : "Effacer");
    expect(searchKeyLabel("a", words)).toBe("A");
    expect(searchKeyLabel("backspace", words)).toBe("⌫");
    expect(searchKeyLabel("space", words)).toBe("Espace");
    expect(searchKeyLabel("clear", words)).toBe("Effacer");
  });

  it("builds five keyboard rows plus one row per result", () => {
    const rows = searchRows(LIBRARY.slice(0, 3));
    expect(rows).toHaveLength(SEARCH_KEY_ROWS.length + 3);
    expect(rows[0]).toHaveLength(10);
    expect(rows[rows.length - 1]).toHaveLength(1);
    const flat = flattenSearchRows(rows);
    expect(flat.filter((cell) => cell.kind === "key")).toHaveLength(
      SEARCH_KEY_ROWS.reduce((total, row) => total + row.length, 0),
    );
    expect(flat.filter((cell) => cell.kind === "result")).toHaveLength(3);
  });

  it("walks letters, then results, with the d-pad", () => {
    const rows = searchRows(LIBRARY.slice(0, 2));
    const keyboardCells = SEARCH_KEY_ROWS.reduce((total, row) => total + row.length, 0);
    // Down from the first key lands on the first key of the second row.
    expect(moveSearchTarget(rows, 0, "down")).toBe(10);
    // Down from the last keyboard row enters the result list.
    expect(moveSearchTarget(rows, keyboardCells - 1, "down")).toBe(keyboardCells);
    // Up from the first result returns to the last keyboard row.
    expect(moveSearchTarget(rows, keyboardCells, "up")).toBe(keyboardCells - 3);
    // Right at the end of a row clamps.
    expect(moveSearchTarget(rows, 0, "left")).toBe(0);
    expect(moveSearchTarget(rows, keyboardCells, "right")).toBe(keyboardCells);
  });

  it("clamps a highlight after the result list shrinks", () => {
    expect(clampSearchIndex(12, 5)).toBe(4);
    expect(clampSearchIndex(-1, 5)).toBe(0);
    expect(clampSearchIndex(3, 0)).toBe(0);
  });
});

describe("locating a result in the shelves", () => {
  const shelves = [
    { id: "recent", games: [{ id: 3 }, { id: 1 }] },
    { id: "all", games: [{ id: 1 }, { id: 2 }, { id: 3 }] },
  ];

  it("prefers the complete shelf", () => {
    expect(findGameShelf(shelves, 3)).toEqual({ shelf: 1, index: 2 });
  });

  it("falls back to the first shelf that holds the game", () => {
    expect(findGameShelf([shelves[0]], 3)).toEqual({ shelf: 0, index: 0 });
  });

  it("returns null when the game is gone", () => {
    expect(findGameShelf(shelves, 99)).toBeNull();
    expect(findGameShelf([], 1)).toBeNull();
  });
});
