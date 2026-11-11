import { describe, expect, it } from "vitest";
import { isHexColor } from "@/theme/contrast";
import { BUILTIN_THEMES, DEFAULT_THEME_ID, contrastResults, themeById } from "@/theme/registry";
import { THEME_TOKEN_KEYS } from "@/theme/types";

/**
 * Token completeness and WCAG guard for every built-in theme (#140).
 *
 * The registry is the only source of colour values, so these assertions cover
 * all five palettes at once instead of parsing CSS blocks.
 */

describe("theme registry", () => {
  it("ships at least four complete themes and a valid default", () => {
    expect(BUILTIN_THEMES.length).toBeGreaterThanOrEqual(4);
    expect(themeById(DEFAULT_THEME_ID)).not.toBeNull();
    expect(new Set(BUILTIN_THEMES.map((theme) => theme.id)).size).toBe(BUILTIN_THEMES.length);
  });

  it("includes dark and light palettes", () => {
    expect(BUILTIN_THEMES.some((theme) => theme.mode === "dark")).toBe(true);
    expect(BUILTIN_THEMES.some((theme) => theme.mode === "light")).toBe(true);
  });

  it("keeps the legacy dark and light ids available", () => {
    expect(themeById("midnight")?.mode).toBe("dark");
    expect(themeById("light")?.mode).toBe("light");
    expect(themeById("nord")?.mode).toBe("dark");
    expect(themeById("gruvbox")?.mode).toBe("dark");
    expect(themeById("amber")?.mode).toBe("dark");
  });
});

describe.each(BUILTIN_THEMES.map((theme) => [theme.id, theme] as const))("%s theme", (id, theme) => {
  it("defines every token and no unknown token", () => {
    expect(Object.keys(theme.tokens).sort()).toEqual([...THEME_TOKEN_KEYS].sort());
  });

  it("uses hex colours and a sane grain value", () => {
    for (const [key, value] of Object.entries(theme.tokens)) {
      if (key === "grain-opacity") {
        expect(Number(value), `${id}.${key}`).toBeGreaterThanOrEqual(0);
        expect(Number(value), `${id}.${key}`).toBeLessThanOrEqual(0.2);
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
