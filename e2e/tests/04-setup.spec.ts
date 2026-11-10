import { expect, test } from "@playwright/test";

import { watchConsole } from "./helpers";

test.describe("first-run setup", () => {
  test("creates the administrator through the browser UI", async ({ page }) => {
    const setupUrl = process.env.LUDEX_E2E_SETUP_URL;
    test.skip(!setupUrl, "LUDEX_E2E_SETUP_URL not set");

    const errors = watchConsole(page);

    await page.goto(setupUrl!);
    await expect(page.getByRole("heading", { name: "Set up this server" })).toBeVisible();

    await page.getByLabel("Server name", { exact: true }).fill("Ludex E2E Setup");
    await page.getByLabel("Username", { exact: true }).fill("e2e-setup-admin");
    await page.getByLabel("Password", { exact: true }).fill("E2E-Setup-Password-123");
    await page.getByLabel("Confirm password", { exact: true }).fill("E2E-Setup-Password-123");
    await page.getByRole("button", { name: "Create administrator" }).click();

    await expect(page.getByRole("heading", { name: "Nothing on the shelf yet" })).toBeVisible();

    errors.assertClean();
  });
});
