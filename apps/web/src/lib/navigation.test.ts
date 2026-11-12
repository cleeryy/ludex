import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import en from "../locales/en.json";
import {
  activeNavId,
  canSeeNav,
  isNavActive,
  LEGACY_NAV_SHORTCUTS,
  mobileBar,
  NAV_DESTINATIONS,
  NAV_GROUPS,
  navDestination,
  navGroups,
  navPath,
  navShortcutTarget,
  navShortcuts,
  systemNav,
  visibleNav,
  type NavContext,
} from "./navigation";
import { navGroupLabel, navLabel } from "@/components/shell/nav-copy";

const DESKTOP: NavContext = { editor: false, admin: false, desktop: true };
const EDITOR: NavContext = { editor: true, admin: false, desktop: true };
const ADMIN: NavContext = { editor: true, admin: true, desktop: true };
const BROWSER: NavContext = { editor: false, admin: false, desktop: false };

const appSource = readFileSync(new URL("../app.tsx", import.meta.url), "utf8");

describe("navigation table", () => {
  it("declares one label per destination, all present in the catalog", () => {
    for (const destination of NAV_DESTINATIONS) {
      expect(destination.labelKey.trim().length).toBeGreaterThan(0);
      expect(en).toHaveProperty(destination.labelKey);
    }
    const labels = NAV_DESTINATIONS.map((destination) => destination.labelKey);
    expect(new Set(labels).size).toBe(labels.length);
  });

  it("resolves every label through the translatable copy table", () => {
    for (const destination of NAV_DESTINATIONS) {
      expect(navLabel(destination), destination.id).toBe(destination.labelKey);
    }
    for (const group of NAV_GROUPS) expect(navGroupLabel(group.id)).toBe(group.labelKey);
  });

  it("has unique ids and does not repeat a destination", () => {
    const ids = NAV_DESTINATIONS.map((destination) => destination.id);
    const paths = NAV_DESTINATIONS.map((destination) => navPath(destination));
    expect(new Set(ids).size).toBe(ids.length);
    expect(new Set(paths).size).toBe(paths.length);
  });

  it("only links to routes the router declares", () => {
    for (const destination of NAV_DESTINATIONS) {
      const route = navPath(destination).replace(/^\//, "");
      if (route === "") {
        expect(appSource).toContain("<Route index");
      } else {
        expect(
          appSource,
          `app.tsx has no route for ${destination.to}`,
        ).toMatch(new RegExp(`path="/?${route.replace(/[/-]/g, "\\$&")}"`));
      }
    }
  });

  it("groups destinations under labelled headings", () => {
    const groups = navGroups(ADMIN);
    expect(groups.map((entry) => entry.group.id)).toEqual(["library", "play", "community", "admin"]);
    for (const entry of groups) {
      expect(en).toHaveProperty(entry.group.labelKey);
      expect(entry.items.length).toBeGreaterThan(0);
    }
    const listed = groups.flatMap((entry) => entry.items.map((item) => item.id));
    expect(listed.sort()).toEqual(
      NAV_DESTINATIONS.filter((destination) => destination.group !== "system")
        .map((destination) => destination.id)
        .sort(),
    );
  });

  it("keeps Settings out of the grouped rail list but in the footer", () => {
    expect(navGroups(DESKTOP).flatMap((entry) => entry.items.map((item) => item.id))).not.toContain("settings");
    expect(systemNav(DESKTOP).map((destination) => destination.id)).toEqual(["settings"]);
  });

  it("keeps the old duplicate Library entry points out of the rail", () => {
    const ids = NAV_DESTINATIONS.map((destination) => destination.id);
    expect(ids).not.toContain("index");
    expect(ids).not.toContain("installed");
  });
});

describe("navigation visibility", () => {
  it("hides admin tools from members and editors", () => {
    expect(visibleNav(DESKTOP).map((destination) => destination.id)).not.toContain("users");
    expect(visibleNav(DESKTOP).map((destination) => destination.id)).not.toContain("ingest");
    expect(visibleNav(EDITOR).map((destination) => destination.id)).toEqual(
      expect.arrayContaining(["ingest", "upload", "scan"]),
    );
    expect(visibleNav(EDITOR).map((destination) => destination.id)).not.toContain("server");
    expect(visibleNav(ADMIN).map((destination) => destination.id)).toEqual(
      expect.arrayContaining(["users", "news-editor", "metadata", "authentication", "server"]),
    );
  });

  it("only offers Downloads where installs exist", () => {
    expect(visibleNav(DESKTOP).map((destination) => destination.id)).toContain("downloads");
    expect(visibleNav(BROWSER).map((destination) => destination.id)).not.toContain("downloads");
  });

  it("falls back to Community in the mobile bar when Downloads is unavailable", () => {
    expect(mobileBar(DESKTOP).map((destination) => destination.id)).toEqual([
      "home",
      "library",
      "downloads",
      "settings",
    ]);
    expect(mobileBar(BROWSER).map((destination) => destination.id)).toEqual([
      "home",
      "library",
      "remote",
      "settings",
    ]);
  });

  it("gates each destination explicitly", () => {
    const users = navDestination("users");
    expect(users && canSeeNav(users, EDITOR)).toBe(false);
    expect(users && canSeeNav(users, ADMIN)).toBe(true);
    const downloads = navDestination("downloads");
    expect(downloads && canSeeNav(downloads, BROWSER)).toBe(false);
    expect(downloads && canSeeNav(downloads, DESKTOP)).toBe(true);
  });
});

describe("active state", () => {
  it("lights exactly one destination per path", () => {
    for (const path of ["/", "/library", "/all", "/recent", "/bookmarks", "/collections/7", "/downloads", "/remote", "/settings", "/news", "/community/u/2", "/admin/ingest"]) {
      const matches = visibleNav(ADMIN).filter((destination) => isNavActive(destination, path));
      expect(matches.length, `two destinations match ${path}`).toBe(1);
      expect(activeNavId(path, ADMIN)).toBe(matches[0].id);
    }
  });

  it("treats /all as the Library, not a second place", () => {
    expect(activeNavId("/all", DESKTOP)).toBe("library");
    expect(activeNavId("/library", DESKTOP)).toBe("library");
    expect(isNavActive(navDestination("recent")!, "/library")).toBe(false);
  });

  it("marks nested paths and settings query strings as their section", () => {
    expect(activeNavId("/collections/12", DESKTOP)).toBe("collections");
    expect(activeNavId("/community/u/3", DESKTOP)).toBe("community");
    expect(activeNavId("/settings", DESKTOP)).toBe("settings");
    expect(isNavActive(navDestination("home")!, "/library")).toBe(false);
  });
});

describe("keyboard shortcuts", () => {
  it("keeps the shipped g-prefixed shortcuts", () => {
    expect(navShortcutTarget("h", DESKTOP)).toBe("/");
    expect(navShortcutTarget("l", DESKTOP)).toBe("/library");
    expect(navShortcutTarget("b", DESKTOP)).toBe("/bookmarks");
    expect(navShortcutTarget("d", DESKTOP)).toBe("/downloads");
    expect(navShortcutTarget("s", DESKTOP)).toBe("/settings");
  });

  it("keeps g i as an alias for the index view and ignores unknown keys", () => {
    expect(navShortcutTarget("i", DESKTOP)).toBe(LEGACY_NAV_SHORTCUTS.i);
    expect(navShortcutTarget("z", DESKTOP)).toBeUndefined();
  });

  it("does not advertise a shortcut for a hidden destination", () => {
    expect(navShortcutTarget("d", BROWSER)).toBeUndefined();
    expect(navShortcuts(BROWSER).map((entry) => entry.kbd)).not.toContain("g d");
  });

  it("assigns each shortcut to one destination", () => {
    const shortcuts = navShortcuts(ADMIN).map((entry) => entry.kbd);
    expect(new Set(shortcuts).size).toBe(shortcuts.length);
  });
});

describe("group metadata", () => {
  it("keeps every group id reachable from the table", () => {
    const ids = new Set(NAV_GROUPS.map((group) => group.id));
    for (const destination of NAV_DESTINATIONS) expect(ids.has(destination.group)).toBe(true);
  });
});
