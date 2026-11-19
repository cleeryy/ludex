import { describe, expect, it } from "vitest";
import {
  availableFilters,
  DEFAULT_DISPLAY_PROFILE,
  DEFAULT_GAMESCOPE_HEIGHT,
  DEFAULT_GAMESCOPE_WIDTH,
  displayProfileSummary,
  withDisplayMode,
} from "./display-profile";
import type { GamescopeInfo } from "@/lib/types";

function gamescope(overrides: Partial<GamescopeInfo> = {}): GamescopeInfo {
  return {
    path: "/usr/bin/gamescope",
    version: "3.15.14",
    hdr: true,
    filters: ["linear", "nearest", "fsr", "nis", "pixel", "sgsr"],
    ...overrides,
  };
}

describe("display profile helpers (#287)", () => {
  it("keeps the desktop default a no-op", () => {
    expect(DEFAULT_DISPLAY_PROFILE).toEqual({
      mode: "desktop",
      width: 0,
      height: 0,
      refresh: 0,
      filter: "",
      hdr: false,
    });
    expect(displayProfileSummary(DEFAULT_DISPLAY_PROFILE)).toBeNull();
  });

  it("seeds a resolution when Gamescope is first enabled", () => {
    const enabled = withDisplayMode(DEFAULT_DISPLAY_PROFILE, "gamescope");
    expect(enabled.mode).toBe("gamescope");
    expect(enabled.width).toBe(DEFAULT_GAMESCOPE_WIDTH);
    expect(enabled.height).toBe(DEFAULT_GAMESCOPE_HEIGHT);
  });

  it("keeps an existing resolution when toggling the mode back and forth", () => {
    const custom = { ...DEFAULT_DISPLAY_PROFILE, width: 2560, height: 1440 };
    const enabled = withDisplayMode(custom, "gamescope");
    expect(enabled.width).toBe(2560);
    expect(enabled.height).toBe(1440);
    const disabled = withDisplayMode(enabled, "desktop");
    expect(disabled.width).toBe(2560);
    expect(displayProfileSummary(disabled)).toBeNull();
  });

  it("describes only the values that will be passed to Gamescope", () => {
    expect(
      displayProfileSummary({ ...DEFAULT_DISPLAY_PROFILE, mode: "gamescope", width: 1920, height: 1080 }),
    ).toBe("1920 × 1080");
    expect(
      displayProfileSummary({
        mode: "gamescope",
        width: 1920,
        height: 1080,
        refresh: 60,
        filter: "fsr",
        hdr: true,
      }),
    ).toBe("1920 × 1080 · 60 Hz · FSR · HDR");
  });

  it("offers only the filters the installed Gamescope advertises", () => {
    expect(availableFilters(gamescope())).toEqual([
      "linear",
      "nearest",
      "fsr",
      "nis",
      "pixel",
      "sgsr",
    ]);
    expect(availableFilters(gamescope({ filters: [] }))).toEqual([]);
    expect(availableFilters(null)).toEqual([]);
  });
});
