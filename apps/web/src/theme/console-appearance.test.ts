import { describe, expect, it } from "vitest";
import { contrastResults } from "./registry";
import { BUILTIN_THEMES, builtinTheme } from "./registry";
import { applyAccent, tvTokens } from "./derive";
import {
  CONSOLE_ACCENT_IDS,
  DEFAULT_CONSOLE_APPEARANCE,
  consoleAccentColor,
  consoleAttributes,
  consoleTokens,
  consoleVars,
  cycleChoice,
  readConsoleAppearance,
  sanitizeConsoleAppearance,
  withConsolePreset,
  writeConsoleAppearance,
  type ConsoleAccentId,
} from "./console-appearance";
import type { StorageLike } from "./storage";
import type { ThemeMode } from "./types";

/* --------------------------------------------------------------------------
   Console appearance (#229, #237): the preference must resolve to a complete,
   safe look without a browser. These tests pin the Studio default, the v1 ->
   v2 migration, the radius/accent/background choices and the curated accent
   tones against the same contrast contract every built-in theme obeys.
   -------------------------------------------------------------------------- */

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

describe("console appearance defaults", () => {
  it("opens Console in Studio with artwork and the archive's sharp corners", () => {
    expect(DEFAULT_CONSOLE_APPEARANCE).toEqual({
      preset: "studio",
      accent: "theme",
      background: "artwork",
      radius: "sharp",
    });
  });

  it("migrates an untouched v1 record to the Studio default", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      "ludex.console.appearance",
      JSON.stringify({
        v: 1,
        appearance: { preset: "soft", accent: "theme", background: "artwork", radius: "soft" },
      }),
    );
    expect(readConsoleAppearance(storage)).toEqual(DEFAULT_CONSOLE_APPEARANCE);
  });

  it("keeps an explicit v1 Soft choice: the preset stays opt-in", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      "ludex.console.appearance",
      JSON.stringify({
        v: 1,
        appearance: { preset: "soft", accent: "mint", background: "solid", radius: "round" },
      }),
    );
    expect(readConsoleAppearance(storage)).toEqual({
      preset: "soft",
      accent: "mint",
      background: "solid",
      radius: "round",
    });
  });

  it("writes the v2 record", () => {
    const storage = new MemoryStorage();
    writeConsoleAppearance(storage, DEFAULT_CONSOLE_APPEARANCE);
    expect(JSON.parse(storage.getItem("ludex.console.appearance") ?? "{}").v).toBe(2);
  });

  it("falls back field by field for a missing, partial or hand-edited record", () => {
    expect(sanitizeConsoleAppearance(null)).toEqual(DEFAULT_CONSOLE_APPEARANCE);
    expect(sanitizeConsoleAppearance("soft")).toEqual(DEFAULT_CONSOLE_APPEARANCE);
    expect(
      sanitizeConsoleAppearance({ preset: "studio", accent: "neon", background: "artwork" }),
    ).toEqual({
      preset: "studio",
      accent: "theme",
      background: "artwork",
      radius: "sharp",
    });
    expect(
      sanitizeConsoleAppearance({ preset: "cinema", radius: "round", background: "gradient" }),
    ).toEqual({
      preset: "studio",
      accent: "theme",
      background: "gradient",
      radius: "round",
    });
  });

  it("reads and writes the versioned record", () => {
    const storage = new MemoryStorage();
    writeConsoleAppearance(storage, {
      preset: "studio",
      accent: "mint",
      background: "solid",
      radius: "sharp",
    });
    expect(readConsoleAppearance(storage)).toEqual({
      preset: "studio",
      accent: "mint",
      background: "solid",
      radius: "sharp",
    });
    // A bare appearance object (older build, hand-written value) still resolves.
    storage.setItem("ludex.console.appearance", JSON.stringify({ preset: "studio" }));
    expect(readConsoleAppearance(storage)).toEqual({
      ...DEFAULT_CONSOLE_APPEARANCE,
      preset: "studio",
    });
    storage.setItem("ludex.console.appearance", "{not json");
    expect(readConsoleAppearance(storage)).toEqual(DEFAULT_CONSOLE_APPEARANCE);
  });

  it("cycles a choice row and wraps in both directions", () => {
    expect(cycleChoice(["soft", "studio"] as const, "soft", 1)).toBe("studio");
    expect(cycleChoice(["soft", "studio"] as const, "studio", 1)).toBe("soft");
    expect(cycleChoice(["soft", "studio"] as const, "soft", -1)).toBe("studio");
    expect(cycleChoice(CONSOLE_ACCENT_IDS, "theme", -1)).toBe("sun");
  });

  it("exposes one data attribute per surface choice", () => {
    expect(consoleAttributes(DEFAULT_CONSOLE_APPEARANCE)).toEqual({
      "data-console-appearance": "studio",
      "data-console-background": "artwork",
      "data-console-radius": "sharp",
    });
  });

  it("bundles a native corner scale with each preset", () => {
    expect(withConsolePreset(DEFAULT_CONSOLE_APPEARANCE, "soft").radius).toBe("soft");
    expect(withConsolePreset({ ...DEFAULT_CONSOLE_APPEARANCE, radius: "round" }, "studio").radius).toBe(
      "sharp",
    );
  });
});

