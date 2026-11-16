#!/usr/bin/env node
/**
 * Screenshot pipeline (#265).
 *
 * Captures the documented screens from the *demo* build of the web UI: the
 * Vite bundle compiled with `VITE_USE_MOCKS=true`, so nothing here needs a
 * server, a token, a real archive or a network connection. The demo adapter
 * seeds a fixed library, and every capture starts from the same storage state.
 *
 *   pnpm screenshots              # build + capture + optimize + manifest
 *   pnpm screenshots --skip-build # reuse apps/web/dist
 *   pnpm screenshots --only readme
 *
 * Output:
 *   docs/assets/screenshots/<set>/<name>.png|jpg
 *   docs/assets/screenshots/manifest.json   (version, commit, date, viewports)
 *   docs/assets/social-preview.png          (1280x640, GitHub repo setting)
 *
 * Requirements: Node 22, `pnpm install`, Playwright Chromium
 * (`pnpm e2e:install`). Image optimization uses the `sharp` dev dependency of
 * this workspace, so there is no ImageMagick dependency.
 */
import { execFileSync, spawn } from "node:child_process";
import { createServer } from "node:net";
import { mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import sharp from "sharp";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const OUT_ROOT = join(ROOT, "docs", "assets", "screenshots");
/**
 * Full-fidelity captures, kept out of git for re-encoding experiments.
 *
 * Created with `mkdtempSync` on first use and removed in the `finally` block:
 * a predictable directory name in the shared temp dir is the
 * `js/insecure-temporary-file` shape (#266).
 */
let rawDir = null;
function rawDirPath() {
  if (!rawDir) rawDir = mkdtempSync(join(tmpdir(), "ludex-screenshots-"));
  return rawDir;
}

/** Replaces `target` in one step so a reader never sees a half-written file. */
function writeFileAtomic(target, contents) {
  const temporary = `${target}.${process.pid}.tmp`;
  writeFileSync(temporary, contents);
  renameSync(temporary, target);
}
const SOCIAL_SVG = join(ROOT, "docs", "assets", "social-preview.svg");
const SOCIAL_PNG = join(ROOT, "docs", "assets", "social-preview.png");

const args = process.argv.slice(2);
const skipBuild = args.includes("--skip-build");
const onlyIndex = args.indexOf("--only");
const only = onlyIndex >= 0 ? args[onlyIndex + 1] : null;

/** Desktop captures are 1920x1080 and ship at 1600 px wide (1.8x on GitHub). */
const DESKTOP = { width: 1920, height: 1080 };
/** Mobile captures use a 390x844 viewport at 2x, kept at 780 px wide. */
const MOBILE = { width: 390, height: 844 };

/**
 * The demo library is generated from `apps/web/src/data/demo.ts`; captures
 * look up the game by title so a reordering of the seeds cannot point a
 * screenshot at the wrong game.
 */
const GAME_TITLE = "HITMAN 3";

const SHOTS = [
  // README hero set ---------------------------------------------------------
  { set: "readme", name: "library-dark", route: "/library", scheme: "dark", waitFor: "h1",
    alt: "Library grid in the dark Midnight theme: 20 demo games with covers, filters and a 73% install badge" },
  { set: "readme", name: "library-light", route: "/library", scheme: "light", waitFor: "h1",
    alt: "The same library grid in the light theme, with warm paper surfaces" },
  { set: "readme", name: "game-page", route: "game", scheme: "dark", waitFor: "h1",
    alt: "Game page for HITMAN 3: cover, rating, size, versions, playtime, download and progress" },
  { set: "readme", name: "console-soft", route: "/bigpicture", scheme: "dark", waitFor: "h1",
    console: "soft", hold: 1400,
    alt: "Console mode with the Soft appearance: cream rounded tiles, artwork backdrop and the recently played shelf" },
  { set: "readme", name: "console-studio", route: "/bigpicture", scheme: "dark", waitFor: "h1",
    console: "studio", press: ["ArrowRight", "ArrowRight", "ArrowRight"], hold: 1600,
    alt: "Console mode with the Studio appearance: the darker archive surface, square corners and sharp artwork" },
  { set: "readme", name: "downloads", route: "/downloads", scheme: "dark", waitFor: "h1",
    alt: "Downloads queue: one active download with a real progress bar and speed, remaining jobs queued with positions" },

  // Docs and admin ----------------------------------------------------------
  { set: "docs", name: "home-dark", route: "/", scheme: "dark", waitFor: "h1",
    alt: "Home: continue playing hero, installs on this device and recently played shelf" },
  { set: "docs", name: "admin-games", route: "/admin/games", scheme: "dark", waitFor: "h1",
    alt: "Admin games manager: a dense table with availability, versions, size, installs and VirusTotal verdicts" },
  { set: "docs", name: "admin-virustotal", route: "/admin/integrations", scheme: "dark", waitFor: "h1",
    alt: "Integrations: VirusTotal key status, daily quota and the periodic re-check policy" },
  { set: "docs", name: "admin-ingest", route: "/admin/ingest", scheme: "dark", waitFor: "h1",
    alt: "Admin ingest: the download queue that adds archives to the library from a URL" },
  { set: "docs", name: "settings", route: "/settings", scheme: "dark", waitFor: "h1",
    alt: "Settings: account, appearance, downloads, saves, streaming and integrations in one place" },

  // Mobile / PWA ------------------------------------------------------------
  { set: "mobile", name: "home", route: "/", viewport: MOBILE, waitFor: "h1",
    alt: "Phone layout of the home screen: continue playing, installs and the bottom tab bar" },
  { set: "mobile", name: "library", route: "/library", viewport: MOBILE, waitFor: "h1",
    alt: "Phone layout of the library: two-column cover grid with filters" },
  { set: "mobile", name: "game-page", route: "game", viewport: MOBILE, waitFor: "h1",
    alt: "Phone layout of a game page: poster, metadata, install button and sections" },
  { set: "mobile", name: "downloads", route: "/downloads", viewport: MOBILE, waitFor: "h1",
    alt: "Phone layout of the downloads queue with progress and queue positions" },
];

/** Budgets from #265: desktop <= 300 KB, mobile <= 120 KB, whole set < 5 MB. */
const BUDGET = { desktop: 300 * 1024, mobile: 120 * 1024, total: 5 * 1024 * 1024 };

/* -------------------------------------------------------------------------- */
/* helpers                                                                    */
/* -------------------------------------------------------------------------- */

function log(message) {
  process.stdout.write(`${message}\n`);
}

function fail(message) {
  process.stderr.write(`\n✗ ${message}\n`);
  process.exit(1);
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function appVersion() {
  return readJson(join(ROOT, "package.json")).version;
}

function commit() {
  try {
    return execFileSync("git", ["rev-parse", "--short", "HEAD"], { cwd: ROOT, encoding: "utf8" }).trim() || "unknown";
  } catch {
    return "unknown";
  }
}

function freePort() {
  return new Promise((resolvePromise, reject) => {
    const server = createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolvePromise(port));
    });
  });
}

