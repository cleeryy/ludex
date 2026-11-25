// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import {
  addCustomPreset,
  applyPresetParams,
  builtinPresets,
  deleteCustomPreset,
  extractPresetParams,
  loadCustomPresets,
  matchesPreset,
} from "./library-presets";

beforeEach(() => {
  window.localStorage.clear();
});

describe("library presets", () => {
  it("includes Ready to play only when desktop installs are available", () => {
    const withInstalls = builtinPresets(true).map((preset) => preset.id);
    const withoutInstalls = builtinPresets(false).map((preset) => preset.id);
    expect(withInstalls).toContain("builtin:ready");
    expect(withoutInstalls).not.toContain("builtin:ready");
    expect(withoutInstalls).toEqual(["builtin:backlog", "builtin:top-rated", "builtin:short"]);
  });

  it("extracts recognized filter params and preserves diagnostic flags on apply", () => {
    const current = new URLSearchParams("genre=RPG&gamepadDebug=1&unknown=ignored");
    expect(extractPresetParams(current)).toEqual({ genre: "RPG" });

    const applied = applyPresetParams(current, { state: "unplayed", sort: "rating", order: "desc" });
    expect(applied.get("genre")).toBeNull();
    expect(applied.get("state")).toBe("unplayed");
    expect(applied.get("sort")).toBe("rating");
    expect(applied.get("order")).toBe("desc");
    expect(applied.get("gamepadDebug")).toBe("1");
  });

  it("matches URL params against a preset accurately", () => {
    const preset = { state: "unplayed", sort: "rating", order: "desc" };
    expect(matchesPreset(new URLSearchParams("state=unplayed&sort=rating&order=desc"), preset)).toBe(true);
    expect(matchesPreset(new URLSearchParams("state=unplayed&sort=rating&order=desc&genre=RPG"), preset)).toBe(false);
    expect(matchesPreset(new URLSearchParams(""), preset)).toBe(false);
  });

  it("persists, deduplicates by name, and deletes custom presets", () => {
    const afterAdd = addCustomPreset("  Co-op weekend ", { players: "2", minRating: "80" });
    expect(afterAdd).toHaveLength(1);
    expect(afterAdd[0].name).toBe("Co-op weekend");
    expect(loadCustomPresets()).toHaveLength(1);

    const afterReplace = addCustomPreset("co-op weekend", { players: "4" });
    expect(afterReplace).toHaveLength(1);
    expect(afterReplace[0].params).toEqual({ players: "4" });

    const afterDelete = deleteCustomPreset(afterReplace[0].id);
    expect(afterDelete).toHaveLength(0);
    expect(loadCustomPresets()).toHaveLength(0);
  });
});
