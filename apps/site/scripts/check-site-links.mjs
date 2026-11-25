#!/usr/bin/env node
/**
 * Static link and asset checker for the built Next.js export (`apps/site/out`).
 *
 * Verifies every internal `href` and `src` in exported HTML pages resolves to a
 * real file or directory `index.html` under the configured `BASE_PATH`.
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SITE_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = join(SITE_DIR, "out");
const rawBasePath = process.env.BASE_PATH ?? "/ludex";
const BASE_PATH = rawBasePath === "/" ? "" : rawBasePath.replace(/\/+$/, "");

if (!existsSync(OUT_DIR)) {
  console.error("error: apps/site/out does not exist; run `pnpm --filter @ludex/site build` first.");
  process.exit(1);
}

function walkHtml(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walkHtml(full, out);
    else if (entry.name.endsWith(".html")) out.push(full);
  }
  return out;
}

function resolvesInOut(urlPath) {
  let clean = decodeURIComponent(urlPath.split("#")[0].split("?")[0]);
  if (BASE_PATH) {
    if (clean === BASE_PATH) {
      clean = "/";
    } else if (clean.startsWith(`${BASE_PATH}/`)) {
      clean = clean.slice(BASE_PATH.length);
    } else {
      return false;
    }
  }
  const rel = clean.replace(/^\/+/, "");
  const exact = join(OUT_DIR, rel);
  if (existsSync(exact) && statSync(exact).isFile()) return true;
  const withIndex = join(OUT_DIR, rel, "index.html");
  if (existsSync(withIndex) && statSync(withIndex).isFile()) return true;
  const withHtml = join(OUT_DIR, `${rel.replace(/\/+$/, "")}.html`);
  if (rel && existsSync(withHtml) && statSync(withHtml).isFile()) return true;
  return false;
}

const htmlFiles = walkHtml(OUT_DIR).sort();
const broken = [];
let checked = 0;

for (const file of htmlFiles) {
  const relFile = relative(OUT_DIR, file);
  const html = readFileSync(file, "utf8");

  for (const match of html.matchAll(/\b(?:href|src)="([^"]+)"/g)) {
    const target = match[1];
    // Only validate root-relative internal paths; skip external URLs, anchors,
    // protocol-relative links, and any scheme-prefixed URIs.
    if (!target || !target.startsWith("/") || target.startsWith("//")) {
      continue;
    }
    checked += 1;
    if (!resolvesInOut(target)) {
      broken.push(`${relFile}: broken target "${target}"`);
    }
  }
}

if (broken.length > 0) {
  console.error(`✗ ${broken.length} broken internal link(s)/asset(s) in apps/site/out:`);
  for (const line of broken.slice(0, 50)) {
    console.error(` - ${line}`);
  }
  if (broken.length > 50) {
    console.error(` ... and ${broken.length - 50} more`);
  }
  process.exit(1);
}

console.log(
  `✓ ${htmlFiles.length} HTML page(s), ${checked} internal link(s)/asset(s) verified under basePath=${BASE_PATH || "/"}`,
);
