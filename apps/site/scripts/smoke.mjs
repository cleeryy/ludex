#!/usr/bin/env node
/**
 * Playwright smoke test against the built static export (`apps/site/out/`)
 * served under the configured `BASE_PATH` (default `/ludex`).
 *
 * Verifies:
 *  1. Landing page renders and all visible images load cleanly.
 *  2. Docs index (`/docs/`) loads with sidebar navigation.
 *  3. A deep doc (`/docs/getting-started/install/`) loads.
 *  4. Static search index (`/api/search/`) is reachable and UI search returns a hit.
 *  5. Missing route renders the custom 404 page.
 */
import { createServer } from "node:http";
import { readdirSync, readFileSync } from "node:fs";
import { extname, join, relative, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const SITE_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ROOT_DIR = resolve(SITE_DIR, "../..");
const OUT_DIR = join(SITE_DIR, "out");
const rawBasePath = process.env.BASE_PATH ?? "/ludex";
const BASE_PATH = rawBasePath === "/" ? "" : rawBasePath.replace(/\/+$/, "");

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".mjs": "application/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".xml": "application/xml; charset=utf-8",
  ".txt": "text/plain; charset=utf-8",
};

// Pre-index all files inside OUT_DIR so request URLs only perform an in-memory
// Map lookup and never flow into filesystem paths.
const OUT_FILES = new Map();
function indexOut(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      indexOut(full);
    } else if (entry.isFile()) {
      const rel = `/${relative(OUT_DIR, full).replaceAll("\\", "/")}`;
      OUT_FILES.set(rel, full);
    }
  }
}
indexOut(OUT_DIR);

function resolveFile(urlPath) {
  let clean = decodeURIComponent(urlPath.split("?")[0]);
  if (BASE_PATH) {
    if (clean === BASE_PATH) clean = "/";
    else if (clean.startsWith(`${BASE_PATH}/`)) clean = clean.slice(BASE_PATH.length);
    else return { status: 404, file: OUT_FILES.get("/404.html") ?? null };
  }
  const trimmed = clean.replace(/\/+$/, "");
  const candidates = [
    clean,
    `${trimmed}/index.html`,
    trimmed ? `${trimmed}.html` : "/index.html",
  ];
  for (const key of candidates) {
    const file = OUT_FILES.get(key);
    if (file) {
      return { status: 200, file };
    }
  }
  return { status: 404, file: OUT_FILES.get("/404.html") ?? null };
}

const server = createServer((req, res) => {
  const { status, file } = resolveFile(req.url ?? "/");
  if (!file) {
    res.writeHead(404);
    res.end("Not found");
    return;
  }
  const ext = extname(file).toLowerCase();
  res.writeHead(status, { "Content-Type": MIME[ext] ?? "application/octet-stream" });
  res.end(readFileSync(file));
});

await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
const { port } = server.address();
const origin = `http://127.0.0.1:${port}`;
const url = (path) => `${origin}${BASE_PATH}${path}`;

const requireFromE2e = createRequire(join(ROOT_DIR, "e2e", "package.json"));
const { chromium } = requireFromE2e("@playwright/test");

const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const failedAssets = [];
  page.on("response", (resp) => {
    if (resp.status() >= 400 && !resp.url().includes("/does-not-exist")) {
      failedAssets.push(`${resp.status()} ${resp.url()}`);
    }
  });

  // 1. Landing page + visible images
  await page.goto(url("/"), { waitUntil: "networkidle" });
  const h1 = await page.locator("h1").first().textContent();
  if (!h1?.includes("The game library you host yourself")) {
    throw new Error(`Unexpected landing H1: ${h1}`);
  }
  const brokenImages = await page.evaluate(() =>
    Array.from(document.querySelectorAll("img"))
      .filter((img) => getComputedStyle(img).display !== "none")
      .filter((img) => !img.complete || img.naturalWidth === 0)
      .map((img) => img.getAttribute("src")),
  );
  if (brokenImages.length > 0) {
    throw new Error(`Broken landing images: ${brokenImages.join(", ")}`);
  }

  // 2. Docs index
  await page.goto(url("/docs/"), { waitUntil: "networkidle" });
  const docsHeading = await page.locator("h1").first().textContent();
  if (!docsHeading?.includes("Documentation")) {
    throw new Error(`Unexpected docs index H1: ${docsHeading}`);
  }

  // 3. Deep doc page
  await page.goto(url("/docs/getting-started/install/"), { waitUntil: "networkidle" });
  const deepHeading = await page.locator("h1").first().textContent();
  if (!deepHeading || deepHeading.trim().length < 3) {
    throw new Error(`Deep doc failed to render H1: ${deepHeading}`);
  }

  // 4. Static search index + interactive search
  const searchResp = await page.request.get(url("/api/search.json"));
  if (!searchResp.ok()) {
    throw new Error(`Static search endpoint failed: ${searchResp.status()}`);
  }
  const searchButton = page.locator("button", { hasText: /Search/i }).first();
  if (await searchButton.isVisible()) {
    await searchButton.click();
    const input = page.locator('input[placeholder*="Search" i]').first();
    await input.fill("PostgreSQL");
    const result = page.locator('[role="dialog"]').getByText("PostgreSQL Backend").first();
    await result.waitFor({ state: "visible", timeout: 10000 });
    await result.click();
    await page.waitForURL(/\/docs\/deployment\/postgresql\/?/, { timeout: 10000 });
  }

  // 5. 404 page
  const res404 = await page.goto(url("/does-not-exist/"), { waitUntil: "networkidle" });
  if (res404?.status() !== 404) {
    throw new Error(`Expected 404 status, got ${res404?.status()}`);
  }
  const h1404 = await page.locator("h1").first().textContent();
  if (!h1404?.includes("This shelf is empty")) {
    throw new Error(`Unexpected 404 H1: ${h1404}`);
  }

  if (failedAssets.length > 0) {
    throw new Error(`Failed asset requests:\n${failedAssets.join("\n")}`);
  }

  console.log(`✓ Playwright site smoke passed under ${BASE_PATH || "/"}`);
} finally {
  await browser.close();
  server.close();
}
