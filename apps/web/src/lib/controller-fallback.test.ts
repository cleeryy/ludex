import { describe, expect, it } from "vitest";
import { presetProfile } from "./controller-profiles";
import {
  classifyAxes,
  fallbackProfile,
  needsFallbackProfile,
  type FallbackPadLike,
} from "./controller-fallback";

/* --------------------------------------------------------------------------
   #230: a pad the browser does not report as standard must still navigate.
   These tests pin the three traps: guessed face buttons, a d-pad reported as a
   discrete hat, and a trigger axis that rests at -1.
   -------------------------------------------------------------------------- */

function pad(overrides: Partial<FallbackPadLike> = {}): FallbackPadLike {
  return { mapping: "", buttons: 10, axes: 4, ...overrides };
}

function buttons(count: number) {
  return Array.from({ length: count }, () => ({ pressed: false, value: 0 }));
}

describe("fallback detection", () => {
  it("asks for the fallback when the mapping is not standard", () => {
    expect(needsFallbackProfile(pad({ mapping: "" }))).toBe(true);
    expect(needsFallbackProfile(pad({ mapping: "standard", buttons: buttons(12) }))).toBe(false);
    expect(needsFallbackProfile(null)).toBe(false);
  });

  it("asks for the fallback when the pad has too few buttons", () => {
    expect(needsFallbackProfile(pad({ mapping: "standard", buttons: buttons(10) }))).toBe(true);
    expect(needsFallbackProfile(pad({ mapping: "standard", buttons: buttons(12) }))).toBe(false);
  });

  it("leaves a standard pad on its own profile", () => {
    const base = presetProfile("dualsense");
    expect(fallbackProfile(pad({ mapping: "standard", buttons: buttons(16) }), { base })).toBe(base);
  });
});

describe("fallback profile", () => {
  it("guesses the four face buttons and the left stick", () => {
    const profile = fallbackProfile(pad({ buttons: buttons(6), axes: 4 }));
    expect(profile.mappings.button0).toBe("confirm");
    expect(profile.mappings.button1).toBe("back");
    expect(profile.mappings.button2).toBe("menu");
    expect(profile.mappings.button3).toBe("downloads");
    expect(profile.mappings["axis0-"]).toBe("left");
    expect(profile.mappings["axis1+"]).toBe("down");
  });

  it("keeps the right stick dead instead of guessing a direction", () => {
    const profile = fallbackProfile(pad({ buttons: buttons(6), axes: 4 }));
    expect(profile.mappings["axis2-"]).toBeUndefined();
    expect(profile.mappings["axis3+"]).toBeUndefined();
  });

  it("reads a d-pad reported as a discrete hat", () => {
    const samples = [
      [0, 0, 0, 0, 0, 0],
      [0, 0, 0, 0, 1, 0],
      [0, 0, 0, 0, 0, -1],
    ];
    const profile = fallbackProfile(pad({ axes: 6 }), { samples });
    expect(profile.mappings["axis4-"]).toBe("left");
    expect(profile.mappings["axis4+"]).toBe("right");
    expect(profile.mappings["axis5-"]).toBe("up");
    expect(profile.mappings["axis5+"]).toBe("down");
  });

  it("never maps a trigger axis that rests at -1", () => {
    // Triggers on a DirectInput pad: axes 4/5 idle at -1 and rise to +1.
    const samples = [
      [0, 0, 0, 0, -1, -1],
      [0, 0, 0, 0, -0.6, -1],
    ];
    const classification = classifyAxes(samples);
    expect(classification.triggers).toEqual([4, 5]);
    expect(classification.hats).toEqual([]);
    const profile = fallbackProfile(pad({ axes: 6 }), { samples });
    expect(profile.mappings["axis4-"]).toBeUndefined();
    expect(profile.mappings["axis4+"]).toBeUndefined();
    expect(profile.mappings["axis5-"]).toBeUndefined();
  });

  it("does not read a single-frame trigger sample as a direction", () => {
    const profile = fallbackProfile(pad({ axes: [0, 0, 0, 0, -1, -1] }));
    expect(profile.mappings["axis4-"]).toBeUndefined();
    expect(profile.mappings["axis5-"]).toBeUndefined();
    // The left stick still navigates.
    expect(profile.mappings["axis0-"]).toBe("left");
  });

  it("uses the standard d-pad cluster when the pad has one", () => {
    const profile = fallbackProfile(pad({ buttons: buttons(16), axes: 4 }));
    expect(profile.mappings.button12).toBe("up");
    expect(profile.mappings.button13).toBe("down");
    expect(profile.mappings.button14).toBe("left");
    expect(profile.mappings.button15).toBe("right");
  });

  it("keeps the tuning of the profile it replaced", () => {
    const base = presetProfile("dualsense");
    const profile = fallbackProfile(pad(), { base });
    expect(profile.deadzone).toBe(base.deadzone);
    expect(profile.curve).toBe(base.curve);
    expect(profile.triggerThreshold).toBe(base.triggerThreshold);
  });

  it("survives a pad with a single button and no axes", () => {
    const profile = fallbackProfile(pad({ buttons: buttons(1), axes: 0 }));
    expect(profile.mappings.button0).toBe("confirm");
    expect(Object.keys(profile.mappings)).toEqual(["button0"]);
  });
});

describe("axis classification", () => {
  it("separates sticks, hats and triggers", () => {
    const samples = [
      [0, 0, 0, 0, 0, 0, -1],
      [0.5, 0, 0, 0, 1, -1, -1],
    ];
    expect(classifyAxes(samples)).toEqual({
      sticks: [0, 1, 2, 3],
      hats: [4, 5],
      triggers: [6],
    });
  });

  it("treats an empty sample set as no axes", () => {
    expect(classifyAxes(null)).toEqual({ sticks: [], hats: [], triggers: [] });
    expect(classifyAxes([])).toEqual({ sticks: [], hats: [], triggers: [] });
  });

  it("ignores an analog axis past the sticks rather than misreading it", () => {
    const samples = [
      [0, 0, 0, 0, 0.08],
      [0, 0, 0, 0, 0.36],
    ];
    expect(classifyAxes(samples).hats).toEqual([]);
  });
});
