#!/usr/bin/env node
/**
 * Ludex web i18n catalog tool.
 *
 * English is the source language: the message id is the English copy, so
 * `en.json` is the identity catalog extracted from the source. Locale files
 * (`fr.json`, `de.json`, …) are flat maps keyed by those ids.
 *
 *   node scripts/i18n.mjs              # gate: parity, coverage, RTL, plurals, pseudo
 *   node scripts/i18n.mjs --write      # regenerate en.json + coverage.json
 *   node scripts/i18n.mjs --coverage   # refresh coverage.json only
 *   node scripts/i18n.mjs --strict     # also require 100 % from shipped top-10 files
 *   node scripts/i18n.mjs --hardcoded  # print hardcoded English strings (advisory)
 *   node scripts/i18n.mjs --pseudo     # write the en-XA catalog for inspection
 *
 * The registry (`src/locales/manifest.json`) is the single source of truth for
 * which locales exist, how they are labelled, their direction and their
 * coverage target. A file that is not on disk is *not shipped*: it is skipped
 * by the gate and hidden from the picker, so adding a locale is a JSON file
 * plus a registry entry, nothing else.
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { formatFinding, lintHardcodedStrings } from "./i18n-lint.mjs";
import { PSEUDO_CLOSE, PSEUDO_OPEN, pseudoCatalog, pseudoText } from "./i18n-pseudo.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const webRoot = path.resolve(here, "..");
const srcRoot = path.join(webRoot, "src");
const localesDir = path.join(srcRoot, "locales");
const flags = new Set(
  process.argv
    .slice(2)
    .filter((arg) => arg.startsWith("--"))
    .map((arg) => arg.slice(2)),
);
const write = flags.has("write");
const coverageOnly = flags.has("coverage");
const strict = flags.has("strict");
const pseudoOnly = flags.has("pseudo");
const hardcodedOnly = flags.has("hardcoded");

/** Locales written right to left, by base language. Mirrored in index.html. */
const RTL_BASES = new Set(["ar", "he", "fa", "ur"]);
/** The top-10 target set from #139, minus the English source. */
const REQUIRED_TOP10 = ["zh-CN", "ru", "es", "pt-BR", "de", "fr", "ja", "ko", "pl", "tr"];
/** A locale below this coverage stays out of the picker (falls back to English). */
const SELECTABLE_PERCENT = 95;
/** A user-visible string shorter than this is never treated as copy. */
const MIN_COPY_LENGTH = 3;

const errors = [];
const warnings = [];

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return null;
  }
}

function readText(file) {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return null;
  }
}

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

function placeholders(value) {
  return [...value.matchAll(/\{(\w+)\}/g)]
    .map((match) => match[1])
    .sort()
    .join(",");
}

function baseKey(key) {
  const pipe = key.indexOf("|");
  return pipe === -1 ? key : key.slice(0, pipe);
}

function pluralSuffix(key) {
  const pipe = key.indexOf("|");
  return pipe === -1 ? null : key.slice(pipe + 1);
}

/** Counts top-level ids in a pretty-printed catalog to catch duplicates. */
function duplicateIds(raw) {
  const seen = new Map();
  for (const match of raw.matchAll(/^ {2}"((?:\\.|[^"\\])*)":/gm)) {
    const key = match[1];
    seen.set(key, (seen.get(key) ?? 0) + 1);
  }
  return [...seen.entries()].filter(([, count]) => count > 1).map(([key]) => key);
}

function intlCategories(intl) {
  try {
    return new Set(new Intl.PluralRules(intl).resolvedOptions().pluralCategories);
  } catch {
    return null;
  }
}

/* ------------------------------ registry -------------------------------- */

const manifestPath = path.join(localesDir, "manifest.json");
const manifest = readJson(manifestPath);
if (!Array.isArray(manifest) || manifest.length === 0) {
  console.error("i18n: src/locales/manifest.json is missing or empty");
  process.exit(1);
}

