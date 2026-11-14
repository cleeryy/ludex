import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  LAUNCH_CEILING_MS,
  LAUNCH_DIM_ALPHA,
  LAUNCH_FAIL_HOLD_MS,
  LAUNCH_FILAMENT_MS,
  LAUNCH_MIN_HOLD_MS,
  PLATE_COPY_KEYS,
  flipFrom,
  launchPlateCopy,
  plateCanDismiss,
} from "./launch-plate";
import { LAUNCH_FLIP_MS } from "./motion";

/* --------------------------------------------------------------------------
   #229, the launch handoff's decisions: the FLIP geometry, the 8 s ceiling, the
   dismissal rule, and the copy (which must come from keys that already ship —
   this pass adds no locale key).
   -------------------------------------------------------------------------- */

const catalog = JSON.parse(readFileSync(new URL("../../locales/en.json", import.meta.url), "utf8")) as Record<
  string,
  string
>;

describe("launch plate FLIP", () => {
  it("maps the selected cover rect onto a full-bleed surface", () => {
    const rect = { left: 240, top: 180, width: 180, height: 240 };
    const flip = flipFrom(rect, { width: 1_920, height: 1_080 })!;
    expect(flip.x).toBe(240);
    expect(flip.y).toBe(180);
    expect(flip.scaleX).toBeCloseTo(1_920 / 180, 6);
    expect(flip.scaleY).toBeCloseTo(1_080 / 240, 6);
    // The plate is anchored top-left, so the start frame is one scale away from
    // the card's own rect and the target is the identity transform.
    expect(flip.scaleX).toBeGreaterThan(1);
    expect(flip.scaleY).toBeGreaterThan(1);
  });

  it("returns null when there is nothing honest to fly from", () => {
    expect(flipFrom(null, { width: 1_920, height: 1_080 })).toBeNull();
    expect(flipFrom({ left: 0, top: 0, width: 0, height: 240 }, { width: 1_920, height: 1_080 })).toBeNull();
    expect(flipFrom({ left: 0, top: 0, width: 180, height: 240 }, { width: 0, height: 0 })).toBeNull();
  });
});

describe("launch plate lifecycle", () => {
  it("ends on the promise and gives up at 8 s", () => {
    expect(LAUNCH_CEILING_MS).toBe(8_000);
    expect(LAUNCH_MIN_HOLD_MS).toBeGreaterThan(LAUNCH_FLIP_MS.min);
    expect(LAUNCH_MIN_HOLD_MS).toBeLessThan(LAUNCH_CEILING_MS);
    expect(LAUNCH_FAIL_HOLD_MS).toBeLessThan(LAUNCH_CEILING_MS);
    expect(LAUNCH_FAIL_HOLD_MS).toBeGreaterThan(LAUNCH_MIN_HOLD_MS);
    expect(LAUNCH_FILAMENT_MS).toBe(1_100);
    // The library reads at 0.35 behind the plate.
    expect(LAUNCH_DIM_ALPHA).toBeCloseTo(0.65, 5);
    expect(1 - LAUNCH_DIM_ALPHA).toBeCloseTo(0.35, 5);
  });

  it("only lets B dismiss a launch that has given up", () => {
    // The native launch IPC has no abort: cancel mid-flight would pretend the
    // game is not starting when it is.
    expect(plateCanDismiss("launching")).toBe(false);
    expect(plateCanDismiss("slow")).toBe(true);
    expect(plateCanDismiss("failed")).toBe(true);
  });
});

describe("launch plate copy", () => {
  it("speaks the launch line, then the calm truth, then the failure", () => {
    expect(launchPlateCopy("launching", "Hollow Knight")).toEqual({
      eyebrow: "Launching Hollow Knight",
      line: "Starting…",
    });
    expect(launchPlateCopy("slow", "Hollow Knight").line).toBe("Waiting for the desktop to pick it up.");
    expect(launchPlateCopy("failed", "Hollow Knight").line).toBe("Could not launch the game");
  });

  it("uses message ids that already ship, so this pass adds no locale key", () => {
    for (const [status, id] of Object.entries(PLATE_COPY_KEYS)) {
      expect(catalog[id], `${status}: ${id}`).toBeDefined();
    }
  });
});
