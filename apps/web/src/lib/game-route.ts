/**
 * Game route ids (#154).
 *
 * Only positive integers name a game on the server. Launcher imports that are
 * not in the server library get a deterministic negative id in the desktop
 * index (`-appId`, `-1`, …), so `/game/-42` can never be a server page. The
 * route guard uses this helper instead of trusting `Number(id)`.
 */

export function isServerGameId(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value > 0;
}

/** Parses a `:id` route parameter; null when it is not a server game id. */
export function parseServerGameId(raw: string | undefined | null): number | null {
  if (raw === undefined || raw === null) return null;
  const value = raw.trim();
  // Digits only: `Number("1e3")` and `Number("0x10")` are positive but are not
  // ids the server could have issued.
  if (!/^\d+$/.test(value)) return null;
  const id = Number(value);
  return isServerGameId(id) ? id : null;
}