const VALID_STATUS = new Set(["official", "preview", "pseudo"]);
const VALID_TIERS = new Set(["source", "top10", "extended", "dev"]);
const VALID_DIRS = new Set(["ltr", "rtl"]);
const CODE_PATTERN = /^[a-z]{2,3}(-[A-Za-z0-9]{2,8})*$/;

const seenCodes = new Set();
for (const entry of manifest) {
  const label = typeof entry?.code === "string" ? entry.code : JSON.stringify(entry);
  if (typeof entry?.code !== "string" || !CODE_PATTERN.test(entry.code)) {
    errors.push(`manifest.json: ${label} is not a BCP-47 locale code`);
    continue;
  }
  if (seenCodes.has(entry.code)) errors.push(`manifest.json: duplicate locale code ${entry.code}`);
  seenCodes.add(entry.code);
  for (const field of ["label", "englishLabel", "intl"]) {
    if (typeof entry[field] !== "string" || entry[field].trim() === "") {
      errors.push(`manifest.json: ${entry.code} is missing ${field}`);
    }
  }
  if (!VALID_DIRS.has(entry.dir)) errors.push(`manifest.json: ${entry.code} has direction ${JSON.stringify(entry.dir)}`);
  const expectedDir = RTL_BASES.has(entry.code.split("-")[0].toLowerCase()) ? "rtl" : "ltr";
  if (entry.dir !== expectedDir) {
    errors.push(`manifest.json: ${entry.code} is ${entry.dir}, expected ${expectedDir} (RTL script list)`);
  }
  if (!VALID_STATUS.has(entry.status)) errors.push(`manifest.json: ${entry.code} has status ${JSON.stringify(entry.status)}`);
  if (!VALID_TIERS.has(entry.tier)) errors.push(`manifest.json: ${entry.code} has tier ${JSON.stringify(entry.tier)}`);
  if (typeof entry.reviewed !== "boolean") errors.push(`manifest.json: ${entry.code} is missing reviewed`);
  if (entry.status === "pseudo") {
    if (entry.file !== null) errors.push(`manifest.json: ${entry.code} is a pseudo-locale and must not ship a file`);
  } else if (entry.file !== `${entry.code}.json`) {
    errors.push(`manifest.json: ${entry.code} must point at ${entry.code}.json, found ${JSON.stringify(entry.file)}`);
  }
  if (typeof entry.intl === "string" && Intl.NumberFormat.supportedLocalesOf([entry.intl]).length === 0) {
    errors.push(`manifest.json: ${entry.code} has an invalid Intl tag ${JSON.stringify(entry.intl)}`);
  }
}

const source = manifest.find((entry) => entry.code === "en");
if (!source) {
  errors.push("manifest.json: the English source locale (en) is missing");
} else {
  if (source.status !== "official") errors.push("manifest.json: en must stay official");
  if (source.tier !== "source") errors.push("manifest.json: en must stay the source tier");
}
for (const code of REQUIRED_TOP10) {
  const entry = manifest.find((candidate) => candidate.code === code);
  if (!entry) errors.push(`manifest.json: top-10 locale ${code} is not registered`);
  else if (entry.tier !== "top10") errors.push(`manifest.json: ${code} must be tier top10`);
}

/* ------------------------------ the catalog ------------------------------ */

/** Every message id must appear literally inside t(), tn() or tr(). */
function collectCatalog() {
  const keys = new Set();
  const pluralIds = new Set();
  for (const file of walk(srcRoot)) {
    if (file.includes(`${path.sep}locales${path.sep}`)) continue;
    const sourceText = fs
      .readFileSync(file, "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, "");
    for (const match of sourceText.matchAll(/\bt\(\s*"((?:\\.|[^"\\])*)"\s*[,)]/g)) {
      keys.add(unescape(match[1]));
    }
    for (const match of sourceText.matchAll(/\btr\(\s*"((?:\\.|[^"\\])*)"\s*,/g)) {
      keys.add(unescape(match[1]));
    }
    for (const match of sourceText.matchAll(
      /\btn\(\s*[^,]+,\s*"((?:\\.|[^"\\])*)"\s*,\s*"((?:\\.|[^"\\])*)"/g,
    )) {
      const one = unescape(match[1]);
      const other = unescape(match[2]);
      keys.add(one);
      keys.add(other);
      pluralIds.add(one);
      pluralIds.add(other);
    }
  }
  return { keys: [...keys].sort((a, b) => a.localeCompare(b)), pluralIds };
}

