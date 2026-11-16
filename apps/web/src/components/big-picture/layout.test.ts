import { describe, expect, it } from "vitest";
import {
  BIG_PICTURE_LAYOUTS,
  BIG_PICTURE_LAYOUT_KEY,
  DEFAULT_BIG_PICTURE_LAYOUT,
  clampIndex,
  gridColumns,
  gridStepIndex,
  isBigPictureLayout,
  readLayout,
  resolveIndex,
  resolveShelfIndex,
  sanitizeLayout,
  stepForLayout,
  stepIndex,
  writeLayout,
  type LayoutStorage,
} from "./layout";
import { jobPercent } from "./model";
import { LIST_PAGE_STEP, RAIL_PAGE_STEP, pageStepForLayout } from "./layout";

/* --------------------------------------------------------------------------
   #268: the three console compositions (Stage, Wall, List), the persisted
   preference and the direction model they share. Pure functions only, so the
   suite stays fast and the screen can trust the geometry without a DOM.
   -------------------------------------------------------------------------- */

function memoryStorage(entries: Record<string, string> = {}): LayoutStorage & { entries: Map<string, string> } {
  const map = new Map(Object.entries(entries));
  return {
    entries: map,
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => {
      map.set(key, value);
    },
  };
}

describe("console composition registry", () => {
  it("lists Stage, Wall and List, Stage first", () => {
    expect(BIG_PICTURE_LAYOUTS).toEqual(["shelf", "grid", "compact"]);
    expect(DEFAULT_BIG_PICTURE_LAYOUT).toBe("shelf");
    expect(BIG_PICTURE_LAYOUTS).toContain(DEFAULT_BIG_PICTURE_LAYOUT);
  });

  it("recognises the three ids and nothing else", () => {
    for (const layout of BIG_PICTURE_LAYOUTS) expect(isBigPictureLayout(layout)).toBe(true);
    for (const value of ["Stage", "tiles", "", null, undefined, 3, {}]) {
      expect(isBigPictureLayout(value)).toBe(false);
      expect(sanitizeLayout(value)).toBe("shelf");
    }
  });

  it("maps the four retired layouts onto the composition that replaces them", () => {
    // The six-layout build stored one of these; an upgrade must not reset the
    // player's choice, and the replacement is the closest reading of the shelf.
    expect(sanitizeLayout("dock")).toBe("shelf");
    expect(sanitizeLayout("spotlight")).toBe("shelf");
    expect(sanitizeLayout("hero")).toBe("shelf");
    expect(sanitizeLayout("grid")).toBe("grid");
    expect(sanitizeLayout("compact")).toBe("compact");
  });

  it("keeps the registry order stable for the settings row", () => {
    expect(BIG_PICTURE_LAYOUTS.map((layout) => sanitizeLayout(layout))).toEqual([...BIG_PICTURE_LAYOUTS]);
    expect(new Set(BIG_PICTURE_LAYOUTS).size).toBe(BIG_PICTURE_LAYOUTS.length);
  });
});

describe("console composition persistence", () => {
  it("defaults to Stage without storage", () => {
    expect(readLayout(null)).toBe("shelf");
    expect(readLayout(undefined)).toBe("shelf");
  });

  it("reads a stored layout, migrates a retired one and ignores junk", () => {
    expect(readLayout(memoryStorage({ [BIG_PICTURE_LAYOUT_KEY]: "grid" }))).toBe("grid");
    expect(readLayout(memoryStorage({ [BIG_PICTURE_LAYOUT_KEY]: "compact" }))).toBe("compact");
    expect(readLayout(memoryStorage({ [BIG_PICTURE_LAYOUT_KEY]: "spotlight" }))).toBe("shelf");
    expect(readLayout(memoryStorage({ [BIG_PICTURE_LAYOUT_KEY]: "tiles" }))).toBe("shelf");
    expect(readLayout(memoryStorage({ [BIG_PICTURE_LAYOUT_KEY]: "" }))).toBe("shelf");
    expect(readLayout(memoryStorage())).toBe("shelf");
  });

  it("reads a custom key, so the preference can be scoped per server", () => {
    const storage = memoryStorage({ "ludex.server.3.ludex.bigPicture.layout": "compact" });
    expect(readLayout(storage, "ludex.server.3.ludex.bigPicture.layout")).toBe("compact");
    expect(readLayout(storage)).toBe("shelf");
  });

  it("writes the sanitised id and returns it", () => {
    const storage = memoryStorage();
    expect(writeLayout(storage, "compact")).toBe("compact");
    expect(storage.entries.get(BIG_PICTURE_LAYOUT_KEY)).toBe("compact");
    expect(writeLayout(storage, "tiles" as never)).toBe("shelf");
    expect(storage.entries.get(BIG_PICTURE_LAYOUT_KEY)).toBe("shelf");
  });

  it("never throws when storage is blocked or full", () => {
    const blocked: LayoutStorage = {
      getItem: () => {
        throw new Error("SecurityError");
      },
      setItem: () => {
        throw new Error("QuotaExceededError");
      },
    };
    expect(readLayout(blocked)).toBe("shelf");
    expect(writeLayout(blocked, "grid")).toBe("grid");
  });
});

