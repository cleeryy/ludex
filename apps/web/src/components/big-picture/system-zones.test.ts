import { describe, expect, it } from "vitest";
import {
  HOME_ITEM_IDS,
  SETTINGS_SECTIONS,
  atLibraryTop,
  closeSystemApp,
  levelFromSearch,
  openSystemApp,
  sameLevel,
  settingsSectionFromSearch,
  stepSettingsSection,
  wrapIndex,
  zoneAfterMove,
  type ShellLevel,
} from "./system-zones";

/* --------------------------------------------------------------------------
   #196: the console shell's navigation state machine.
   -------------------------------------------------------------------------- */

describe("console home row", () => {
  it("keeps the couch order stable", () => {
    expect([...HOME_ITEM_IDS]).toEqual([
      "continue",
      "library",
      "downloads",
      "collections",
      "settings",
      "power",
    ]);
  });

  it("wraps at both ends instead of dead-ending on Power", () => {
    expect(wrapIndex(0, -1, HOME_ITEM_IDS.length)).toBe(HOME_ITEM_IDS.length - 1);
    expect(wrapIndex(HOME_ITEM_IDS.length - 1, 1, HOME_ITEM_IDS.length)).toBe(0);
    expect(wrapIndex(2, 2, 6)).toBe(4);
    expect(wrapIndex(0, -7, 6)).toBe(5);
    expect(wrapIndex(3, 0, 6)).toBe(3);
    expect(wrapIndex(4, 1, 0)).toBe(0);
  });

  it("moves Up into the home row only when the library has run out of rows", () => {
    expect(zoneAfterMove("library", "up", { atLibraryTop: true })).toBe("home");
    expect(zoneAfterMove("library", "up", { atLibraryTop: false })).toBe("library");
    expect(zoneAfterMove("home", "down", { atLibraryTop: false })).toBe("library");
    expect(zoneAfterMove("home", "up", { atLibraryTop: false })).toBe("home");
    expect(zoneAfterMove("library", "left", { atLibraryTop: true })).toBe("library");
  });

  it("knows the first row of every layout", () => {
    expect(atLibraryTop({ layout: "shelf", index: 0, shelfIndex: 0, columns: 6 })).toBe(true);
    expect(atLibraryTop({ layout: "shelf", index: 0, shelfIndex: 1, columns: 6 })).toBe(false);
    expect(atLibraryTop({ layout: "hero", index: 4, shelfIndex: 0, columns: 6 })).toBe(true);
    expect(atLibraryTop({ layout: "grid", index: 5, shelfIndex: 0, columns: 6 })).toBe(true);
    expect(atLibraryTop({ layout: "grid", index: 6, shelfIndex: 0, columns: 6 })).toBe(false);
    expect(atLibraryTop({ layout: "compact", index: 0, shelfIndex: 0, columns: 6 })).toBe(true);
    expect(atLibraryTop({ layout: "compact", index: 1, shelfIndex: 0, columns: 6 })).toBe(false);
  });
});

describe("console app level", () => {
  it("opens settings and power as app levels and always returns to the grid", () => {
    const settings = openSystemApp("settings");
    expect(settings).toEqual({ kind: "app", id: "settings" });
    expect(closeSystemApp()).toEqual({ kind: "library" });
    expect(closeSystemApp()).toEqual(closeSystemApp());
  });

  it("round-trips Settings back to the game grid", () => {
    const opened = openSystemApp("settings");
    const returned = closeSystemApp();
    expect(opened.kind).toBe("app");
    expect(returned).toEqual({ kind: "library" });
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
    expect([...SETTINGS_SECTIONS]).toEqual(["language", "appearance", "controllers", "streaming"]);
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