async function waitForHttp(url, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      /* not listening yet */
    }
    await new Promise((done) => setTimeout(done, 250));
  }
  throw new Error(`timed out waiting for ${url}`);
}

function run(command, argv, { cwd = ROOT, env = {} } = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, argv, { cwd, env: { ...process.env, ...env }, stdio: "inherit" });
    child.on("error", reject);
    child.on("close", (code) => (code === 0 ? resolvePromise() : reject(new Error(`${command} ${argv.join(" ")} exited ${code}`))));
  });
}

/**
 * A poster for the demo trailer facade. YouTube itself is blocked (no
 * third-party request, no flake offline); the frame keeps the app's own play
 * button over a neutral archive plate. `docs/assets/screenshots/README.md`
 * documents this substitution.
 */
async function posterBuffer(browser) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
  await page.setContent(`<!doctype html><html><body style="margin:0">
    <div style="width:1280px;height:720px;background:#0a0a09;position:relative;overflow:hidden">
      <div style="position:absolute;inset:0;background-image:linear-gradient(#1c1c16 1px,transparent 1px),linear-gradient(90deg,#1c1c16 1px,transparent 1px);background-size:64px 64px;opacity:.45"></div>
      <!-- The game page shows the top of this 16:9 plate; keep the glyph in
           the upper third so a cropped capture still reads as a player. -->
      <div style="position:absolute;left:0;right:0;top:92px;display:grid;place-items:center">
        <div style="width:150px;height:150px;border-radius:999px;border:2px solid #ffc61a;display:grid;place-items:center;background:rgba(255,198,26,.08)">
          <svg viewBox="0 0 100 100" width="58" height="58" fill="#ffc61a"><path d="M34 22L78 50L34 78Z"/></svg>
        </div>
      </div>
      <div style="position:absolute;left:48px;top:42px;font:13px/1 'DejaVu Sans Mono',ui-monospace,monospace;letter-spacing:3px;color:#6f6a5e">TRAILER</div>
      <div style="position:absolute;right:48px;top:42px;font:13px/1 'DejaVu Sans Mono',ui-monospace,monospace;letter-spacing:3px;color:#6f6a5e">YOUTUBE</div>
    </div></body></html>`);
  const buffer = await page.screenshot({ type: "png" });
  await page.close();
  return buffer;
}

