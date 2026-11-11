#!/usr/bin/env node
/**
 * Advisory hardcoded-English lint for the web client.
 *
 * English is the message id, so a user-visible string that never went through
 * `t()`/`tn()`/`tr()` stays English in every locale. This scan flags the
 * obvious cases — JSX text nodes and the copy-bearing attributes (`aria-label`,
 * `placeholder`, `title`, `alt`) — and ignores technical literals (paths, file
 * names, codes, CSS, URLs, format tokens).
 *
 * It is deliberately heuristic and **advisory**: run it with
 * `node scripts/i18n.mjs --hardcoded` while translating a screen. The main
 * check prints the count but never fails the build on it, because a false
 * positive must not block a release.
 */
import fs from "node:fs";
import path from "node:path";

const COPY_ATTRIBUTES = ["aria-label", "placeholder", "title", "alt"];

/** Technical literals that are legitimately untranslated. */
function isTechnical(text) {
  if (!/[A-Za-z]/.test(text)) return true;
  if (/^[A-Z0-9][A-Z0-9 ._/+-]*$/.test(text)) return true; // codes, acronyms, keys
  if (/[:/\\{}<>%$#=*\[\]`]/.test(text)) return true; // paths, URLs, formats, code
  if (/\.(exe|json|yaml|yml|png|svg|zip|7z|rar|tar|gz|md|iso|cue|bin)\b/i.test(text)) return true;
  if (/\b[A-Za-z]+[A-Z][A-Za-z]*\b/.test(text)) return true; // camelCase / PascalCase identifiers
  if (/\b[a-z]+_[a-z_]+\b/.test(text)) return true; // snake_case identifiers
  // A visible English sentence needs at least two words with a lowercase run.
  const words = text.split(/\s+/).filter(Boolean);
  if (words.length < 2) return true;
  return !words.some((word) => /[a-z]{2,}/.test(word));
}

/** Drops comments without moving line numbers. */
function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, (match) => match.replace(/[^\n]/g, " "))
    .split("\n")
    .map((line) => line.replace(/(^|[^:])\/\/.*$/, "$1"))
    .join("\n");
}

function walk(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === "locales" ? [] : walk(full);
    return /\.tsx$/.test(entry.name) ? [full] : [];
  });
}

/**
 * @param {string} srcRoot absolute path to `apps/web/src`
 * @returns {{file: string, line: number, text: string, kind: string}[]}
 */
export function lintHardcodedStrings(srcRoot) {
  const findings = [];
  for (const file of walk(srcRoot)) {
    const source = stripComments(fs.readFileSync(file, "utf8"));
    const lines = source.split("\n");
    lines.forEach((line, index) => {
      // A line that already translates something is trusted; the copy may still
      // be wrapped on the next line, which the reviewer catches.
      if (/\b(t|tn|tr)\s*\(/.test(line)) return;

      for (const match of line.matchAll(/>([^<>{}]+)</g)) {
        const text = match[1].trim();
        if (text.length < 3 || isTechnical(text)) continue;
        const before = line.slice(0, match.index + 1);
        if (/<(code|pre|kbd|Kbd)[^>]*>[^<]*$/.test(before)) continue;
        findings.push({ file, line: index + 1, text, kind: "jsx-text" });
      }

      for (const attribute of COPY_ATTRIBUTES) {
        const pattern = new RegExp(`\\b${attribute}="([^"]{3,200})"`, "g");
        for (const match of line.matchAll(pattern)) {
          const text = match[1].trim();
          if (isTechnical(text)) continue;
          findings.push({ file, line: index + 1, text, kind: attribute });
        }
      }
    });
  }
  return findings;
}

export function formatFinding(finding, root) {
  const relative = path.relative(root, finding.file).split(path.sep).join("/");
  return `${relative}:${finding.line} [${finding.kind}] ${JSON.stringify(finding.text)}`;
}
