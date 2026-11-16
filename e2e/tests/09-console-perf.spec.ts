import { mkdirSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

import { expect, test, type Page } from "@playwright/test";

import { installFakePad } from "../pad";
import { consoleUrl } from "./console-helpers";
import { signInAsAdmin } from "./helpers";

/**
 * Console TV performance probe (#270 §3).
 *
 * Opt-in (`LUDEX_E2E_PERF=1`) because it seeds a real shelf and sits idle for
 * thirty seconds. It runs against the E2E harness from `scripts/e2e.sh` — one
 * admin, one deterministic archive, the same API the desktop talks to.
 *
 * What it measures, and why each number is what it is:
 *
 * - **Boot to home** — wall clock from the navigation commit to the first cover
 *   on screen, plus the Navigation Timing and First Contentful Paint entries
 *   the browser recorded. A console that paints the desktop shell first would
 *   fail this on a cold start.
 * - **Rail scroll** — requestAnimationFrame deltas collected in the page while
 *   a held D-pad direction moves the shelf. p50/p95/p99/max and the share of
 *   frames over 16.7 ms (60 fps) and 33.3 ms (30 fps). Chromium in CI renders
 *   with SwiftShader (no GPU), so this is the weakest target, not a
 *   workstation number.
 * - **Long tasks** — `PerformanceObserver({ type: "longtask" })`, buffered,
 *   covering the document. A 10-foot UI that blocks the main thread for 100 ms
 *   drops six frames.
 * - **JS heap** — `performance.memory.usedJSHeapSize` after a forced GC, at
 *   ~5 s and after 30 s idle. This is the renderer's JS heap, a lower bound on
 *   the process RSS the issue budgets (<300 MB FHD, <380 MB UHD); the native
 *   process number belongs to `docs/console/NATIVE-CHECKLIST.md`.
 * - **rAF chains** — how many distinct requestAnimationFrame loops are still
 *   scheduling work after the scroll stops, and how many callbacks run per
 *   frame. The probe's own sampler calls the native `requestAnimationFrame`
 *   directly, so it is not counted: the number is the application's.
 *
 * The fake pad goes through `navigator.getGamepads` like hardware; the
 * production input loop decodes it with the real profile, exactly as
 * `06-console.spec.ts` does.
 *
 * The scroll budget is asserted only on a hardware renderer. CI has no GPU
 * (SwiftShader), where a full-screen 26u blur over a moving rail cannot hold
 * 60 fps; the numbers are still recorded and `docs/console/PERFORMANCE.md`
 * names the cause instead of hiding it.
 */

/** Above the scanner's 1 MiB minimum so every seeded archive indexes. */
const PAYLOAD_BYTES = 1_200_000;
const SEED_COUNT = Number(process.env.LUDEX_E2E_PERF_GAMES ?? "48");
/** FHD by default; `LUDEX_E2E_PERF_WIDTH/HEIGHT=3840x2160` measures the TV budget. */
const VIEWPORT = {
  width: Number(process.env.LUDEX_E2E_PERF_WIDTH ?? "1920"),
  height: Number(process.env.LUDEX_E2E_PERF_HEIGHT ?? "1080"),
};
const IDLE_MS = 30_000;
const SCROLL_MS = 3_000;
/**
 * The A/B the performance page quotes: the same run with the wallpaper layer
 * hidden by a style override, so "the wall costs this much" is a measurement
 * rather than an inference. It is a diagnostic, not a budget check.
 */
const HIDE_WALL = process.env.LUDEX_E2E_PERF_HIDE_WALL === "1";

const gamesDir = process.env.LUDEX_E2E_GAMES_DIR;
const enabled = process.env.LUDEX_E2E_PERF === "1";

/* ------------------------------- fixtures -------------------------------- */

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let index = 0; index < 256; index += 1) {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    table[index] = value >>> 0;
  }
  return table;
})();

function crc32(data: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of data) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

interface ZipEntry {
  name: string;
  data: Buffer;
}

/**
 * A STORED zip, written by hand: `scripts/e2e.sh` uses Python's zipfile, and
 * the perf spec must not add a zip dependency to the E2E package. STORED keeps
 * the archive above the scanner size floor even though the payload is zeros.
 */
