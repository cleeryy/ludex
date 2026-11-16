import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { HOME_ITEM_IDS } from "../components/big-picture/system-zones";

/* --------------------------------------------------------------------------
   Cabinet anti-slop (#268).

   Every rule that matters is a test, because a rule without one is gone in three
   releases. These assertions cover the kill list across the whole console tree —
   the shared components, the `/console` shell, its surfaces and its gates: no
   Soft preset or pastel accents, no six-tile grid or decorative indexes, no
   layout picker, no uppercase mono words, no second focus ring, one hint strip,
   no toasts and no outside icon set inside the console chrome.
   -------------------------------------------------------------------------- */

const srcRoot = new URL("..", import.meta.url).pathname;
const bigPictureDir = path.join(srcRoot, "components", "big-picture");
const consoleDir = path.join(srcRoot, "components", "console");

function tree(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return tree(full);
    // Tests are not product code: the toast-boundary test imports the desktop
    // provider on purpose, to prove it cannot paint inside Console.
    if (/\.test\.(ts|tsx)$/.test(entry.name)) return [];
    return /\.(ts|tsx)$/.test(entry.name) ? [full] : [];
  });
}

const consoleSources = [
  ...fs
    .readdirSync(bigPictureDir)
    .filter((file) => /\.(ts|tsx)$/.test(file) && !file.includes(".test."))
    .map((file) => path.join(bigPictureDir, file)),
  ...tree(consoleDir),
];
const componentTsx = consoleSources.filter((file) => file.endsWith(".tsx"));
const shell = fs.readFileSync(path.join(consoleDir, "shell", "console-shell.tsx"), "utf8");
const css = fs.readFileSync(path.join(srcRoot, "styles", "index.css"), "utf8");

