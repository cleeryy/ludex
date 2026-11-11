import { describe, expect, it } from "vitest";
import type { GameExecutable } from "./types";
import {
  activeIndexForKey,
  clampActiveIndex,
  filterExecutables,
  pickerState,
  recommendedExecutable,
} from "./executable-picker";

function entry(patch: Partial<GameExecutable> & { path: string }): GameExecutable {
  return {
    name: patch.path.split("/").pop() ?? patch.path,
    sizeBytes: 1024,
    modifiedAt: "2026-01-01T00:00:00Z",
    kind: "launch",
    recommended: false,
    ...patch,
  };
}

const entries: GameExecutable[] = [
  entry({ path: "Game/Game.exe", sizeBytes: 4096, recommended: true }),
  entry({ path: "Game/bin/helper.exe" }),
  entry({ path: "setup.exe", kind: "installer" }),
  entry({ path: "unins000.exe", kind: "uninstaller" }),
  entry({ path: "Redist/vcredist_x64.exe", kind: "other" }),
];

describe("pickerState (#143)", () => {
  it("reports loading before any listing arrives", () => {
    expect(pickerState({ isLoading: true, error: null, entries: undefined, kind: "launch" })).toBe(
      "loading",
    );
  });

  it("reports error when the scan failed", () => {
    expect(
      pickerState({ isLoading: false, error: new Error("missing folder"), entries: undefined, kind: "launch" }),
    ).toBe("error");
  });

  it("reports empty when the folder has no program of that kind", () => {
    expect(pickerState({ isLoading: false, error: null, entries: [], kind: "launch" })).toBe("empty");
    expect(
      pickerState({
        isLoading: false,
        error: null,
        entries: [entry({ path: "Game.exe" })],
        kind: "uninstaller",
      }),
    ).toBe("empty");
  });

  it("reports ready when at least one entry matches", () => {
    expect(pickerState({ isLoading: false, error: null, entries, kind: "installer" })).toBe("ready");
  });
});

describe("filterExecutables (#143)", () => {
  it("keeps only the requested kind", () => {
    expect(filterExecutables(entries, "installer", "").map((item) => item.path)).toEqual([
      "setup.exe",
    ]);
    expect(filterExecutables(entries, "uninstaller", "").map((item) => item.path)).toEqual([
      "unins000.exe",
    ]);
  });

  it("searches paths and names case-insensitively", () => {
    expect(filterExecutables(entries, "launch", "BIN").map((item) => item.path)).toEqual([
      "Game/bin/helper.exe",
    ]);
    expect(filterExecutables(entries, "launch", "helper").map((item) => item.path)).toEqual([
      "Game/bin/helper.exe",
    ]);
    expect(filterExecutables(entries, "launch", "game.exe").map((item) => item.path)).toEqual([
      "Game/Game.exe",
    ]);
    expect(filterExecutables(entries, "launch", "missing")).toEqual([]);
  });

  it("exposes the detected main executable only to the launch picker", () => {
    expect(recommendedExecutable(entries, "launch")?.path).toBe("Game/Game.exe");
    expect(recommendedExecutable(entries, "installer")).toBeNull();
  });
});

describe("keyboard cursor (#143)", () => {
  it("moves down, wraps and starts from the first row", () => {
    expect(activeIndexForKey(-1, "ArrowDown", 3)).toBe(0);
    expect(activeIndexForKey(0, "ArrowDown", 3)).toBe(1);
    expect(activeIndexForKey(2, "ArrowDown", 3)).toBe(0);
  });

  it("moves up and wraps to the last row", () => {
    expect(activeIndexForKey(1, "ArrowUp", 3)).toBe(0);
    expect(activeIndexForKey(0, "ArrowUp", 3)).toBe(2);
  });

  it("handles Home, End, empty lists and unrelated keys", () => {
    expect(activeIndexForKey(1, "Home", 3)).toBe(0);
    expect(activeIndexForKey(0, "End", 3)).toBe(2);
    expect(activeIndexForKey(0, "ArrowDown", 0)).toBeNull();
    expect(activeIndexForKey(0, "Enter", 3)).toBeNull();
    expect(activeIndexForKey(0, "a", 3)).toBeNull();
  });

  it("clamps the cursor when filtering shrinks the list", () => {
    expect(clampActiveIndex(4, 2)).toBe(1);
    expect(clampActiveIndex(-1, 2)).toBe(-1);
    expect(clampActiveIndex(1, 0)).toBe(-1);
  });
});