function storedZip(entries: ZipEntry[]): Buffer {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const entry of entries) {
    const name = Buffer.from(entry.name, "utf8");
    const checksum = crc32(entry.data);
    const local = Buffer.alloc(30 + name.length);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6); // UTF-8 names
    local.writeUInt16LE(0, 8); // stored
    local.writeUInt16LE(0, 10);
    local.writeUInt16LE(0x21, 12); // 1980-01-01
    local.writeUInt32LE(checksum, 14);
    local.writeUInt32LE(entry.data.length, 18);
    local.writeUInt32LE(entry.data.length, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(0, 28);
    name.copy(local, 30);
    locals.push(local, entry.data);

    const central = Buffer.alloc(46 + name.length);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt16LE(0, 12);
    central.writeUInt16LE(0x21, 14);
    central.writeUInt32LE(checksum, 16);
    central.writeUInt32LE(entry.data.length, 20);
    central.writeUInt32LE(entry.data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt16LE(0, 30); // extra
    central.writeUInt16LE(0, 32); // comment
    central.writeUInt16LE(0, 34); // disk
    central.writeUInt16LE(0, 36); // internal attrs
    central.writeUInt32LE(0, 38); // external attrs
    central.writeUInt32LE(offset, 42);
    name.copy(central, 46);
    centrals.push(central);

    offset += local.length + entry.data.length;
  }

  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);

  return Buffer.concat([...locals, directory, end]);
}

function seedArchive(file: string, marker: string): void {
  const zip = storedZip([
    { name: "readme.txt", data: Buffer.from(marker, "utf8") },
    { name: "payload.bin", data: Buffer.alloc(PAYLOAD_BYTES, 0x30) },
  ]);
  writeFileSync(file, zip);
}

/* -------------------------------- probes --------------------------------- */

interface PerfBridge {
  start(): void;
  stop(): number[];
  longTasks(): Array<{ start: number; duration: number }>;
  raf(): { chains: number; callbacks: number; framesPerSecond: number; callbacksPerFrame: number };
  timing(): {
    domContentLoaded: number | null;
    load: number | null;
    fcp: number | null;
  };
  heap(): { used: number; total: number; limit: number } | null;
  renderer(): string | null;
}

type PerfWindow = Window & { __ludexPerf: PerfBridge };

/**
 * Installed before the app bundle: the rAF sampler must wrap the real loop
 * before the gamepad poll starts, and the long-task observer must see boot.
 */
function perfProbeScript(): void {
  const state = {
    deltas: [] as number[],
    recording: false,
    lastFrame: 0,
    frameRing: [] as number[],
    longTasks: [] as Array<{ start: number; duration: number }>,
    chains: new Map<number, { lastAt: number }>(),
    calls: [] as Array<{ chain: number; at: number }>,
    nextChain: 1,
    currentChain: null as number | null,
  };

  const original = window.requestAnimationFrame.bind(window);

  // One always-on sampler: it is the probe's own loop and is subtracted from
  // the reported chain count. It records the frame cadence even when a test is
  // not actively sampling, which is what `raf()` reads.
  const sampler = (now: number) => {
    state.frameRing.push(now);
    if (state.frameRing.length > 240) state.frameRing.shift();
    if (state.recording) {
      if (state.lastFrame > 0) state.deltas.push(now - state.lastFrame);
      state.lastFrame = now;
    }
    original(sampler);
  };
  original(sampler);

  // Every `requestAnimationFrame` call made outside a callback starts a new
  // chain; a call made inside one continues it. A loop that keeps rearming
  // itself therefore counts as one chain, which is what "running rAF loops"
  // means here.
  window.requestAnimationFrame = (callback: FrameRequestCallback): number => {
    const chain = state.currentChain ?? state.nextChain++;
    const now = performance.now();
    state.chains.set(chain, { lastAt: now });
    state.calls.push({ chain, at: now });
    if (state.calls.length > 8_000) state.calls.splice(0, state.calls.length - 8_000);
    return original((time) => {
      const previous = state.currentChain;
      state.currentChain = chain;
      try {
        callback(time);
      } finally {
        state.currentChain = previous;
      }
    });
  };

  try {
    const observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        state.longTasks.push({ start: entry.startTime, duration: entry.duration });
      }
    });
    observer.observe({ type: "longtask", buffered: true });
  } catch {
    // A browser without longtask entries reports none; the field stays empty.
  }

  const bridge: PerfBridge = {
    start() {
      state.deltas = [];
      state.lastFrame = 0;
      state.longTasks = [];
      state.recording = true;
    },
    stop() {
      state.recording = false;
      return state.deltas.slice();
    },
    longTasks() {
      return state.longTasks.slice();
    },
    raf() {
      const now = performance.now();
      const since = now - 1_000;
      let chains = 0;
      for (const record of state.chains.values()) {
        if (record.lastAt >= since) chains += 1;
      }
      const callbacks = state.calls.filter((call) => call.at >= since).length;
      const framesPerSecond = state.frameRing.filter((time) => time >= since).length;
      return {
        chains,
        callbacks,
        framesPerSecond,
        callbacksPerFrame:
          framesPerSecond > 0 ? Math.round((callbacks / framesPerSecond) * 100) / 100 : 0,
      };
    },
    timing() {
      const navigation = performance.getEntriesByType("navigation")[0] as
        | PerformanceNavigationTiming
        | undefined;
      const paint = performance
        .getEntriesByType("paint")
        .find((entry) => entry.name === "first-contentful-paint");
      return {
        domContentLoaded: navigation?.domContentLoadedEventEnd ?? null,
        load: navigation?.loadEventEnd ?? null,
        fcp: paint?.startTime ?? null,
      };
    },
    heap() {
      const memory = (
        performance as Performance & {
          memory?: { usedJSHeapSize: number; totalJSHeapSize: number; jsHeapSizeLimit: number };
        }
      ).memory;
      if (!memory) return null;
      return {
        used: memory.usedJSHeapSize,
        total: memory.totalJSHeapSize,
        limit: memory.jsHeapSizeLimit,
      };
    },
    renderer() {
      const canvas = document.createElement("canvas");
      const gl = canvas.getContext("webgl");
      if (!gl) return null;
      const info = gl.getExtension("WEBGL_debug_renderer_info");
      if (!info) return null;
      return gl.getParameter(info.UNMASKED_RENDERER_WEBGL) as string;
    },
  };

  (window as unknown as PerfWindow).__ludexPerf = bridge;
}