/* -------------------------------------------------------------------------- */
/* page driving                                                               */
/* -------------------------------------------------------------------------- */

/**
 * Seeds the same device state for every capture: a demo admin token, English,
 * dismissed onboarding and install prompt. The console appearance is written
 * after the first boot because the key is scoped by server id.
 */
function seedScript() {
  localStorage.setItem("ludex.token", "mock.1.0");
  localStorage.setItem("ludex.locale", "en");
  localStorage.setItem("ludex.pwa.installDismissedAt", String(Date.now()));
  localStorage.setItem("ludex.onboarding.gettingStartedDismissed", "1");
  sessionStorage.setItem("ludex.onboarding.skipped", "1");
}

/** Console appearance is stored per server; resolve the key the app will read. */
async function seedConsoleAppearance(page, preset) {
  await page.evaluate(
    ({ preset }) => {
      const servers = JSON.parse(localStorage.getItem("ludex.servers.v1") ?? "[]");
      const active = localStorage.getItem("ludex.activeServer.v1");
      const scoped = servers.find((server) => server.id === active);
      const key = scoped ? `ludex.server.${scoped.id}.ludex.console.appearance` : "ludex.console.appearance";
      localStorage.setItem(
        key,
        JSON.stringify({
          v: 1,
          appearance: {
            preset,
            accent: "theme",
            background: "artwork",
            radius: preset === "soft" ? "soft" : "sharp",
          },
        }),
      );
    },
    { preset },
  );
}

/** Waits for fonts, decoded images and a quiet network, then a short settle. */
async function settle(page, selector, hold = 500) {
  await page.waitForSelector(selector, { timeout: 30_000 });
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(
      Array.from(document.images)
        .filter((image) => !image.complete)
        .map((image) => new Promise((done) => {
          image.addEventListener("load", done, { once: true });
          image.addEventListener("error", done, { once: true });
        })),
    );
  });
  await page.waitForLoadState("networkidle").catch(() => {});
  await page.waitForTimeout(hold);
}

/**
 * True when two frames differ by more than a few live pixels. The status bar
 * ticks an ETA and the console clock changes minute by minute; those are the
 * real UI, not a transition. A reveal, a late artwork or a scroll is bigger by
 * orders of magnitude.
 */