describe("console tokens", () => {
  const soft = builtinTheme("soft").tokens;
  const midnight = builtinTheme("midnight").tokens;
  const base = {
    tokens: midnight,
    accent: null,
    tint: null,
    mode: "dark" as ThemeMode,
    tv: false,
  };

  it("keeps the app palette for both presets: Soft owns no colour (#237)", () => {
    const studio = consoleTokens(DEFAULT_CONSOLE_APPEARANCE, base);
    const softTokens = consoleTokens({ ...DEFAULT_CONSOLE_APPEARANCE, preset: "soft" }, base);
    expect(studio).toBeNull();
    expect(softTokens).toBeNull();
    // The retired cream palette must not leak back in through any path.
    expect(softTokens?.bg).toBeUndefined();
    expect(soft.bg).not.toBe(midnight.bg);
  });

  it("keeps the app's rendered tokens for Studio with the theme accent", () => {
    expect(
      consoleTokens({ ...DEFAULT_CONSOLE_APPEARANCE, preset: "studio" }, base),
    ).toBeNull();
  });

  it("applies a curated accent over the Studio tokens", () => {
    const tokens = consoleTokens(
      { ...DEFAULT_CONSOLE_APPEARANCE, preset: "studio", accent: "coral" },
      base,
    );
    expect(tokens?.signal).toBe("#ffc2b8");
    expect(tokens?.["on-signal"]).toBeTruthy();
    expect(tokens?.focus).not.toBe(midnight.focus);
  });

  it("uses the same curated tone in both presets", () => {
    const softTokens = consoleTokens({ ...DEFAULT_CONSOLE_APPEARANCE, preset: "soft", accent: "mint" }, base);
    const studioTokens = consoleTokens({ ...DEFAULT_CONSOLE_APPEARANCE, accent: "mint" }, base);
    // Both read the app's polarity (dark here), so the tone cannot depend on
    // which material the player picked.
    expect(softTokens?.signal).toBe("#93e3b6");
    expect(studioTokens?.signal).toBe("#93e3b6");
  });

  it("lifts a curated accent to the TV-safe contract when asked", () => {
    const tokens = consoleTokens(
      { ...DEFAULT_CONSOLE_APPEARANCE, accent: "sun" },
      { ...base, tv: true },
    );
    expect(tokens).not.toBeNull();
    const failures = contrastResults(tokens!).filter((check) => !check.ok);
    expect(failures).toEqual([]);
    expect(contrastResults(tvTokens(midnight)).every((check) => check.ok)).toBe(true);
  });

  it("returns CSS custom properties for the root and nothing for the app theme", () => {
    expect(Object.keys(consoleVars(null))).toEqual([]);
    expect(consoleVars(consoleTokens(DEFAULT_CONSOLE_APPEARANCE, base))).toEqual({});
    const accented = consoleVars(
      consoleTokens({ ...DEFAULT_CONSOLE_APPEARANCE, accent: "coral" }, base),
    );
    expect(accented["--bg"]).toBe(midnight.bg);
    expect(accented["--signal"]).toBe("#ffc2b8");
  });
});

describe("curated accents", () => {
  const ACCENTS = CONSOLE_ACCENT_IDS.filter(
    (accent): accent is Exclude<ConsoleAccentId, "theme"> => accent !== "theme",
  );

  it("clears WCAG AA on every built-in palette in its own polarity", () => {
    const failures: string[] = [];
    for (const accent of ACCENTS) {
      for (const theme of BUILTIN_THEMES) {
        const tone = consoleAccentColor(accent, theme.mode);
        expect(tone).toBeTruthy();
        const result = applyAccent(theme.tokens, tone);
        if (result.issue) {
          failures.push(`${accent} on ${theme.id} (${theme.mode}): ${result.issue.code}`);
        }
      }
    }
    expect(failures).toEqual([]);
  });
});
