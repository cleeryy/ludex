import { expect, test } from "@playwright/test";

import { consoleUrl } from "./console-helpers";
import { signInAsAdmin } from "./helpers";

/**
 * Console anti-slop gates (#270 §8.3, rules from #268 / the console-mode-ui
 * skill).
 *
 * These run against the 2.0 `/console` shell (`data-console-route`,
 * `[data-bp-card]`, rooms from `theme/console-room.ts`), not the retired
 * `/bigpicture` markup. A rule that lives only in a source test can pass while
 * the rendered page still breaks it; each assertion below reads the real DOM.
 *
 * Run with `LUDEX_E2E_CONSOLE_PATH=/console`; the default is already
 * `/console` after #267.
 */

const MIN_TYPE_PX_AT_1080P = 24;

test.describe("console anti-slop", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("type floor: no visible text below 24u at 1080p (#268, #270)", async ({ page }) => {
    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.goto(consoleUrl());
    await expect(page).toHaveTitle(/console/i);
    // Wait for a real surface, not the boot silhouette: the gate screens are
    // allowed to be sparse but the home must be fully styled.
    await expect(page.locator("[data-bp-card='true']").first()).toBeVisible();
    await page.waitForTimeout(1_200);

    // 1u ~= 1px at 1920, so the floor is 24px. Leaf text nodes only: a
    // container inheriting a small size is not an offender by itself. A
    // visually hidden node (`sr-only`, `hidden`, zero-size) is not visible
    // text; the console has a few for the route and the live region.
    const offenders = await page.evaluate((floor) => {
      const hits: string[] = [];
      for (const node of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
        if (node.children.length > 0) continue;
        const text = (node.textContent ?? "").trim();
        if (!text) continue;
        if (node.closest("[aria-hidden='true']")) continue;
        const box = node.getBoundingClientRect();
        if (box.width === 0 || box.height === 0) continue;
        const style = getComputedStyle(node);
        if (style.visibility === "hidden" || style.display === "none") continue;
        const size = Number.parseFloat(style.fontSize);
        if (Number.isFinite(size) && size < floor) {
          hits.push(`${size}px ${node.tagName.toLowerCase()}: ${text.slice(0, 48)}`);
        }
      }
      return hits;
    }, MIN_TYPE_PX_AT_1080P);

    expect(offenders).toEqual([]);
  });

  test("Cabinet rules: no layout picker, Soft/pastel tokens or index grid (#268, #270)", async ({ page }) => {
    await page.goto(consoleUrl());
    await expect(page).toHaveTitle(/console/i);
    await expect(page.locator("[data-bp-card='true']").first()).toBeVisible();

    // The 2.0 shell keeps three compositions and deletes the picker; Soft and
    // the five pastel accents are gone, so their names must not survive in the
    // DOM. The room attributes come from `theme/console-room.ts` (Night by
    // default) and the layout attribute from the composition.
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

    // The shell really is the Cabinet: one of the four room palettes and one
    // of the three compositions are on the root, and the retired six-layout
    // registry is not reachable.
    const root = page.locator(".bp-root");
    await expect(root).toHaveAttribute("data-console-room", /^(night|projector|daylight|arcade)$/);
    await expect(page.locator("[data-bp-layout]")).toHaveAttribute("data-bp-layout", /^(shelf|grid|compact)$/);
    await expect(page.locator("[data-console-route]")).toHaveAttribute("data-console-route", /^(home|library)$/);
  });

  test("no desktop leaks: no toast, desktop dialog or desktop route (#269, #270)", async ({ page }) => {
    await page.goto(consoleUrl());
    await expect(page).toHaveTitle(/console/i);
    await expect(page.locator("[data-bp-card='true']").first()).toBeVisible();

    // A toast viewport, an aria-modal desktop dialog and a link back into the
    // desktop router are the three leaks the shell boundary must prevent. A
    // pad layer is a desktop/onboarding concept: the Console owns the pad
    // through its scope stack, never through a layer.
    await expect(page.locator("[data-sonner-toaster], [data-toast-viewport]")).toHaveCount(0);
    await expect(page.locator("[role='dialog'][aria-modal='true']")).toHaveCount(0);
    await expect(page.locator("[data-pad-layer='true']")).toHaveCount(0);
    // The desktop shell never mounts inside the Console.
    await expect(page.getByLabel("Sidebar")).toHaveCount(0);

    const desktopRoutes = await page.evaluate(() => {
      const routes = ["/library", "/downloads", "/settings", "/game/", "/collections"];
      return Array.from(document.querySelectorAll<HTMLAnchorElement>("a[href]"))
        .map((anchor) => anchor.getAttribute("href") ?? "")
        .filter((href) => routes.some((route) => href.startsWith(`#${route}`) || href.startsWith(route)));
    });

    expect(desktopRoutes).toEqual([]);
  });
});
