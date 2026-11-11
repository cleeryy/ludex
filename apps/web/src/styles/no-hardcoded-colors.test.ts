import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Hardcoded-colour guard (#140). Components must paint with theme tokens only;
 * literal colours are allowed exclusively in `src/theme/` and in the generated
 * theme stylesheet. Comments are stripped first so issue references such as
 * `#140` are not mistaken for colours.
 */

const webRoot = new URL("../../", import.meta.url).pathname;
const COLOR_PATTERN = /#[0-9a-fA-F]{3,8}\b|\brgba?\([^)]*\)|\bhsla?\([^)]*\)|\b(?:bg|text|border|ring|from|via|to|fill|stroke|outline|decoration|caret|divide|shadow)-(?:white|black)(?:\/\d+)?\b/g;

const COMPONENT_ROOTS = ["src/components", "src/screens", "src/state"];
const EXTRA_SOURCES = ["src/app.tsx", "src/main.tsx"];
const ALLOWED = new Set(["src/styles/themes.generated.css"]);

function walk(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return walk(full);
    return /\.(ts|tsx|css)$/.test(entry.name) ? [full] : [];
  });
}

function relative(file: string): string {
  return path.relative(webRoot, file).split(path.sep).join("/");
}

export function colorLiterals(source: string): string[] {
  const stripped = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
  return [...new Set(stripped.match(COLOR_PATTERN) ?? [])];
}

function componentSources(): string[] {
  const files = COMPONENT_ROOTS.flatMap((root) => walk(path.join(webRoot, root)));
  files.push(...EXTRA_SOURCES.map((file) => path.join(webRoot, file)));
  files.push(...walk(path.join(webRoot, "src/styles")).filter((file) => /\.css$/.test(file)));
  return files.filter((file) => fs.existsSync(file) && !ALLOWED.has(relative(file)));
}

describe("hardcoded colours", () => {
  it("the detector recognises hex, rgb(), hsl() and Tailwind's white/black", () => {
    expect(colorLiterals("color: #ffc61a; background: rgba(0,0,0,.5); border: 1px solid hsl(0 0% 0%)")).toEqual([
      "#ffc61a",
      "rgba(0,0,0,.5)",
      "hsl(0 0% 0%)",
    ]);
    expect(colorLiterals("bg-black/50 text-white border-white/30 whitespace-nowrap")).toEqual([
      "bg-black/50",
      "text-white",
      "border-white/30",
    ]);
  });

  it("ignores issue references in comments", () => {
    expect(colorLiterals("// issue #140\n/* see #129 */\nconst a = 1;")).toEqual([]);
  });

  it("keeps component sources free of literal colours", () => {
    const offenders: string[] = [];
    for (const file of componentSources()) {
      const found = colorLiterals(fs.readFileSync(file, "utf8"));
      if (found.length > 0) offenders.push(`${relative(file)}: ${found.join(", ")}`);
    }
    expect(offenders).toEqual([]);
  });

  it("allows literals only in the theme registry and generated stylesheet", () => {
    expect(fs.existsSync(path.join(webRoot, "src/theme/themes.json"))).toBe(true);
    expect(fs.existsSync(path.join(webRoot, "src/theme/art-palettes.json"))).toBe(true);
    expect(fs.existsSync(path.join(webRoot, "src/styles/themes.generated.css"))).toBe(true);
  });
});
