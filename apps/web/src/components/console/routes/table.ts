import { CONSOLE_ROOT, consolePathFor, type ConsoleSurface } from "@/components/console/routes/surface";

/**
 * The Console's addressable surfaces (#267). One mounted shell resolves this
 * table from the URL, so `/console`, `/console/library`, `/console/game/:id`,
 * `/console/settings/:section`, `/console/downloads`, `/console/search` and
 * `/console/power` share the selection, the layout and the overlays.
 */
export const CONSOLE_ROUTES: ReadonlyArray<{ path: string; surface: ConsoleSurface }> = [
  { path: CONSOLE_ROOT, surface: "home" },
  { path: consolePathFor("library"), surface: "library" },
  { path: `${CONSOLE_ROOT}/game/:id`, surface: "game" },
  { path: consolePathFor("settings"), surface: "settings" },
  { path: `${CONSOLE_ROOT}/settings/:section`, surface: "settings" },
  { path: consolePathFor("downloads"), surface: "downloads" },
  { path: consolePathFor("search"), surface: "search" },
  { path: consolePathFor("power"), surface: "power" },
];
