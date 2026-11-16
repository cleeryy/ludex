import { statSync } from "node:fs";

import { expect, test, type Page } from "@playwright/test";

import { installFakePad } from "../pad";
import { consoleUrl, isConsoleUrl, pngSize } from "./console-helpers";
import { signInAsAdmin, watchConsole } from "./helpers";

/**
 * Console shell evidence (#270, Phase 0.6).
 *
 * These five tests are true today and must stay true after Console 2.0:
 * a direct URL boots the shell, a deep link opens a surface, Escape backs out
 * one surface at a time without trapping the player, the fake pad drives the
 * production input loop, and a 1920x1080 evidence frame is captured. The pad
 * probe uses the same Gamepad API as hardware, not a React hook mock.
 *
 * `console-helpers.ts` documents the `LUDEX_E2E_CONSOLE_PATH` override for the
 * `/console` shell; nothing here asserts a flow that only the 2.0 shell can
 * pass. The overlay single-dispatch gate is a `fixme` until #267 gives input a
 * single owner (#270 acceptance: "catches a double-dispatch or a dead
 * controller path").
 */

/** The title is the one console marker that survives the 2.0 shell rewrite. */
async function waitForConsoleShell(page: Page) {
  await expect(page).toHaveTitle(/console/i);
  // The desktop sidebar never mounts inside the console shell.
  await expect(page.getByLabel("Sidebar")).toHaveCount(0);
}

test.describe("console shell", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("boots straight into the console shell from a direct URL", async ({ page }) => {
    const errors = watchConsole(page);

    await page.goto(consoleUrl());
    await waitForConsoleShell(page);
    expect(isConsoleUrl(page.url())).toBe(true);

    errors.assertClean();
  });

  test("deep links open a surface and Escape returns to the shell root", async ({ page }) => {
    const errors = watchConsole(page);

    // Today: /bigpicture?app=settings. The 2.0 shell keeps this reachable as a
    // URL (route segment); the assertion is on the surface, not the syntax.
    await page.goto(consoleUrl("?app=settings"));
    const settingsHeading = page.getByRole("heading", { name: /^settings$/i });
    await expect(settingsHeading).toBeVisible();

    // B/Escape always returns to the exact previous target.
    await page.keyboard.press("Escape");
    await expect(settingsHeading).toHaveCount(0);
    await expect(page).not.toHaveURL(/app=settings/);
    await expect(page).toHaveTitle(/console/i);

    errors.assertClean();
  });

  test("Escape at the shell root backs out without trapping the player", async ({ page }) => {
    const errors = watchConsole(page);

    // Put a real desktop document behind the console, like a user leaving the
    // room. (Playwright starts on about:blank, where localStorage is denied and
    // a back-out would land on an opaque origin.)
    await page.goto("/");
    await expect(page.getByLabel("Sidebar")).toBeVisible();

    await page.goto(consoleUrl());
    await waitForConsoleShell(page);

    // Escape at the shell root is a deliberate back-out: v0.4 returns to the
    // desktop, Console 2.0 opens the Power confirm surface with a Cancel
    // default. Both are accepted; a silent process exit is not.
    await page.keyboard.press("Escape");
    await expect
      .poll(
        async () => {
          if (!isConsoleUrl(page.url())) return "desktop";
          return (await page.getByRole("heading", { name: /^power$/i }).count()) > 0 ? "power" : "console";
        },
        { timeout: 5_000 },
      )
      .not.toBe("console");

    if (!isConsoleUrl(page.url())) {
      await expect(page.getByLabel("Sidebar")).toBeVisible();
    }

    errors.assertClean();
  });

  test("fake pad drives the real input loop", async ({ page }) => {
    const errors = watchConsole(page);
    const pad = await installFakePad(page, {
      id: "Xbox Wireless Controller (STANDARD GAMEPAD)",
    });

    // ?gamepadDebug=1 renders the diagnostics panel from the production loop.
    await page.goto(consoleUrl("?gamepadDebug=1"));
    const diagnostics = page.getByLabel("Gamepad diagnostics");
    await expect(diagnostics).toContainText(/Xbox Wireless Controller/);

    await pad.tap("right");
    await expect(diagnostics.getByText("right", { exact: true })).toBeVisible();

    await pad.tap("down");
    await expect(diagnostics.getByText("down", { exact: true })).toBeVisible();

    // Hot-plug is the WebView2 failure mode the harness must reproduce.
    await pad.disconnect();
    await expect(diagnostics).toContainText(/no pad|Waiting for a controller/i);
    await pad.connect();
    await expect(diagnostics).toContainText(/Xbox Wireless Controller/);

    errors.assertClean();
  });

  test("captures the 1920x1080 evidence frame", async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.goto(consoleUrl());
    await waitForConsoleShell(page);
    // Let the entrance settle; screenshots are evidence, not a baseline.
    await page.waitForTimeout(1_200);

    const file = testInfo.outputPath("console-1920x1080.png");
    await page.screenshot({ path: file, animations: "disabled", caret: "hide" });

    expect(pngSize(file)).toEqual({ width: 1920, height: 1080 });
    expect(statSync(file).size).toBeGreaterThan(40_000);
  });

  // #270 acceptance: "catches a double-dispatch or a dead controller path".
  // Today the shell, the overlay and the input row each own a polling loop, so
  // one press with a surface open reaches two handlers. Enable this test when
  // #267 lands the single InputProvider + scope stack; do not delete it.
  test.fixme("overlay single-dispatch: one press moves one step (#270)", async ({ page }) => {
    const pad = await installFakePad(page);
    await page.goto(consoleUrl());
    await waitForConsoleShell(page);

    // X opens the command overlay on the v0.4 shell.
    await pad.tap("x");
    const overlay = page.getByRole("dialog").first();
    await expect(overlay).toBeVisible();

    // TODO(#267): read the overlay's highlighted row, tap Down once, assert it
    // advanced exactly one row and that the library focus index below did not
    // change. The assertion below is a placeholder for that check.
    await pad.tap("down");
    await expect(overlay).toBeVisible();
  });
});
