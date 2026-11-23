import { expect, test } from "@playwright/test";

import { signInAsAdmin, waitForGame, watchConsole } from "./helpers";

test.describe("admin ingest", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("downloads a local archive, reaches Done and indexes the game", async ({ page, request }) => {
    const ingestUrl = process.env.LUDEX_E2E_INGEST_URL;
    const baseTitle = process.env.LUDEX_E2E_INGEST_TITLE;
    test.skip(!ingestUrl || !baseTitle, "LUDEX_E2E_INGEST_URL/LUDEX_E2E_INGEST_TITLE not set");
    // A retry reuses the same server: a fixed title leaves two Done jobs and
    // the History article lookup becomes a strict-mode violation. One title
    // per attempt keeps every assertion unambiguous.
    const ingestTitle = `${baseTitle!} ${Date.now().toString(36)}-${test.info().retry}`;

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
    await waitForGame(request, ingestTitle);

    await page.goto(`/#/library?q=${encodeURIComponent(ingestTitle)}`);
    const openLink = page.getByRole("link", { name: `Open ${ingestTitle}` });
    // The library query can still be serving the previous page while the
    // ingest finishes indexing; reload once and poll instead of failing at 15 s.
    if (!(await openLink.isVisible().catch(() => false))) {
      await page.reload();
    }
    await expect(openLink).toBeVisible({ timeout: 30_000 });

    errors.assertClean();
  });
});