describe("grid geometry", () => {
  it("maps TV and laptop widths to cover columns", () => {
    expect(gridColumns(3840)).toBe(8);
    expect(gridColumns(2560)).toBe(6);
    expect(gridColumns(1920)).toBe(6);
    expect(gridColumns(1680)).toBe(6);
    expect(gridColumns(1366)).toBe(5);
    expect(gridColumns(1024)).toBe(4);
    expect(gridColumns(800)).toBe(3);
    expect(gridColumns(520)).toBe(2);
    expect(gridColumns(390)).toBe(2);
  });

  it("falls back to the 1080p grid for a missing width", () => {
    expect(gridColumns(0)).toBe(6);
    expect(gridColumns(-1)).toBe(6);
    expect(gridColumns(Number.NaN)).toBe(6);
  });

  it("clamps the index into the list", () => {
    expect(clampIndex(0, 5)).toBe(0);
    expect(clampIndex(4, 5)).toBe(4);
    expect(clampIndex(9, 5)).toBe(4);
    expect(clampIndex(-3, 5)).toBe(0);
    expect(clampIndex(2.7, 5)).toBe(2);
    expect(clampIndex(Number.NaN, 5)).toBe(0);
    expect(clampIndex(3, 0)).toBe(0);
  });

  it("steps by one and stays on either end", () => {
    expect(stepIndex(0, -1, 5)).toBe(0);
    expect(stepIndex(3, 1, 5)).toBe(4);
    expect(stepIndex(4, 1, 5)).toBe(4);
    expect(stepIndex(4, -2, 5)).toBe(2);
    expect(stepIndex(1, 1, 0)).toBe(0);
  });

  it("steps a grid by row and keeps the column", () => {
    // 7 columns, 18 games: rows of 7, 7 and 4.
    expect(gridStepIndex(8, -1, 7, 18)).toBe(1);
    expect(gridStepIndex(8, 1, 7, 18)).toBe(15);
    // Up from the first row and down from the last row do not move.
    expect(gridStepIndex(2, -1, 7, 18)).toBe(2);
    expect(gridStepIndex(13, 1, 7, 18)).toBe(17);
    expect(gridStepIndex(17, 1, 7, 18)).toBe(17);
    // Down into a short last row lands on its last card.
    expect(gridStepIndex(6, 1, 7, 10)).toBe(9);
    // A zero column count is treated as a single column.
    expect(gridStepIndex(3, 1, 0, 10)).toBe(4);
  });
});

describe("layout direction model", () => {
  const base = { index: 2, total: 10, columns: 5 };

  it("Stage: sideways walks the rail, Up is the actions, Down is the band", () => {
    expect(stepForLayout({ ...base, layout: "shelf", direction: "left" })).toEqual({ type: "move", index: 1 });
    expect(stepForLayout({ ...base, layout: "shelf", direction: "right" })).toEqual({ type: "move", index: 3 });
    expect(stepForLayout({ ...base, layout: "shelf", direction: "up" })).toEqual({
      type: "zone",
      target: "actions",
    });
    expect(stepForLayout({ ...base, layout: "shelf", direction: "down" })).toEqual({
      type: "zone",
      target: "home",
    });
  });

  it("Wall: rows inside the wall, actions above the first row, band under the last", () => {
    expect(stepForLayout({ ...base, layout: "grid", direction: "left" })).toEqual({ type: "move", index: 1 });
    expect(stepForLayout({ ...base, layout: "grid", direction: "down" })).toEqual({ type: "move", index: 7 });
    expect(stepForLayout({ ...base, index: 7, layout: "grid", direction: "up" })).toEqual({ type: "move", index: 2 });
    expect(stepForLayout({ ...base, index: 3, layout: "grid", direction: "up" })).toEqual({
      type: "zone",
      target: "actions",
    });
    expect(stepForLayout({ ...base, index: 9, layout: "grid", direction: "down" })).toEqual({
      type: "zone",
      target: "home",
    });
  });

  it("List: rows vertically, shelves sideways, the same two exits", () => {
    expect(stepForLayout({ ...base, layout: "compact", direction: "down" })).toEqual({ type: "move", index: 3 });
    expect(stepForLayout({ ...base, layout: "compact", direction: "up" })).toEqual({ type: "move", index: 1 });
    expect(stepForLayout({ ...base, layout: "compact", direction: "left" })).toEqual({ type: "shelf", delta: -1 });
    expect(stepForLayout({ ...base, layout: "compact", direction: "right" })).toEqual({ type: "shelf", delta: 1 });
    expect(stepForLayout({ ...base, index: 0, layout: "compact", direction: "up" })).toEqual({
      type: "zone",
      target: "actions",
    });
    expect(stepForLayout({ ...base, index: 9, layout: "compact", direction: "down" })).toEqual({
      type: "zone",
      target: "home",
    });
  });

  it("does nothing on an empty shelf", () => {
    for (const layout of BIG_PICTURE_LAYOUTS) {
      for (const direction of ["left", "right", "up", "down"] as const) {
        expect(stepForLayout({ layout, direction, index: 0, total: 0, columns: 6 })).toEqual({ type: "none" });
      }
    }
  });

  it("a zone step never changes the selected game", () => {
    const zone = stepForLayout({ ...base, layout: "shelf", direction: "down" });
    expect(zone).toEqual({ type: "zone", target: "home" });
    expect("index" in zone).toBe(false);
  });
});

