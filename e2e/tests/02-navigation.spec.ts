import { expect, test } from "@playwright/test";

import { signInAsAdmin, watchConsole } from "./helpers";

test.describe("navigation and Big Picture", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("renders the main routes without console errors and exits Big Picture with Escape", async ({ page }) => {
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
    await page.getByRole("button", { name: "Big Picture mode" }).click();
    await expect(page.getByText("Big Picture", { exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page).not.toHaveURL(/bigpicture/);

    errors.assertClean();
  });
});
