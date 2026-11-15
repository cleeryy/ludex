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
   Console appearance (#229): the preference must resolve to a complete, safe
   look without a browser. These tests pin the Soft default, the Studio
   fallback, the radius/accent/background choices and the curated accent tones
   against the same contrast contract every built-in theme obeys.
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
  it("opens Console in the Soft preset with artwork and soft corners", () => {
    expect(DEFAULT_CONSOLE_APPEARANCE).toEqual({
      preset: "soft",
      accent: "theme",
      background: "artwork",
      radius: "soft",
    });
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
      radius: "soft",
    });
    expect(
      sanitizeConsoleAppearance({ preset: "cinema", radius: "round", background: "gradient" }),
    ).toEqual({
      preset: "soft",
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
      "data-console-appearance": "soft",
      "data-console-background": "artwork",
      "data-console-radius": "soft",
    });
  });

  it("bundles a native corner scale with each preset", () => {
    expect(withConsolePreset(DEFAULT_CONSOLE_APPEARANCE, "studio").radius).toBe("sharp");
    expect(withConsolePreset({ ...DEFAULT_CONSOLE_APPEARANCE, radius: "sharp" }, "soft").radius).toBe(
      "soft",
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

  it("paints the Soft preset with the cream palette, not the app theme", () => {
    const tokens = consoleTokens(DEFAULT_CONSOLE_APPEARANCE, base);
    expect(tokens).not.toBeNull();
    expect(tokens?.bg).toBe(soft.bg);
    expect(tokens?.ink).toBe(soft.ink);
    // The stage stays dark in every background: hero copy uses stage tokens and
    // a cream stage would put cream text on cream (#229 first pass).
    expect(tokens?.stage).toBe(soft.stage);
    expect(tokens?.["stage-ink"]).toBe(soft["stage-ink"]);
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

  it("keeps the curated accent on the Soft palette even when the app is dark", () => {
    const tokens = consoleTokens(
      { ...DEFAULT_CONSOLE_APPEARANCE, accent: "mint" },
      base,
    );
    // The mint light tone (#2c6f4e) is the one that survives on cream; the dark
    // tone would be rejected and the base signal would leak through.
    expect(tokens?.signal).toBe("#2c6f4e");
  });

  it("lifts the Soft palette to the TV-safe contract when asked", () => {
    const tokens = consoleTokens(DEFAULT_CONSOLE_APPEARANCE, { ...base, tv: true });
    expect(tokens).not.toBeNull();
    const failures = contrastResults(tokens!).filter((check) => !check.ok);
    expect(failures).toEqual([]);
    expect(contrastResults(tvTokens(soft)).every((check) => check.ok)).toBe(true);
  });

  it("returns CSS custom properties for the root and nothing for inherited Studio", () => {
    expect(Object.keys(consoleVars(null))).toEqual([]);
    const vars = consoleVars(consoleTokens({ ...DEFAULT_CONSOLE_APPEARANCE, preset: "studio" }, base));
    expect(vars).toEqual({});
    const softVars = consoleVars(consoleTokens(DEFAULT_CONSOLE_APPEARANCE, base));
    expect(softVars["--bg"]).toBe(soft.bg);
    expect(softVars["--stage"]).toBe(soft.stage);
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