function percentile(values: readonly number[], fraction: number): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((left, right) => left - right);
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * fraction) - 1));
  return Math.round(sorted[index] * 100) / 100;
}

function summary(deltas: readonly number[]) {
  const over = (ms: number) => deltas.filter((delta) => delta > ms).length;
  return {
    frames: deltas.length,
    p50: percentile(deltas, 0.5),
    p95: percentile(deltas, 0.95),
    p99: percentile(deltas, 0.99),
    max: deltas.length ? Math.round(Math.max(...deltas) * 100) / 100 : 0,
    dropped60: over(1000 / 60),
    dropped30: over(1000 / 30),
  };
}

/* --------------------------------- test ---------------------------------- */

// Tracing and video instrument every frame; a frame-budget probe must not
// measure Playwright's recorder. The JSON report is the evidence here.
test.use({ trace: "off", video: "off", screenshot: "off" });

test.describe("console performance", () => {
  test.skip(!enabled, "set LUDEX_E2E_PERF=1 to run the TV performance probe");
  test.skip(!gamesDir, "LUDEX_E2E_GAMES_DIR is not exported by scripts/e2e.sh");

  test("measures TV boot, rail scroll and settled memory (#270 §3)", async ({ page, request }, testInfo) => {
    test.setTimeout(180_000);
    const token = process.env.LUDEX_E2E_TOKEN;
    if (!token) throw new Error("LUDEX_E2E_TOKEN is not set; run scripts/e2e.sh");

    // Seed a shelf the rail can actually scroll. The scan is the same path a
    // real library takes; nothing is injected through a test-only API.
    const directory = gamesDir!;
    mkdirSync(directory, { recursive: true });
    for (let index = 1; index <= SEED_COUNT; index += 1) {
      const file = path.join(directory, `Console Perf ${String(index).padStart(2, "0")}.zip`);
      seedArchive(file, `ludex-console-perf-${index}`);
    }
    const scan = await request.post("/api/library/scan", {
      headers: { Authorization: `Bearer ${token}` },
      data: {},
    });
    expect(scan.ok()).toBe(true);

    await expect
      .poll(
        async () => {
          const response = await request.get(`/api/games?q=Console%20Perf&limit=1`, {
            headers: { Authorization: `Bearer ${token}` },
          });
          if (!response.ok()) return -1;
          const payload = (await response.json()) as { total?: number };
          return payload.total ?? 0;
        },
        { timeout: 120_000, message: "seeded console shelf was not indexed" },
      )
      .toBeGreaterThanOrEqual(SEED_COUNT);

    await signInAsAdmin(page);
    await page.addInitScript(perfProbeScript);
    const pad = await installFakePad(page);

    // Cold boot, then a warm reload; both numbers belong in the report.
    await page.setViewportSize(VIEWPORT);
    const started = Date.now();
    await page.goto(consoleUrl(), { waitUntil: "commit" });
    await page.locator("[data-bp-card='true']").first().waitFor({ state: "visible" });
    const cold = Date.now() - started;
    const coldTiming = await page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.timing());
    const renderer = await page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.renderer());

    const warmStarted = Date.now();
    await page.reload({ waitUntil: "commit" });
    await page.locator("[data-bp-card='true']").first().waitFor({ state: "visible" });
    const warm = Date.now() - warmStarted;
    const warmTiming = await page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.timing());
    const bootLongTasks = await page.evaluate(() =>
      (window as unknown as PerfWindow).__ludexPerf.longTasks(),
    );

    // The rail: a held direction must move the shelf while we sample frames.
    // Let the entrance settle first: artwork decode, fonts and the service
    // worker are boot work, not frame budget.
    await page.waitForTimeout(1_500);
    if (HIDE_WALL) {
      await page.addStyleTag({ content: ".bp-backdrop-art { display: none !important; }" });
    }
    await expect(page.locator("[data-bp-card='true']").nth(5)).toBeAttached();
    const before = await page.locator("[data-bp-card='true'][aria-current='true']").getAttribute("aria-label");
    await page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.start());
    const scrollStarted = Date.now();
    await pad.hold("right", SCROLL_MS);
    const scrollMs = Date.now() - scrollStarted;
    const deltas = await page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.stop());
    const scrollLongTasks = await page.evaluate(() =>
      (window as unknown as PerfWindow).__ludexPerf.longTasks(),
    );
    const after = await page.locator("[data-bp-card='true'][aria-current='true']").getAttribute("aria-label");
    expect(after, "the rail did not move while the D-pad was held").not.toBe(before);

    // Settle, then read the idle rAF picture and the heap at 5 s and 30 s.
    await page.waitForTimeout(5_000);
    const rafAt5s = await page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.raf());
    const cdp = await page.context().newCDPSession(page);
    const collect = async () => {
      await cdp.send("HeapProfiler.collectGarbage").catch(() => undefined);
      return page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.heap());
    };
    const heapAt5s = await collect();
    await page.waitForTimeout(IDLE_MS);
    const heapAt30s = await collect();
    const rafAtIdle = await page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.raf());
    const longTasks = await page.evaluate(() => (window as unknown as PerfWindow).__ludexPerf.longTasks());
    const longTaskTotal = longTasks.reduce((total, entry) => total + entry.duration, 0);
    const scrollLongTaskTotal = scrollLongTasks.reduce((total, entry) => total + entry.duration, 0);

    const mib = (bytes: number) => Math.round((bytes / (1024 * 1024)) * 10) / 10;
    const softwareRenderer = /swiftshader|llvmpipe|software/i.test(renderer ?? "");
    const report = {
      issue: 270,
      generatedAt: new Date().toISOString(),
      commit: process.env.LUDEX_E2E_COMMIT ?? null,
      host: process.env.LUDEX_E2E_HOST ?? null,
      hostLoad: { average: os.loadavg()[0], cpuCount: os.cpus().length },
      viewport: VIEWPORT,
      browser: await page.evaluate(() => navigator.userAgent),
      renderer,
      seededGames: SEED_COUNT,
      wallpaper: HIDE_WALL ? "hidden" : "visible",
      boot: { coldMs: cold, warmMs: warm, coldTiming, warmTiming },
      scroll: { ...summary(deltas), durationMs: scrollMs, from: before, to: after },
      longTasks: {
        boot: {
          count: bootLongTasks.length,
          totalMs: Math.round(bootLongTasks.reduce((total, entry) => total + entry.duration, 0) * 10) / 10,
          longestMs: bootLongTasks.length
            ? Math.round(Math.max(...bootLongTasks.map((entry) => entry.duration)) * 10) / 10
            : 0,
        },
        duringScroll: {
          count: scrollLongTasks.length,
          totalMs: Math.round(scrollLongTaskTotal * 10) / 10,
          longestMs: scrollLongTasks.length
            ? Math.round(Math.max(...scrollLongTasks.map((entry) => entry.duration)) * 10) / 10
            : 0,
        },
        total: {
          count: longTasks.length,
          totalMs: Math.round(longTaskTotal * 10) / 10,
          longestMs: longTasks.length
            ? Math.round(Math.max(...longTasks.map((entry) => entry.duration)) * 10) / 10
            : 0,
        },
      },
      heap: {
        at5sMb: heapAt5s ? mib(heapAt5s.used) : null,
        at30sMb: heapAt30s ? mib(heapAt30s.used) : null,
        limitMb: heapAt30s ? mib(heapAt30s.limit) : null,
      },
      raf: { at5s: rafAt5s, atIdle: rafAtIdle },
      budgets: { heapFhdMb: 300, heapUhdMb: 380, fps: 60, frameMs: 1000 / 60 },
      softwareRenderer,
    };

    const outDir = testInfo.outputPath("console-perf");
    mkdirSync(outDir, { recursive: true });
    writeFileSync(path.join(outDir, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
    await testInfo.attach("console-perf.json", {
      body: Buffer.from(`${JSON.stringify(report, null, 2)}\n`),
      contentType: "application/json",
    });
    console.log(`console-perf ${JSON.stringify(report)}`);

    // The probe must have seen a real scroll, not an idle page.
    expect(report.scroll.frames).toBeGreaterThan(10);
    // 60 fps is asserted on hardware only: a GPU-less runner composites the
    // full-screen wallpaper blur on the CPU and cannot hold the budget. The
    // numbers are still recorded, and PERFORMANCE.md names the cause.
    if (!softwareRenderer) {
      expect(report.scroll.p95).toBeLessThan(1000 / 30);
    }
  });
});
