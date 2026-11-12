import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";

import { gameDescription, gameFile, gameTitle, signInAsAdmin, watchConsole } from "./helpers";

test.describe("library, search, filters, detail and download", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("shows the seeded game and completes the browser flows", async ({ page }) => {
    const errors = watchConsole(page);
    const title = gameTitle();
    const file = gameFile();
    const description = gameDescription();

    await page.goto("/");
    await expect(page.getByRole("link", { name: `Open ${title}` })).toBeVisible();
    await expect(page.getByRole("heading", { name: title }).first()).toBeVisible();

    // Search through the command palette, not a direct URL.
    await page.getByRole("button", { name: /Search games/ }).click();
    const palette = page.getByRole("dialog", { name: "Search and commands" });
    await expect(palette).toBeVisible();
    await palette.getByRole("combobox", { name: "Search games and commands" }).fill(title);
    const option = palette.getByRole("option", { name: new RegExp(title) });
    await expect(option).toBeVisible();
    await option.click();

    await expect(page.getByRole("heading", { name: title, level: 1 })).toBeVisible();
    await expect(page.getByText("1 version")).toBeVisible();
    await expect(page.getByText(description)).toBeVisible();
    await expect(page.getByText(file).first()).toBeVisible();

    // Progress-state filter through the library UI.
    await page.goto("/#/library");
    await expect(page.getByRole("heading", { name: "Library", level: 1 })).toBeVisible();
    await page.getByRole("button", { name: "Filter by progress state" }).click();
    await page.getByRole("option", { name: "Completed" }).click();
    await expect(page.getByText("No games match")).toBeVisible();
    await page.getByRole("button", { name: "Clear filters" }).click();
    await expect(page.getByRole("link", { name: `Open ${title}` })).toBeVisible();

    // Browser download: ticket URL, attachment filename, real archive bytes.
    await page.getByRole("link", { name: `Open ${title}` }).click();
    const downloadPromise = page.waitForEvent("download");
    await page.getByRole("button", { name: "Download", exact: true }).first().click();
    const download = await downloadPromise;

    expect(download.suggestedFilename()).toBe(file);
    const path = await download.path();
    expect(path).toBeTruthy();
    const bytes = readFileSync(path!);
    expect(bytes.length).toBeGreaterThan(1024 * 1024);
    expect(bytes.includes(Buffer.from("ludex-e2e-known-file"))).toBe(true);

    errors.assertClean();
  });
});
