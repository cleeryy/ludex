import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

/* --------------------------------------------------------------------------
   Console type scale (#237, #268).

   The 10-foot floor is 24u: nothing smaller carries words, a size or a count.
   28u for anything a player must read to act, 34u for a surface title, 56u for
   the stage hero. These assertions pin the scale, keep every console surface on
   it, and stop a new arbitrary `text-[11px]` from creeping back in.
   -------------------------------------------------------------------------- */

const srcRoot = new URL("..", import.meta.url).pathname;
const css = fs.readFileSync(path.join(srcRoot, "styles", "index.css"), "utf8");

const CONSOLE_FILES = [
  ...fs
    .readdirSync(path.join(srcRoot, "components", "big-picture"))
    .filter((file) => file.endsWith(".tsx"))
    .map((file) => path.join(srcRoot, "components", "big-picture", file)),
  // #267: the screen moved into components/console/{shell,routes,input,surfaces}.
  ...fs
    .readdirSync(path.join(srcRoot, "components", "console", "shell"))
    .filter((file) => file.endsWith(".tsx"))
    .map((file) => path.join(srcRoot, "components", "console", "shell", file)),
  ...fs
    .readdirSync(path.join(srcRoot, "components", "console", "surfaces"))
    .filter((file) => file.endsWith(".tsx"))
    .map((file) => path.join(srcRoot, "components", "console", "surfaces", file)),
];

/** `--bp-text-*: calc(var(--bp-u) * N)` → N, in 1080p pixels. */
function step(name: string): number {
  const match = new RegExp(`--bp-text-${name}:\\s*calc\\(var\\(--bp-u\\) \\* (\\d+(?:\\.\\d+)?)\\)`).exec(css);
  expect(match, `--bp-text-${name} must be expressed in --bp-u`).not.toBeNull();
  return Number(match?.[1]);
}

describe("console type scale", () => {
  it("keeps the 24u floor, 28u body, 34u title and a 48u+ hero at 1080p", () => {
    // --bp-u is 1.0003 px at 1920, so the multiplier is the 1080p pixel size.
    expect(step("meta")).toBe(24);
    expect(step("body")).toBe(28);
    expect(step("title")).toBe(34);
    expect(step("hero")).toBeGreaterThanOrEqual(48);
  });

  it("keeps the scale ordered and above the 24u floor", () => {
    const meta = step("meta");
    const body = step("body");
    const title = step("title");
    const hero = step("hero");
    expect(meta).toBeLessThan(body);
    expect(body).toBeLessThan(title);
    expect(title).toBeLessThan(hero);
    expect(meta).toBeGreaterThanOrEqual(24);
    // No retired 20u step: a numeral is not an excuse to go below the floor.
    expect(css).not.toContain("--bp-text-micro");
  });

  it("exposes one utility per step, bound to the console unit", () => {
    for (const name of ["hero", "title", "body", "meta"]) {
      expect(css).toContain(`--text-bp-${name}: var(--bp-text-${name});`);
    }
    expect(css).not.toContain("--text-bp-micro");
  });

  it("scales with the display instead of staying at 1080p", () => {
    // The unit doubles from 1080p to 4K; the type must follow it.
    const unit = /--bp-u:\s*clamp\(0\.72px,\s*0\.0521vw,\s*2\.2px\)/.exec(css);
    expect(unit).not.toBeNull();
    // 0.0521vw * 3840 ≈ 2.0 px against 1.0003 px at 1920.
    expect((0.0521 * 3840) / 100).toBeGreaterThan(1.9);
  });

  it("keeps every console surface on the scale", () => {
    const offenders: string[] = [];
    for (const file of CONSOLE_FILES) {
      const source = fs.readFileSync(file, "utf8");
      // No arbitrary pixel type, no ad-hoc clamp, no retired micro step.
      if (/text-\[\d+(?:\.\d+)?px\]/.test(source)) offenders.push(path.relative(srcRoot, file));
      if (/text-\[clamp\(/.test(source)) offenders.push(path.relative(srcRoot, file));
      if (/text-bp-micro/.test(source)) offenders.push(path.relative(srcRoot, file));
    }
    expect(offenders).toEqual([]);
  });

  it("still ships the desktop scale for the screens that migrate one by one", () => {
    for (const token of ["micro", "meta", "label", "body", "lead", "title", "display", "hero"]) {
      expect(css).toContain(`--text-${token}:`);
    }
    // 11.5 px is the dominant metadata size in the desktop audit; the token keeps it.
    expect(css).toMatch(/--text-meta:\s*11\.5px;/);
  });

  it("keeps core UI primitives and shell components free of arbitrary text-[Npx] sizes", () => {
    const dirs = [
      path.join(srcRoot, "components", "ui"),
      path.join(srcRoot, "components", "shell"),
    ];
    const extraFiles = [
      path.join(srcRoot, "components", "page.tsx"),
      path.join(srcRoot, "components", "public-page.tsx"),
      path.join(srcRoot, "components", "auth-layout.tsx"),
      path.join(srcRoot, "components", "game", "game-card.tsx"),
    ];
    const files = [
      ...dirs.flatMap((dir) =>
        fs
          .readdirSync(dir)
          .filter((file) => file.endsWith(".tsx") && !file.endsWith(".test.tsx"))
          .map((file) => path.join(dir, file)),
      ),
      ...extraFiles,
    ];
    const offenders: string[] = [];
    for (const file of files) {
      const source = fs.readFileSync(file, "utf8");
      if (/text-\[\d+(?:\.\d+)?px\]/.test(source)) {
        offenders.push(path.relative(srcRoot, file));
      }
    }
    expect(offenders).toEqual([]);
  });
});
