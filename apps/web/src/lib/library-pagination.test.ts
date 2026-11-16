// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  LIBRARY_PAGE_SIZE,
  flattenLibraryPages,
  nextLibraryPage,
} from "@/lib/hooks";
import type { GameList, GameSummary } from "@/lib/types";

function game(id: number): GameSummary {
  return { id, title: `Game ${id}` } as GameSummary;
}

function page(number: number, ids: number[], total: number, limit = 200): GameList {
  return { items: ids.map(game), total, page: number, limit };
}

describe("paginated library (#267)", () => {
  it("asks for the next page until the server total is covered", () => {
    expect(nextLibraryPage(undefined, 0)).toBe(1);
    expect(nextLibraryPage(page(1, [1, 2], 401), 1)).toBe(2);
    expect(nextLibraryPage(page(2, [3, 4], 401), 2)).toBe(3);
    // 3 pages × 200 ≥ 401: the walk ends on the last page.
    expect(nextLibraryPage(page(3, [5], 401), 3)).toBeUndefined();
  });

  it("stops on an empty page instead of looping", () => {
    expect(nextLibraryPage(page(2, [], 500), 2)).toBeUndefined();
  });

  it("walks a 600-game library in three 200-row pages", () => {
    const ids = Array.from({ length: 600 }, (_, index) => index + 1);
    const pages = [
      page(1, ids.slice(0, 200), 600),
      page(2, ids.slice(200, 400), 600),
      page(3, ids.slice(400), 600),
    ];
    expect(LIBRARY_PAGE_SIZE).toBe(200);
    expect(nextLibraryPage(pages[0], 1)).toBe(2);
    expect(nextLibraryPage(pages[1], 2)).toBe(3);
    expect(nextLibraryPage(pages[2], 3)).toBeUndefined();
    const flat = flattenLibraryPages(pages);
    expect(flat.items).toHaveLength(600);
    expect(flat.total).toBe(600);
    expect(flat.items[599].id).toBe(600);
  });

  it("drops a duplicate that arrived between two fetches, keeping server order", () => {
    const flat = flattenLibraryPages([
      page(1, [1, 2, 3], 5),
      page(2, [3, 4, 5], 5),
    ]);
    expect(flat.items.map((entry) => entry.id)).toEqual([1, 2, 3, 4, 5]);
    // The reported total is never lowered by de-duplication.
    expect(flat.total).toBe(5);
  });

  it("reports no total before the first page", () => {
    expect(flattenLibraryPages([])).toEqual({ items: [], total: 0 });
  });
});