const { keys, pluralIds } = collectCatalog();
const keySet = new Set(keys);

if (keys.length === 0) {
  console.error("i18n: no message ids found — did the extraction regexp break?");
  process.exit(1);
}

const catalogPath = path.join(localesDir, "en.json");
const existingCatalog = readJson(catalogPath) ?? {};
const catalogMissing = keys.filter((key) => !(key in existingCatalog));
const catalogStale = Object.keys(existingCatalog).filter((key) => !keySet.has(key));
if (catalogMissing.length) {
  errors.push(
    `en.json is missing ${catalogMissing.length} message id(s): ${catalogMissing
      .slice(0, 8)
      .map((key) => JSON.stringify(key))
      .join(", ")}`,
  );
}
if (catalogStale.length) {
  errors.push(
    `en.json has ${catalogStale.length} stale id(s) not present in source: ${catalogStale
      .slice(0, 8)
      .map((key) => JSON.stringify(key))
      .join(", ")}`,
  );
}
const catalogRaw = readText(catalogPath);
if (catalogRaw) {
  const duplicates = duplicateIds(catalogRaw);
  if (duplicates.length) errors.push(`en.json: duplicate id(s): ${duplicates.slice(0, 8).map((key) => JSON.stringify(key)).join(", ")}`);
  const sorted = [...Object.keys(existingCatalog)].sort((a, b) => a.localeCompare(b));
  if (JSON.stringify(sorted) !== JSON.stringify(Object.keys(existingCatalog))) {
    errors.push("en.json is not sorted — run `pnpm i18n:write`");
  }
}

const english = {};
for (const key of keys) english[key] = key;

/* ------------------------- locale files & coverage ------------------------ */

/** @type {Record<string, {translated:number,total:number,percent:number}>} */
const coverage = {};
const shipped = [];
const missingFiles = [];

