import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

import { installNativeBridge } from "../native";
import { installFakePad } from "../pad";
import { consoleUrl, isConsoleUrl } from "./console-helpers";
import { gameTitle, signInAsAdmin, watchConsole } from "./helpers";

/**
 * The four input flows #302 still owed, driven by the fake pad through the
 * production loop.
 *
 * `06-console.spec.ts` proves boot, deep links, the raw pad path and the
 * overlay single-dispatch rule. This file proves the flows a player actually
 * walks:
 *
 * 1. Home -> Play opens the launch plate, and B closes it back onto the cover.
 * 2. Install -> the job shows up in the Status bar and the Downloads surface.
 * 3. A Console setting survives a reload (it is device-local).
 * 4. Power -> Sleep asks first, the confirmation starts on Cancel, and B
 *    cancels without touching the OS.
 *
 * The browser build cannot install, launch or sleep, so the three native flows
 * install `e2e/native.ts`: a Tauri bridge stub that makes `isNative()` true and
 * answers the desktop commands the shell calls. Nothing mocks the input loop or
 * a React hook. A real process spawn cannot be asserted in Chromium; the plate
 * and its launch call are, and the native checklist owns the real spawn.
 */

const serverUrl = process.env.LUDEX_E2E_BASE_URL ?? "http://127.0.0.1:8080";
const token = process.env.LUDEX_E2E_TOKEN ?? "";

