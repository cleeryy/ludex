#!/usr/bin/env node
/**
 * Packaging check: the AUR `ludex-bin` PKGBUILD against the release it pins.
 *
 * The package repackages a published AppImage, so its whole value is one digest:
 * if `sha256sums` drifts from the release `SHA256SUMS`, AUR users get a package
 * that either fails to build or installs the wrong bytes. This script reads the
 * PKGBUILD, fetches the `SHA256SUMS` of the tag it pins and compares — no
 * dependencies, no `makepkg` required.
 *
 *   node scripts/check-packaging.mjs            # fetch the release SHA256SUMS
 *   node scripts/check-packaging.mjs --offline  # internal consistency only
 *
 * Options:
 *   --offline             do not touch the network
 *   --pkgbuild PATH       PKGBUILD to check (default: packaging/aur/ludex-bin/PKGBUILD)
 *   --srcinfo PATH        .SRCINFO to compare (default: beside the PKGBUILD)
 *   --repo OWNER/NAME     GitHub repository (default: the PKGBUILD `url`, then
 *                         $GITHUB_REPOSITORY, then the origin remote)
 *   --tag TAG             release tag (default: v<pkgver>)
 *   --verbose             print the parsed fields
 *   -h, --help            this help
 *
 * `SHA256SUMS` is fetched with `gh release download` when the CLI is installed,
 * otherwise through the GitHub API with `GITHUB_TOKEN`/`GH_TOKEN`, otherwise as
 * the plain release asset URL (public repositories). CI runs it with a token.
 *
 * Exit codes: 0 everything matches, 1 a check failed, 2 bad usage.
 */
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_PKGBUILD = join(ROOT, "packaging/aur/ludex-bin/PKGBUILD");
const SHA_RE = /^[0-9a-f]{64}$/;
const STABLE_VERSION_RE = /^\d+\.\d+\.\d+$/;

const failures = [];
const verbose = process.argv.includes("--verbose");

/** @param {string} message */
function fail(message) {
  failures.push(message);
}

/** @param {string} message */
function ok(message) {
  console.log(`  ok   ${message}`);
}

/** @param {...string} lines */
function die(...lines) {
  for (const line of lines) console.error(line);
  process.exit(2);
}

function usage() {
  const header = readFileSync(fileURLToPath(import.meta.url), "utf8")
    .split("\n")
    .slice(1)
    .filter((line) => line.startsWith(" *") || line === " */")
    .map((line) => line.replace(/^ \* ?/, ""))
    .join("\n");
  console.log(header);
}

function parseArgs(argv) {
  const options = {
    offline: false,
    pkgbuild: DEFAULT_PKGBUILD,
    srcinfo: "",
    repo: "",
    tag: "",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const value = () => {
      const next = argv[index + 1];
      if (!next) die(`error: ${arg} needs a value`);
      index += 1;
      return next;
    };
    switch (arg) {
      case "--offline":
        options.offline = true;
        break;
      case "--pkgbuild":
        options.pkgbuild = value();
        break;
      case "--srcinfo":
        options.srcinfo = value();
        break;
      case "--repo":
        options.repo = value();
        break;
      case "--tag":
        options.tag = value();
        break;
      case "--verbose":
        break;
      case "-h":
      case "--help":
        usage();
        process.exit(0);
        break;
      default:
        die(`error: unknown option ${arg} (see --help)`);
    }
  }
  if (!options.srcinfo) options.srcinfo = join(dirname(options.pkgbuild), ".SRCINFO");
  return options;
}

/**
 * Statement-level view of a PKGBUILD: comment lines dropped, `\` continuations
 * joined, and parsing stopped at the first function body (heredocs inside
 * `package()` contain `Key=value` lines that are not variables).
 *
 * @param {string} text
 * @returns {string[]}
 */
