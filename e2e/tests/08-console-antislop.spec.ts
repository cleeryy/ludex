import { expect, test } from "@playwright/test";

import { consoleUrl } from "./console-helpers";
import { signInAsAdmin } from "./helpers";

/**
 * Console anti-slop gates (#270 §8.3, rules from #268 / the console-mode-ui
 * skill).
 *
 * Every test is `fixme`: the v0.4 `/bigpicture` shell still ships desktop
 * sizes, a layout picker, the Soft preset and desktop surfaces, so these
 * assertions cannot pass yet. They are written now so the harness is ready
 * when the Cabinet shell lands (#268) and the `/console` route replaces
 * `/bigpicture` (#267). Enable them in the same PR that deletes the old
 * screen; a rule with a skipped test is already better than a rule with none,
 * a rule with an enabled test is the goal.
 *
 * Run with `LUDEX_E2E_CONSOLE_PATH=/console` once the shell lands.
 */

const MIN_TYPE_PX_AT_1080P = 24;

test.describe("console anti-slop", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test.fixme("type floor: no visible text below 24u at 1080p (#268, #270)", async ({ page }) => {
    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.goto(consoleUrl());
    await expect(page).toHaveTitle(/console/i);
    await page.waitForTimeout(1_200);

    // 1u ~= 1px at 1920, so the floor is 24px. Leaf text nodes only: a
    // container inheriting a small size is not an offender by itself.
    const offenders = await page.evaluate((floor) => {
      const hits: string[] = [];
      for (const node of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
        if (node.children.length > 0) continue;
        const text = (node.textContent ?? "").trim();
        if (!text) continue;
        if (node.closest("[aria-hidden='true']")) continue;
        const size = Number.parseFloat(getComputedStyle(node).fontSize);
        if (Number.isFinite(size) && size < floor) {
          hits.push(`${size}px ${node.tagName.toLowerCase()}: ${text.slice(0, 48)}`);
        }
      }
      return hits;
    }, MIN_TYPE_PX_AT_1080P);

    expect(offenders).toEqual([]);
  });

  test.fixme("Cabinet rules: no layout picker, Soft/pastel tokens or index grid (#268, #270)", async ({ page }) => {
    await page.goto(consoleUrl());
    await expect(page).toHaveTitle(/console/i);

    // The 2.0 shell keeps two layouts and deletes the picker; Soft and the five
    // pastel accents are gone, so their names must not survive in the DOM.
    const violations = await page.evaluate(() => {
      const hits: string[] = [];
      if (document.querySelector("[aria-label*='layout' i], [data-layout-picker]")) hits.push("layout-picker");
      const markup = document.documentElement.outerHTML.toLowerCase();
      for (const token of ["bp-soft", "soft-accent", "coral", "mint", "lilac"]) {
        if (markup.includes(token)) hits.push(`token:${token}`);
      }
      const indexed = Array.from(document.querySelectorAll("[data-index]"))
        .map((node) => (node as HTMLElement).dataset.index ?? "")
        .filter((value) => /^0[1-6]$/.test(value));
      if (indexed.length >= 6) hits.push("six-index-grid");
      return hits;
    });

    expect(violations).toEqual([]);
  });

  test.fixme("no desktop leaks: no toast, desktop dialog or desktop route (#269, #270)", async ({ page }) => {
    await page.goto(consoleUrl());
    await expect(page).toHaveTitle(/console/i);

    // A toast viewport, an aria-modal desktop dialog and a link back into the
    // desktop router are the three leaks the shell boundary must prevent.
    await expect(page.locator("[data-sonner-toaster], [data-toast-viewport]")).toHaveCount(0);
    await expect(page.locator("[role='dialog'][aria-modal='true']")).toHaveCount(0);

    const desktopRoutes = await page.evaluate(() => {
      const routes = ["/library", "/downloads", "/settings", "/game/", "/collections"];
      return Array.from(document.querySelectorAll<HTMLAnchorElement>("a[href]"))
        .map((anchor) => anchor.getAttribute("href") ?? "")
        .filter((href) => routes.some((route) => href.startsWith(`#${route}`) || href.startsWith(route)));
    });

    expect(desktopRoutes).toEqual([]);
  });
});