for (const entry of manifest) {
  if (entry.status === "pseudo") continue;
  const { code } = entry;
  if (code === "en") {
    coverage.en = { translated: keys.length, total: keys.length, percent: 100 };
    shipped.push(code);
    continue;
  }
  const file = path.join(localesDir, entry.file ?? `${code}.json`);
  if (!fs.existsSync(file)) {
    // Not shipped yet: a locale under construction is not a failure. The
    // picker hides it because coverage is 0 %.
    coverage[code] = { translated: 0, total: keys.length, percent: 0 };
    missingFiles.push(code);
    continue;
  }
  shipped.push(code);
  const raw = readText(file) ?? "";
  const messages = readJson(file);
  if (!messages) {
    errors.push(`${entry.file}: not valid JSON`);
    coverage[code] = { translated: 0, total: keys.length, percent: 0 };
    continue;
  }
  const duplicates = duplicateIds(raw);
  if (duplicates.length) {
    errors.push(`${entry.file}: duplicate id(s): ${duplicates.slice(0, 8).map((key) => JSON.stringify(key)).join(", ")}`);
  }
  const categories = intlCategories(entry.intl);
  const translated = keys.filter((key) => typeof messages[key] === "string" && messages[key].trim() !== "");
  coverage[code] = {
    translated: translated.length,
    total: keys.length,
    percent: keys.length === 0 ? 100 : Math.round((translated.length / keys.length) * 1000) / 10,
  };

  const pluralBases = new Set();
  for (const [key, value] of Object.entries(messages)) {
    const base = baseKey(key);
    if (!keySet.has(base)) {
      errors.push(`${entry.file}: unknown message id ${JSON.stringify(key)}`);
      continue;
    }
    if (typeof value !== "string" || value.trim() === "") {
      errors.push(`${entry.file}: empty translation for ${JSON.stringify(key)}`);
      continue;
    }
    const suffix = pluralSuffix(key);
    if (suffix !== null) {
      pluralBases.add(base);
      if (categories && !categories.has(suffix)) {
        errors.push(
          `${entry.file}: ${JSON.stringify(key)} uses the plural category "${suffix}", which ${entry.intl} does not have (${[...categories].join(", ")})`,
        );
      }
    }
    const sourceText = existingCatalog[base] ?? base;
    if (placeholders(sourceText) !== placeholders(value)) {
      errors.push(
        `${entry.file}: placeholder mismatch for ${JSON.stringify(key)} (en: {${placeholders(sourceText)}} / ${code}: {${placeholders(value)}})`,
      );
    }
  }
  for (const base of pluralBases) {
    if (!(`${base}|other` in messages)) {
      errors.push(`${entry.file}: ${JSON.stringify(base)} has plural forms but no "${base}|other" fallback`);
    }
  }

  const percent = coverage[code].percent;
  if (entry.status === "official" && percent < 100) {
    const missingCount = keys.length - translated.length;
    errors.push(
      `${entry.file}: ${missingCount} untranslated id(s), ${percent}% (${translated.length}/${keys.length}) — official locales must be complete`,
    );
  }
  if (entry.status === "preview") {
    // The picker hides anything under the floor, so a preview locale that slips
    // while the source grows is safe: the build stays green, the user keeps
    // English, and `--strict` (the release gate) refuses to ship it that way.
    if (entry.tier === "top10" && percent < SELECTABLE_PERCENT) {
      const message = `${entry.file}: ${percent}% is below the ${SELECTABLE_PERCENT}% floor — hidden from the picker (${translated.length}/${keys.length})`;
      if (strict) errors.push(message);
      else warnings.push(message);
    } else if (entry.tier === "top10" && percent < 100) {
      const message = `${entry.file}: ${percent}% — a top-10 locale must reach 100% before it is promoted to official (${translated.length}/${keys.length})`;
      if (strict) errors.push(message);
      else warnings.push(message);
    } else if (entry.tier !== "top10" && percent < SELECTABLE_PERCENT) {
      warnings.push(`${entry.file}: ${percent}% — below ${SELECTABLE_PERCENT}%, hidden from the picker until it catches up`);
    }
  }
}

const coveragePath = path.join(localesDir, "coverage.json");
const existingCoverage = readJson(coveragePath);
if (JSON.stringify(existingCoverage) !== JSON.stringify(coverage)) {
  if (write || coverageOnly) {
    fs.writeFileSync(coveragePath, `${JSON.stringify(coverage, null, 2)}\n`);
  } else {
    errors.push("coverage.json is stale — run `pnpm i18n:write`");
  }
}

/* --------------------------- RTL manifest coherence ---------------------- */

