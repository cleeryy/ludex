import { describe, expect, it } from "vitest";
import { isServerGameId, parseServerGameId } from "./game-route";

describe("game route ids (#154)", () => {
  it("accepts positive integers only", () => {
    expect(parseServerGameId("1")).toBe(1);
    expect(parseServerGameId("42")).toBe(42);
    expect(isServerGameId(7)).toBe(true);
  });

  it("rejects the negative ids used by unmatched launcher imports", () => {
    expect(parseServerGameId("-42")).toBeNull();
    expect(parseServerGameId("-1")).toBeNull();
    expect(isServerGameId(-42)).toBe(false);
  });

  it("rejects zero, fractions, text and missing params", () => {
    expect(parseServerGameId("0")).toBeNull();
    expect(parseServerGameId("1.5")).toBeNull();
    expect(parseServerGameId("abc")).toBeNull();
    expect(parseServerGameId("1e3")).toBeNull();
    expect(parseServerGameId("0x10")).toBeNull();
    expect(parseServerGameId("")).toBeNull();
    expect(parseServerGameId(undefined)).toBeNull();
    expect(parseServerGameId(null)).toBeNull();
    expect(isServerGameId(Number.NaN)).toBe(false);
  });
});
