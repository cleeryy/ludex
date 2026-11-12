import { describe, expect, it } from "vitest";
import {
  clampOverlayOpacity,
  clampOverlayScale,
  defaultOverlayTab,
  formatOverlayClock,
  isOverlayCloseKey,
  isOverlayShortcut,
  moveFocusIndex,
  normalizeOverlayStatus,
  overlayDurationParts,
  overlayElapsedSeconds,
  switchOverlayTab,
  trapFocusIndex,
  visibleOverlayTabs,
} from "./overlay";

describe("overlay tabs", () => {
  it("always keeps the game tab and the canonical order", () => {
    expect(visibleOverlayTabs([])).toEqual(["game"]);
    expect(visibleOverlayTabs(["downloads", "game"])).toEqual(["game", "downloads"]);
    expect(visibleOverlayTabs(["achievements", "library", "downloads", "game"])).toEqual([
      "game",
      "library",
      "downloads",
      "achievements",
    ]);
  });

  it("drops unknown ids and duplicates", () => {
    expect(visibleOverlayTabs(["game", "social", "game", "library"])).toEqual(["game", "library"]);
    expect(visibleOverlayTabs(null)).toEqual(["game"]);
    expect(visibleOverlayTabs(undefined)).toEqual(["game"]);
  });

  it("opens on the game tab when a game is running", () => {
    const tabs = visibleOverlayTabs(["game", "library", "downloads"]);
    expect(defaultOverlayTab({ game: null, tabs })).toBe("library");
    expect(
      defaultOverlayTab({
        game: {
          sessionId: "s",
          gameId: 1,
          title: "Game",
          startedAt: "2026-01-01T00:00:00Z",
          elapsedSeconds: 0,
          totalSeconds: 0,
          disabled: false,
        },
        tabs,
      }),
    ).toBe("game");
    // With no content tab enabled the game tab is the only choice.
    expect(defaultOverlayTab({ game: null, tabs: ["game"] })).toBe("game");
  });

  it("switches tabs with wrap-around", () => {
    const tabs = visibleOverlayTabs(["game", "library", "downloads"]);
    expect(switchOverlayTab(tabs, "game", 1)).toBe("library");
    expect(switchOverlayTab(tabs, "downloads", 1)).toBe("game");
    expect(switchOverlayTab(tabs, "game", -1)).toBe("downloads");
    expect(switchOverlayTab([], "game", 1)).toBe("game");
  });
});

describe("overlay appearance", () => {
  it("clamps opacity and scale, replacing bad values with defaults", () => {
    expect(clampOverlayOpacity(0.92)).toBe(0.92);
    expect(clampOverlayOpacity(0)).toBe(0.35);
    expect(clampOverlayOpacity(9)).toBe(1);
    expect(clampOverlayOpacity(Number.NaN)).toBe(0.92);
    expect(clampOverlayOpacity(null)).toBe(0.92);
    expect(clampOverlayScale(0.1)).toBe(0.6);
    expect(clampOverlayScale(4)).toBe(1.3);
    expect(clampOverlayScale(undefined)).toBe(1);
  });
});

describe("overlay playtime", () => {
  it("computes elapsed seconds and ignores incomplete timestamps", () => {
    const now = Date.parse("2026-01-01T01:00:00Z");
    expect(overlayElapsedSeconds("2026-01-01T00:30:00Z", now)).toBe(1800);
    expect(overlayElapsedSeconds(null, now)).toBe(0);
    expect(overlayElapsedSeconds("not a date", now)).toBe(0);
    // Clock skew must never show a negative session time.
    expect(overlayElapsedSeconds("2026-01-01T02:00:00Z", now)).toBe(0);
  });

  it("formats a session clock without localized copy", () => {
    expect(formatOverlayClock(0)).toBe("0:00");
    expect(formatOverlayClock(59)).toBe("0:59");
    expect(formatOverlayClock(60)).toBe("1:00");
    expect(formatOverlayClock(3_600)).toBe("1:00:00");
    expect(formatOverlayClock(3_661)).toBe("1:01:01");
    expect(formatOverlayClock(-5)).toBe("0:00");
  });

  it("splits durations for translation", () => {
    expect(overlayDurationParts(0)).toEqual({ hours: 0, minutes: 0 });
    expect(overlayDurationParts(90)).toEqual({ hours: 0, minutes: 1 });
    expect(overlayDurationParts(3_600 + 25 * 60)).toEqual({ hours: 1, minutes: 25 });
  });
});

