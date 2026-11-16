import { expect, test } from "@playwright/test";

import { signInAsAdmin, watchConsole } from "./helpers";

test.describe("navigation and Console mode", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("renders the main routes without console errors and exits Console mode with Escape", async ({ page }) => {
    const errors = watchConsole(page);

    const routes: Array<[string, string]> = [
      ["/downloads", "Downloads"],
      ["/settings", "Settings"],
      ["/news", "News"],
      ["/admin/scan", "Library scan"],
    ];

    for (const [route, heading] of routes) {
      await page.goto(`/#${route}`);
      await expect(page.getByRole("heading", { name: heading, level: 1 })).toBeVisible();
    }

    await page.goto("/");
    await page.getByRole("button", { name: "Console mode" }).click();
    // #268 replaced the bar wordmark with the monogram, so the shell is
    // identified by its root rather than a text label.
    await expect(page.locator(".bp-root")).toBeVisible();
    await expect(page.locator('[data-console-route]')).toHaveAttribute("data-console-route", /.+/);
    await page.keyboard.press("Escape");
    await expect(page).not.toHaveURL(/bigpicture/);

    errors.assertClean();
  });
});
