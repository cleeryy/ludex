import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { BUILTIN_THEMES } from "./registry";
import { tvTokens } from "./derive";
import { contrastRatio, mixHex, roundRatio } from "./contrast";
import type { ThemeTokens } from "./types";

/* --------------------------------------------------------------------------
   Console contrast (#237).

   Entering Console used to paint a cream palette under translucent cards over
   dark art: tile captions sampled 4.65–5.45:1 against a 7:1 target. Soft now
   keeps the app theme and its plates are opaque, so every surface pair is the
   token contract. The one composed surface left is the wallpaper: stage text
   sits over art under a veil, and this file bounds that worst case.

   Values are mirrored from `styles/index.css`; the test fails if the two drift.
   -------------------------------------------------------------------------- */

const css = fs.readFileSync(
  path.join(new URL("../styles", import.meta.url).pathname, "index.css"),
  "utf8",
);

/** Mirrors `.bp-root[data-console-background="artwork"]`. */
const ART_OPACITY = 0.45;
const WASH = 0.8;

function surfaceOver(stage: string, art: string): string {
  return mixHex(mixHex(stage, art, ART_OPACITY), stage, WASH);
}

/** A theme as the console sees it: the TV-safe map (7:1 text). */
function consoleTokens(tokens: ThemeTokens): ThemeTokens {
  return tvTokens(tokens);
}

describe("console contrast", () => {
  it("keeps the wallpaper veil in sync with the stylesheet", () => {
    expect(css).toContain(`--bp-wall-opacity: ${ART_OPACITY};`);
    expect(css).toContain(`var(--stage) ${WASH * 100}%`);
  });

  it("keeps the console plates opaque so no surface pair is composed", () => {
    const soft = /\.bp-root\[data-console-appearance="soft"\]\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";
    for (const token of [
      "--bp-material-chrome",
      "--bp-material-card",
      "--bp-material-sheet",
      "--bp-material-panel",
    ]) {
      const value = new RegExp(`${token}:\\s*([^;]+);`).exec(soft)?.[1] ?? "";
      expect(value, `${token} must be a token, not a translucent mix`).toMatch(/var\(--/);
      expect(value).not.toContain("transparent");
    }
  });

  it("clears 7:1 for the text the console paints on its surfaces", () => {
    const failures: string[] = [];
    for (const theme of BUILTIN_THEMES) {
      const tokens = consoleTokens(theme.tokens);
      const pairs: [keyof ThemeTokens, keyof ThemeTokens][] = [
        ["ink", "bg"],
        ["ink", "surface"],
        ["ink", "elevated"],
        ["ink-2", "surface"],
        ["ink-2", "elevated"],
        ["ink-3", "surface"],
        ["ink-3", "elevated"],
        ["signal-text", "surface"],
        ["danger", "surface"],
        ["ok", "surface"],
      ];
      for (const [foreground, background] of pairs) {
        const ratio = roundRatio(contrastRatio(tokens[foreground], tokens[background]));
        if (ratio < 7) failures.push(`${theme.id}: ${foreground}/${background} = ${ratio}`);
      }
    }
    expect(failures).toEqual([]);
  });

  it("keeps primary stage text above 7:1 over the worst possible artwork", () => {
    // Pure white and pure black art are the two extremes; the veil is centred
    // on the stage, so one of them is the worst case for any stage polarity.
    const failures: string[] = [];
    for (const theme of BUILTIN_THEMES) {
      const tokens = consoleTokens(theme.tokens);
      for (const art of ["#ffffff", "#000000"]) {
        const surface = surfaceOver(tokens.stage, art);
        const ratio = roundRatio(contrastRatio(tokens["stage-ink"], surface));
        if (ratio < 7) failures.push(`${theme.id} over ${art}: stage-ink = ${ratio}`);
      }
    }
    expect(failures).toEqual([]);
  });

  it("documents the remaining worst pair: secondary stage text over art", () => {
    // `stage-ink-2` is the caption tone over the backdrop. It cannot reach 7:1
    // against pure white art without killing the wallpaper, so the bound is
    // recorded here: 6.2:1 or better in the worst case, above AA.
    let worst = Number.POSITIVE_INFINITY;
    let where = "";
    for (const theme of BUILTIN_THEMES) {
      const tokens = consoleTokens(theme.tokens);
      for (const art of ["#ffffff", "#000000"]) {
        const ratio = roundRatio(contrastRatio(tokens["stage-ink-2"], surfaceOver(tokens.stage, art)));
        if (ratio < worst) {
          worst = ratio;
          where = `${theme.id} over ${art}`;
        }
      }
    }
    expect(worst, `worst secondary stage pair: ${where}`).toBeGreaterThanOrEqual(6.2);
  });
});
