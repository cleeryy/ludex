#!/usr/bin/env node
/**
 * Cabinet screenshot and overscan check (#268).
 *
 * Captures the Console (Stage, Wall, List, Settings, Power) from the *demo*
 * build of the web UI — `VITE_USE_MOCKS=true`, no server, no archive — at the
 * three TV resolutions the blueprint names: 1280x720, 1920x1080 and 3840x2160.
 *
 * The check that matters is measured, not eyeballed: every visible control must
 * sit inside the action-safe area (the outer 5 % band a TV may crop). The
 * report is written next to the images so a regression is a diff, not a
 * conversation.
 *
 *   node e2e/console-screenshots.mjs               # build, capture, measure
 *   node e2e/console-screenshots.mjs --skip-build  # reuse apps/web/dist
 *   node e2e/console-screenshots.mjs --only 1080   # one resolution
 *
 * Output:
 *   docs/assets/screenshots/console/<view>-<height>.png
 *   docs/assets/screenshots/console/<view>-<height>-overscan.png  (5 % crop)
 *   docs/assets/screenshots/console/manifest.json                 (measurements)
 */
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:net";
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import sharp from "sharp";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const OUT = join(ROOT, "docs", "assets", "screenshots", "console");
const SAFE = 0.05;

const args = process.argv.slice(2);
const skipBuild = args.includes("--skip-build");
const onlyIndex = args.indexOf("--only");
const only = onlyIndex >= 0 ? args[onlyIndex + 1] : null;

const RESOLUTIONS = [
  { height: 720, width: 1280 },
  { height: 1080, width: 1920 },
  { height: 2160, width: 3840 },
];

/** The five Cabinet views, captured at every TV resolution (#267 routes). */
const VIEWS = [
  { id: "stage", label: "Stage", layout: "shelf", route: "/console" },
  { id: "wall", label: "Wall", layout: "grid", route: "/console" },
  { id: "list", label: "List", layout: "compact", route: "/console" },
  { id: "settings", label: "Settings · Appearance", layout: "shelf", route: "/console/settings/appearance" },
  { id: "power", label: "Power", layout: "shelf", route: "/console/power" },
];

function log(message) {
  process.stdout.write(`${message}\n`);
}

function fail(message) {
  process.stderr.write(`\n✗ ${message}\n`);
  process.exit(1);
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

function run(command, argv, { env = {} } = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, argv, { cwd: ROOT, env: { ...process.env, ...env }, stdio: "inherit" });
    child.on("error", reject);
    child.on("close", (code) => (code === 0 ? resolvePromise() : reject(new Error(`${command} exited ${code}`))));
  });
}

function commit() {
  try {
    return execFileSync("git", ["rev-parse", "--short", "HEAD"], { cwd: ROOT, encoding: "utf8" }).trim();
  } catch {
    return "unknown";
  }
}

/** Seeds the demo session and the Cabinet room, scoped per server like the app. */
function seedRoom({ room = "night", layout = "shelf" } = {}) {
  localStorage.setItem("ludex.token", "mock.1.0");
  localStorage.setItem("ludex.locale", "en");
  localStorage.setItem("ludex.pwa.installDismissedAt", "1");
  localStorage.setItem("ludex.onboarding.gettingStartedDismissed", "1");
  sessionStorage.setItem("ludex.onboarding.skipped", "1");
  const servers = JSON.parse(localStorage.getItem("ludex.servers.v1") ?? "[]");
  const active = localStorage.getItem("ludex.activeServer.v1");
  const scoped = servers.find((server) => server.id === active);
  const prefix = scoped ? `ludex.server.${scoped.id}.` : "";
  localStorage.setItem(`${prefix}ludex.bigPicture.layout`, layout);
  localStorage.setItem(
    `${prefix}ludex.console.room`,
    JSON.stringify({
      v: 3,
      room: {
        room,
        accent: "theme",
        wall: "artwork",
        corners: "soft",
        density: "comfortable",
        motion: "full",
        sound: "reduced",
        haptics: "standard",
      },
    }),
  );
}

async function settle(page, selector, hold = 700) {
  await page.waitForSelector(selector, { timeout: 30_000 });
  await page.evaluate(async () => {
    await document.fonts.ready;
  });
  // The shell fades in; a capture taken during that fade is a black frame.
  await page.waitForFunction(
    () => {
      const visible = (element) => {
        if (!element) return true;
        const style = getComputedStyle(element);
        return Number(style.opacity) > 0.99 && style.visibility !== "hidden";
      };
      return [document.querySelector("header"), document.querySelector("h1")].every(visible);
    },
    null,
    { timeout: 10_000 },
  );
  await page.waitForLoadState("networkidle").catch(() => {});
  await page.waitForTimeout(hold);
}

