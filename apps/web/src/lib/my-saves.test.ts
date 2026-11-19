import { describe, expect, it } from "vitest";
import { clampRetention, DEFAULT_RETENTION, MAX_RETENTION, pointsToPrune } from "./my-saves";

describe("retention window", () => {
  it("keeps the desktop default when the value is missing", () => {
    expect(clampRetention(null)).toBe(DEFAULT_RETENTION);
    expect(clampRetention(undefined)).toBe(DEFAULT_RETENTION);
    expect(clampRetention(Number.NaN)).toBe(DEFAULT_RETENTION);
    expect(DEFAULT_RETENTION).toBe(5);
  });

  it("clamps out-of-range windows into 1–50", () => {
    expect(clampRetention(0)).toBe(1);
    expect(clampRetention(-3)).toBe(1);
    expect(clampRetention(7)).toBe(7);
    expect(clampRetention(999)).toBe(MAX_RETENTION);
    expect(clampRetention(3.4)).toBe(3);
  });
});

describe("prune preview", () => {
  it("never counts the newest restore point as removable", () => {
    // Three points, keep one: two older points go, the newest stays.
    expect(pointsToPrune(3, 1)).toBe(2);
    expect(pointsToPrune(5, 5)).toBe(0);
    expect(pointsToPrune(5, 10)).toBe(0);
    expect(pointsToPrune(0, 1)).toBe(0);
  });

  it("treats a zero window as one instead of deleting everything", () => {
    expect(pointsToPrune(4, 0)).toBe(3);
  });
});
