import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

/* --------------------------------------------------------------------------
   Console material budget (#229).

   Static blur is the whole point of the Soft preset, and the way it stops
   being affordable is an animated `backdrop-filter` or a blur behind a
   scrolling list. These assertions keep the stylesheet honest: the fallbacks
   exist, the radius scale stays in the 16–24 px band, and no transition or
   animation ever targets a blur.
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
      // `all` would pick the blur up too; only listed properties are safe.
      return transitions.some((value) => /backdrop-filter|\ball\b/.test(value));
    });
    expect(offenders).toEqual([]);
  });

  it("keeps the Soft card and panel radii in the 16–24 px band", () => {
    const soft = /\[data-console-radius="soft"\]\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";
    const read = (name: string) => Number(new RegExp(`--bp-radius-${name}:\\s*(\\d+(?:\\.\\d+)?)px`).exec(soft)?.[1]);
    // Covers are a step smaller than cards; panels and sheets own the band.
    expect(read("sm")).toBeGreaterThanOrEqual(10);
    expect(read("md")).toBeGreaterThanOrEqual(16);
    expect(read("md")).toBeLessThanOrEqual(24);
    expect(read("lg")).toBeGreaterThanOrEqual(16);
    expect(read("lg")).toBeLessThanOrEqual(24);
  });

  it("defines the material tokens for Studio before any console mounts", () => {
    // Root defaults are the studio material; a console that never sets an
    // attribute must still resolve every token it reads.
    for (const token of [
      "--bp-material-filter",
      "--bp-material-chrome",
      "--bp-material-card",
      "--bp-material-sheet",
      "--bp-material-border",
      "--bp-radius-sm",
      "--bp-wall",
    ]) {
      expect(css).toMatch(new RegExp(`${token}:`));
    }
  });
});
