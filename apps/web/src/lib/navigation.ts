/**
 * One table for every destination the app can be reached through (#174).
 *
 * The rail, the mobile bar, the More sheet, the command palette and the
 * `g`-prefixed shortcuts all read this module, so a destination cannot be
 * named, hidden or keyed differently on one surface than another. Labels are
 * English message ids: each surface translates them with `t()`.
 *
 * This module is deliberately free of React and of the router: it is the part
 * of the navigation that can be tested for completeness (every destination has
 * a route, a label, a group and one shortcut at most).
 */

export type NavIconName =
  | "home"
  | "library"
  | "clock"
  | "bookmark"
  | "collections"
  | "download"
  | "gamepad"
  | "refresh"
  | "community"
  | "news"
  | "users"
  | "ingest"
  | "upload"
  | "health"
  | "metadata"
  | "authentication"
  | "server"
  | "settings";

export type NavGroupId = "library" | "play" | "community" | "admin" | "system";

/** `desktop` items only exist where installs do (native app or demo adapter). */
export type NavVisibility = "everyone" | "desktop" | "editor" | "admin";

export interface NavGroupMeta {
  id: NavGroupId;
  labelKey: string;
}

export interface NavDestination {
  id: string;
  /** Link target, query string included. */
  to: string;
  /** English message id; surfaces translate it with `t()`. */
  labelKey: string;
  group: NavGroupId;
  icon: NavIconName;
  visibility: NavVisibility;
  /** `g`-prefixed shortcut, written as both keys ("g l"). */
  kbd?: string;
  /** Aliases that show this destination as current (`/all` is the Library). */
  alsoActiveOn?: readonly string[];
  /** Treat nested paths as this destination (`/collections/5`). */
  prefix?: boolean;
}

export interface NavContext {
  editor: boolean;
  admin: boolean;
  desktop: boolean;
}

/**
 * Order matters: it is the order of the rail, the palette categories and the
 * More sheet. `system` is not a visible group; its destinations (Settings) are
 * rendered in the rail footer and in the mobile bar.
 */
export const NAV_GROUPS = [
  { id: "library", labelKey: "Library" },
  { id: "play", labelKey: "Play" },
  { id: "community", labelKey: "Community" },
  { id: "admin", labelKey: "Admin" },
  { id: "system", labelKey: "Settings" },
] as const satisfies readonly NavGroupMeta[];

export const NAV_DESTINATIONS = [
  {
    id: "home",
    to: "/",
    labelKey: "Home",
    group: "library",
    icon: "home",
    visibility: "everyone",
    kbd: "g h",
  },
  {
    id: "library",
    to: "/library",
    labelKey: "Library",
    group: "library",
    icon: "library",
    visibility: "everyone",
    kbd: "g l",
    // `/all` renders the same screen under another title; marking both keeps a
    // single rail entry lit instead of two destinations competing.
    alsoActiveOn: ["/all"],
  },
  {
    id: "recent",
    to: "/recent",
    labelKey: "Recently played",
    group: "library",
    icon: "clock",
    visibility: "everyone",
  },
  {
    id: "bookmarks",
    to: "/bookmarks",
    labelKey: "Bookmarks",
    group: "library",
    icon: "bookmark",
    visibility: "everyone",
    kbd: "g b",
  },
  {
    id: "collections",
    to: "/collections",
    labelKey: "Collections",
    group: "library",
    icon: "collections",
    visibility: "everyone",
    prefix: true,
  },
  {
    id: "downloads",
    to: "/downloads",
    labelKey: "Downloads",
    group: "play",
    icon: "download",
    visibility: "desktop",
    kbd: "g d",
  },
  {
    id: "bigpicture",
    to: "/bigpicture",
    labelKey: "Big Picture mode",
    group: "play",
    icon: "gamepad",
    visibility: "everyone",
  },
  {
    id: "updates",
    to: "/updates",
    labelKey: "Updates",
    group: "play",
    icon: "refresh",
    visibility: "everyone",
  },
  {
    id: "community",
    to: "/community",
    labelKey: "Community",
    group: "community",
    icon: "community",
    visibility: "everyone",
    prefix: true,
  },
  {
    id: "news",
    to: "/news",
    labelKey: "News",
    group: "community",
    icon: "news",
    visibility: "everyone",
  },
  {
    id: "ingest",
    to: "/admin/ingest",
    labelKey: "Ingest",
    group: "admin",
    icon: "ingest",
    visibility: "editor",
  },
  {
    id: "upload",
    to: "/admin/upload",
    labelKey: "Upload game",
    group: "admin",
    icon: "upload",
    visibility: "editor",
  },
  {
    id: "scan",
    to: "/admin/scan",
    labelKey: "Library scan",
    group: "admin",
    icon: "refresh",
    visibility: "editor",
  },
  {
    id: "health",
    to: "/admin/health",
    labelKey: "Library health",
    group: "admin",
    icon: "health",
    visibility: "editor",
  },
  {
    id: "users",
    to: "/admin/users",
    labelKey: "Users",
    group: "admin",
    icon: "users",
    visibility: "admin",
  },
  {
    id: "news-editor",
    to: "/admin/news",
    labelKey: "News editor",
    group: "admin",
    icon: "news",
    visibility: "admin",
  },
  {
    id: "metadata",
    to: "/admin/metadata",
    labelKey: "Metadata providers",
    group: "admin",
    icon: "metadata",
    visibility: "admin",
  },
  {
    id: "authentication",
    to: "/admin/authentication",
    labelKey: "Authentication",
    group: "admin",
    icon: "authentication",
    visibility: "admin",
  },
  {
    id: "server",
    to: "/admin/server",
    labelKey: "Server",
    group: "admin",
    icon: "server",
    visibility: "admin",
  },
  {
    id: "settings",
    to: "/settings",
    labelKey: "Settings",
    group: "system",
    icon: "settings",
    visibility: "everyone",
    kbd: "g s",
    prefix: true,
  },
] as const satisfies readonly NavDestination[];

