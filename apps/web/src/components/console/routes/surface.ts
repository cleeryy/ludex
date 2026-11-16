/**
 * Console route table (#267).
 *
 * The Console 2.0 shell is a lazy top-level sibling of the desktop shell, so
 * its URL is the source of truth for which surface is on screen: a cold deep
 * link, a browser Back and an in-shell push all resolve through this module.
 * Keeping it pure means the routing rules are tested without a DOM.
 */

export const CONSOLE_ROOT = "/console";
/** The pre-2.0 route. It redirects for one release, then disappears. */
export const LEGACY_CONSOLE_ROOT = "/bigpicture";

export type ConsoleSurface =
  | "home"
  | "library"
  | "game"
  | "settings"
  | "downloads"
  | "search"
  | "power";

export interface ConsoleLocation {
  surface: ConsoleSurface;
  /** `/console/game/:id`; `null` when the segment is not a positive integer. */
  gameId: number | null;
  /** `/console/settings/:section`; `null` defers to the device's last section. */
  section: string | null;
}

const SURFACE_SEGMENTS: Record<string, ConsoleSurface> = {
  "": "home",
  library: "library",
  settings: "settings",
  downloads: "downloads",
  search: "search",
  power: "power",
};

/**
 * Parse a pathname. Anything under `/console` that is not a known surface is
 * Home: a typo, a stale bookmark or a removed surface lands on the front room
 * instead of a dead screen. A path outside the tree returns `null` so callers
 * can tell "not the console" apart from "the console's home".
 */
export function consoleLocationFor(pathname: string): ConsoleLocation | null {
  const path = pathname.replace(/\/+$/, "");
  if (path !== CONSOLE_ROOT && !path.startsWith(`${CONSOLE_ROOT}/`)) return null;
  const rest = path.slice(CONSOLE_ROOT.length).replace(/^\/+/, "");
  const [head = "", tail = ""] = rest.split("/");

  if (head === "game") {
    const id = Number(tail);
    return {
      surface: Number.isInteger(id) && id > 0 ? "game" : "home",
      gameId: Number.isInteger(id) && id > 0 ? id : null,
      section: null,
    };
  }
  if (head === "settings") {
    return {
      surface: "settings",
      gameId: null,
      section: tail || null,
    };
  }
  return {
    surface: SURFACE_SEGMENTS[head] ?? "home",
    gameId: null,
    section: null,
  };
}

/** One release of compatibility: `/bigpicture` lands on `/console`. */
export function legacyConsoleRedirect(pathname: string): string | null {
  return pathname.replace(/\/+$/, "") === LEGACY_CONSOLE_ROOT ? CONSOLE_ROOT : null;
}

export interface ConsolePathOptions {
  gameId?: number | null;
  section?: string | null;
}

/** The canonical path for a surface; transitions in the shell use this. */
export function consolePathFor(surface: ConsoleSurface, options: ConsolePathOptions = {}): string {
  switch (surface) {
    case "home":
      return CONSOLE_ROOT;
    case "library":
      return `${CONSOLE_ROOT}/library`;
    case "game":
      return options.gameId ? `${CONSOLE_ROOT}/game/${options.gameId}` : `${CONSOLE_ROOT}/library`;
    case "settings":
      return options.section
        ? `${CONSOLE_ROOT}/settings/${encodeURIComponent(options.section)}`
        : `${CONSOLE_ROOT}/settings`;
    case "downloads":
      return `${CONSOLE_ROOT}/downloads`;
    case "search":
      return `${CONSOLE_ROOT}/search`;
    case "power":
      return `${CONSOLE_ROOT}/power`;
  }
}
