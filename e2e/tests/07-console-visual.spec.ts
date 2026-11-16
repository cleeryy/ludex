import { readFileSync } from "node:fs";

import { expect, test } from "@playwright/test";

import { consoleUrl, pngSize } from "./console-helpers";
import { signInAsAdmin } from "./helpers";

/**
 * TV screenshot matrix (#270, Phase 0.6).
 *
 * Captures the console at 1280x720, 1920x1080 and 3840x2160, plus the 90%
 * safe area a 5% overscan TV actually shows: a title or a hint sitting in the
 * outer 5% is invisible on the couch even though the browser rendered it.
 *
 * These are evidence frames, not approved baselines. They land in
 * `e2e/test-results/console-visual/` (gitignored, uploaded by CI) and are
 * reviewed by a human. The approved set for the README goes through the #265
 * pipeline into `docs/assets/screenshots/`; never commit a raw 4K PNG.
 */

const VIEWPORTS = [
  { name: "720p", width: 1280, height: 720 },
  { name: "1080p", width: 1920, height: 1080 },
  { name: "4k", width: 3840, height: 2160 },
] as const;

/** TV overscan: the outer 5% on each edge is not guaranteed to be visible. */
const OVERSCAN = 0.05;

test.describe("console visual matrix", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  for (const viewport of VIEWPORTS) {
    test(`captures ${viewport.name} with a 5% overscan safe area`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await page.goto(consoleUrl());
      await expect(page).toHaveTitle(/console/i);
      // Entrance animation and artwork decode settle before the frame.
      await page.waitForTimeout(1_200);

      const full = testInfo.outputPath(`${viewport.name}-full.png`);
      await page.screenshot({ path: full, animations: "disabled", caret: "hide" });
      expect(pngSize(full)).toEqual({ width: viewport.width, height: viewport.height });
      expect(readFileSync(full).length).toBeGreaterThan(40_000);

      // What a 5% overscan TV shows; also the frame to inspect for clipped
      // hero titles, hints and the focus ring.
      const insetX = Math.round(viewport.width * OVERSCAN);
      const insetY = Math.round(viewport.height * OVERSCAN);
      const safe = testInfo.outputPath(`${viewport.name}-safe-90.png`);
      await page.screenshot({
        path: safe,
        animations: "disabled",
        caret: "hide",
        clip: {
          x: insetX,
          y: insetY,
          width: viewport.width - insetX * 2,
          height: viewport.height - insetY * 2,
        },
      });
      expect(pngSize(safe)).toEqual({
        width: viewport.width - insetX * 2,
        height: viewport.height - insetY * 2,
      });
    });
  }
});
