import { expect, test } from "@playwright/test";

import { signInAsAdmin, waitForGame, watchConsole } from "./helpers";

test.describe("admin ingest", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("downloads a local archive, reaches Done and indexes the game", async ({ page, request }) => {
    const ingestUrl = process.env.LUDEX_E2E_INGEST_URL;
    const ingestTitle = process.env.LUDEX_E2E_INGEST_TITLE;
    test.skip(!ingestUrl || !ingestTitle, "LUDEX_E2E_INGEST_URL/LUDEX_E2E_INGEST_TITLE not set");

    const errors = watchConsole(page);

    await page.goto("/#/admin/ingest");
    await expect(page.getByRole("heading", { name: "Ingest", level: 1 })).toBeVisible();

    await page.getByLabel("Download URL", { exact: true }).fill(ingestUrl!);
    await page.getByLabel(/Title/).fill(ingestTitle!);
    await page.getByRole("button", { name: "Download", exact: true }).click();

    // Finished jobs live in a collapsed History section; wait for it to appear.
    await expect(page.getByText("Finished, failed and cancelled downloads")).toBeVisible({
      timeout: 30_000,
    });
    await page.getByText("Finished, failed and cancelled downloads").click();
    await expect(page.getByRole("article", { name: `${ingestTitle}, Done` })).toBeVisible();

    // The file list refreshes every 10 s; the job also triggers a library scan.
    await expect(page.getByText(`${ingestTitle}.zip`)).toBeVisible({ timeout: 20_000 });
    await waitForGame(request, ingestTitle!);

    await page.goto(`/#/library?q=${encodeURIComponent(ingestTitle!)}`);
    await expect(page.getByRole("link", { name: `Open ${ingestTitle}` })).toBeVisible();

    errors.assertClean();
  });
});