/** `g i` kept from the old flat rail so the shortcut outlives the entry. */
export const LEGACY_NAV_SHORTCUTS: Readonly<Record<string, string>> = { i: "/all" };

/** Path of a destination without its query string. */
export function navPath(destination: NavDestination): string {
  return destination.to.split("?")[0];
}

export function canSeeNav(destination: NavDestination, context: NavContext): boolean {
  switch (destination.visibility) {
    case "everyone":
      return true;
    case "desktop":
      return context.desktop;
    case "editor":
      return context.editor;
    case "admin":
      return context.admin;
  }
}

/** Destinations visible to this account and device, in table order. */
export function visibleNav(context: NavContext): NavDestination[] {
  return NAV_DESTINATIONS.filter((destination) => canSeeNav(destination, context));
}

export interface NavGroup {
  group: NavGroupMeta;
  items: NavDestination[];
}

/** Visible destinations grouped for display; empty groups are dropped. */
export function navGroups(context: NavContext): NavGroup[] {
  const visible = visibleNav(context);
  return NAV_GROUPS.filter((group) => group.id !== "system")
    .map((group) => ({
      group,
      items: visible.filter((destination) => destination.group === group.id),
    }))
    .filter((entry) => entry.items.length > 0);
}

/** Label for a group id, including `system` (palette categories). */
export function navGroupMeta(id: NavGroupId): NavGroupMeta {
  return NAV_GROUPS.find((group) => group.id === id) ?? NAV_GROUPS[0];
}

/** The rail footer links: destinations outside the grouped list. */
export function systemNav(context: NavContext): NavDestination[] {
  return visibleNav(context).filter((destination) => destination.group === "system");
}

export function navDestination(id: string): NavDestination | undefined {
  return NAV_DESTINATIONS.find((destination) => destination.id === id);
}

/**
 * The current destination for a pathname. At most one destination matches:
 * exact paths win, then aliases, then prefixes.
 */
export function isNavActive(destination: NavDestination, path: string): boolean {
  const target = navPath(destination);
  if (path === target) return true;
  if (destination.alsoActiveOn?.includes(path)) return true;
  if (!destination.prefix || target === "/") return false;
  return path.startsWith(`${target}/`);
}

export function activeNavId(path: string, context: NavContext): string | null {
  return visibleNav(context).find((destination) => isNavActive(destination, path))?.id ?? null;
}

/** Second key of a `g`-prefixed shortcut → where it goes. */
export function navShortcutTarget(key: string, context: NavContext): string | undefined {
  const letter = key.toLowerCase();
  const destination = visibleNav(context).find((entry) => entry.kbd === `g ${letter}`);
  return destination?.to ?? LEGACY_NAV_SHORTCUTS[letter];
}

/** Every shortcut that can be advertised next to a destination. */
export function navShortcuts(context: NavContext): { kbd: string; to: string; labelKey: string }[] {
  return visibleNav(context)
    .filter((destination) => destination.kbd)
    .map((destination) => ({ kbd: destination.kbd as string, to: destination.to, labelKey: destination.labelKey }));
}

/**
 * The four destinations in the mobile bottom bar, plus More. Downloads needs a
 * desktop bridge; the slot falls back to Community instead of leaving a hole.
 */
export function mobileBar(context: NavContext): NavDestination[] {
  const ids = ["home", "library", context.desktop ? "downloads" : "community", "settings"];
  return ids.flatMap((id) => {
    const destination = navDestination(id);
    return destination && canSeeNav(destination, context) ? [destination] : [];
  });
}
