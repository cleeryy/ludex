import { describe, expect, it } from "vitest";
import { isHexColor } from "@/theme/contrast";
import { BUILTIN_THEMES, DEFAULT_THEME_ID, contrastResults, themeById } from "@/theme/registry";
import { THEME_TOKEN_KEYS } from "@/theme/types";

/**
 * Token completeness and WCAG guard for every built-in theme (#140, #333).
 *
 * The registry is the only source of colour values, so these assertions cover
 * all eight #333 palettes at once instead of parsing CSS blocks.
 */

const EXPECTED_PRESETS = [
  ["speranza", "dark"],
  ["bauhaus", "light"],
  ["braun", "dark"],
  ["telemetry", "dark"],
  ["blueprint", "dark"],
  ["olive", "dark"],
  ["kyoto", "dark"],
  ["studio", "light"],
] as const;

describe("theme registry", () => {
  it("ships all eight #333 presets with speranza as the default", () => {
    expect(BUILTIN_THEMES).toHaveLength(8);
    expect(DEFAULT_THEME_ID).toBe("speranza");
    expect(themeById(DEFAULT_THEME_ID)).not.toBeNull();
    expect(new Set(BUILTIN_THEMES.map((theme) => theme.id)).size).toBe(BUILTIN_THEMES.length);
  });

  it("includes dark and light palettes", () => {
    expect(BUILTIN_THEMES.some((theme) => theme.mode === "dark")).toBe(true);
    expect(BUILTIN_THEMES.some((theme) => theme.mode === "light")).toBe(true);
  });

  it("exposes every #333 preset with its expected polarity", () => {
    for (const [id, mode] of EXPECTED_PRESETS) {
      expect(themeById(id)?.mode, id).toBe(mode);
    }
  });

  it("includes info and the 4-band ARC stripe tokens", () => {
    for (const token of ["info", "stripe-1", "stripe-2", "stripe-3", "stripe-4"] as const) {
      expect(THEME_TOKEN_KEYS).toContain(token);
    }
  });
});

describe.each(BUILTIN_THEMES.map((theme) => [theme.id, theme] as const))("%s theme", (id, theme) => {
  it("defines every token and no unknown token", () => {
    expect(Object.keys(theme.tokens).sort()).toEqual([...THEME_TOKEN_KEYS].sort());
  });

  it("uses hex colours and a matte grain opacity in 0.026–0.03", () => {
    for (const [key, value] of Object.entries(theme.tokens)) {
      if (key === "grain-opacity") {
        expect(Number(value), `${id}.${key}`).toBeGreaterThanOrEqual(0.026);
        expect(Number(value), `${id}.${key}`).toBeLessThanOrEqual(0.03);
      } else {
        expect(isHexColor(value), `${id}.${key} = ${value}`).toBe(true);
      }
    }
  });

  it("meets WCAG AA on every painted pair", () => {
    const failures = contrastResults(theme.tokens)
      .filter((check) => !check.ok)
      .map((check) => `${check.pair.foreground}/${check.pair.background} ${check.ratio}:1 < ${check.pair.min}:1`);
    expect(failures).toEqual([]);
  });

  it("keeps the stage text readable at TV contrast", () => {
    const { tokens } = theme;
    expect(tokens["stage"]).toBeTruthy();
    expect(tokens["stage-ink"]).not.toBe(tokens["stage"]);
  });
});
