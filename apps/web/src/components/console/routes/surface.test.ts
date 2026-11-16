// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  CONSOLE_ROOT,
  LEGACY_CONSOLE_ROOT,
  consoleLocationFor,
  consolePathFor,
  legacyConsoleRedirect,
} from "@/components/console/routes/surface";
import { CONSOLE_ROUTES } from "@/components/console/routes/table";

describe("console route table", () => {
  it("parses every deep link the shell and the desktop can send", () => {
    expect(consoleLocationFor("/console")).toEqual({ surface: "home", gameId: null, section: null });
    expect(consoleLocationFor("/console/")).toEqual({ surface: "home", gameId: null, section: null });
    expect(consoleLocationFor("/console/library")).toEqual({ surface: "library", gameId: null, section: null });
    expect(consoleLocationFor("/console/game/42")).toEqual({ surface: "game", gameId: 42, section: null });
    expect(consoleLocationFor("/console/settings")).toEqual({ surface: "settings", gameId: null, section: null });
    expect(consoleLocationFor("/console/settings/appearance")).toEqual({
      surface: "settings",
      gameId: null,
      section: "appearance",
    });
    expect(consoleLocationFor("/console/downloads")?.surface).toBe("downloads");
    expect(consoleLocationFor("/console/search")?.surface).toBe("search");
    expect(consoleLocationFor("/console/power")?.surface).toBe("power");
  });

  it("does not hijack a desktop path", () => {
    expect(consoleLocationFor("/library")).toBeNull();
    expect(consoleLocationFor("/bigpicture")).toBeNull();
    expect(consoleLocationFor("/")).toBeNull();
  });

  it("falls back to home for an unknown or malformed console path", () => {
    expect(consoleLocationFor("/console/unknown")?.surface).toBe("home");
    expect(consoleLocationFor("/console/game/nope")).toEqual({ surface: "home", gameId: null, section: null });
    expect(consoleLocationFor("/console/game/0")?.surface).toBe("home");
  });

  it("builds the canonical path for every surface", () => {
    expect(consolePathFor("home")).toBe(CONSOLE_ROOT);
    expect(consolePathFor("library")).toBe("/console/library");
    expect(consolePathFor("game", { gameId: 7 })).toBe("/console/game/7");
    expect(consolePathFor("game")).toBe("/console/library");
    expect(consolePathFor("settings")).toBe("/console/settings");
    expect(consolePathFor("settings", { section: "controllers" })).toBe("/console/settings/controllers");
    expect(consolePathFor("downloads")).toBe("/console/downloads");
    expect(consolePathFor("search")).toBe("/console/search");
    expect(consolePathFor("power")).toBe("/console/power");
    // One release of compatibility: the old route is a redirect, not a surface.
    expect(LEGACY_CONSOLE_ROOT).toBe("/bigpicture");
  });

  it("declares the seven addressable surfaces once, without duplicates", () => {
    const surfaces = CONSOLE_ROUTES.map((route) => route.surface);
    expect(new Set(surfaces)).toEqual(
      new Set(["home", "library", "game", "settings", "downloads", "search", "power"]),
    );
    expect(CONSOLE_ROUTES.find((route) => route.surface === "home")?.path).toBe(CONSOLE_ROOT);
    expect(CONSOLE_ROUTES.find((route) => route.surface === "settings")?.path).toBe("/console/settings");
  });

  it("redirects /bigpicture to /console for one release", () => {
    expect(legacyConsoleRedirect("/bigpicture")).toBe(CONSOLE_ROOT);
    expect(legacyConsoleRedirect("/bigpicture/")).toBe(CONSOLE_ROOT);
    expect(legacyConsoleRedirect("/console")).toBeNull();
    expect(legacyConsoleRedirect("/bigpicture/game/1")).toBeNull();
  });

  it("keeps the section slug out of the game parser", () => {
    const location = consoleLocationFor("/console/settings/streaming");
    expect(location?.surface).toBe("settings");
    expect(location?.section).toBe("streaming");
    expect(location?.gameId).toBeNull();
  });
});
