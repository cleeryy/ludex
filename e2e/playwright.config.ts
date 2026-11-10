import { defineConfig, devices } from "@playwright/test";

/**
 * Browser E2E suite.
 *
 * The server and its fixtures are started by scripts/e2e.sh, which exports
 * LUDEX_E2E_BASE_URL and the tokens/URLs used by the tests. Playwright does not
 * start the server itself: the harness needs two isolated servers (one already
 * set up, one fresh for the setup flow) and a local HTTP server for ingest.
 */
const baseURL = process.env.LUDEX_E2E_BASE_URL ?? "http://127.0.0.1:8080";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: [
    ["list"],
    ["html", { open: "never", outputFolder: "playwright-report" }],
  ],
  outputDir: "test-results",
  use: {
    baseURL,
    acceptDownloads: true,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    actionTimeout: 10_000,
    navigationTimeout: 30_000,
    viewport: { width: 1440, height: 900 },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
