import { describe, expect, it } from "vitest";
import {
  activeDisplayOutput,
  availableFilters,
  DEFAULT_DISPLAY_PROFILE,
  DEFAULT_GAMESCOPE_HEIGHT,
  DEFAULT_GAMESCOPE_WIDTH,
  displayCapabilityBlock,
  displayCapabilityLine,
  displayProfileSummary,
  withDisplayMode,
} from "./display-profile";
import type { DisplayCapabilities, GamescopeInfo } from "@/lib/types";

function gamescope(overrides: Partial<GamescopeInfo> = {}): GamescopeInfo {
  return {
    path: "/usr/bin/gamescope",
    version: "3.15.14",
    hdr: true,
    adaptiveSync: true,
    filters: ["linear", "nearest", "fsr", "nis", "pixel", "sgsr"],
    ...overrides,
  };
}

function capabilities(overrides: Partial<DisplayCapabilities> = {}): DisplayCapabilities {
  return {
    outputs: [
      {
        connector: "card0-DP-1",
        status: "connected",
        active: true,
        vrr: "supported",
        hdr: "supported",
        maxRefresh: 240,
        colorDepth: 10,
      },
    ],
    active: "card0-DP-1",
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
      adaptiveSync: false,
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
        adaptiveSync: true,
        hdr: true,
      }),
    ).toBe("1920 × 1080 · 60 Hz · FSR · VRR · HDR");
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

describe("display capability gating (#292)", () => {
  it("keeps both switches available when the pair supports them", () => {
    expect(displayCapabilityBlock("vrr", gamescope(), capabilities())).toBeNull();
    expect(displayCapabilityBlock("hdr", gamescope(), capabilities())).toBeNull();
  });

  it("blocks both with no gamescope, before any output check", () => {
    expect(displayCapabilityBlock("vrr", null, capabilities())).toBe("noGamescope");
    expect(displayCapabilityBlock("hdr", null, capabilities())).toBe("noGamescope");
  });

  it("blocks a switch the installed build does not advertise", () => {
    expect(
      displayCapabilityBlock("vrr", gamescope({ adaptiveSync: false }), capabilities()),
    ).toBe("unsupportedBuild");
    expect(displayCapabilityBlock("hdr", gamescope({ hdr: false }), capabilities())).toBe(
      "unsupportedBuild",
    );
    // The other switch is independent.
    expect(displayCapabilityBlock("hdr", gamescope({ adaptiveSync: false }), capabilities())).toBeNull();
    expect(displayCapabilityBlock("vrr", gamescope({ hdr: false }), capabilities())).toBeNull();
  });

  it("blocks a switch the active output reports as incompatible", () => {
    expect(
      displayCapabilityBlock(
        "vrr",
        gamescope(),
        capabilities({ outputs: [{ ...capabilities().outputs[0], vrr: "incapable" }] }),
      ),
    ).toBe("output");
    expect(
      displayCapabilityBlock(
        "hdr",
        gamescope(),
        capabilities({ outputs: [{ ...capabilities().outputs[0], hdr: "incapable" }] }),
      ),
    ).toBe("output");
  });

  it("never blocks on an unknown capability", () => {
    const unknown = capabilities({
      outputs: [{ ...capabilities().outputs[0], vrr: "unknown", hdr: "unknown" }],
    });
    expect(displayCapabilityBlock("vrr", gamescope(), unknown)).toBeNull();
    expect(displayCapabilityBlock("hdr", gamescope(), unknown)).toBeNull();
    expect(displayCapabilityBlock("vrr", gamescope(), null)).toBeNull();
  });

  it("reads the active output, then the first connected one", () => {
    const caps = capabilities({
      outputs: [
        { ...capabilities().outputs[0], connector: "card0-HDMI-A-1", active: false },
        { ...capabilities().outputs[0], connector: "card0-DP-2", active: true },
      ],
      active: "card0-DP-2",
    });
    expect(activeDisplayOutput(caps)?.connector).toBe("card0-DP-2");
    expect(activeDisplayOutput(capabilities({ active: null }))?.connector).toBe("card0-DP-1");
    expect(activeDisplayOutput(null)).toBeNull();
  });

  it("summarises the active output as one data line", () => {
    expect(displayCapabilityLine(capabilities())).toBe(
      "card0-DP-1 · VRR: supported · HDR: supported · 240 Hz · 10-bit",
    );
    expect(
      displayCapabilityLine(
        capabilities({
          outputs: [
            {
              ...capabilities().outputs[0],
              vrr: "incapable",
              hdr: "incapable",
              maxRefresh: null,
              colorDepth: null,
            },
          ],
        }),
      ),
    ).toBe("card0-DP-1 · VRR: incompatible · HDR: no metadata");
    expect(displayCapabilityLine(capabilities({ outputs: [] }))).toBeNull();
    expect(displayCapabilityLine(null)).toBeNull();
  });
});
