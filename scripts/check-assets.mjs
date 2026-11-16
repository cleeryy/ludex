#!/usr/bin/env node
/**
 * Documentation link and asset check.
 *
 * The README is the front door: a broken image or a link to a file that was
 * renamed is worse than a missing paragraph. This script walks every Markdown
 * file in the repository and resolves:
 *
 *   - `![alt](path)` and `<img src="path">` — must exist on disk;
 *   - `[text](path.md)` — must exist on disk (URLs and `#anchors` are skipped);
 *   - every screenshot listed in `docs/assets/screenshots/manifest.json` must
 *     exist and be referenced by at least one document.
 *
 * It also enforces the image budgets from #265 so a hand-added 4 MB PNG fails
 * review instead of shipping.
 *
 *   node scripts/check-assets.mjs
 *   node scripts/check-assets.mjs --verbose
 *
 * No dependencies: it runs on a stock Node 22 checkout, before `pnpm install`.
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const verbose = process.argv.includes("--verbose");

const SKIP_DIRS = new Set([
  ".git",
  ".github/ISSUE_TEMPLATE",
  "node_modules",
  "target",
  "dist",
  "playwright-report",
  "test-results",
  ".dev",
  ".opencode",
  "vendor",
]);

/** Budgets from #265. */
const BUDGETS = [
  { pattern: /^docs\/assets\/screenshots\/mobile\//, limit: 120 * 1024, label: "mobile screenshot" },
  { pattern: /^docs\/assets\/screenshots\//, limit: 300 * 1024, label: "screenshot" },
  { pattern: /^docs\/assets\//, limit: 400 * 1024, label: "artwork" },
];

const problems = [];
const warnings = [];

function walk(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".") && entry.name !== ".github") continue;
    const full = join(dir, entry.name);
    const rel = relative(ROOT, full).split(sep).join("/");
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(rel) || SKIP_DIRS.has(entry.name)) continue;
      walk(full, out);
    } else if (extname(entry.name) === ".md") {
      out.push(full);
    }
  }
  return out;
}

/** Markdown fences hold examples, not references. */
function stripFences(source) {
  return source.replace(/^```[\s\S]*?^```/gm, "");
}

function isExternal(target) {
  return (
    /^[a-z][a-z0-9+.-]*:/i.test(target) ||
    target.startsWith("#") ||
    target.startsWith("//")
  );
}

/** `path#anchor` and `path?query` both resolve to the file. */
function localPath(target) {
  const withoutFragment = target.split("#")[0].replace(/\?.*$/, "");
  return decodeURIComponent(withoutFragment);
}

const files = walk(ROOT).sort();
const referenced = new Set();
const documents = [];

for (const file of files) {
  const rel = relative(ROOT, file).split(sep).join("/");
  const source = stripFences(readFileSync(file, "utf8"));
  const refs = [];

  for (const match of source.matchAll(/!\[[^\]]*\]\(\s*<?([^)>\s]+)>?(?:\s+"[^"]*")?\s*\)/g)) {
    refs.push({ target: match[1], kind: "image" });
  }
  for (const match of source.matchAll(/<img\b[^>]*?\bsrc=\s*["']([^"']+)["']/gi)) {
    refs.push({ target: match[1], kind: "image" });
  }
  // `<picture>` sources: the dark/light variants of a README image.
  for (const match of source.matchAll(/<source\b[^>]*?\bsrcset=\s*["']([^"']+)["']/gi)) {
    refs.push({ target: match[1], kind: "image" });
  }
  for (const match of source.matchAll(/(?<!!)\[[^\]]*\]\(\s*<?([^)>\s]+)>?(?:\s+"[^"]*")?\s*\)/g)) {
    refs.push({ target: match[1], kind: "link" });
  }

  for (const { target, kind } of refs) {
    if (isExternal(target)) continue;
    const clean = localPath(target);
    if (!clean) continue;
    const absolute = resolve(dirname(file), clean);
    if (!absolute.startsWith(ROOT)) {
      problems.push(`${rel}: ${target} points outside the repository`);
      continue;
    }
    const exists = statSync(absolute, { throwIfNoEntry: false });
    if (!exists) {
      problems.push(`${rel}: ${kind === "image" ? "image" : "link"} target missing — ${target}`);
      continue;
    }
    const targetRel = relative(ROOT, absolute).split(sep).join("/");
    referenced.add(targetRel);
    if (verbose) documents.push(`${rel} -> ${targetRel}`);
  }
}

// Screenshot manifest: every listed file exists, is referenced, and fits.
const manifestPath = join(ROOT, "docs", "assets", "screenshots", "manifest.json");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
for (const shot of manifest.shots ?? []) {
  const absolute = join(ROOT, shot.path);
  const stats = statSync(absolute, { throwIfNoEntry: false });
  if (!stats) {
    problems.push(`docs/assets/screenshots/manifest.json: ${shot.path} is listed but not on disk`);
    continue;
  }
  if (!referenced.has(shot.path)) {
    warnings.push(`${shot.path} is captured but no document uses it`);
  }
  if (stats.size > 300 * 1024) {
    problems.push(`${shot.path} is ${(stats.size / 1024).toFixed(0)} KB (budget 300 KB)`);
  }
}

// Every committed image under docs/assets stays inside its budget.
const assetRoot = join(ROOT, "docs", "assets");
const walkAssets = (dir) => {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    const rel = relative(ROOT, full).split(sep).join("/");
    if (entry.isDirectory()) {
      walkAssets(full);
      continue;
    }
    if (!/\.(png|jpe?g|gif|webp|svg)$/i.test(entry.name)) continue;
    const size = statSync(full).size;
    for (const budget of BUDGETS) {
      if (budget.pattern.test(rel) && size > budget.limit) {
        problems.push(`${rel} is ${(size / 1024).toFixed(0)} KB, over the ${budget.label} budget (${budget.limit / 1024} KB)`);
        break;
      }
    }
  }
};
walkAssets(assetRoot);

const totalScreenshots = (manifest.shots ?? []).reduce((sum, shot) => {
  const stats = statSync(join(ROOT, shot.path), { throwIfNoEntry: false });
  return sum + (stats?.size ?? 0);
}, 0);

if (verbose) {
  process.stdout.write(`${documents.join("\n")}\n`);
}

for (const warning of warnings) process.stdout.write(`warning: ${warning}\n`);

if (problems.length > 0) {
  process.stderr.write(`\n✗ ${problems.length} documentation problem(s):\n`);
  for (const problem of problems) process.stderr.write(` - ${problem}\n`);
  process.stderr.write("\nRun `pnpm screenshots` to regenerate the screenshots, or fix the path.\n");
  process.exit(1);
}

process.stdout.write(
  `✓ ${files.length} Markdown files, ${referenced.size} local targets, ` +
    `${(manifest.shots ?? []).length} screenshots (${(totalScreenshots / 1024 / 1024).toFixed(2)} MB), ` +
    `all references resolve\n`,
);
