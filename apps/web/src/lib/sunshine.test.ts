import { describe, expect, it } from "vitest";
import { needsSunshineElevation } from "./sunshine";

/**
 * #182: the "Grant write access" button must depend on the structured flag, not
 * on the error text, and must stay hidden for every other rejection shape.
 */
describe("needsSunshineElevation", () => {
  it("reads the structured Sunshine rejection", () => {
    expect(
      needsSunshineElevation({
        code: "sunshine_needs_elevation",
        message: "Access is denied",
        needsElevation: true,
      }),
    ).toBe(true);
    expect(
      needsSunshineElevation({
        code: "sunshine_write_failed",
        message: "Disk full",
        needsElevation: false,
      }),
    ).toBe(false);
  });

  it("ignores plain Tauri strings and unrelated objects", () => {
    expect(needsSunshineElevation("Access is denied (os error 5)")).toBe(false);
    expect(needsSunshineElevation(new Error("Access is denied"))).toBe(false);
    expect(needsSunshineElevation({ message: "Access is denied" })).toBe(false);
    expect(needsSunshineElevation({ needsElevation: "true" })).toBe(false);
    expect(needsSunshineElevation(null)).toBe(false);
    expect(needsSunshineElevation(undefined)).toBe(false);
  });
});
