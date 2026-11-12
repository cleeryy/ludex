import { describe, expect, it } from "vitest";
import {
  formatMs,
  formatRate,
  formatShare,
  isObservabilityWindow,
  OBSERVABILITY_WINDOWS,
  seriesValues,
  sparkline,
  windowLabel,
} from "@/lib/observability";
import type { ObservabilityPoint } from "@/lib/types";

function point(overrides: Partial<ObservabilityPoint> = {}): ObservabilityPoint {
  return {
    at: "2026-11-12T10:00:00Z",
    requests: 10,
    success: 9,
    redirects: 0,
    clientErrors: 1,
    serverErrors: 0,
    averageMs: 20,
    ...overrides,
  };
}

describe("observability helpers (#170)", () => {
  it("accepts only the supported windows", () => {
    expect(OBSERVABILITY_WINDOWS).toEqual(["1h", "24h"]);
    expect(isObservabilityWindow("1h")).toBe(true);
    expect(isObservabilityWindow("24h")).toBe(true);
    for (const invalid of ["7d", "1", "", "24H"]) {
      expect(isObservabilityWindow(invalid), invalid).toBe(false);
    }
    expect(windowLabel("1h")).toBe("1 h");
    expect(windowLabel("24h")).toBe("24 h");
  });

  it("returns an empty geometry for an empty series", () => {
    expect(sparkline([])).toEqual({ line: "", area: "", max: 0, last: 0 });
  });

  it("draws a flat line for a single point", () => {
    const geometry = sparkline([12], 240, 56, 4);
    expect(geometry.max).toBe(12);
    expect(geometry.last).toBe(12);
    expect(geometry.line).toBe("0,4 240,4");
    expect(geometry.area).toBe("M0,56 L0,4 L240,4 L240,56 Z");
  });

  it("keeps coordinates bounded and the last point visible", () => {
    const geometry = sparkline([0, 5, 10, 2], 240, 56, 4);
    expect(geometry.max).toBe(10);
    expect(geometry.last).toBe(2);
    const coords = geometry.line.split(" ").map((pair) => pair.split(",").map(Number));
    expect(coords).toHaveLength(4);
    for (const [x, y] of coords) {
      expect(x).toBeGreaterThanOrEqual(0);
      expect(x).toBeLessThanOrEqual(240);
      expect(y).toBeGreaterThanOrEqual(4);
      expect(y).toBeLessThanOrEqual(56);
    }
    expect(coords[0][1]).toBe(52);
    expect(coords[2][1]).toBe(4);
  });

  it("does not divide by zero on a flat series", () => {
    const geometry = sparkline([0, 0, 0], 100, 50, 5);
    expect(geometry.max).toBe(0);
    expect(geometry.line).toBe("0,45 50,45 100,45");
  });

  it("computes series values for requests, errors and latency", () => {
    const points = [
      point({ requests: 10, clientErrors: 1, serverErrors: 2, averageMs: 20 }),
      point({ requests: 4, clientErrors: 0, serverErrors: 1, averageMs: 45 }),
    ];
    expect(seriesValues(points)).toEqual([10, 4]);
    expect(seriesValues(points, "errors")).toEqual([3, 1]);
    expect(seriesValues(points, "latency")).toEqual([20, 45]);
    expect(seriesValues([], "errors")).toEqual([]);
  });

  it("formats shares, latencies and rates without noise", () => {
    expect(formatShare(0)).toBe("0%");
    expect(formatShare(0.0004)).toBe("0.0%");
    expect(formatShare(0.0234)).toBe("2.3%");
    expect(formatShare(0.42)).toBe("42%");
    expect(formatMs(1.234)).toBe("1.2 ms");
    expect(formatMs(18.6)).toBe("19 ms");
    expect(formatRate(0)).toBe("0/s");
    expect(formatRate(0.25)).toBe("0.25/s");
    expect(formatRate(12.34)).toBe("12.3/s");
  });
});