async function framesDiffer(first, second, tolerance = 0.002) {
  const [a, b] = await Promise.all([
    sharp(first).raw().toBuffer({ resolveWithObject: true }),
    sharp(second).raw().toBuffer({ resolveWithObject: true }),
  ]);
  if (a.info.width !== b.info.width || a.info.height !== b.info.height) return true;
  const channels = a.info.channels;
  const pixels = a.info.width * a.info.height;
  let changed = 0;
  for (let index = 0; index < a.data.length; index += channels) {
    // Alpha is constant for the window; only compare the colour channels.
    if (Math.abs(a.data[index] - b.data[index]) > 8) changed += 1;
    else if (Math.abs(a.data[index + 1] - b.data[index + 1]) > 8) changed += 1;
    else if (Math.abs(a.data[index + 2] - b.data[index + 2]) > 8) changed += 1;
  }
  return changed / pixels > tolerance;
}

/**
 * Screenshots only once the page has stopped changing. Console mode reveals
 * its chrome in a short cascade and artwork can land after the first paint, so
 * a fixed wait is a guess; a settled frame is not.
 */
async function stableScreenshot(page, { attempts = 12, interval = 250 } = {}) {
  let previous = await page.screenshot({ type: "png" });
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    await page.waitForTimeout(interval);
    const next = await page.screenshot({ type: "png" });
    if (!(await framesDiffer(previous, next))) return next;
    previous = next;
  }
  throw new Error("the page never stopped repainting; a capture would catch an animation");
}

/** Finds the demo game id by title from the rendered library grid. */
async function resolveGameRoute(page, title) {
  await page.evaluate(() => {
    window.location.hash = "#/library";
  });
  await page.waitForFunction(() => document.querySelector("h1")?.textContent?.trim() === "Library", null, {
    timeout: 30_000,
  });
  await page.waitForSelector('a[href*="/game/"]', { timeout: 30_000 });
  const href = await page.evaluate((wanted) => {
    const needle = wanted.toLowerCase();
    for (const link of Array.from(document.querySelectorAll('a[href*="/game/"]'))) {
      const label = `${link.textContent ?? ""} ${link.getAttribute("aria-label") ?? ""}`.toLowerCase();
      if (label.includes(needle)) return link.getAttribute("href");
    }
    return null;
  }, title);
  if (!href) throw new Error(`could not find "${title}" in the demo library`);
  const match = /#?\/?game\/(\d+)/.exec(href);
  if (!match) throw new Error(`unexpected game link: ${href}`);
  return `/game/${match[1]}`;
}

/* -------------------------------------------------------------------------- */
/* main                                                                       */
/* -------------------------------------------------------------------------- */

const started = Date.now();
let server = null;
let browser = null;

