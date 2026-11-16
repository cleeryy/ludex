import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { BUILTIN_THEMES, builtinTheme } from "./registry";
import { contrastRatio, mixHex, roundRatio } from "./contrast";
import { CONSOLE_ROOMS, DEFAULT_CONSOLE_ROOM, cabinetTokens, type ConsoleRoom } from "./console-room";
import { consoleAccent } from "./console-room";
import type { ThemeTokens } from "./types";

/* --------------------------------------------------------------------------
   Cabinet contrast (#237, #268).

   The console paints words on near-black at three metres, so the target is not
   AA's 4.5:1 but 7:1. Cabinet resolves every room from the app's own tokens
   (Night casts them warm, Projector steps them to black, Daylight renders the
   app's light map) and then runs the TV-safe pass, so every pair below is the
   contract the components inherit. The one composed surface is the wallpaper:
   stage text sits over art under a veil, and this file bounds that worst case.

   Values are mirrored from `styles/index.css`; the test fails if the two drift.
   -------------------------------------------------------------------------- */

const css = fs.readFileSync(
  path.join(new URL("../styles", import.meta.url).pathname, "index.css"),
  "utf8",
);

/** Mirrors `.bp-root[data-console-room="night"][data-console-wall="artwork"]`. */
const ART_OPACITY = 0.4;
const WASH = 0.8;

function surfaceOver(stage: string, art: string): string {
  return mixHex(mixHex(stage, art, ART_OPACITY), stage, WASH);
}

function lightTokens(): ThemeTokens {
  return builtinTheme("light").tokens;
}

function forRoom(room: ConsoleRoom, themeId: string): ThemeTokens {
  const theme = BUILTIN_THEMES.find((entry) => entry.id === themeId);
  if (!theme) throw new Error(`unknown theme ${themeId}`);
  return cabinetTokens({
    settings: { ...DEFAULT_CONSOLE_ROOM, room },
    tokens: theme.tokens,
    lightTokens: lightTokens(),
    darkTokens: builtinTheme("midnight").tokens,
    mode: theme.mode,
  });
}

describe("console contrast", () => {
  it("keeps the wallpaper veil in sync with the stylesheet", () => {
    expect(css).toContain(`--bp-wall-opacity: ${ART_OPACITY};`);
    expect(css).toContain(`var(--stage) ${WASH * 100}%`);
  });

  it("clears 7:1 for every pair the console paints, in every room and theme", () => {
    const failures: string[] = [];
    const pairs: [keyof ThemeTokens, keyof ThemeTokens][] = [
      ["ink", "bg"],
      ["ink", "bg-deep"],
      ["ink", "surface"],
      ["ink", "surface-2"],
      ["ink-2", "bg-deep"],
      ["ink-2", "surface"],
      ["ink-2", "elevated"],
      ["ink-3", "bg-deep"],
      ["ink-3", "surface"],
      ["ink-3", "surface-2"],
      ["signal-text", "bg-deep"],
      ["signal-text", "surface"],
      ["danger", "bg-deep"],
      ["ok", "bg-deep"],
    ];
    for (const theme of BUILTIN_THEMES) {
      for (const room of CONSOLE_ROOMS) {
        const tokens = forRoom(room, theme.id);
        for (const [foreground, background] of pairs) {
          const ratio = roundRatio(contrastRatio(tokens[foreground], tokens[background]));
          if (ratio < 7) failures.push(`${theme.id}/${room}: ${foreground}/${background} = ${ratio}`);
        }
      }
    }
    expect(failures).toEqual([]);
  });

  it("keeps primary stage text above 7:1 over the worst possible artwork", () => {
    // Pure white and pure black art are the two extremes; the veil is centred
    // on the stage, so one of them is the worst case for any stage polarity.
    const failures: string[] = [];
    for (const theme of BUILTIN_THEMES) {
      for (const room of CONSOLE_ROOMS) {
        const tokens = forRoom(room, theme.id);
        for (const art of ["#ffffff", "#000000"]) {
          const surface = surfaceOver(tokens.stage, art);
          const ratio = roundRatio(contrastRatio(tokens["stage-ink"], surface));
          if (ratio < 7) failures.push(`${theme.id}/${room} over ${art}: stage-ink = ${ratio}`);
        }
      }
    }
    expect(failures).toEqual([]);
  });

  it("documents the worst secondary stage pair", () => {
    // `stage-ink-2` is the description tone over the backdrop. It cannot reach
    // 7:1 against pure white art without killing the wallpaper, so the bound is
    // recorded here: 6.2:1 or better in the worst case, above AA.
    let worst = Number.POSITIVE_INFINITY;
    let where = "";
    for (const theme of BUILTIN_THEMES) {
      for (const room of CONSOLE_ROOMS) {
        const tokens = forRoom(room, theme.id);
        for (const art of ["#ffffff", "#000000"]) {
          const ratio = roundRatio(contrastRatio(tokens["stage-ink-2"], surfaceOver(tokens.stage, art)));
          if (ratio < worst) {
            worst = ratio;
            where = `${theme.id}/${room} over ${art}`;
          }
        }
      }
    }
    expect(worst, `worst secondary stage pair: ${where}`).toBeGreaterThanOrEqual(6.2);
  });

  it("lifts an artwork accent to 7:1 or keeps the theme signal", () => {
    const tokens = forRoom("night", "midnight");
    // A saturated mid-tone is lifted until it clears every Cabinet surface.
    const lifted = consoleAccent("#c2410c", tokens);
    expect(lifted.fellBack).toBe(false);
    expect(lifted.ratio).toBeGreaterThanOrEqual(7);
    expect(lifted.color).not.toBe("#c2410c");
    // A near-grey cannot become a colour without lying about the cover.
    const grey = consoleAccent("#7a7a7a", tokens);
    expect(grey.fellBack).toBe(true);
    expect(grey.color).toBe(tokens.signal);
  });
});