function code(file: string): string {
  return fs
    .readFileSync(file, "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
}

function offenders(rule: (source: string) => boolean): string[] {
  return consoleSources.filter((file) => rule(code(file))).map((file) => path.relative(srcRoot, file));
}

describe("the kill list", () => {
  it("has no Soft preset, no curated pastel accent and no appearance branch", () => {
    expect(fs.existsSync(path.join(srcRoot, "theme", "console-appearance.ts"))).toBe(false);
    expect(fs.existsSync(path.join(bigPictureDir, "use-console-appearance.ts"))).toBe(false);
    for (const file of consoleSources) {
      const source = code(file);
      expect(source).not.toMatch(/CONSOLE_PRESETS|CONSOLE_ACCENT_IDS|data-console-appearance/);
      expect(source).not.toMatch(/"soft"\s*\|\s*"studio"|preset:\s*"soft"/);
      for (const pastel of ["coral", "mint", "sky", "lilac", "sun"]) {
        // A pastel hex or a curated accent id is slop; the words themselves may
        // appear in a comment about their removal.
        expect(source).not.toMatch(new RegExp(`#ffc2b8|#93e3b6|#a6d2ff|#dccbff|${pastel}:\\s*\\{`));
      }
    }
    expect(css).not.toContain("data-console-appearance");
    expect(css).not.toMatch(/coral|mint|lilac/);
  });

  it("keeps five destinations in the band and no six-tile grid", () => {
    expect(HOME_ITEM_IDS).toHaveLength(5);
    expect(offenders((source) => /grid-cols-6|lg:grid-cols-6/.test(source))).toEqual([]);
    expect(shell).not.toContain("HOME_ITEM_IDS.map");
  });

  it("has no decorative index", () => {
    expect(offenders((source) => /padStart\(2,\s*"0"\)/.test(source))).toEqual([]);
    expect(offenders((source) => /String\(index \+ 1\)/.test(source))).toEqual([]);
  });

  it("has no layout picker", () => {
    for (const retired of [
      "layout-switcher",
      "layout-preview",
      "layout-panel",
      "layout-dock",
      "layout-hero",
      "layout-spotlight",
    ]) {
      expect(fs.existsSync(path.join(bigPictureDir, `${retired}.tsx`))).toBe(false);
    }
    expect(shell).not.toMatch(/LayoutPicker|pickerOpen|openPicker/);
    expect(fs.readFileSync(path.join(consoleDir, "surfaces", "console-dialogs.tsx"), "utf8")).not.toContain(
      "LayoutPicker",
    );
  });

  it("uses no uppercase mono words", () => {
    expect(offenders((source) => /uppercase/.test(source))).toEqual([]);
  });

  it("keeps mono for numerals and units, not for labels", () => {
    const offendersList = componentTsx.filter((file) => {
      const lines = code(file).split("\n");
      return lines.some((line) => /font-mono/.test(line) && !/\bnum\b|text-bp|%"|\$\{|title=/.test(line));
    });
    expect(offendersList.map((file) => path.relative(srcRoot, file))).toEqual([]);
  });

  it("never goes below the 24u type floor", () => {
    expect(offenders((source) => /text-bp-micro|text-\[\d+(?:\.\d+)?px\]|text-\[clamp\(/.test(source))).toEqual(
      [],
    );
  });

  it("draws one bracket per object and no second ring", () => {
    expect(offenders((source) => /ring-2\b|role="button"/.test(source))).toEqual([]);
    expect(offenders((source) => /data-bp-zone-underline/.test(source))).toEqual([]);
    const bracketUsers = componentTsx.filter((file) => code(file).includes("<Bracket"));
    expect(bracketUsers.map((file) => path.basename(file)).sort()).toEqual(["parts.tsx"]);
  });

  it("keeps one hint strip: the footer hides while a sheet or the plate is up", () => {
    const strips = componentTsx.filter((file) => code(file).includes("<HintStrip"));
    expect(strips.map((file) => path.basename(file)).sort()).toEqual(["overlays.tsx", "parts.tsx"]);
    expect(shell).toContain("const footerVisible = !overlay && !pendingPower && !launchPlate;");
    expect(shell).toContain("{footerVisible && (");
    expect(fs.readFileSync(path.join(bigPictureDir, "parts.tsx"), "utf8")).toContain(".slice(0, 3)");
  });

  it("has no toast inside Console", () => {
    expect(offenders((source) => /useToast|from "@\/state\/toasts"/.test(source))).toEqual([]);
    expect(shell).toContain("useConsoleNotice()");
  });

  it("uses the drawn glyph set, not an outside icon library", () => {
    expect(offenders((source) => /from "lucide-react"/.test(source))).toEqual([]);
    const glyphs = fs.readFileSync(path.join(bigPictureDir, "glyphs.tsx"), "utf8");
    expect(glyphs).toContain("strokeWidth={2}");
  });

  it("keeps one blurred wallpaper layer and no per-panel glass", () => {
    expect(css).not.toContain("bp-glass-tile");
    expect(css).not.toContain(".bp-home");
    expect(css).toContain(".bp-backdrop-art");
    expect(css).toContain("--bp-wall-opacity: 0.4;");
  });

  it("keeps the wordmark out of the bar and the monogram in", () => {
    const bar = fs.readFileSync(path.join(bigPictureDir, "system-bar.tsx"), "utf8");
    expect(bar).not.toContain("<Wordmark");
    expect(bar).toContain("<Monogram");
    expect(bar).not.toMatch(/tracking-\[0\.18em\]/);
  });

  it("keeps the stage composition: stage, rail and band, no KPI grid", () => {
    const stage = fs.readFileSync(path.join(bigPictureDir, "layout-shelf.tsx"), "utf8");
    expect(stage).toContain("bp-stage");
    expect(stage).toContain("bp-rail");
    expect(fs.readFileSync(path.join(bigPictureDir, "home-row.tsx"), "utf8")).toContain("bp-band");
    expect(shell).toContain("<HomeBand");
  });
});
