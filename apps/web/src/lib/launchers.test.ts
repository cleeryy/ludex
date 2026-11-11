import { describe, expect, it } from "vitest";
import {
  ALL_LAUNCHER_IDS,
  LAUNCHER_SOURCES,
  launcherSourceState,
  normalizeLauncherSources,
} from "@/lib/launchers";
import type { LauncherSource } from "@/lib/types";

const source = (overrides: Partial<LauncherSource> = {}): LauncherSource => ({
  id: "epic",
  label: "Epic Games",
  found: 0,
  available: false,
  disabled: false,
  detail: "Epic Games Launcher was not detected",
  ...overrides,
});

describe("launcher sources (#155)", () => {
  it("lists every supported source in display order", () => {
    expect(ALL_LAUNCHER_IDS).toEqual([
      "steam",
      "epic",
      "gog",
      "ea",
      "ubisoft",
      "xbox",
      "itch",
      "heroic",
      "lutris",
    ]);
    expect(LAUNCHER_SOURCES).toHaveLength(9);
    expect(LAUNCHER_SOURCES[0]).toEqual({ id: "steam", label: "Steam" });
  });

  it("treats a missing settings value as every source", () => {
    // A settings file written before #155 has no key at all.
    expect(normalizeLauncherSources(undefined)).toEqual(ALL_LAUNCHER_IDS);
    expect(normalizeLauncherSources(null)).toEqual(ALL_LAUNCHER_IDS);
  });

  it("keeps an explicit empty list empty", () => {
    // "None" is a real choice: it must not fall back to all sources.
    expect(normalizeLauncherSources([])).toEqual([]);
  });

  it("drops unknown ids, duplicates and canonicalizes the order", () => {
    expect(normalizeLauncherSources([" Lutris ", "epic", "epic", "epicgames", "nope"])).toEqual([
      "epic",
      "lutris",
    ]);
  });

  it("reports disabled, missing, empty and detected states", () => {
    expect(launcherSourceState(source({ available: true, found: 3 }), false)).toBe("disabled");
    // A disabled source was never read: it is never "not detected".
    expect(launcherSourceState(undefined, false)).toBe("disabled");
    expect(launcherSourceState(undefined, true)).toBe("missing");
    expect(launcherSourceState(source(), true)).toBe("missing");
    expect(launcherSourceState(source({ available: true }), true)).toBe("empty");
    expect(launcherSourceState(source({ available: true, found: 2 }), true)).toBe("detected");
  });
});