/**
 * True when two frames differ by more than a few live pixels. The clock ticks
 * and a download percentage moves; those are the real UI. A reveal, a late
 * artwork or a scroll is bigger by orders of magnitude.
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
    if (
      Math.abs(a.data[index] - b.data[index]) > 8 ||
      Math.abs(a.data[index + 1] - b.data[index + 1]) > 8 ||
      Math.abs(a.data[index + 2] - b.data[index + 2]) > 8
    ) {
      changed += 1;
    }
  }
  return changed / pixels > tolerance;
}

/**
 * Captures only a settled frame. The Console reveals its chrome in a short
 * cascade and artwork can land after the first paint, so a fixed wait is a
 * guess: a capture taken mid-fade is a black rectangle, not a screenshot.
 */
async function stableScreenshot(page, { attempts = 16, interval = 250 } = {}) {
  let previous = await page.screenshot({ type: "png" });
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    await page.waitForTimeout(interval);
    const next = await page.screenshot({ type: "png" });
    if (!(await framesDiffer(previous, next))) return next;
    previous = next;
  }
  throw new Error("the console never stopped repainting; a capture would catch an animation");
}

/**
 * Every visible control inside the 5 % action-safe area. Returns the measured
 * boxes and the ones that enter the outer band.
 */
async function measureSafeArea(page) {
  return page.evaluate((safeFraction) => {
    const width = window.innerWidth;
    const height = window.innerHeight;
    const insetX = Math.round(width * safeFraction);
    const insetY = Math.round(height * safeFraction);
    const selectors = [
      "[data-bp-card='true']",
      "[data-bp-band-item='true']",
      "[data-bp-settings-row='true']",
      "[data-bp-action='true'] button",
      // The footer and the rails are full-bleed strips whose *content* is
      // padded; measure the hint keys, not the strip behind them.
      "[data-bp-footer='true'] .kbd",
      "[data-bp-footer='true'] > span:last-child",
    ];
    const boxes = [];
    const violations = [];
    for (const selector of selectors) {
      for (const element of Array.from(document.querySelectorAll(selector))) {
        const rect = element.getBoundingClientRect();
        if (rect.width < 1 || rect.height < 1) continue;
        const style = window.getComputedStyle(element);
        if (style.visibility === "hidden" || style.display === "none") continue;
        // A card peeking at the edge of a rail is scrolling content, not a
        // control the crop can eat: only fully painted controls are measured.
        if (rect.left < -1 || rect.top < -1 || rect.right > window.innerWidth + 1 || rect.bottom > window.innerHeight + 1) {
          continue;
        }
        // Clip to every scrolling ancestor: a row half-scrolled under the
        // footer is painted up to the clip edge, and only that part exists.
        let top = rect.top;
        let bottom = rect.bottom;
        let left = rect.left;
        let right = rect.right;
        for (let parent = element.parentElement; parent; parent = parent.parentElement) {
          const style = window.getComputedStyle(parent);
          const scrolls =
            /(auto|scroll|hidden)/.test(style.overflowY) || /(auto|scroll|hidden)/.test(style.overflowX);
          if (!scrolls) continue;
          const clip = parent.getBoundingClientRect();
          top = Math.max(top, clip.top);
          bottom = Math.min(bottom, clip.bottom);
          left = Math.max(left, clip.left);
          right = Math.min(right, clip.right);
        }
        if (bottom - top < 8 || right - left < 8) continue;
        const box = {
          selector,
          left: Math.round(left),
          top: Math.round(top),
          right: Math.round(right),
          bottom: Math.round(bottom),
        };
        boxes.push(box);
        if (
          box.left < insetX - 1 ||
          box.top < insetY - 1 ||
          box.right > width - insetX + 1 ||
          box.bottom > height - insetY + 1
        ) {
          violations.push(box);
        }
      }
    }
    return { width, height, insetX, insetY, boxes, violations };
  }, SAFE);
}

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
  server = spawn(
    "pnpm",
    ["--filter", "@ludex/web", "exec", "vite", "preview", "--host", "127.0.0.1", "--port", String(port), "--strictPort"],
    { cwd: ROOT, stdio: "ignore" },
  );
  server.on("error", (error) => fail(`vite preview failed: ${error.message}`));
  await waitForHttp(base);

  mkdirSync(OUT, { recursive: true });
  browser = await chromium.launch();

  const captures = [];
  for (const resolution of RESOLUTIONS) {
    if (only && only !== String(resolution.height) && only !== "all") continue;
    const views = VIEWS;
    for (const view of views) {
      const context = await browser.newContext({
        viewport: { width: resolution.width, height: resolution.height },
        deviceScaleFactor: 1,
        colorScheme: "dark",
        locale: "en-US",
      });
      const page = await context.newPage();
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
      await page.clock.setFixedTime(new Date("2026-11-16T10:24:00Z"));
      await page.addInitScript(seedRoom, { room: "night", layout: view.layout });

      // Boot once so the demo server registers its id, then seed the room key
      // against that id and cold-load the console.
      await page.goto(`${base}/#/`, { waitUntil: "domcontentloaded" });
      await settle(page, "main, h1", 300);
      await page.evaluate(seedRoom, { room: "night", layout: view.layout });
      await page.goto(`${base}/?view=${view.id}#${view.route}`, { waitUntil: "domcontentloaded" });
      await settle(page, "[data-bp-band-item='true'], [data-bp-settings-row='true'], h1", 900);

      const measurement = await measureSafeArea(page);
      const raw = await stableScreenshot(page);
      const name = `${view.id}-${resolution.height}`;
      const file = join(OUT, `${name}.png`);
      // 4K is stored at 1920 wide; the measurement above is the 4K one.
      const pipeline = sharp(raw);
      if (resolution.width > 1920) pipeline.resize({ width: 1920, kernel: "lanczos3" });
      await pipeline.png({ palette: true, quality: 88, effort: 10, compressionLevel: 9 }).toFile(file);

      // A 5 % crop of each side: what a TV with overscan actually shows.
      const cropped = await sharp(raw)
        .extract({
          left: measurement.insetX,
          top: measurement.insetY,
          width: resolution.width - measurement.insetX * 2,
          height: resolution.height - measurement.insetY * 2,
        })
        .png({ palette: true, quality: 88, effort: 10, compressionLevel: 9 })
        .toFile(join(OUT, `${name}-overscan.png`));

      captures.push({
        view: view.id,
        label: view.label,
        resolution: `${resolution.width}x${resolution.height}`,
        file: `docs/assets/screenshots/console/${name}.png`,
        overscanFile: `docs/assets/screenshots/console/${name}-overscan.png`,
        safeArea: `${measurement.insetX}x${measurement.insetY}`,
        measured: measurement.boxes.length,
        violations: measurement.violations,
        overscan: { width: cropped.width, height: cropped.height },
        pageErrors,
      });
      log(
        `   ${name}  ${measurement.boxes.length} controls measured, ` +
          `${measurement.violations.length} inside the ${SAFE * 100}% band` +
          `${pageErrors.length ? `  (page errors: ${pageErrors.join(" | ")})` : ""}`,
      );
      for (const box of measurement.violations) {
        log(`      ${box.selector} ${box.left},${box.top} → ${box.right},${box.bottom}`);
      }
      await context.close();
    }
  }

  const violations = captures.flatMap((capture) =>
    capture.violations.map((box) => `${capture.view}/${capture.resolution}: ${box.selector} at ${box.left},${box.top}`),
  );
  const errors = captures.flatMap((capture) =>
    capture.pageErrors.map((error) => `${capture.view}/${capture.resolution}: ${error}`),
  );

  writeFileSync(
    join(OUT, "manifest.json"),
    `${JSON.stringify(
      {
        generator: "e2e/console-screenshots.mjs",
        commit: commit(),
        generatedAt: new Date().toISOString(),
        overscan: SAFE,
        notes: [
          "Captured from the web build compiled with VITE_USE_MOCKS=true (demo adapter, no server).",
          "Locale en-US, reduced motion, fixed clock at 2026-11-16T10:24:00Z, Night room.",
          "`-overscan` images are a 5 % crop of each side: what a TV with overscan shows.",
          "4K captures are measured at 3840x2160 and stored at 1920 px wide.",
          "Every listed control must sit inside the action-safe area; `violations` must stay empty.",
        ],
        captures,
        violations,
      },
      null,
      2,
    )}\n`,
  );

  if (errors.length > 0) fail(`page errors:\n - ${errors.join("\n - ")}`);
  if (violations.length > 0) fail(`controls inside the ${SAFE * 100}% overscan band:\n - ${violations.join("\n - ")}`);
  log(`\n${captures.length} captures, 0 overscan violations, done in ${((Date.now() - started) / 1000).toFixed(1)}s`);
} finally {
  await browser?.close().catch(() => {});
  if (server) server.kill("SIGTERM");
}
