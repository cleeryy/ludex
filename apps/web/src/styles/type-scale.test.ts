import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

/* --------------------------------------------------------------------------
   Console type scale (#237).

   The console grew its geometry with `--bp-u` but kept 9.5–13 px type, so a
   4K panel showed the same label inside a tile twice as tall. These assertions
   pin the scale to the 10-foot rule (28 px for primary metadata at 1080p),
   keep every console surface on it, and stop a new arbitrary `text-[11px]`
   from creeping back in.
   -------------------------------------------------------------------------- */

const srcRoot = new URL("..", import.meta.url).pathname;
const css = fs.readFileSync(path.join(srcRoot, "styles", "index.css"), "utf8");

const CONSOLE_FILES = [
  ...fs
    .readdirSync(path.join(srcRoot, "components", "big-picture"))
    .filter((file) => file.endsWith(".tsx"))
    .map((file) => path.join(srcRoot, "components", "big-picture", file)),
  path.join(srcRoot, "screens", "big-picture.tsx"),
];

/** `--bp-text-*: calc(var(--bp-u) * N)` → N, in 1080p pixels. */
function step(name: string): number {
  const match = new RegExp(`--bp-text-${name}:\\s*calc\\(var\\(--bp-u\\) \\* (\\d+(?:\\.\\d+)?)\\)`).exec(css);
  expect(match, `--bp-text-${name} must be expressed in --bp-u`).not.toBeNull();
  return Number(match?.[1]);
}

describe("console type scale", () => {
  it("sets 28 px for primary metadata and 48 px for a hero at 1080p", () => {
    // --bp-u is 1.0003 px at 1920, so the multiplier is the 1080p pixel size.
    expect(step("body")).toBe(28);
    expect(step("hero")).toBe(48);
    expect(step("title")).toBe(34);
    expect(step("meta")).toBe(24);
    expect(step("micro")).toBe(20);
  });

  it("keeps the scale ordered and above the 20 px floor", () => {
    const micro = step("micro");
    const meta = step("meta");
    const body = step("body");
    const title = step("title");
    const hero = step("hero");
    expect(micro).toBeLessThan(meta);
    expect(meta).toBeLessThan(body);
    expect(body).toBeLessThan(title);
    expect(title).toBeLessThan(hero);
    expect(micro).toBeGreaterThanOrEqual(20);
  });

  it("exposes one utility per step, bound to the console unit", () => {
    for (const name of ["hero", "title", "body", "meta", "micro"]) {
      expect(css).toContain(`--text-bp-${name}: var(--bp-text-${name});`);
    }
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
      if (/text-\[\d+(?:\.\d+)?px\]/.test(source)) {
        offenders.push(path.relative(srcRoot, file));
      }
    }
    expect(offenders).toEqual([]);
  });

  it("still ships the desktop scale for the screens that migrate one by one", () => {
    for (const token of ["micro", "meta", "label", "body", "lead", "title", "display", "hero"]) {
      expect(css).toContain(`--text-${token}:`);
    }
    // 11.5 px is the dominant metadata size in the audit; the token keeps it.
    expect(css).toMatch(/--text-meta:\s*11\.5px;/);
  });
});