describe("page jumps", () => {
  const base = { index: 6, total: 30, columns: 7 };

  it("skips a rail page without wrapping", () => {
    expect(pageStepForLayout({ ...base, layout: "shelf", direction: "right" })).toEqual({
      type: "move",
      index: 6 + RAIL_PAGE_STEP,
    });
    expect(pageStepForLayout({ ...base, layout: "shelf", direction: "left", index: 1 })).toEqual({
      type: "move",
      index: 0,
    });
    expect(pageStepForLayout({ ...base, layout: "shelf", direction: "right", index: 28 })).toEqual({
      type: "move",
      index: 29,
    });
  });

  it("skips two grid rows", () => {
    expect(pageStepForLayout({ ...base, layout: "grid", direction: "right" })).toEqual({
      type: "move",
      index: 6 + 14,
    });
    expect(pageStepForLayout({ ...base, layout: "grid", direction: "up", index: 20, columns: 5 })).toEqual({
      type: "move",
      index: 10,
    });
    expect(pageStepForLayout({ ...base, layout: "grid", direction: "down", index: 25 })).toEqual({
      type: "move",
      index: 29,
    });
  });

  it("skips a dense-list page and clamps", () => {
    expect(pageStepForLayout({ ...base, layout: "compact", direction: "down" })).toEqual({
      type: "move",
      index: 6 + LIST_PAGE_STEP,
    });
    expect(pageStepForLayout({ ...base, layout: "compact", direction: "up", index: 3 })).toEqual({
      type: "move",
      index: 0,
    });
  });

  it("does nothing on an empty shelf", () => {
    for (const layout of BIG_PICTURE_LAYOUTS) {
      expect(pageStepForLayout({ layout, direction: "right", index: 0, total: 0, columns: 6 })).toEqual({
        type: "none",
      });
    }
  });
});

describe("selection after a refresh", () => {
  it("follows the selected game to its new slot", () => {
    expect(resolveIndex([5, 3, 7], 7, 0)).toBe(2);
    expect(resolveIndex([5, 3, 7], 5, 2)).toBe(0);
  });

  it("clamps the old slot when the game is gone", () => {
    expect(resolveIndex([5, 3], 9, 4)).toBe(1);
    expect(resolveIndex([5, 3], null, 1)).toBe(1);
    expect(resolveIndex([], 5, 3)).toBe(0);
  });

  it("clamps the shelf slot when a shelf disappears", () => {
    expect(resolveShelfIndex(1, 2)).toBe(1);
    expect(resolveShelfIndex(5, 2)).toBe(1);
    expect(resolveShelfIndex(-1, 2)).toBe(0);
    expect(resolveShelfIndex(0, 0)).toBe(0);
  });
});

describe("install progress", () => {
  const job = { bytesDone: 50, bytesTotal: 200 } as Parameters<typeof jobPercent>[0];
  it("reports a percent for a known size", () => {
    expect(jobPercent(job)).toBe(25);
  });
  it("reports zero while the total is unknown and null without a job", () => {
    expect(jobPercent({ ...job!, bytesTotal: null })).toBe(0);
    expect(jobPercent(undefined)).toBeNull();
  });
  it("never exceeds 100", () => {
    expect(jobPercent({ ...job!, bytesDone: 400 })).toBe(100);
  });
});