describe("overlay focus and close", () => {
  it("recognises only Shift+Tab as the keyboard trigger", () => {
    const base = { key: "Tab", shiftKey: true };
    expect(isOverlayShortcut(base)).toBe(true);
    expect(isOverlayShortcut({ ...base, shiftKey: false })).toBe(false);
    expect(isOverlayShortcut({ ...base, ctrlKey: true })).toBe(false);
    expect(isOverlayShortcut({ ...base, altKey: true })).toBe(false);
    expect(isOverlayShortcut({ ...base, metaKey: true })).toBe(false);
    expect(isOverlayShortcut({ ...base, key: "Escape" })).toBe(false);
  });

  it("closes on Escape and B", () => {
    expect(isOverlayCloseKey("Escape")).toBe(true);
    expect(isOverlayCloseKey("b")).toBe(true);
    expect(isOverlayCloseKey("B")).toBe(true);
    expect(isOverlayCloseKey("Enter")).toBe(false);
    expect(isOverlayCloseKey("Tab")).toBe(false);
  });

  it("keeps Tab inside the overlay", () => {
    expect(trapFocusIndex(0, 4, false)).toBe(1);
    expect(trapFocusIndex(3, 4, false)).toBe(0);
    expect(trapFocusIndex(0, 4, true)).toBe(3);
    expect(trapFocusIndex(2, 4, true)).toBe(1);
    expect(trapFocusIndex(0, 0, false)).toBe(0);
    // Out-of-range indexes are clamped, never used as-is.
    expect(trapFocusIndex(99, 4, false)).toBe(0);
  });

  it("moves the gamepad ring with wrap-around", () => {
    expect(moveFocusIndex(0, 3, 1)).toBe(1);
    expect(moveFocusIndex(2, 3, 1)).toBe(0);
    expect(moveFocusIndex(0, 3, -1)).toBe(2);
    expect(moveFocusIndex(0, 0, 1)).toBe(0);
  });
});

describe("normalizeOverlayStatus", () => {
  it("fills every field for a browser or an older desktop", () => {
    const status = normalizeOverlayStatus(null);
    expect(status.enabled).toBe(true);
    expect(status.opacity).toBe(0.92);
    expect(status.scale).toBe(1);
    expect(status.tabs).toEqual(["game", "library", "downloads", "achievements"]);
    expect(status.presentation).toBe("closed");
    expect(status.listener.available).toBe(false);
    expect(status.keyboardShortcut).toBe("Shift+Tab");
  });

  it("keeps a valid desktop payload intact", () => {
    const status = normalizeOverlayStatus({
      enabled: false,
      open: true,
      trigger: "chord",
      opacity: 0.7,
      scale: 1.2,
      tabs: ["game", "downloads"],
      disabledGames: [7],
      presentation: "window",
      listener: {
        backend: "gilrs",
        available: true,
        platform: "linux",
        reason: null,
        controllers: [{ id: "0", name: "Pad", active: true, battery: 80, charging: false }],
      },
      platformNote: "note",
      keyboardShortcut: "Shift+Tab",
    });
    expect(status.enabled).toBe(false);
    expect(status.open).toBe(true);
    expect(status.trigger).toBe("chord");
    expect(status.tabs).toEqual(["game", "downloads"]);
    expect(status.disabledGames).toEqual([7]);
    expect(status.listener.controllers[0]?.name).toBe("Pad");
  });

  it("drops invalid disabled game ids", () => {
    expect(normalizeOverlayStatus({ disabledGames: [3, -1, 0, 8] }).disabledGames).toEqual([3, 8]);
  });
});