/** The seeded game's id, for the installed list the bridge reports. */
async function seededGameId(request: APIRequestContext): Promise<number> {
  const response = await request.get(`/api/games?q=${encodeURIComponent(gameTitle())}&limit=1`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(response.ok()).toBe(true);
  const payload = (await response.json()) as { items?: Array<{ id: number }> };
  const id = payload.items?.[0]?.id;
  if (!id) throw new Error("the seeded game was not indexed");
  return id;
}

async function waitForHome(page: Page) {
  await expect(page).toHaveTitle(/console/i);
  await expect(page.locator("[data-bp-card='true']").first()).toBeVisible();
  // The entrance settles before the ring and the primary action are stable.
  await page.waitForTimeout(900);
}

/**
 * The suite shares one library, and earlier specs add games (ingest, perf).
 * Select the seeded game explicitly instead of assuming it is first.
 */
async function selectSeedGame(page: Page) {
  const title = gameTitle();
  const card = page.getByRole("button", { name: title, exact: true }).first();
  await expect(card).toBeVisible();
  await card.click();
  await expect(page.locator("[data-bp-card='true'][aria-current='true']")).toHaveAttribute("aria-label", title);
}

async function openPowerFromBand(page: Page) {
  // The band is the real Home destination row; clicking it is the same command
  // the pad's A press sends. The pad then owns the confirmation.
  const power = page.locator("[data-bp-band-item='true'][aria-label^='Power']");
  await expect(power).toBeVisible();
  await power.click();
  await expect(page).toHaveURL(/\/console\/power$/);
  await expect(page.getByRole("heading", { name: "Power", exact: true })).toBeVisible();
}

test.describe("console player flows (#302)", () => {
  test.beforeEach(async ({ page }) => {
    await signInAsAdmin(page);
  });

  test("Home -> Play opens the launch plate; B closes it onto the same cover", async ({ page, request }) => {
    const errors = watchConsole(page);
    const gameId = await seededGameId(request);

    // The desktop reports the seeded game as installed, so the primary action
    // is Play. The launch promise is held: the plate's 8 s line is the state in
    // which B is allowed to close it (the native call has no abort).
    const bridge = await installNativeBridge(page, {
      serverUrl,
      token,
      installed: [{ gameId, executable: "Ludex E2E Game.exe" }],
      holdLaunch: true,
    });
    const pad = await installFakePad(page);

    await page.goto(consoleUrl());
    await waitForHome(page);
    await selectSeedGame(page);

    const primary = page.locator("[data-bp-action='true']").getByRole("button", { name: "Play", exact: true });
    await expect(primary).toBeVisible();
    await expect(primary).toBeEnabled();

    const selected = page.locator("[data-bp-card='true'][aria-current='true']");
    const coverLabel = await selected.getAttribute("aria-label");
    expect(coverLabel).toBe(gameTitle());

    await pad.tap("a");
    const plate = page.locator("[data-launch-plate]");
    await expect(plate).toHaveAttribute("data-launch-plate", "launching");
    await expect(plate).toContainText(gameTitle());

    // Past the ceiling the plate stops pretending. This host's launch promise
    // never resolves, so the honest line is the state B may dismiss.
    await expect(plate).toHaveAttribute("data-launch-plate", "slow", { timeout: 12_000 });

    await pad.tap("b");
    await expect(plate).toHaveCount(0);
    expect(isConsoleUrl(page.url())).toBe(true);

    // Focus returns to the exact cover and the library owns the pad again.
    const restored = page.locator("[data-bp-card='true'][aria-current='true']");
    await expect(restored).toHaveAttribute("aria-label", coverLabel ?? "");
    await expect(restored).toHaveAttribute("data-gamepad-focus", "true");

    // The visible flow is asserted; the real spawn is the native checklist's.
    expect((await bridge.commands()).filter((command) => command === "launch_game")).toHaveLength(1);

    errors.assertClean();
  });

  test("Install starts a job that reaches the Status bar and Downloads", async ({ page }) => {
    const errors = watchConsole(page);
    const bridge = await installNativeBridge(page, { serverUrl, token, installed: [] });
    const pad = await installFakePad(page);

    await page.goto(consoleUrl());
    await waitForHome(page);
    await selectSeedGame(page);

    // A desktop build installs: Play becomes Install and the shell opens the
    // install dialog (one of the two allowed Console modals).
    const primary = page.locator("[data-bp-action='true']").getByRole("button", { name: "Install", exact: true });
    await expect(primary).toBeVisible();
    await pad.tap("a");

    const dialog = page.getByRole("dialog", { name: `Install ${gameTitle()}` });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Install", exact: true }).click();

    // The dialog switches to the live job; continuing in background keeps the
    // install running while the player browses.
    await expect(dialog.getByRole("button", { name: "Continue in background" })).toBeVisible();
    await dialog.getByRole("button", { name: "Continue in background" }).click();
    await expect(dialog).toHaveCount(0);

    expect(await bridge.jobs()).toHaveLength(1);

    // The Status bar is a live subscription, not a copy passed down by the
    // shell: the job alone must make the count appear.
    const status = page.getByLabel("Status");
    await expect(status.getByTitle("1 downloading")).toBeVisible();

    // One progress event through the real `install://progress` listener: the
    // percent comes from the job the desktop sent, not from a timer.
    const [job] = await bridge.jobs();
    await bridge.emit("install://progress", { ...job, bytesDone: 1_800_000 });

    // Downloads is a console surface, reachable from the band.
    const downloads = page.locator("[data-bp-band-item='true'][aria-label^='Downloads']");
    await downloads.click();
    const sheet = page.locator("[data-big-picture-overlay='downloads']");
    await expect(sheet).toBeVisible();
    await expect(sheet).toContainText(gameTitle());
    await expect(sheet).toContainText("Downloading");
    await expect(sheet).toContainText("75%");

    // Opening Downloads is a route, so Back leaves the surface, not the shell.
    expect(isConsoleUrl(page.url())).toBe(true);

    errors.assertClean();
  });

  test("a Console setting survives a reload because it is device-local", async ({ page }) => {
    const errors = watchConsole(page);
    const pad = await installFakePad(page);

    await page.goto(consoleUrl("/settings/appearance"));
    await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeVisible();

    // Row 0 is the composition; row 1 is the room. Night is the default.
    const room = page.getByRole("button", { name: /^Room: / });
    await expect(room).toHaveAttribute("aria-label", "Room: Night");
    await pad.tap("down");
    await expect(room).toHaveAttribute("data-system-active", "true");

    await pad.tap("a");
    await expect(room).toHaveAttribute("aria-label", "Room: Projector");
    // The applied value is the preview: the shell carries it as an attribute.
    await expect(page.locator("[data-console-room='projector']").first()).toBeVisible();

    // It is a device preference: the room record is in localStorage (scoped per
    // server when the app has one) and nowhere near the account document.
    const stored = await page.evaluate(() =>
      Object.entries(localStorage).find(([key]) => key.endsWith("ludex.console.room")),
    );
    expect(stored?.[1]).toContain('"room":"projector"');

    await page.reload();
    await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: /^Room: / })).toHaveAttribute("aria-label", "Room: Projector");
    await expect(page.locator("[data-console-room='projector']").first()).toBeVisible();

    errors.assertClean();
  });

  test("Power -> Sleep confirms on Cancel; B cancels and the shell stays up", async ({ page }) => {
    const errors = watchConsole(page);
    const bridge = await installNativeBridge(page, { serverUrl, token, installed: [] });
    const pad = await installFakePad(page);

    await page.goto(consoleUrl());
    await waitForHome(page);
    await openPowerFromBand(page);

    // Row 0 is Sleep on a native desktop; A opens the confirmation.
    await pad.tap("a");
    const confirm = page.locator("[data-big-picture-overlay='exit']");
    await expect(confirm).toBeVisible();
    await expect(page.getByRole("heading", { name: "Sleep this PC" })).toBeVisible();

    // The confirmation always starts on Cancel, never on the destructive row.
    const cancel = confirm.getByRole("button", { name: /Cancel/ });
    await expect(cancel).toHaveAttribute("data-overlay-active", "true");
    const sleepRow = confirm.getByRole("button", { name: /Sleep this PC/ });
    await expect(sleepRow).not.toHaveAttribute("data-overlay-active", "true");

    await pad.tap("b");
    await expect(confirm).toHaveCount(0);
    // The shell is still running the Power surface; nothing was sent to the OS.
    await expect(page).toHaveURL(/\/console\/power$/);
    await expect(page.getByRole("heading", { name: "Power", exact: true })).toBeVisible();
    expect(await bridge.callCount("system_sleep")).toBe(0);

    // And the pad still drives the surface: right along the two-column grid.
    await pad.tap("right");
    await expect(
      page.locator("[data-bp-settings-row='true'][data-system-active='true']"),
    ).toHaveAccessibleName(/Restart Ludex/);

    errors.assertClean();
  });
});