function statements(text) {
  const out = [];
  let buffer = "";
  let depth = 0;
  for (const raw of text.split(/\r?\n/)) {
    const line = buffer === "" ? raw.trimStart() : raw;
    if (buffer === "" && (line.trim() === "" || line.startsWith("#"))) continue;
    if (buffer === "") {
      if (/^(package|prepare|build|check|pkgver|_pkgver)\s*\(\s*\)/.test(line)) break;
      if (/^function\s+/.test(line)) break;
    }
    let quote = null;
    for (const char of line) {
      if (quote) {
        if (char === quote) quote = null;
        continue;
      }
      if (char === "'" || char === '"') quote = char;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
    if (line.endsWith("\\")) {
      buffer += `${line.slice(0, -1)}\n`;
      continue;
    }
    buffer += `${line}\n`;
    if (depth <= 0) {
      out.push(buffer.trimEnd());
      buffer = "";
    }
  }
  if (buffer.trim()) out.push(buffer.trimEnd());
  return out;
}

/**
 * Split `( 'a' "b c" d )` into its items, dropping quotes.
 *
 * @param {string} body
 * @returns {string[]}
 */
function splitArray(body) {
  const items = [];
  let current = "";
  let quote = null;
  for (const char of body) {
    if (quote) {
      if (char === quote) quote = null;
      else current += char;
      continue;
    }
    if (char === "'" || char === '"') {
      quote = char;
      continue;
    }
    if (/\s/.test(char)) {
      if (current) items.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  if (current) items.push(current);
  return items;
}

/**
 * @param {string} text
 * @returns {{scalars: Map<string,string>, arrays: Map<string,string[]>}}
 */
function parsePkgbuild(text) {
  const scalars = new Map();
  const arrays = new Map();
  for (const statement of statements(text)) {
    const match = /^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/s.exec(statement.trimEnd());
    if (!match) continue;
    const [, name, rawValue] = match;
    const value = rawValue.trim();
    if (value.startsWith("(")) {
      const end = value.lastIndexOf(")");
      arrays.set(name, splitArray(value.slice(1, end === -1 ? undefined : end)));
    } else {
      scalars.set(name, value.replace(/^(['"])(.*)\1$/s, "$2"));
    }
  }
  return { scalars, arrays };
}

/**
 * Expand `${name}` and `$name` from the PKGBUILD's own scalars, so `source`
 * entries can be compared as concrete URLs.
 *
 * @param {string} value
 * @param {Map<string,string>} scalars
 * @returns {string}
 */
function expand(value, scalars) {
  let current = value;
  for (let pass = 0; pass < 10; pass += 1) {
    const next = current.replace(/\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)/g, (whole, braced, bare) => {
      const name = braced || bare;
      return scalars.has(name) ? scalars.get(name) : whole;
    });
    if (next === current) break;
    current = next;
  }
  return current;
}

/**
 * `.SRCINFO`: every field is `\tkey = value` under a `pkgbase`/`pkgname` header,
 * repeated keys are lists, and source/digest order is meaningful.
 *
 * @param {string} text
 * @returns {Map<string, Map<string,string[]>>}
 */
function parseSrcinfo(text) {
  const sections = new Map();
  let section = null;
  let current = "";
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    // A `pkgname` line starts a split-package section; `ludex-bin` is a single
    // package, so its fields live under `pkgbase` and the trailing pkgname line
    // carries none.
    const header = /^(pkgbase|pkgname)\s*=\s*(.+)$/.exec(line);
    if (header) {
      current = header[2].trim();
      if (header[1] === "pkgbase" || !sections.has(current)) {
        section = new Map();
        if (header[1] === "pkgbase") sections.set(current, section);
      }
      continue;
    }
    if (!section || !sections.has(current)) continue;
    const field = /^([A-Za-z0-9_]+)\s*=\s*(.*)$/.exec(line);
    if (!field) continue;
    const [, key, value] = field;
    if (!section.has(key)) section.set(key, []);
    section.get(key).push(value.trim());
  }
  return sections;
}

/** @param {string[]} left @param {string[]} right */
function sameSet(left, right) {
  const a = [...new Set(left)].sort();
  const b = [...new Set(right)].sort();
  return a.length === b.length && a.every((item, index) => item === b[index]);
}

/** @param {string[]} left @param {string[]} right */
function sameList(left, right) {
  return left.length === right.length && left.every((item, index) => item === right[index]);
}

/**
 * `makepkg --printsrcinfo` output for a copy of the package directory, or null
 * when `makepkg` is unavailable (not installed, or running as root); the field
 * comparison still runs in that case.
 *
 * @param {string} pkgbuildPath
 * @returns {string|null}
 */
function printSrcinfo(pkgbuildPath) {
  const probe = spawnSync("makepkg", ["--version"], { encoding: "utf8" });
  if (probe.error || probe.status !== 0) return null;
  const workdir = mkdtempSync(join(tmpdir(), "ludex-check-packaging-"));
  try {
    const pkgdir = dirname(pkgbuildPath);
    const copy = readFileSync(pkgbuildPath);
    writeFileSync(join(workdir, "PKGBUILD"), copy);
    const srcinfo = join(pkgdir, ".SRCINFO");
    if (existsSync(srcinfo)) writeFileSync(join(workdir, ".SRCINFO"), readFileSync(srcinfo));
    const result = spawnSync("makepkg", ["--printsrcinfo"], { cwd: workdir, encoding: "utf8" });
    if (result.error || result.status !== 0) return null;
    return result.stdout;
  } finally {
    rmSync(workdir, { recursive: true, force: true });
  }
}

/** @param {string} name */
function which(name) {
  const result = spawnSync(name, ["--version"], { encoding: "utf8" });
  return !result.error;
}

/**
 * Download `SHA256SUMS` for a release tag.
 *
 * @param {{repo: string, tag: string}} options
 * @returns {Promise<string>} the file contents
 */
async function fetchSums({ repo, tag }) {
  const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN || "";
  if (which("gh")) {
    const dir = mkdtempSync(join(tmpdir(), "ludex-sums-"));
    try {
      const result = spawnSync(
        "gh",
        ["release", "download", tag, "--repo", repo, "--pattern", "SHA256SUMS", "--dir", dir, "--clobber"],
        { encoding: "utf8", env: { ...process.env, GH_TOKEN: token || process.env.GH_TOKEN || "" } },
      );
      if (result.status === 0) return readFileSync(join(dir, "SHA256SUMS"), "utf8");
      const detail = (result.stderr || result.stdout || "").trim();
      if (token) throw new Error(`gh release download ${tag} failed: ${detail}`);
    } catch (error) {
      if (token) throw error;
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }

  const assetUrl = `https://github.com/${repo}/releases/download/${tag}/SHA256SUMS`;
  if (token) {
    // The API hands out a signed redirect; the token stays on api.github.com.
    const release = await fetch(`https://api.github.com/repos/${repo}/releases/tags/${tag}`, {
      headers: { Authorization: `Bearer ${token}`, Accept: "application/vnd.github+json" },
    });
    if (release.status === 404) throw new Error(`release ${tag} not found in ${repo}`);
    if (!release.ok) throw new Error(`GitHub API returned ${release.status} for ${repo} ${tag}`);
    const asset = (await release.json()).assets?.find((entry) => entry.name === "SHA256SUMS");
    if (!asset) throw new Error(`release ${tag} has no SHA256SUMS asset`);
    const first = await fetch(asset.url, {
      headers: { Authorization: `Bearer ${token}`, Accept: "application/octet-stream" },
      redirect: "manual",
    });
    if (first.status >= 300 && first.status < 400) {
      const location = first.headers.get("location");
      if (!location) throw new Error("the SHA256SUMS download redirected without a Location header");
      const second = await fetch(location);
      if (!second.ok) throw new Error(`downloading SHA256SUMS returned ${second.status}`);
      return second.text();
    }
    if (!first.ok) throw new Error(`downloading SHA256SUMS returned ${first.status}`);
    return first.text();
  }

  const response = await fetch(assetUrl, { redirect: "follow" });
  if (response.status === 404) throw new Error(`release ${tag} or its SHA256SUMS is missing (${assetUrl})`);
  if (!response.ok) throw new Error(`GET ${assetUrl} returned ${response.status}`);
  return response.text();
}

/**
 * @param {string} text
 * @returns {Map<string,string>} file name -> digest
 */
function parseSums(text) {
  const digests = new Map();
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const match = /^([0-9a-fA-F]{64})\s+\*?(.+)$/.exec(line);
    if (!match) continue;
    digests.set(match[2].trim(), match[1].toLowerCase());
  }
  return digests;
}

/** @param {string|undefined} value */
function repoFromRemote(value) {
  if (!value) return "";
  const match = /github\.com[:/]+([^/]+\/[^/]+?)(?:\.git)?$/.exec(value.trim());
  return match ? match[1] : "";
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (!existsSync(options.pkgbuild)) die(`error: PKGBUILD not found: ${options.pkgbuild}`);
  if (!existsSync(options.srcinfo)) die(`error: .SRCINFO not found: ${options.srcinfo} (run makepkg --printsrcinfo)`);

  const { scalars, arrays } = parsePkgbuild(readFileSync(options.pkgbuild, "utf8"));
  const pkgname = scalars.get("pkgname") || "";
  const pkgver = scalars.get("pkgver") || "";
  const pkgrel = scalars.get("pkgrel") || "";
  const sources = (arrays.get("source") || []).map((entry) => expand(entry, scalars));
  const sums = arrays.get("sha256sums") || [];
  const depends = arrays.get("depends") || [];
  const conflicts = arrays.get("conflicts") || [];
  const provides = arrays.get("provides") || [];
  const arch = arrays.get("arch") || [];
  const license = arrays.get("license") || [];
  const pkgoptions = arrays.get("options") || [];
  const url = scalars.get("url") || "";
  const pkgdesc = scalars.get("pkgdesc") || "";

  console.log(`check-packaging: ${options.pkgbuild}`);
  if (verbose) {
    console.log(`  pkgname=${pkgname} pkgver=${pkgver} pkgrel=${pkgrel}`);
    console.log(`  sources=${JSON.stringify(sources)}`);
    console.log(`  sha256sums=${JSON.stringify(sums)}`);
  }

  // --- PKGBUILD internal consistency ---------------------------------------
  if (pkgname === "ludex-bin") ok(`pkgname is ludex-bin`);
  else fail(`pkgname is ${JSON.stringify(pkgname)}, expected "ludex-bin"`);

  if (STABLE_VERSION_RE.test(pkgver)) ok(`pkgver ${pkgver} is a stable X.Y.Z version`);
  else fail(`pkgver ${JSON.stringify(pkgver)} is not a stable X.Y.Z version (the edge channel is not packaged)`);

  if (/^\d+$/.test(pkgrel)) ok(`pkgrel is ${pkgrel}`);
  else fail(`pkgrel ${JSON.stringify(pkgrel)} is not an integer`);

  if (sameList(arch, ["x86_64"])) ok("arch is x86_64 only");
  else fail(`arch is ${JSON.stringify(arch)}, expected ["x86_64"]`);

  if (url === "https://github.com/cleeryy/ludex") ok("url points at the upstream repository");
  else if (/^https:\/\/github\.com\/[^/]+\/[^/]+$/.test(url)) ok(`url is ${url}`);
  else fail(`url ${JSON.stringify(url)} is not a GitHub repository URL`);

  if (pkgdesc) ok("pkgdesc is set");
  else fail("pkgdesc is empty");

  if (license.length > 0) ok(`license is ${license.join(", ")}`);
  else fail("license is empty");

  if (sameSet(provides, ["ludex"])) ok("provides ludex");
  else fail(`provides is ${JSON.stringify(provides)}, expected ["ludex"]`);

  if (sameSet(conflicts, ["ludex"])) ok("conflicts ludex");
  else fail(`conflicts is ${JSON.stringify(conflicts)}, expected ["ludex"]`);

  const requiredDeps = ["webkit2gtk-4.1", "gtk3", "libayatana-appindicator"];
  const missingDeps = requiredDeps.filter((dep) => !depends.includes(dep));
  if (missingDeps.length === 0) ok(`depends include ${requiredDeps.join(", ")}`);
  else fail(`depends is missing ${missingDeps.join(", ")}`);

  if (pkgoptions.includes("!strip")) ok("options keep !strip");
  else fail("options should contain !strip (the AppImage is a prebuilt runtime)");

  const appimages = sources.filter((entry) => (entry.split("::").pop() || "").endsWith(".AppImage"));
  if (appimages.length === 1) {
    const entry = appimages[0];
    const [namePart, urlPart] = entry.includes("::") ? entry.split("::") : ["", entry];
    const fileName = namePart || urlPart.split("/").pop();
    const expectedName = `Ludex_${pkgver}_amd64.AppImage`;
    if (fileName === expectedName) ok(`AppImage source is ${expectedName}`);
    else fail(`AppImage source is ${fileName}, expected ${expectedName} for pkgver ${pkgver}`);
    const expectedUrl = `${url}/releases/download/v${pkgver}/${expectedName}`;
    if (urlPart === expectedUrl) ok(`AppImage URL is the v${pkgver} release asset`);
    else fail(`AppImage URL is ${urlPart}, expected ${expectedUrl}`);
  } else if (appimages.length === 0) {
    fail("source has no AppImage entry");
  } else {
    fail(`source has ${appimages.length} AppImage entries, expected one`);
  }

  if (sources.length > 0) ok(`source has ${sources.length} entries`);
  else fail("source is empty");

  if (sources.length === sums.length) ok(`sha256sums has one digest per source (${sums.length})`);
  else fail(`sha256sums has ${sums.length} entries for ${sources.length} sources`);

  const badDigests = sums.filter((digest) => !SHA_RE.test(digest));
  if (badDigests.length === 0 && sums.length > 0) ok("every sha256sums entry is a 64-character digest");
  else if (badDigests.length > 0) fail(`sha256sums has invalid digests: ${badDigests.join(", ")}`);

  // --- .SRCINFO matches the PKGBUILD ---------------------------------------
  const srcinfoText = readFileSync(options.srcinfo, "utf8");
  const sections = parseSrcinfo(srcinfoText);
  const section = sections.get(pkgname);
  if (!section) {
    fail(`${options.srcinfo} has no pkgbase/pkgname section for ${pkgname}`);
  } else {
    const field = (key) => section.get(key) || [];
    const comparisons = [
      { key: "pkgdesc", expected: [pkgdesc], mode: "list" },
      { key: "pkgver", expected: [pkgver], mode: "list" },
      { key: "pkgrel", expected: [pkgrel], mode: "list" },
      { key: "url", expected: [url], mode: "list" },
      { key: "arch", expected: arch, mode: "set" },
      { key: "license", expected: license, mode: "set" },
      { key: "depends", expected: depends, mode: "set" },
      { key: "provides", expected: provides, mode: "set" },
      { key: "conflicts", expected: conflicts, mode: "set" },
      { key: "options", expected: pkgoptions, mode: "set" },
      { key: "source", expected: sources, mode: "list" },
      { key: "sha256sums", expected: sums, mode: "list" },
    ];
    let drift = 0;
    for (const entry of comparisons) {
      const actual = field(entry.key);
      const identical = entry.mode === "set" ? sameSet(actual, entry.expected) : sameList(actual, entry.expected);
      if (identical) continue;
      drift += 1;
      fail(`.SRCINFO ${entry.key} does not match the PKGBUILD: got ${JSON.stringify(actual)}, expected ${JSON.stringify(entry.expected)}`);
    }
    if (drift === 0) ok(`.SRCINFO matches the PKGBUILD (${comparisons.length} fields)`);
  }

  const generated = printSrcinfo(options.pkgbuild);
  if (generated === null) {
    console.log("  info makepkg is unavailable here (not installed, or refusing to run as root); compared .SRCINFO fields instead");
  } else {
    const normalize = (text) =>
      text
        .split(/\r?\n/)
        .map((line) => line.trimEnd())
        .filter((line) => line !== "")
        .join("\n");
    if (normalize(generated) === normalize(srcinfoText)) ok("makepkg --printsrcinfo reproduces the committed .SRCINFO");
    else fail(".SRCINFO is stale: run `makepkg --printsrcinfo > .SRCINFO` in packaging/aur/ludex-bin");
  }

  // --- the pinned release -------------------------------------------------
  // The PKGBUILD's `url` is the repository whose release it repackages: a fork
  // PR still has to point at the upstream digests.
  const repo =
    options.repo ||
    repoFromRemote(url) ||
    process.env.GITHUB_REPOSITORY ||
    repoFromRemote(spawnSync("git", ["config", "--get", "remote.origin.url"], { encoding: "utf8" }).stdout) ||
    "cleeryy/ludex";
  const tag = options.tag || `v${pkgver}`;

  if (options.offline) {
    console.log(`  skip release check (--offline): ${repo} ${tag}`);
  } else {
    let sumsText;
    try {
      sumsText = await fetchSums({ repo, tag });
    } catch (error) {
      fail(`could not read the ${tag} SHA256SUMS from ${repo}: ${error.message}`);
      sumsText = null;
    }
    if (sumsText !== null) {
      const release = parseSums(sumsText);
      if (release.size === 0) fail(`${repo} ${tag} SHA256SUMS is empty or unreadable`);
      const expectedName = `Ludex_${pkgver}_amd64.AppImage`;
      const published = release.get(expectedName);
      const appimageIndex = sources.findIndex((entry) => entry.includes(expectedName));
      if (!published) {
        fail(`${repo} ${tag} SHA256SUMS has no ${expectedName} entry`);
      } else if (appimageIndex === -1) {
        fail(`PKGBUILD does not package ${expectedName}`);
      } else if (sums[appimageIndex] !== published) {
        fail(`AppImage digest drift: PKGBUILD has ${sums[appimageIndex]}, ${tag} SHA256SUMS has ${published}`);
      } else {
        ok(`AppImage digest matches the ${tag} release SHA256SUMS (${published})`);
      }

      // Any other packaged file that is also a release asset must match too.
      for (const [index, entry] of sources.entries()) {
        const fileName = (entry.includes("::") ? entry.split("::")[0] : entry.split("/").pop()) || "";
        if (fileName === expectedName) continue;
        const digest = release.get(fileName);
        if (!digest) continue;
        if (sums[index] === digest) ok(`${fileName} digest matches the release`);
        else fail(`${fileName} digest drift: PKGBUILD has ${sums[index]}, release has ${digest}`);
      }
    }
  }

  if (failures.length > 0) {
    console.error(`\ncheck-packaging: ${failures.length} problem(s)`);
    for (const failure of failures) console.error(`  error ${failure}`);
    process.exit(1);
  }
  console.log(`\ncheck-packaging: OK (${pkgname} ${pkgver}-${pkgrel}${options.offline ? ", offline" : `, ${tag}`})`);
}

await main();
