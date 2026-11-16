import { readFileSync } from "node:fs";

/**
 * Shared console evidence helpers (#270).
 *
 * `CONSOLE_PATH` is the one knob the Phase 0.6 suites need: the shell lives at
 * `/bigpicture` today and moves to `/console` when the `console_v2` shell lands
 * (#267). Point it at the new route instead of editing four specs:
 *
 *   LUDEX_E2E_CONSOLE_PATH=/console pnpm --filter @ludex/e2e test
 *
 * The deep-link suffix is a separate override because the 2.0 shell replaces
 * `?app=settings` with a route segment.
 */
export const CONSOLE_PATH = (process.env.LUDEX_E2E_CONSOLE_PATH ?? "/bigpicture").replace(/\/+$/, "");

export function consoleUrl(suffix = ""): string {
  return `/#${CONSOLE_PATH}${suffix}`;
}

/** True when the current page URL still points at the console shell. */
export function isConsoleUrl(url: string): boolean {
  return new URL(url).hash.startsWith(`#${CONSOLE_PATH}`);
}

/** PNG IHDR dimensions; fails loudly when the file is not a PNG at all. */
export function pngSize(path: string): { width: number; height: number } {
  const bytes = readFileSync(path);
  const signature = bytes.subarray(0, 8).toString("hex");
  if (signature !== "89504e470d0a1a0a") throw new Error(`${path} is not a PNG`);
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}
