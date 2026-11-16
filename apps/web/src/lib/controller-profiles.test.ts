import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import type { ControllerProfile } from "@/lib/types";
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

const sharedFixture = JSON.parse(
  readFileSync(
    new URL("../../../../crates/ludex-core/tests/fixtures/controller-profiles.json", import.meta.url),
    "utf8",
  ),
) as {
  profile: ControllerProfile;
  axisCases: { name: string; x: number; expected: number; tolerance?: number }[];
  buttonCases: { name: string; button: number; value: number; action: string | null }[];
  axisMappingCases: { name: string; axis: number; x: number; y: number; action: string | null }[];
};

describe("shared controller-profile fixture (#237)", () => {
  const profile = sharedFixture.profile;

  it("accepts the fixture profile", () => {
    expect(validateControllerProfile(profile)).toBeNull();
  });

  it("matches every axis case asserted by the Rust suite", () => {
    for (const testCase of sharedFixture.axisCases) {
      const actual = applyStick(profile, testCase.x, 0, true);
      expect(Math.abs(actual - testCase.expected), testCase.name).toBeLessThanOrEqual(
        testCase.tolerance ?? 0,
      );
    }
  });

  it("matches every button case asserted by the Rust suite", () => {
    for (const testCase of sharedFixture.buttonCases) {
      expect(actionForButton(profile, testCase.button, testCase.value), testCase.name).toBe(
        testCase.action,
      );
    }
  });

  it("matches every axis-mapping case asserted by the Rust suite", () => {
    for (const testCase of sharedFixture.axisMappingCases) {
      expect(
        actionForAxis(profile, testCase.axis, testCase.x, testCase.y),
        testCase.name,
      ).toBe(testCase.action);
    }
  });
});
