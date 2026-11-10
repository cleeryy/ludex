import { expect, test } from "@playwright/test";

import { signInAsAdmin, watchConsole } from "./helpers";

/**
 * Regression: the dialog effect must not depend on the caller's inline
 * `onOpenChange`, otherwise every keystroke re-runs it, restores focus to the
 * trigger and refocuses the panel, making controlled inputs impossible to type
 * in (observed on the news editor and the create-user form).
 */
test.describe("dialog forms keep focus while typing", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("typing in the create-user and news dialogs keeps the input focused", async ({ page }) => {
    const errors = watchConsole(page);

    await page.goto("/#/admin/users");
    await page.getByRole("button", { name: "Add user", exact: true }).click();
    const username = page.getByLabel("Username", { exact: true });
    await username.click();
    await username.pressSequentially("alice");
    await expect(username).toBeFocused();
    await expect(username).toHaveValue("alice");
    await page.keyboard.press("Escape");
    await expect(username).toBeHidden();

    await page.goto("/#/admin/news");
    await page.getByRole("button", { name: "New post", exact: true }).click();
    const title = page.getByLabel("Title", { exact: true });
    await title.click();
    await title.pressSequentially("hello");
    await expect(title).toBeFocused();
    await expect(title).toHaveValue("hello");

    errors.assertClean();
  });
});
