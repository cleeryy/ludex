import { describe, expect, it } from "vitest";
import { contrastRatio, normalizeHex } from "@/theme/contrast";
import { applyAccent, applyTint, tvTokens } from "@/theme/derive";
import { builtinTheme, resolveMode } from "@/theme/registry";
import {
  applyAppearance,
  buildAppearance,
  clearAppearance,
  customThemeId,
  defaultConfig,
  readConfig,
  readCustomThemes,
  writeAppearance,
  writeCustomThemes,
  type StorageLike,
} from "@/theme/storage";
import {
  parseThemeFile,
  serializeThemeFile,
  themeFileFor,
  validateThemeFile,
} from "@/theme/validate";

/**
 * Theme behaviour tests (#140): accent and tint derivation, TV contrast,
 * import/export validation, persistence and the system option.
 */

class MemoryStorage implements StorageLike {
  private values = new Map<string, string>();
  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
  removeItem(key: string): void {
    this.values.delete(key);
  }
}

const midnight = builtinTheme("midnight");
const light = builtinTheme("light");

describe("accents", () => {
  it("applies a readable accent and derives its foreground", () => {
    const result = applyAccent(midnight.tokens, "#ffc61a");
    expect(result.issue).toBeNull();
    expect(result.tokens.signal).toBe("#ffc61a");
    expect(result.tokens["signal-text"]).toBe("#ffc61a");
    expect(contrastRatio(result.tokens["on-signal"], result.tokens.signal)).toBeGreaterThanOrEqual(4.5);
    expect(result.ratio).toBeGreaterThanOrEqual(4.5);
  });

  it("accepts a dark accent on the light theme", () => {
    const result = applyAccent(light.tokens, light.tokens["signal-text"]);
    expect(result.issue).toBeNull();
    expect(result.tokens.focus).toBeTruthy();
  });

  it("rejects an accent that is too close to the background", () => {
    const result = applyAccent(midnight.tokens, "#1a1a18");
    expect(result.issue?.code).toBe("low-contrast");
    expect(result.issue?.min).toBe(4.5);
    expect(result.ratio).toBeLessThan(4.5);
    expect(result.tokens).toBe(midnight.tokens);
  });

  it("rejects malformed values instead of guessing", () => {
    const result = applyAccent(midnight.tokens, "orange");
    expect(result.issue?.code).toBe("bad-color");
  });

  it("normalises short hex", () => {
    const result = applyAccent(midnight.tokens, "#FC0");
    expect(result.issue).toBeNull();
    expect(result.tokens.signal).toBe("#ffcc00");
  });
});

describe("background tint", () => {
  it("washes the surfaces without dropping muted text below AA", () => {
    const result = applyTint(midnight.tokens, "#3a2a66");
    expect(result.issue).toBeNull();
    expect(result.tokens.bg).not.toBe(midnight.tokens.bg);
    expect(contrastRatio(result.tokens["ink-3"], result.tokens.surface)).toBeGreaterThanOrEqual(4.5);
    expect(result.ratio).toBeGreaterThanOrEqual(4.5);
  });

  it("refuses a tint that breaks text contrast", () => {
    const fragile = { ...midnight.tokens, "ink-3": "#6e6a60" };
    const result = applyTint(fragile, "#ffffff");
    expect(result.issue?.code).toBe("low-contrast");
    expect(result.tokens).toBe(fragile);
  });

  it("rejects a non-hex tint", () => {
    expect(applyTint(midnight.tokens, "rgb(1,2,3)").issue?.code).toBe("bad-color");
  });
});

describe("TV variant", () => {
  const tv = tvTokens(midnight.tokens);

  it("lifts body, secondary, muted and accent text to 7:1", () => {
    for (const [foreground, backgrounds] of [
      ["ink", ["bg", "surface", "elevated"]],
      ["ink-2", ["bg", "surface"]],
      ["ink-3", ["bg", "surface"]],
      ["signal-text", ["bg", "surface"]],
    ] as const) {
      for (const background of backgrounds) {
        expect(
          contrastRatio(tv[foreground], tv[background]),
          `${foreground}/${background}`,
        ).toBeGreaterThanOrEqual(7);
      }
    }
  });

  it("strengthens focus and removes decorative grain", () => {
    expect(contrastRatio(tv.focus, tv.bg)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(tv["on-signal"], tv.signal)).toBeGreaterThanOrEqual(7);
    expect(tv["grain-opacity"]).toBe("0");
    expect(tv.line).not.toBe(midnight.tokens.line);
  });

  it("keeps stage text readable over artwork", () => {
    expect(contrastRatio(tv["stage-ink"], tv.stage)).toBeGreaterThanOrEqual(7);
    expect(contrastRatio(tv["stage-ink-2"], tv.stage)).toBeGreaterThanOrEqual(7);
  });
});

