import { NAV_DESTINATIONS } from "./navigation";

/* --------------------------------------------------------------------------
   Route focus and route titles (#237).

   Two questions the shell has to answer on every navigation, kept pure so
   they can be tested without a router:

   1. Is this the same screen as before? `/game/12/rom` → `/game/12/files` is
      an in-page section switch: moving focus to the heading would throw the
      reader back to the top of the page they are already on.
   2. What is this screen called? The navigation table already names every
      destination once, so the title comes from there instead of a second map
      that can drift.
   -------------------------------------------------------------------------- */

/** Collapses an in-page section link onto the screen that owns it. */
export function routeScreenKey(pathname: string): string {
  const segments = pathname.split("/").filter(Boolean);
  // `/game/:id/:section` is one page with internal tabs (#189). A different id
  // is a different game and gets a fresh focus move.
  if (segments[0] === "game" && segments.length >= 2) return `/game/${segments[1]}`;
  return `/${segments.join("/")}`;
}

export function routePathname(pathname: string): string {
  const cut = pathname.split(/[?#]/)[0];
  return cut === "" ? "/" : cut;
}

/** Every title the shell can set is one of the navigation labels. */
export type RouteTitleId = (typeof NAV_DESTINATIONS)[number]["labelKey"];

/** The navigation entry that owns a path, exact match first. */
export function routeDestination(pathname: string): (typeof NAV_DESTINATIONS)[number] | null {
  const path = routePathname(pathname);
  const exact = NAV_DESTINATIONS.find((entry) => entry.to === path);
  if (exact) return exact;
  const prefixed = NAV_DESTINATIONS.filter(
    (entry) => "prefix" in entry && entry.prefix === true,
  ).find((entry) => path === entry.to || path.startsWith(`${entry.to}/`));
  return prefixed ?? null;
}

/** Message id for the screen title, or `null` when the screen names itself. */
export function routeTitleId(pathname: string): RouteTitleId | null {
  const path = routePathname(pathname);
  if (path === "/game" || path.startsWith("/game/")) return null;
  return routeDestination(path)?.labelKey ?? null;
}

/** A focus move is only due when the screen changed, not the query or hash. */
export function shouldMoveRouteFocus(previous: string | null, next: string): boolean {
  return previous !== null && previous !== next;
}
