import { describe, expect, it } from "vitest";
import {
  actionForAxis,
  actionForButton,
  applyStick,
  CONTROLLER_PRESETS,
  correctAxis,
  parseControllerProfile,
  presetProfile,
  resolveControllerProfile,
  serialiseControllerProfile,
  validateControllerProfile,
} from "@/lib/controller-profiles";

describe("controller profile maths", () => {
  it("zeros the centre and rescales outside the deadzone", () => {
    const profile = { deadzone: 0.2, curve: 1, sensitivity: 1 };
    expect(correctAxis(0.1, profile)).toBeCloseTo(0);
    expect(correctAxis(-0.2, profile)).toBeCloseTo(0);
    expect(correctAxis(0.6, profile)).toBeCloseTo(0.5);
    expect(correctAxis(-0.6, profile)).toBeCloseTo(-0.5);
  });

  it("applies curve, sensitivity and inversion", () => {
    const linear = correctAxis(0.6, { deadzone: 0.1, curve: 1, sensitivity: 1 });
    expect(correctAxis(0.6, { deadzone: 0.1, curve: 1, sensitivity: 1.5 })).toBeGreaterThan(linear);
    expect(correctAxis(0.6, { deadzone: 0.1, curve: 0.5, sensitivity: 1 })).toBeGreaterThan(linear);
    expect(correctAxis(0.6, { deadzone: 0.1, curve: 2, sensitivity: 1 })).toBeLessThan(linear);
    expect(correctAxis(0.7, { deadzone: 0, curve: 1, sensitivity: 1 }, true)).toBeLessThan(0);
    expect(correctAxis(1, { deadzone: 0, curve: 1, sensitivity: 3 })).toBeLessThanOrEqual(1);
  });

  it("resolves per-game over account over preset", () => {
    const global = presetProfile("dualsense");
    const game = presetProfile("steamdeck");
    expect(resolveControllerProfile(global, game).name).toBe("Steam Deck");
    expect(resolveControllerProfile(global, null).name).toBe("DualSense");
    expect(resolveControllerProfile(null, null).name).toBe("Xbox");
  });

  it("maps buttons, triggers and axes", () => {
    const profile = { ...presetProfile("xbox"), mappings: { ...presetProfile("xbox").mappings, button0: "menu" as const, button6: "downloads" as const } };
    expect(actionForButton(profile, 0, 1)).toBe("menu");
    expect(actionForButton(profile, 0, 0.2)).toBeNull();
    expect(actionForButton(profile, 6, 0.3)).toBeNull();
    expect(actionForButton(profile, 6, 0.9)).toBe("downloads");
    expect(actionForAxis(profile, 0, 0.9, 0)).toBe("right");
    expect(actionForAxis(profile, 0, -0.9, 0)).toBe("left");
    expect(actionForAxis(profile, 1, 0, -0.9)).toBe("up");
    expect(actionForAxis(profile, 1, 0, 0.1)).toBeNull();
    expect(applyStick(profile, 0.9, 0, true)).toBeGreaterThan(0);
  });

  it("round-trips every preset and validates imports", () => {
    for (const id of CONTROLLER_PRESETS) {
      const profile = presetProfile(id);
      expect(validateControllerProfile(profile)).toBeNull();
      expect(parseControllerProfile(serialiseControllerProfile(profile))).toEqual(profile);
    }
    expect(validateControllerProfile({ ...presetProfile("xbox"), deadzone: 2 })).not.toBeNull();
    expect(() => parseControllerProfile('{"deadzone": -1}')).toThrow();
    expect(() => parseControllerProfile('{"mappings":{"button0":"teleport"}}')).toThrow();
  });
});
