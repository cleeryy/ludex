import { describe, expect, it } from "vitest";
import {
  HOME_ITEM_IDS,
  SETTINGS_SECTIONS,
  atLibraryBottom,
  atLibraryTop,
  closeSystemApp,
  levelFromSearch,
  openSystemApp,
  restoreZone,
  sameLevel,
  settingsSectionFromSearch,
  stepSettingsSection,
  wrapIndex,
  zoneAfterMove,
  zoneExit,
  zonesForLayout,
  type ShellLevel,
} from "./system-zones";

/* --------------------------------------------------------------------------
   #268: the console shell's navigation state machine. Console home is one axis
   with three zones: stage actions, games, band. Settings and Power replace the
   games but keep the bar and the hints.
   -------------------------------------------------------------------------- */

describe("the console band", () => {
  it("keeps the five destinations in couch order", () => {
    expect([...HOME_ITEM_IDS]).toEqual(["search", "library", "downloads", "settings", "power"]);
  });

  it("wraps at both ends instead of dead-ending on Power", () => {
    expect(wrapIndex(0, -1, HOME_ITEM_IDS.length)).toBe(HOME_ITEM_IDS.length - 1);
    expect(wrapIndex(HOME_ITEM_IDS.length - 1, 1, HOME_ITEM_IDS.length)).toBe(0);
    expect(wrapIndex(2, 2, 5)).toBe(4);
    expect(wrapIndex(0, -6, 5)).toBe(4);
    expect(wrapIndex(3, 0, 5)).toBe(3);
    expect(wrapIndex(4, 1, 0)).toBe(0);
  });

  it("moves Up from a single-row rail into the actions and Down into the band", () => {
    expect(zoneAfterMove("library", "up", { layout: "shelf", atLibraryTop: true })).toBe("actions");
    expect(zoneAfterMove("library", "down", { layout: "shelf", atLibraryBottom: true })).toBe("home");
    expect(zoneAfterMove("actions", "down", { layout: "shelf", atLibraryTop: true })).toBe("library");
    expect(zoneAfterMove("home", "up", { layout: "shelf", atLibraryBottom: true })).toBe("library");
    expect(zoneAfterMove("library", "left", { layout: "shelf", atLibraryTop: true })).toBe("library");
  });

  it("only hands a direction to a zone when the games run out", () => {
    expect(zoneAfterMove("library", "up", { layout: "grid", atLibraryTop: false })).toBe("library");
    expect(zoneAfterMove("library", "down", { layout: "grid", atLibraryBottom: false })).toBe("library");
    expect(zoneAfterMove("library", "up", { layout: "grid", atLibraryTop: true })).toBe("actions");
    expect(zoneAfterMove("library", "down", { layout: "grid", atLibraryBottom: true })).toBe("home");
  });

  it("knows the first and last row of every composition", () => {
    expect(atLibraryTop({ layout: "shelf", index: 0, columns: 6 })).toBe(true);
    expect(atLibraryTop({ layout: "shelf", index: 4, columns: 6 })).toBe(true);
    expect(atLibraryTop({ layout: "grid", index: 5, columns: 6 })).toBe(true);
    expect(atLibraryTop({ layout: "grid", index: 6, columns: 6 })).toBe(false);
    expect(atLibraryTop({ layout: "compact", index: 0, columns: 6 })).toBe(true);
    expect(atLibraryTop({ layout: "compact", index: 1, columns: 6 })).toBe(false);

    expect(atLibraryBottom({ layout: "shelf", index: 0, total: 9, columns: 6 })).toBe(true);
    expect(atLibraryBottom({ layout: "grid", index: 8, total: 9, columns: 6 })).toBe(true);
    expect(atLibraryBottom({ layout: "grid", index: 3, total: 9, columns: 6 })).toBe(false);
    expect(atLibraryBottom({ layout: "compact", index: 8, total: 9, columns: 6 })).toBe(true);
    expect(atLibraryBottom({ layout: "compact", index: 7, total: 9, columns: 6 })).toBe(false);
  });
});

describe("the zone graph", () => {
  it("draws the same three zones in every composition", () => {
    for (const layout of ["shelf", "grid", "compact"] as const) {
      expect(zonesForLayout(layout)).toEqual(["actions", "library", "home"]);
    }
  });

  it("exits the actions downwards and the band upwards", () => {
    expect(zoneExit("shelf", "actions")).toBe("down");
    expect(zoneExit("shelf", "home")).toBe("up");
    expect(zoneExit("grid", "actions")).toBe("down");
    expect(zoneExit("compact", "home")).toBe("up");
  });

  it("restores a zone after a composition switch", () => {
    expect(restoreZone("grid", "home")).toBe("home");
    expect(restoreZone("compact", "actions")).toBe("actions");
    expect(restoreZone("shelf", "library")).toBe("library");
  });
});

describe("console app level", () => {
  it("opens settings and power as app levels and always returns to the games", () => {
    const settings = openSystemApp("settings");
    expect(settings).toEqual({ kind: "app", id: "settings" });
    expect(closeSystemApp()).toEqual({ kind: "library" });
    expect(closeSystemApp()).toEqual(closeSystemApp());
  });

  it("compares levels without treating two apps as the same surface", () => {
    expect(sameLevel({ kind: "library" }, { kind: "library" })).toBe(true);
    expect(sameLevel(openSystemApp("settings"), openSystemApp("settings"))).toBe(true);
    expect(sameLevel(openSystemApp("settings"), openSystemApp("power"))).toBe(false);
  });

  it("restores the shell level from the URL after a locale remount", () => {
    expect(levelFromSearch("?app=settings&section=language")).toEqual({ kind: "app", id: "settings" });
    expect(levelFromSearch("?app=power")).toEqual({ kind: "app", id: "power" });
    expect(levelFromSearch("?app=unknown")).toEqual({ kind: "library" });
    expect(levelFromSearch("")).toEqual({ kind: "library" });
  });

  it("validates the settings section instead of trusting the URL", () => {
    expect(settingsSectionFromSearch("?app=settings&section=controllers")).toBe("controllers");
    expect(settingsSectionFromSearch("?section=nope")).toBe(SETTINGS_SECTIONS[0]);
    expect(settingsSectionFromSearch("")).toBe(SETTINGS_SECTIONS[0]);
  });
});

describe("settings sections", () => {
  it("keeps the four categories in shoulder order", () => {
    expect([...SETTINGS_SECTIONS]).toEqual(["language", "appearance", "controllers", "audio", "streaming"]);
  });

  it("wraps with LB/RB", () => {
    expect(stepSettingsSection("language", -1)).toBe("streaming");
    expect(stepSettingsSection("streaming", 1)).toBe("language");
    expect(stepSettingsSection("appearance", 1)).toBe("controllers");
  });
});

// A compile-time guard against an accidental widening of the level type.
const _library: ShellLevel = { kind: "library" };
void _library;