try {
  if (!skipBuild) {
    log("== building the web UI with the demo adapter ==");
    await run("pnpm", ["--filter", "@ludex/web", "build"], { env: { VITE_USE_MOCKS: "true" } });
  } else if (!statSync(join(ROOT, "apps", "web", "dist", "index.html"), { throwIfNoEntry: false })) {
    fail("apps/web/dist is missing; run without --skip-build first");
  }

  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  log(`== serving apps/web/dist on ${base} ==`);
  server = spawn("pnpm", ["--filter", "@ludex/web", "exec", "vite", "preview", "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
    cwd: ROOT,
    stdio: "ignore",
  });
  server.on("error", (error) => fail(`vite preview failed: ${error.message}`));
  // The demo adapter must be compiled in; a mismatch is a silent white page.
  await waitForHttp(base);

  browser = await chromium.launch();
  const poster = await posterBuffer(browser);

  const socialOnly = only === "social";
  const shots = socialOnly ? [] : SHOTS.filter((shot) => !only || shot.set === only);
  if (!socialOnly && shots.length === 0) fail(`no shots match --only ${only}`);

  const captured = [];
  for (const shot of shots) {
    const viewport = shot.viewport ?? DESKTOP;
    const mobile = Boolean(shot.viewport);
    const context = await browser.newContext({
      viewport,
      deviceScaleFactor: mobile ? 2 : 1,
      colorScheme: shot.scheme ?? "dark",
      locale: "en-US",
      isMobile: mobile,
      hasTouch: mobile,
    });
    const page = await context.newPage();
    // Pin the accessibility media features instead of inheriting the host:
    // motion reduced so nothing is mid-animation, transparency and contrast
    // left at their defaults so the console keeps its designed materials.
    const cdp = await context.newCDPSession(page);
    await cdp.send("Emulation.setEmulatedMedia", {
      media: "",
      features: [
        { name: "prefers-reduced-motion", value: "reduce" },
        { name: "prefers-reduced-transparency", value: "no-preference" },
        { name: "prefers-contrast", value: "no-preference" },
      ],
    });
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    // Any YouTube request is a third-party call; the iframe never loads unless
    // the visitor clicks, and the thumbnail is replaced by the neutral poster.
    // Host-anchored: `[^/]*` used to let `evilyoutube.com` through. Every
    // label before the known domain must end with a dot, so only a real
    // YouTube host matches (#266).
    await page.route(/^https?:\/\/(?:[a-z0-9-]+\.)*(?:youtube\.com|youtu\.be|ytimg\.com|googlevideo\.com)(?::\d+)?\//i, (route) => {
      // YouTube thumbnails (img.youtube.com, i.ytimg.com) are the poster of
      // the trailer facade; everything else (the embed, the player) is dropped.
      if (/youtube\.com\/vi\/|ytimg\.com/.test(route.request().url())) {
        return route.fulfill({ status: 200, contentType: "image/png", body: poster });
      }
      return route.abort();
    });
    await page.clock.setFixedTime(new Date("2026-11-16T10:24:00Z"));
    await page.addInitScript(seedScript);

    // Boot once on the home route: the app registers its server profile here,
    // which is what the console appearance key is scoped by.
    await page.goto(`${base}/#/`, { waitUntil: "domcontentloaded" });
    await settle(page, "main, h1", 300);

    if (shot.console) await seedConsoleAppearance(page, shot.console);

    const route = shot.route === "game" ? await resolveGameRoute(page, GAME_TITLE) : shot.route;
    // A distinct query string forces a real document load on the target route,
    // so every capture starts from the same cold boot a visitor would get.
    await page.goto(`${base}/?shot=${shot.name}#${route}`, { waitUntil: "domcontentloaded" });
    try {
      await settle(page, shot.waitFor, 700);
      // A real interaction on the console shelf (keyboard navigation), not a
      // seeded state: it selects the second demo game so the hero artwork is
      // not the same flat plate in both appearance captures.
      for (const key of shot.press ?? []) {
        await page.keyboard.press(key);
        await page.waitForTimeout(300);
      }
      await page.waitForTimeout(shot.hold ? shot.hold - 700 : 200);
    } catch (error) {
      fail(`${shot.set}/${shot.name}: ${error.message}${pageErrors.length ? `\n  page errors: ${pageErrors.join(" | ")}` : ""}`);
    }

    const file = join(OUT_ROOT, shot.set, `${shot.name}.${mobile ? "jpg" : "png"}`);
    mkdirSync(dirname(file), { recursive: true });
    const raw = await stableScreenshot(page);
    writeFileSync(join(rawDirPath(), `${shot.set}-${shot.name}.png`), raw);

    if (mobile) {
      await sharp(raw).jpeg({ quality: 82, mozjpeg: true, chromaSubsampling: "4:4:4" }).toFile(file);
    } else {
      await sharp(raw)
        .resize({ width: 1600, kernel: "lanczos3" })
        // Adaptive palette (pngquant-style quality target): a fixed 256-colour
        // palette bands the artwork gradients on the game pages.
        .png({ palette: true, quality: 82, effort: 10, compressionLevel: 9 })
        .toFile(file);
    }

    const bytes = statSync(file).size;
    captured.push({ ...shot, route, viewport: `${viewport.width}x${viewport.height}`, dpr: mobile ? 2 : 1, file, bytes });
    log(`   ${shot.set}/${shot.name}  ${(bytes / 1024).toFixed(0)} KB  ${route}`);
    await context.close();
  }

  // Social preview: rasterize the committed vector at the size GitHub wants.
  if (!only || socialOnly) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 640 }, deviceScaleFactor: 1 });
    await page.goto(`file://${SOCIAL_SVG}`);
    await page.waitForTimeout(300);
    const raw = await page.screenshot({ type: "png" });
    await sharp(raw).png({ palette: true, quality: 90, effort: 10, compressionLevel: 9 }).toFile(SOCIAL_PNG);
    await page.close();
    log(`   social-preview.png  ${(statSync(SOCIAL_PNG).size / 1024).toFixed(0)} KB  1280x640`);
  }

  // Budgets and manifest ----------------------------------------------------
  const problems = [];
  let total = 0;
  for (const shot of captured) {
    total += shot.bytes;
    const limit = shot.viewport === `${MOBILE.width}x${MOBILE.height}` ? BUDGET.mobile : BUDGET.desktop;
    if (shot.bytes > limit) problems.push(`${shot.set}/${shot.name} is ${(shot.bytes / 1024).toFixed(0)} KB (budget ${limit / 1024} KB)`);
  }
  if (total > BUDGET.total) problems.push(`the set is ${(total / 1024 / 1024).toFixed(2)} MB (budget 5 MB)`);

  for (const shot of captured) delete shot.file;
  const manifestPath = join(OUT_ROOT, "manifest.json");
  // Read once and let a missing file be a miss, instead of probing and then
  // re-reading (a check-then-use race, #266).
  let previous = null;
  try {
    previous = readJson(manifestPath);
  } catch {
    previous = null;
  }
  const fresh = captured.map((shot) => ({
    set: shot.set,
    name: shot.name,
    path: `docs/assets/screenshots/${shot.set}/${shot.name}.${shot.viewport === `${MOBILE.width}x${MOBILE.height}` ? "jpg" : "png"}`,
    route: shot.route,
    viewport: shot.viewport,
    dpr: shot.dpr,
    theme: shot.scheme ?? "dark",
    consolePreset: shot.console ?? null,
    locale: "en-US",
    bytes: shot.bytes,
    alt: shot.alt,
  }));
  // `--only <set>` refreshes one set; the manifest keeps the other entries so it
  // always describes the full committed collection.
  const kept = only && previous?.shots ? previous.shots.filter((shot) => shot.set !== only) : [];
  const order = ["readme", "docs", "mobile"];
  const manifestShots = [...kept, ...fresh].sort(
    (a, b) => order.indexOf(a.set) - order.indexOf(b.set) || a.name.localeCompare(b.name),
  );
  const manifestShotsTotal = manifestShots.reduce((sum, shot) => sum + shot.bytes, 0);
  const manifest = {
    appVersion: appVersion(),
    commit: await commit(),
    generatedAt: new Date().toISOString(),
    generator: "e2e/screenshots.mjs",
    notes: [
      "Captured from the web build compiled with VITE_USE_MOCKS=true (demo adapter, no server).",
      "Locale en-US, reduced motion, fixed clock at 2026-11-16T10:24:00Z.",
      "YouTube is blocked: trailer thumbnails are a neutral Ludex poster, the facade and play button are the real UI.",
      "Desktop images are 1920x1080 captures resized to 1600 px wide (PNG, adaptive 82 quality palette).",
      "Mobile images are 390x844 viewports at 2x, kept at 780 px wide (JPEG q82).",
    ],
    shots: manifestShots,
    totalBytes: manifestShotsTotal,
    previous: previous ? { commit: previous.commit, generatedAt: previous.generatedAt } : null,
  };
  writeFileAtomic(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

  log(
    `\n${captured.length} captured this run, ${(manifestShotsTotal / 1024 / 1024).toFixed(2)} MB in the set, ` +
      "manifest at docs/assets/screenshots/manifest.json",
  );
  if (problems.length > 0) fail(`image budget exceeded:\n - ${problems.join("\n - ")}`);
  log(`done in ${((Date.now() - started) / 1000).toFixed(1)}s`);
} finally {
  await browser?.close().catch(() => {});
  if (server) server.kill("SIGTERM");
  if (rawDir) rmSync(rawDir, { recursive: true, force: true });
}
