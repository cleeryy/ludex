#!/usr/bin/env node
/**
 * Ludex web i18n catalog tool.
 *
 * English is the source language: the message id is the English copy, so
 * `en.json` is the identity catalog extracted from the source. Locale files
 * (`fr.json`, `de.json`, …) are flat maps keyed by those ids.
 *
 *   node scripts/i18n.mjs            # check parity and freshness (CI)
 *   node scripts/i18n.mjs --write    # regenerate en.json + coverage.json
 *   node scripts/i18n.mjs --strict   # also require 100% on community locales
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const webRoot = path.resolve(here, "..");
const srcRoot = path.join(webRoot, "src");
const localesDir = path.join(srcRoot, "locales");
const write = process.argv.includes("--write");
const strict = process.argv.includes("--strict");

const manifestPath = path.join(localesDir, "manifest.json");
const manifest = readJson(manifestPath);
if (!Array.isArray(manifest) || manifest.length === 0) {
  console.error("i18n: src/locales/manifest.json is missing or empty");
  process.exit(1);
}
const LOCALES = manifest.map((locale) => locale.code);
const COMPLETE_LOCALES = new Set(manifest.filter((locale) => locale.status === "official").map((locale) => locale.code));

function walk(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return walk(full);
    return /\.(ts|tsx)$/.test(entry.name) ? [full] : [];
  });
}

function unescape(raw) {
  return JSON.parse(`"${raw}"`);
}

/** Every message id must appear literally inside t(), tn() or tr(). */
function collectKeys() {
  const keys = new Set();
  for (const file of walk(srcRoot)) {
    if (file.includes(`${path.sep}locales${path.sep}`)) continue;
    const source = fs
      .readFileSync(file, "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, "");
    for (const match of source.matchAll(/\bt\(\s*"((?:\\.|[^"\\])*)"\s*[,)]/g)) {
      keys.add(unescape(match[1]));
    }
    for (const match of source.matchAll(/\btr\(\s*"((?:\\.|[^"\\])*)"\s*,/g)) {
      keys.add(unescape(match[1]));
    }
    for (const match of source.matchAll(
      /\btn\(\s*[^,]+,\s*"((?:\\.|[^"\\])*)"\s*,\s*"((?:\\.|[^"\\])*)"/g,
    )) {
      keys.add(unescape(match[1]));
      keys.add(unescape(match[2]));
    }
  }
  return [...keys].sort((a, b) => a.localeCompare(b));
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return null;
  }
}

function placeholders(value) {
  return [...value.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort().join(",");
}

function baseKey(key) {
  const pipe = key.indexOf("|");
  return pipe === -1 ? key : key.slice(0, pipe);
}

const keys = collectKeys();
const keySet = new Set(keys);
const errors = [];
const warnings = [];

if (keys.length === 0) {
  console.error("i18n: no message ids found — did the extraction regexp break?");
  process.exit(1);
}

const catalogPath = path.join(localesDir, "en.json");
const existingCatalog = readJson(catalogPath) ?? {};
const catalogMissing = keys.filter((key) => !(key in existingCatalog));
const catalogStale = Object.keys(existingCatalog).filter((key) => !keySet.has(key));
if (catalogMissing.length) errors.push(`en.json is missing ${catalogMissing.length} message id(s): ${catalogMissing.slice(0, 8).map((k) => JSON.stringify(k)).join(", ")}`);
if (catalogStale.length) errors.push(`en.json has ${catalogStale.length} stale id(s) not present in source: ${catalogStale.slice(0, 8).map((k) => JSON.stringify(k)).join(", ")}`);

const english = {};
for (const key of keys) english[key] = key;

const coverage = {};
for (const code of LOCALES) {
  if (code === "en") {
    coverage.en = { translated: keys.length, total: keys.length, percent: 100 };
    continue;
  }
  const file = path.join(localesDir, `${code}.json`);
  const messages = readJson(file);
  if (!messages) {
    errors.push(`${code}.json is missing or not valid JSON`);
    coverage[code] = { translated: 0, total: keys.length, percent: 0 };
    continue;
  }
  const translated = keys.filter((key) => typeof messages[key] === "string" && messages[key].trim() !== "");
  coverage[code] = {
    translated: translated.length,
    total: keys.length,
    percent: keys.length === 0 ? 100 : Math.round((translated.length / keys.length) * 1000) / 10,
  };

  for (const [key, value] of Object.entries(messages)) {
    if (!keySet.has(baseKey(key))) {
      errors.push(`${code}.json: unknown message id ${JSON.stringify(key)}`);
      continue;
    }
    if (typeof value !== "string" || value.trim() === "") {
      errors.push(`${code}.json: empty translation for ${JSON.stringify(key)}`);
      continue;
    }
    const source = existingCatalog[baseKey(key)];
    if (source && placeholders(source) !== placeholders(value)) {
      errors.push(
        `${code}.json: placeholder mismatch for ${JSON.stringify(key)} (en: {${placeholders(source)}} / ${code}: {${placeholders(value)}})`,
      );
    }
  }

  const missing = keys.filter((key) => !(key in messages));
  if (COMPLETE_LOCALES.has(code) && missing.length) {
    errors.push(
      `${code}.json: ${missing.length} missing translation(s): ${missing.slice(0, 8).map((k) => JSON.stringify(k)).join(", ")}`,
    );
  }
  if (strict && missing.length) {
    errors.push(`${code}.json: ${missing.length} missing translation(s) (--strict)`);
  }
  if (!COMPLETE_LOCALES.has(code) && coverage[code].percent < 10) {
    warnings.push(`${code}.json covers only ${coverage[code].percent}% of the catalog; it falls back to English`);
  }
}

const coveragePath = path.join(localesDir, "coverage.json");
const existingCoverage = readJson(coveragePath);
if (JSON.stringify(existingCoverage) !== JSON.stringify(coverage)) {
  if (write) {
    fs.writeFileSync(coveragePath, `${JSON.stringify(coverage, null, 2)}\n`);
  } else {
    errors.push("coverage.json is stale — run `pnpm i18n:write`");
  }
}

if (write) {
  fs.writeFileSync(catalogPath, `${JSON.stringify(english, null, 2)}\n`);
  if (errors.length) {
    // A regeneration always resolves catalog freshness; re-check translations
    // on the next run so this command stays useful while French is filled in.
    console.log("i18n: catalog regenerated; translation gaps remain:");
    for (const message of errors) console.log(`  - ${message}`);
  }
  console.table(coverage);
  process.exit(0);
}

console.table(coverage);
for (const message of warnings) console.warn(`warning: ${message}`);
if (errors.length) {
  console.error(`\ni18n check failed (${errors.length}):`);
  for (const message of errors) console.error(`  - ${message}`);
  console.error("\nRun `pnpm i18n:write` after changing source copy.");
  process.exit(1);
}
console.log(`i18n: ${keys.length} message ids, parity OK (${LOCALES.join(", ")})`);
