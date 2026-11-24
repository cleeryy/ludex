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
 * Visual gate (#303): Stage and Wall are compared against the approved
 * baselines instead of being overwritten. The gate is tolerant of font
 * rasterisation and palette-quantisation noise (see `GATE_TOLERANCE`) and
 * fails on anything larger than that, which is what a layout regression is.
 *
 *   node e2e/console-screenshots.mjs --gate --skip-build
 *   LUDEX_VISUAL_UPDATE=1 node e2e/console-screenshots.mjs   # re-approve
 *
 * Output:
 *   docs/assets/screenshots/console/<view>-<height>.png
 *   docs/assets/screenshots/console/<view>-<height>-overscan.png  (5 % crop)
 *   docs/assets/screenshots/console/manifest.json                 (measurements)
 *   e2e/test-results/console-visual-gate/<frame>.diff.png         (gate failure)
 */
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:net";
import { mkdirSync, statSync, writeFileSync } from "node:fs";
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
const gate = args.includes("--gate");
/**
 * `--gate` compares instead of writing; `LUDEX_VISUAL_UPDATE=1` turns the same
 * command back into the approval run, so one command re-approves the set after
 * a redesign and the diff is never a hand edit of a PNG.
 */
const update = process.env.LUDEX_VISUAL_UPDATE === "1";
const compareOnly = gate && !update;
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

/**
 * The gated views (#303). Stage and Wall are the two compositions a player
 * actually browses; List/Settings/Power carry release-specific content (the
 * version in the Settings footer) and stay capture-only, approved by a human
 * through #265. A gate that fails on every version bump stops being read.
 */
const GATED_VIEWS = ["stage", "wall"];

/**
 * What the gate compares, and what it forgives (#303).
 *
 * Both frames are normalised to `GATE_WIDTH` with a Lanczos downscale and a
 * 1 px blur before the diff. That is the measurement decision: a 4K frame and
 * a 720p frame are judged by the same geometry, palette quantisation (the
 * baselines are 88-quality palette PNGs) stops being a per-pixel lottery, and
 * font hinting/antialiasing becomes sub-pixel noise. Structural facts survive
 * the normalisation: a cover is ~3 % of the frame at 720 px wide, a moved band
 * strip ~5 %.
 *
 * A pixel counts as changed when any channel moves by more than `perChannel`
 * (out of 255). A frame fails when more than `maxRatio` of its pixels changed.
 * Measured against the committed baseline on this host, twice: worst frame
 * 0.152 % then 0.076 %, so 0.5 % is >3x headroom. Sensitivity was checked on
 * the same frames: a band shifted 12 px fails at 1.116 %, a hero title moved
 * 12 px at 1.052 %, a hidden cover at 23.093 % — a real layout change is not
 * in the same order of magnitude as the noise.
 */
const GATE_TOLERANCE = { perChannel: 24, maxRatio: 0.005, width: 720, blur: 1 };

/** Where a failed frame's diff is written for the reviewer. */
const GATE_OUT = join(ROOT, "e2e", "test-results", "console-visual-gate");

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
  // Keep online state deterministic across sandboxed/offline hosts, just like the fixed clock.
  try {
    Object.defineProperty(navigator, "onLine", { get: () => true, configurable: true });
  } catch {
    /* read-only in some engines */
  }
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
  await page
    .waitForFunction(
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
    )
    .catch(async (cause) => {
      // A bare timeout hides the one fact a reviewer needs: which element is
      // still faded and what the page actually is.
      const state = await page.evaluate(() => {
        const read = (element) => {
          if (!element) return "missing";
          const style = getComputedStyle(element);
          return `${style.opacity}/${style.visibility}/${style.display}`;
        };
        return {
          url: location.href,
          title: document.title,
          header: read(document.querySelector("header")),
          h1: read(document.querySelector("h1")),
          text: document.body.innerText.slice(0, 160),
        };
      });
      throw new Error(
        `the console never settled after ${selector}: ${JSON.stringify(state)} (${cause.message})`,
      );
    });
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
 * Compare one encoded frame against its approved baseline (#303).
 *
 * Both sides are decoded, so the comparison is on pixels, not file bytes: the
 * palette encoder is deterministic but a caption's antialiasing is not. The
 * changed pixels are painted into a diff PNG so a failure names its own cause
 * instead of "0.31 % differ".
 */
async function compareFrame(candidate, baselineFile, name) {
  const normalise = (input) =>
    sharp(input)
      .resize({ width: GATE_TOLERANCE.width, kernel: "lanczos3" })
      .blur(GATE_TOLERANCE.blur)
      .raw()
      .toBuffer({ resolveWithObject: true });

  const baseline = await normalise(baselineFile).catch(() => null);
  if (!baseline) return { name, ok: false, reason: `baseline missing: ${baselineFile}` };
  const actual = await normalise(candidate);
  if (actual.info.width !== baseline.info.width || actual.info.height !== baseline.info.height) {
    return {
      name,
      ok: false,
      reason: `size ${actual.info.width}x${actual.info.height} != baseline ${baseline.info.width}x${baseline.info.height}`,
    };
  }

  const channels = actual.info.channels;
  const pixels = actual.info.width * actual.info.height;
  const diff = Buffer.alloc(actual.info.width * actual.info.height * 3);
  let changed = 0;
  for (let index = 0; index < actual.data.length; index += channels) {
    const raw = Math.abs(actual.data[index] - baseline.data[index]);
    const green = Math.abs(actual.data[index + 1] - baseline.data[index + 1]);
    const blue = Math.abs(actual.data[index + 2] - baseline.data[index + 2]);
    const hit = raw > GATE_TOLERANCE.perChannel
      || green > GATE_TOLERANCE.perChannel
      || blue > GATE_TOLERANCE.perChannel;
    const pixel = index / channels;
    if (hit) {
      changed += 1;
      // Red on a dim grey version of the candidate: the geometry stays legible.
      diff[pixel * 3] = 255;
      diff[pixel * 3 + 1] = 32;
      diff[pixel * 3 + 2] = 32;
    } else {
      const luma = Math.round((actual.data[index] + actual.data[index + 1] + actual.data[index + 2]) / 12);
      diff[pixel * 3] = luma;
      diff[pixel * 3 + 1] = luma;
      diff[pixel * 3 + 2] = luma;
    }
  }

  const ratio = changed / pixels;
  if (ratio > GATE_TOLERANCE.maxRatio) {
    mkdirSync(GATE_OUT, { recursive: true });
    await sharp(diff, { raw: { width: actual.info.width, height: actual.info.height, channels: 3 } })
      .png()
      .toFile(join(GATE_OUT, `${name}.diff.png`));
  }
  return { name, ok: ratio <= GATE_TOLERANCE.maxRatio, ratio, changed, pixels };
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

  log(
    compareOnly
      ? "== visual gate: Stage/Wall vs the approved baselines =="
      : update
        ? "== visual update: writing the approved baselines =="
        : "== capture: writing every view and the manifest ==",
  );

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
  const gateResults = [];
  for (const resolution of RESOLUTIONS) {
    if (only && only !== String(resolution.height) && only !== "all") continue;
    const views = compareOnly ? VIEWS.filter((view) => GATED_VIEWS.includes(view.id)) : VIEWS;
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
      const encoded = await pipeline
        .png({ palette: true, quality: 88, effort: 10, compressionLevel: 9 })
        .toBuffer();

      // A 5 % crop of each side: what a TV with overscan actually shows.
      const crop = {
        left: measurement.insetX,
        top: measurement.insetY,
        width: resolution.width - measurement.insetX * 2,
        height: resolution.height - measurement.insetY * 2,
      };
      const encodedOverscan = await sharp(raw)
        .extract(crop)
        .png({ palette: true, quality: 88, effort: 10, compressionLevel: 9 })
        .toBuffer();

      if (compareOnly) {
        // The gate compares exactly what the approved pipeline would have
        // written: same resize, same palette encode, decoded and diffed.
        gateResults.push(
          await compareFrame(encoded, file, name),
          await compareFrame(encodedOverscan, join(OUT, `${name}-overscan.png`), `${name}-overscan`),
        );
      } else {
        writeFileSync(file, encoded);
        writeFileSync(join(OUT, `${name}-overscan.png`), encodedOverscan);
      }

      captures.push({
        view: view.id,
        label: view.label,
        resolution: `${resolution.width}x${resolution.height}`,
        file: `docs/assets/screenshots/console/${name}.png`,
        overscanFile: `docs/assets/screenshots/console/${name}-overscan.png`,
        safeArea: `${measurement.insetX}x${measurement.insetY}`,
        measured: measurement.boxes.length,
        violations: measurement.violations,
        overscan: { width: crop.width, height: crop.height },
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

  if (!compareOnly) {
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
  }

  if (errors.length > 0) fail(`page errors:\n - ${errors.join("\n - ")}`);
  if (violations.length > 0) fail(`controls inside the ${SAFE * 100}% overscan band:\n - ${violations.join("\n - ")}`);

  if (compareOnly) {
    const failures = gateResults.filter((result) => !result.ok);
    for (const result of gateResults) {
      const detail = result.ok
        ? `${result.changed}/${result.pixels} px over ±${GATE_TOLERANCE.perChannel} (${(result.ratio * 100).toFixed(3)}%)`
        : (result.reason ?? `${result.changed}/${result.pixels} px (${(result.ratio * 100).toFixed(3)}%)`);
      log(`   ${result.ok ? "✓" : "✗"} ${result.name}  ${detail}`);
    }
    if (failures.length > 0) {
      fail(
        `${failures.length} console frame(s) outside the approved baseline ` +
          `(tolerance: ±${GATE_TOLERANCE.perChannel}/255 per channel, ` +
          `${(GATE_TOLERANCE.maxRatio * 100).toFixed(1)}% of pixels). ` +
          `Diffs: ${GATE_OUT}\n` +
          failures.map((result) => ` - ${result.name}: ${result.reason ?? (result.ratio * 100).toFixed(3) + "%"}`).join("\n"),
      );
    }
    log(
      `\n${gateResults.length} frames within the approved baseline ` +
        `(±${GATE_TOLERANCE.perChannel}/255, ${(GATE_TOLERANCE.maxRatio * 100).toFixed(1)}% max), ` +
        `done in ${((Date.now() - started) / 1000).toFixed(1)}s`,
    );
  } else {
    log(`\n${captures.length} captures, 0 overscan violations, done in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  }
} finally {
  await browser?.close().catch(() => {});
  if (server) server.kill("SIGTERM");
}
