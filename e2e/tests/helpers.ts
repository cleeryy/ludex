import { expect, type APIRequestContext, type Page } from "@playwright/test";

const IGNORED_CONSOLE = [
  /favicon/i,
  /Download the React DevTools/i,
  // A download starts by aborting a navigation request; Chromium can surface
  // that as ERR_ABORTED without anything being wrong.
  /net::ERR_ABORTED/i,
];

/**
 * Collects browser console errors and uncaught page errors. Call assertClean()
 * at the end of a test so an unexpected React/network error fails it.
 */
export function watchConsole(page: Page) {
  const errors: string[] = [];

  page.on("console", (message) => {
    if (message.type() !== "error") return;
    const text = message.text();
    if (IGNORED_CONSOLE.some((pattern) => pattern.test(text))) return;
    errors.push(`console: ${text}`);
  });

  page.on("pageerror", (error) => {
    if (IGNORED_CONSOLE.some((pattern) => pattern.test(error.message))) return;
    errors.push(`pageerror: ${error.message}`);
  });

  return {
    assertClean() {
      expect(errors, `Unexpected browser errors:\n${errors.join("\n")}`).toEqual([]);
    },
  };
}

/** Injects the API token before the app boots. */
export async function signInAsAdmin(page: Page) {
  const token = process.env.LUDEX_E2E_TOKEN;
  if (!token) throw new Error("LUDEX_E2E_TOKEN is not set; run scripts/e2e.sh");

  await page.addInitScript((value: string) => {
    window.localStorage.setItem("ludex.token", value);
  }, token);
}

/** Polls the API until a game matching `query` is indexed. */
export async function waitForGame(request: APIRequestContext, query: string) {
  const token = process.env.LUDEX_E2E_TOKEN;
  if (!token) throw new Error("LUDEX_E2E_TOKEN is not set");

  await expect
    .poll(
      async () => {
        const response = await request.get(`/api/games?q=${encodeURIComponent(query)}&limit=10`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!response.ok()) return -1;
        const payload = (await response.json()) as { total?: number };
        return payload.total ?? 0;
      },
      { timeout: 30_000, message: `game "${query}" was not indexed` },
    )
    .toBeGreaterThan(0);
}

export function gameTitle(): string {
  const title = process.env.LUDEX_E2E_GAME_TITLE;
  if (!title) throw new Error("LUDEX_E2E_GAME_TITLE is not set");
  return title;
}

export function gameFile(): string {
  const file = process.env.LUDEX_E2E_GAME_FILE;
  if (!file) throw new Error("LUDEX_E2E_GAME_FILE is not set");
  return file;
}

export function gameDescription(): string {
  const description = process.env.LUDEX_E2E_DESCRIPTION;
  if (!description) throw new Error("LUDEX_E2E_DESCRIPTION is not set");
  return description;
}
