import { describe, expect, it } from "vitest";
import {
  POINTER_ACTIVITY_EVENTS,
  POINTER_IDLE_ATTRIBUTE,
  POINTER_IDLE_MS,
  pointerIdleExpired,
} from "./pointer-idle";

describe("big picture pointer idle", () => {
  it("keeps the cursor while the pointer is still moving", () => {
    expect(pointerIdleExpired(1_000, 999)).toBe(false);
    expect(pointerIdleExpired(1_000, 1_000)).toBe(false);
    expect(pointerIdleExpired(1_000 + POINTER_IDLE_MS - 1, 1_000)).toBe(false);
  });

  it("hides it at the delay and keeps it hidden afterwards", () => {
    expect(pointerIdleExpired(1_000 + POINTER_IDLE_MS, 1_000)).toBe(true);
    expect(pointerIdleExpired(60_000, 1_000)).toBe(true);
  });

  it("never hides it when the clock jumps backwards", () => {
    expect(pointerIdleExpired(500, 1_000)).toBe(false);
  });

  it("honours an explicit delay", () => {
    expect(pointerIdleExpired(1_200, 1_000, 250)).toBe(false);
    expect(pointerIdleExpired(1_250, 1_000, 250)).toBe(true);
  });

  it("sits inside the 1.5–2 s window the issue asks for", () => {
    expect(POINTER_IDLE_MS).toBeGreaterThanOrEqual(1_500);
    expect(POINTER_IDLE_MS).toBeLessThanOrEqual(2_000);
  });

  it("wakes on mouse movement, presses and wheel scrolls", () => {
    expect([...POINTER_ACTIVITY_EVENTS]).toEqual(["pointermove", "pointerdown", "wheel"]);
    expect(POINTER_IDLE_ATTRIBUTE).toBe("data-pointer-idle");
  });
});