describe("theme files", () => {
  const file = themeFileFor("Archive Blue", midnight, midnight.tokens, null);
  const json = serializeThemeFile(file);

  it("round-trips a complete theme", () => {
    const parsed = parseThemeFile(json);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.theme.name).toBe("Archive Blue");
      expect(parsed.theme.base).toBe("midnight");
      expect(parsed.theme.tokens).toEqual(midnight.tokens);
      expect(parsed.theme.accent).toBeNull();
    }
  });

  it("rejects unknown tokens with the token name", () => {
    const source = { ...file, tokens: { ...file.tokens, "surface-3": "#ffffff" } };
    const result = validateThemeFile(source);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.issues.some((issue) => issue.code === "unknown-token" && issue.token === "surface-3")).toBe(true);
    }
  });

  it("rejects missing tokens", () => {
    const withoutWarn: Record<string, string> = { ...file.tokens };
    delete withoutWarn.warn;
    const result = validateThemeFile({ ...file, tokens: withoutWarn });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.issues.some((issue) => issue.code === "missing-token" && issue.token === "warn")).toBe(true);
    }
  });

  it("rejects malformed colours", () => {
    const result = validateThemeFile({ ...file, tokens: { ...file.tokens, signal: "yellow" } });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.issues.some((issue) => issue.code === "bad-color" && issue.token === "signal")).toBe(true);
    }
  });

  it("accepts a numeric grain opacity from a hand-written file", () => {
    const result = validateThemeFile({ ...file, tokens: { ...file.tokens, "grain-opacity": 0.05 } });
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.theme.tokens["grain-opacity"]).toBe("0.05");
  });

  it("rejects low-contrast token sets with the measured ratio", () => {
    const result = validateThemeFile({
      ...file,
      tokens: { ...file.tokens, "ink-3": file.tokens.bg },
    });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      const issue = result.issues.find((entry) => entry.code === "low-contrast");
      expect(issue).toBeTruthy();
      expect(issue?.ratio).toBeLessThan(issue?.min ?? 0);
      expect(issue?.foreground).toBeTruthy();
      expect(issue?.background).toBeTruthy();
    }
  });

  it("rejects an unknown base and a mismatched mode", () => {
    expect(validateThemeFile({ ...file, base: "solarized" }).ok).toBe(false);
    const mismatch = validateThemeFile({ ...file, mode: "light" });
    expect(mismatch.ok).toBe(false);
    if (!mismatch.ok) expect(mismatch.issues.some((issue) => issue.code === "bad-mode")).toBe(true);
  });

  it("rejects empty names and imported accents that fail contrast", () => {
    expect(validateThemeFile({ ...file, name: "   " }).ok).toBe(false);
    const badAccent = validateThemeFile({ ...file, accent: "#121212" });
    expect(badAccent.ok).toBe(false);
    if (!badAccent.ok) expect(badAccent.issues.some((issue) => issue.code === "low-contrast")).toBe(true);
  });

  it("rejects invalid JSON and foreign formats", () => {
    expect(parseThemeFile("{not json").ok).toBe(false);
    expect(validateThemeFile({ ...file, format: "vscode-theme" }).ok).toBe(false);
  });
});