const indexHtml = readText(path.join(webRoot, "index.html")) ?? "";
const rtlMatch = /LUDEX_RTL_LOCALES\s*=\s*\[([^\]]*)\]/.exec(indexHtml);
if (!rtlMatch) {
  warnings.push("index.html: no LUDEX_RTL_LOCALES list found for the pre-paint direction script");
} else {
  const fromHtml = new Set(
    rtlMatch[1]
      .split(",")
      .map((value) => value.trim().replace(/^["']|["']$/g, ""))
      .filter(Boolean)
      .map((value) => value.split("-")[0].toLowerCase()),
  );
  for (const base of RTL_BASES) {
    if (!fromHtml.has(base)) errors.push(`index.html: LUDEX_RTL_LOCALES is missing "${base}" (manifest says it is RTL)`);
  }
  for (const base of fromHtml) {
    if (!RTL_BASES.has(base)) errors.push(`index.html: LUDEX_RTL_LOCALES lists "${base}", but no RTL locale is registered for it`);
  }
}

/* ------------------------------ pseudo-locale ---------------------------- */

const pseudo = pseudoCatalog(english);
for (const [key, value] of Object.entries(pseudo)) {
  if (!value.startsWith(PSEUDO_OPEN) || !value.endsWith(PSEUDO_CLOSE)) {
    errors.push(`en-XA: ${JSON.stringify(key)} is not bracketed`);
    continue;
  }
  if (value.length <= english[key].length) {
    errors.push(`en-XA: ${JSON.stringify(key)} does not expand (truncation checks need longer text)`);
  }
  if (placeholders(value) !== placeholders(english[key])) {
    errors.push(`en-XA: ${JSON.stringify(key)} lost a placeholder`);
  }
}

/* --------------------------- hardcoded English --------------------------- */

const findings = lintHardcodedStrings(srcRoot).filter((finding) => finding.text.length >= MIN_COPY_LENGTH);
if (hardcodedOnly) {
  for (const finding of findings) console.log(formatFinding(finding, webRoot));
  console.log(`\ni18n: ${findings.length} hardcoded English string(s) (advisory)`);
  process.exit(0);
}
if (findings.length) {
  warnings.push(
    `${findings.length} hardcoded English string(s) — run \`node scripts/i18n.mjs --hardcoded\` for the list`,
  );
}

/* -------------------------------- output -------------------------------- */

function report() {
  const table = manifest
    .filter((entry) => entry.status !== "pseudo")
    .map((entry) => ({
      code: entry.code,
      native: entry.label,
      status: entry.status,
      tier: entry.tier,
      dir: entry.dir,
      shipped: entry.code === "en" || !missingFiles.includes(entry.code),
      translated: coverage[entry.code]?.translated ?? 0,
      total: coverage[entry.code]?.total ?? keys.length,
      percent: coverage[entry.code]?.percent ?? 0,
      selectable: entry.status === "official" || (coverage[entry.code]?.percent ?? 0) >= SELECTABLE_PERCENT,
    }));
  console.table(table);
  console.log(`i18n: ${keys.length} message ids · ${shipped.length} shipped locale(s) · ${missingFiles.length} not shipped yet${missingFiles.length ? ` (${missingFiles.join(", ")})` : ""}`);
  console.log(`i18n: pseudo-locale ${pseudoOnly ? "written" : "checked"} (${Object.keys(pseudo).length} ids) — open the app with ?lang=en-XA`);
}

if (pseudoOnly) {
  const target = path.join(os.tmpdir(), "ludex-en-XA.json");
  fs.writeFileSync(target, `${JSON.stringify(pseudo, null, 2)}\n`);
  console.log(`i18n: wrote ${target}`);
  report();
  console.log(`en-XA sample: ${JSON.stringify(pseudo[keys[0]] ?? "")}`);
  process.exit(0);
}

if (coverageOnly) {
  report();
  console.log(`i18n: coverage.json refreshed for ${Object.keys(coverage).length} locale(s)`);
  process.exit(0);
}

if (write) {
  fs.writeFileSync(catalogPath, `${JSON.stringify(english, null, 2)}\n`);
  if (errors.length) {
    // A regeneration always resolves catalog freshness; re-check translations
    // on the next run so this command stays useful while catalogs are filled in.
    console.log("i18n: catalog regenerated; remaining gaps:");
    for (const message of errors) console.log(`  - ${message}`);
  }
  report();
  process.exit(0);
}

report();
for (const message of warnings) console.warn(`warning: ${message}`);
if (errors.length) {
  console.error(`\ni18n check failed (${errors.length}):`);
  for (const message of errors) console.error(`  - ${message}`);
  console.error("\nRun `pnpm i18n:write` after changing source copy.");
  process.exit(1);
}
console.log(
  `i18n: parity OK for ${shipped.join(", ")}${strict ? " (strict)" : ""} — coverage gate ${SELECTABLE_PERCENT}%`,
);
