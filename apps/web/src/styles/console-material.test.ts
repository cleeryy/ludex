import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

/* --------------------------------------------------------------------------
   Cabinet material budget (#229, #268).

   The blur budget is one layer — the wallpaper — painted once. The way it stops
   being affordable is an animated `backdrop-filter` or a blur behind a scrolling
   list, so these assertions keep the stylesheet honest: no panel blurs, the
   radius family never exceeds 24u, the four Cabinet steps exist, and the
   accessibility fallbacks ship.
   -------------------------------------------------------------------------- */

const css = fs.readFileSync(
  path.join(new URL("../styles", import.meta.url).pathname, "index.css"),
  "utf8",
);

function declarations(source: string): string[] {
  const cleaned = source.replace(/\/\*[\s\S]*?\*\//g, "");
  return cleaned
    .split("}")
    .map((part) => part.slice(part.lastIndexOf("{") + 1));
}

describe("console materials", () => {
  it("ships both accessibility fallbacks", () => {
    expect(css).toContain("@media (prefers-reduced-transparency: reduce)");
    expect(css).toContain("@media (prefers-contrast: more)");
  });

  it("never animates a backdrop filter", () => {
    const offenders = declarations(css).filter((block) => {
      if (!/backdrop-filter\s*:/.test(block)) return false;
      const transitions = [...block.matchAll(/transition(?:-property)?\s*:\s*([^;]+);/g)].map(
        (match) => match[1],
      );
      return transitions.some((value) => /backdrop-filter|\ball\b/.test(value));
    });
    expect(offenders).toEqual([]);
  });

  it("never blurs a panel: the wallpaper is the only blurred layer", () => {
    for (const selector of [".bp-chrome", ".bp-tile", ".bp-sheet", ".bp-panel", ".bp-band"]) {
      const block = new RegExp(`\\${selector}\\s*\\{([^}]*)\\}`).exec(css)?.[1] ?? "";
      expect(block, `${selector} must not blur its own backdrop`).not.toContain("backdrop-filter");
    }
    const declarationsOnly = css.replace(/\/\*[\s\S]*?\*\//g, "");
    const blurred = [...declarationsOnly.matchAll(/(?:^|\s)(?:-webkit-)?backdrop-filter\s*:/gm)];
    expect(blurred.length).toBeLessThanOrEqual(1);
    expect(css).toContain(".bp-backdrop-art");
    // The blur is baked into the plate (#270): the live filter only survives as
    // the fallback for the frames before the bake and for engines that cannot
    // blur a canvas.
    expect(css).toMatch(
      /\.bp-backdrop-art\[data-bp-wall="live"\]\s*\{[^}]*filter:\s*blur\(var\(--bp-wall-blur\)\)/,
    );
  });

  it("paints the wallpaper once instead of filtering the layer", () => {
    const layer = /(?:^|\n)\.bp-backdrop-art\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";
    expect(layer, "the wall layer itself must not carry a filter").not.toMatch(/(?:^|\s)filter\s*:/);
    expect(layer, "the wall layer is a static texture").toContain("contain: paint");
    expect(css).toContain(".bp-wall-plate");
    expect(css, "the baked plate replaces the live one").toContain(
      '.bp-backdrop-art[data-bp-wall="baked"] .bp-wall-source',
    );
  });

  it("keeps the Console radius family aligned to #333 geometry and at or below 24u", () => {
    for (const name of ["cover", "object", "surface", "sheet"]) {
      const match = new RegExp(`--bp-radius-${name}:\\s*calc\\(var\\(--bp-u\\) \\* (\\d+(?:\\.\\d+)?)\\)`).exec(css);
      expect(match, `--bp-radius-${name} must be expressed in --bp-u`).not.toBeNull();
      expect(Number(match?.[1])).toBeLessThanOrEqual(24);
    }
    // Restrained #333-aligned geometry scaled by --bp-u: 8 / 10 / 12 / 14u.
    expect(css).toContain("--bp-radius-cover: calc(var(--bp-u) * 8)");
    expect(css).toContain("--bp-radius-object: calc(var(--bp-u) * 10)");
    expect(css).toContain("--bp-radius-surface: calc(var(--bp-u) * 12)");
    expect(css).toContain("--bp-radius-sheet: calc(var(--bp-u) * 14)");
    // The retiring scale is gone: no token can reintroduce a 34 px card.
    expect(css).not.toMatch(/--bp-radius-(?:sm|md|lg):/);
  });

  it("defines the material tokens before any console mounts", () => {
    // Root defaults are the Night material; a console that never sets an
    // attribute must still resolve every token it reads.
    for (const token of [
      "--bp-material-filter",
      "--bp-material-chrome",
      "--bp-material-card",
      "--bp-material-sheet",
      "--bp-material-border",
      "--bp-radius-cover",
      "--bp-wall",
      "--bp-rim",
      "--bp-safe-x",
    ]) {
      expect(css).toMatch(new RegExp(`${token}:`));
    }
  });

  it("ships depth by luminance, not by drop shadow", () => {
    const tile = /\.bp-tile\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";
    expect(tile).toContain("box-shadow: none");
    const sheet = /\.bp-sheet\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";
    // A sheet separates with a 1px ring (zero blur), never a 40u blur.
    expect(sheet).toMatch(/box-shadow:\s*0 0 0 1px/);
  });
});
