import { describe, expect, it } from "vitest";
import { contrastRatio, normalizeHex } from "@/theme/contrast";
import { applyAccent, applyInfo, applyTint, tvTokens } from "@/theme/derive";
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
 * Theme behaviour tests (#140, #333): C1/C2 accent and tint derivation,
 * geometry/stripe controls, TV contrast, import/export validation, legacy
 * migration and persistence.
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

const speranza = builtinTheme("speranza");
const bauhaus = builtinTheme("bauhaus");

describe("accents (C1 & C2)", () => {
  it("applies a readable C1 accent and derives its foreground", () => {
    const result = applyAccent(speranza.tokens, "#ff4422");
    expect(result.issue).toBeNull();
    expect(result.tokens.signal).toBe("#ff4422");
    expect(result.tokens["signal-text"]).toBe("#ff4422");
    expect(contrastRatio(result.tokens["on-signal"], result.tokens.signal)).toBeGreaterThanOrEqual(4.5);
    expect(result.ratio).toBeGreaterThanOrEqual(4.5);
  });

  it("accepts a dark C1 accent on the light theme", () => {
    const result = applyAccent(bauhaus.tokens, bauhaus.tokens["signal-text"]);
    expect(result.issue).toBeNull();
    expect(result.tokens.focus).toBeTruthy();
  });

  it("rejects a C1 accent that is too close to the background", () => {
    const result = applyAccent(speranza.tokens, "#1a1a18");
    expect(result.issue?.code).toBe("low-contrast");
    expect(result.issue?.min).toBe(4.5);
    expect(result.ratio).toBeLessThan(4.5);
    expect(result.tokens).toBe(speranza.tokens);
  });

  it("rejects malformed values instead of guessing", () => {
    const result = applyAccent(speranza.tokens, "orange");
    expect(result.issue?.code).toBe("bad-color");
  });

  it("normalises short hex", () => {
    const result = applyAccent(speranza.tokens, "#FC0");
    expect(result.issue).toBeNull();
    expect(result.tokens.signal).toBe("#ffcc00");
  });

  it("validates and applies a custom C2 (--info) accent", () => {
    const ok = applyInfo(speranza.tokens, "#00a8e8");
    expect(ok.issue).toBeNull();
    expect(ok.tokens.info).toBe("#00a8e8");
    expect(ok.ratio).toBeGreaterThanOrEqual(4.5);

    const low = applyInfo(speranza.tokens, "#191c22");
    expect(low.issue?.code).toBe("low-contrast");
    expect(low.issue?.foreground).toBe("info");
    expect(low.tokens).toBe(speranza.tokens);

    expect(applyInfo(speranza.tokens, "cyan").issue?.code).toBe("bad-color");
  });
});

describe("background tint", () => {
  it("washes the surfaces without dropping muted text below AA", () => {
    const result = applyTint(speranza.tokens, "#3a2a66");
    expect(result.issue).toBeNull();
    expect(result.tokens.bg).not.toBe(speranza.tokens.bg);
    expect(contrastRatio(result.tokens["ink-3"], result.tokens.surface)).toBeGreaterThanOrEqual(4.5);
    expect(result.ratio).toBeGreaterThanOrEqual(4.5);
  });

  it("refuses a tint that breaks text contrast", () => {
    const fragile = { ...speranza.tokens, "ink-3": "#6e6a60" };
    const result = applyTint(fragile, "#ffffff");
    expect(result.issue?.code).toBe("low-contrast");
    expect(result.tokens).toBe(fragile);
  });

  it("rejects a non-hex tint", () => {
    expect(applyTint(speranza.tokens, "rgb(1,2,3)").issue?.code).toBe("bad-color");
  });
});

describe("TV variant", () => {
  const tv = tvTokens(speranza.tokens);

  it("lifts body, secondary, muted, info and accent text to 7:1", () => {
    for (const [foreground, backgrounds] of [
      ["ink", ["bg", "surface", "elevated"]],
      ["ink-2", ["bg", "surface"]],
      ["ink-3", ["bg", "surface"]],
      ["signal-text", ["bg", "surface"]],
      ["info", ["bg", "surface"]],
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
    expect(contrastRatio(tv["on-signal"], tv.signal)).toBeGreaterThanOrEqual(4.5);
    expect(tv["grain-opacity"]).toBe("0");
    expect(tv.line).not.toBe(speranza.tokens.line);
  });

  it("keeps stage text readable over artwork", () => {
    expect(contrastRatio(tv["stage-ink"], tv.stage)).toBeGreaterThanOrEqual(7);
    expect(contrastRatio(tv["stage-ink-2"], tv.stage)).toBeGreaterThanOrEqual(7);
  });
});

describe("theme files", () => {
  const file = themeFileFor("Archive Blue", speranza, speranza.tokens, null, "#38bdf8");
  const json = serializeThemeFile(file);

  it("round-trips a complete theme including optional C2 info", () => {
    const parsed = parseThemeFile(json);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.theme.name).toBe("Archive Blue");
      expect(parsed.theme.base).toBe("speranza");
      expect(parsed.theme.tokens).toEqual(speranza.tokens);
      expect(parsed.theme.accent).toBeNull();
      expect(parsed.theme.info).toBe("#38bdf8");
    }
  });

  it("backfills info and stripe tokens when importing a pre-#333 theme file", () => {
    const legacyTokens: Record<string, string> = { ...speranza.tokens };
    delete legacyTokens.info;
    delete legacyTokens["stripe-1"];
    delete legacyTokens["stripe-2"];
    delete legacyTokens["stripe-3"];
    delete legacyTokens["stripe-4"];
    const result = validateThemeFile({
      format: "ludex-theme",
      version: 1,
      name: "Legacy Dark",
      base: "midnight",
      mode: "dark",
      tokens: legacyTokens,
      accent: null,
    });
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.theme.base).toBe("speranza");
      expect(result.theme.tokens.info).toBe(speranza.tokens.info);
      expect(result.theme.tokens["stripe-1"]).toBe(speranza.tokens["stripe-1"]);
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

  it("rejects missing core tokens", () => {
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
    const result = validateThemeFile({ ...file, tokens: { ...file.tokens, "grain-opacity": 0.03 } });
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.theme.tokens["grain-opacity"]).toBe("0.03");
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
  it("defaults to system, shadcn radius and ARC stripe enabled", () => {
    expect(readConfig(new MemoryStorage())).toEqual(defaultConfig());
    expect(defaultConfig()).toEqual({
      selection: "system",
      accent: null,
      tint: null,
      info: null,
      radius: "shadcn",
      stripe: "on",
    });
  });

  it("migrates legacy theme ids cleanly to #333 presets", () => {
    const storage = new MemoryStorage();
    storage.setItem("ludex.theme", "light");
    expect(readConfig(storage).selection).toBe("bauhaus");
    storage.setItem("ludex.theme", "dark");
    expect(readConfig(storage).selection).toBe("speranza");
    storage.setItem("ludex.theme", JSON.stringify({ config: { selection: "midnight" } }));
    expect(readConfig(storage).selection).toBe("speranza");
    storage.setItem("ludex.theme", JSON.stringify({ config: { selection: "nord" } }));
    expect(readConfig(storage).selection).toBe("telemetry");
    storage.setItem("ludex.theme", JSON.stringify({ config: { selection: "gruvbox" } }));
    expect(readConfig(storage).selection).toBe("braun");
    storage.setItem("ludex.theme", "system");
    expect(readConfig(storage).selection).toBe("system");
    storage.setItem("ludex.theme", "not-a-theme");
    expect(readConfig(storage).selection).toBe("system");
  });

  it("persists config, C2 info, radius, stripe and resolved maps for both modes", () => {
    const storage = new MemoryStorage();
    const appearance = buildAppearance({
      selection: "telemetry",
      accent: null,
      tint: null,
      info: "#38bdf8",
      radius: "swiss",
      stripe: "off",
    });
    writeAppearance(appearance, storage);
    const read = readConfig(storage);
    expect(read.selection).toBe("telemetry");
    expect(read.info).toBe("#38bdf8");
    expect(read.radius).toBe("swiss");
    expect(read.stripe).toBe("off");
    expect(appearance.vars.dark["--bg"]).toBe(builtinTheme("telemetry").tokens.bg);
    expect(appearance.vars.dark["--info"]).toBe("#38bdf8");
    expect(appearance.bases.dark).toBe("telemetry");
    expect(appearance.mode).toBe("dark");
  });

  it("resolves the system option to speranza or bauhaus", () => {
    const appearance = buildAppearance({ selection: "system", accent: null, tint: null });
    expect(appearance.bases.dark).toBe("speranza");
    expect(appearance.bases.light).toBe("bauhaus");
    expect(appearance.vars.dark["--bg"]).toBe(speranza.tokens.bg);
    expect(appearance.vars.light["--bg"]).toBe(bauhaus.tokens.bg);
    expect(resolveMode("system", true)).toBe("light");
    expect(resolveMode("system", false)).toBe("dark");
    expect(resolveMode("bauhaus", false)).toBe("light");
    expect(resolveMode("studio", false)).toBe("light");
    expect(resolveMode("blueprint", true)).toBe("dark");
  });

  it("stores imported themes and ignores corrupt records", () => {
    const storage = new MemoryStorage();
    const file = themeFileFor("Night Shift", speranza, speranza.tokens, null);
    writeCustomThemes([file], storage);
    expect(readCustomThemes(storage)).toHaveLength(1);
    expect(readCustomThemes(storage)[0].name).toBe("Night Shift");

    storage.setItem("ludex.theme.custom.v1", '{ "not": "an array" }');
    expect(readCustomThemes(storage)).toEqual([]);
    storage.setItem("ludex.theme.custom.v1", '[{"name":"broken"}]');
    expect(readCustomThemes(storage)).toEqual([]);
  });

  it("selects an imported theme by its stable id", () => {
    const file = themeFileFor("Night Shift", speranza, speranza.tokens, "#ffc61a");
    const id = customThemeId(file.name, file.tokens);
    expect(id.startsWith("custom:")).toBe(true);
    expect(customThemeId(file.name, file.tokens)).toBe(id);
    const appearance = buildAppearance({ selection: id, accent: file.accent, tint: null }, [file]);
    expect(appearance.config.selection).toBe(id);
    expect(appearance.vars.dark["--signal"]).toBe("#ffc61a");
    expect(appearance.bases.dark).toBe("speranza");
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

  it("writes every token, data-radius, data-stripe, the mode and a wider TV focus ring", () => {
    const { props, dataset, root } = fakeRoot();
    const appearance = buildAppearance({
      selection: "braun",
      accent: null,
      tint: null,
      radius: "brutalist",
      stripe: "off",
    });
    applyAppearance(appearance, "dark", false, root);
    expect(props.get("--bg")).toBe(builtinTheme("braun").tokens.bg);
    expect(props.get("--info")).toBe(builtinTheme("braun").tokens.info);
    expect(props.get("--stripe-1")).toBe(builtinTheme("braun").tokens["stripe-1"]);
    expect(props.get("--focus-width")).toBe("2px");
    expect(dataset.theme).toBe("braun");
    expect(dataset.themeMode).toBe("dark");
    expect(dataset.radius).toBe("brutalist");
    expect(dataset.stripe).toBe("off");
    expect(dataset.tv).toBe("false");

    applyAppearance(appearance, "dark", true, root);
    expect(props.get("--focus-width")).toBe("3px");
    expect(props.get("--focus-offset")).toBe("4px");
    expect(dataset.tv).toBe("true");
  });

  it("clears inline overrides and dataset attributes when a theme is removed", () => {
    const { props, dataset, root } = fakeRoot();
    const appearance = buildAppearance({ selection: "olive", accent: null, tint: null });
    applyAppearance(appearance, "dark", false, root);
    clearAppearance(root);
    expect(props.size).toBe(0);
    expect(dataset.theme).toBeUndefined();
    expect(dataset.radius).toBeUndefined();
    expect(dataset.stripe).toBeUndefined();
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