describe("persistence", () => {
  it("defaults to the system option", () => {
    expect(readConfig(new MemoryStorage())).toEqual(defaultConfig());
  });

  it("migrates the legacy plain-string value", () => {
    const storage = new MemoryStorage();
    storage.setItem("ludex.theme", "light");
    expect(readConfig(storage).selection).toBe("light");
    storage.setItem("ludex.theme", "dark");
    expect(readConfig(storage).selection).toBe("midnight");
    storage.setItem("ludex.theme", "system");
    expect(readConfig(storage).selection).toBe("system");
    storage.setItem("ludex.theme", "not-a-theme");
    expect(readConfig(storage).selection).toBe("system");
  });

  it("persists config and resolved maps for both modes", () => {
    const storage = new MemoryStorage();
    const appearance = buildAppearance({ selection: "nord", accent: null, tint: null });
    writeAppearance(appearance, storage);
    expect(readConfig(storage).selection).toBe("nord");
    expect(appearance.vars.dark["--bg"]).toBe(builtinTheme("nord").tokens.bg);
    expect(appearance.bases.dark).toBe("nord");
    expect(appearance.mode).toBe("dark");
  });

  it("resolves the system option to midnight or paper", () => {
    const appearance = buildAppearance({ selection: "system", accent: null, tint: null });
    expect(appearance.bases.dark).toBe("midnight");
    expect(appearance.bases.light).toBe("light");
    expect(appearance.vars.dark["--bg"]).toBe(midnight.tokens.bg);
    expect(appearance.vars.light["--bg"]).toBe(light.tokens.bg);
    expect(resolveMode("system", true)).toBe("light");
    expect(resolveMode("system", false)).toBe("dark");
    expect(resolveMode("light", false)).toBe("light");
    expect(resolveMode("nord", true)).toBe("dark");
  });

  it("stores imported themes and ignores corrupt records", () => {
    const storage = new MemoryStorage();
    const file = themeFileFor("Night Shift", midnight, midnight.tokens, null);
    writeCustomThemes([file], storage);
    expect(readCustomThemes(storage)).toHaveLength(1);
    expect(readCustomThemes(storage)[0].name).toBe("Night Shift");

    storage.setItem("ludex.theme.custom.v1", '{ "not": "an array" }');
    expect(readCustomThemes(storage)).toEqual([]);
    storage.setItem("ludex.theme.custom.v1", '[{"name":"broken"}]');
    expect(readCustomThemes(storage)).toEqual([]);
  });

  it("selects an imported theme by its stable id", () => {
    const file = themeFileFor("Night Shift", midnight, midnight.tokens, "#ffc61a");
    const id = customThemeId(file.name, file.tokens);
    expect(id.startsWith("custom:")).toBe(true);
    expect(customThemeId(file.name, file.tokens)).toBe(id);
    const appearance = buildAppearance({ selection: id, accent: file.accent, tint: null }, [file]);
    expect(appearance.config.selection).toBe(id);
    expect(appearance.vars.dark["--signal"]).toBe("#ffc61a");
    expect(appearance.bases.dark).toBe("midnight");
  });
});

describe("document application", () => {
  function fakeRoot() {
    const props = new Map<string, string>();
    const dataset: Record<string, string> = {};
    return {
      props,
      dataset,
      root: {
        dataset,
        style: {
          setProperty: (key: string, value: string) => props.set(key, value),
          removeProperty: (key: string) => props.delete(key),
        },
      } as unknown as HTMLElement,
    };
  }

  it("writes every token, the mode and a wider TV focus ring", () => {
    const { props, dataset, root } = fakeRoot();
    const appearance = buildAppearance({ selection: "gruvbox", accent: null, tint: null });
    applyAppearance(appearance, "dark", false, root);
    expect(props.get("--bg")).toBe(builtinTheme("gruvbox").tokens.bg);
    expect(props.get("--focus-width")).toBe("2px");
    expect(dataset.theme).toBe("gruvbox");
    expect(dataset.themeMode).toBe("dark");
    expect(dataset.tv).toBe("false");

    applyAppearance(appearance, "dark", true, root);
    expect(props.get("--focus-width")).toBe("3px");
    expect(props.get("--focus-offset")).toBe("4px");
    expect(dataset.tv).toBe("true");
  });

  it("clears inline overrides when a theme is removed", () => {
    const { props, dataset, root } = fakeRoot();
    const appearance = buildAppearance({ selection: "amber", accent: null, tint: null });
    applyAppearance(appearance, "dark", false, root);
    clearAppearance(root);
    expect(props.size).toBe(0);
    expect(dataset.theme).toBeUndefined();
  });
});

describe("normalisation", () => {
  it("accepts only three- or six-digit hex", () => {
    expect(normalizeHex("#abc")).toBe("#aabbcc");
    expect(normalizeHex("#AABBCC")).toBe("#aabbcc");
    expect(normalizeHex("aabbcc")).toBeNull();
    expect(normalizeHex("#abcd")).toBeNull();
    expect(normalizeHex("#12345678")).toBeNull();
  });
});
