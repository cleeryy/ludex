import { describe, expect, it } from "vitest";

import {
  formatBitsPerSecond,
  formatBytes,
  formatSpeed,
  kibToMbps,
  mbpsToKib,
} from "@/lib/format";

const MiB = 1024 * 1024;
const GiB = 1024 * MiB;
/** A decimal megabyte, the unit a link rate is quoted in. */
const MB = 1_000_000;

describe("formatBytes", () => {
  // #199 must not touch file sizes.
  it("keeps byte units for sizes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(36 * MiB)).toBe("36.0 MB");
    expect(formatBytes(GiB)).toBe("1.0 GB");
    expect(formatBytes(null)).toBe("—");
  });
});

describe("formatSpeed", () => {
  it("converts bytes per second to decimal bits per second", () => {
    expect(formatSpeed(0)).toBe("—");
    expect(formatSpeed(null)).toBe("—");
    expect(formatSpeed(undefined)).toBe("—");
    expect(formatSpeed(36 * MB)).toBe("288 Mb/s");
    expect(formatSpeed(MB)).toBe("8 Mb/s");
    expect(formatSpeed(1000 * MB)).toBe("8 Gb/s");
    expect(formatSpeed(62_500)).toBe("500 Kb/s");
    expect(formatSpeed(1)).toBe("8 b/s");
  });

  it("keeps one decimal below 100 units and none above", () => {
    expect(formatSpeed(1_562_500)).toBe("12.5 Mb/s");
    expect(formatSpeed(2_000_000)).toBe("16 Mb/s");
  });
});

describe("formatBitsPerSecond", () => {
  it("renders unknown and zero as an em dash", () => {
    expect(formatBitsPerSecond(0)).toBe("—");
    expect(formatBitsPerSecond(Number.NaN)).toBe("—");
    expect(formatBitsPerSecond(null)).toBe("—");
  });

  it("steps through the decimal unit table", () => {
    expect(formatBitsPerSecond(999)).toBe("999 b/s");
    expect(formatBitsPerSecond(1_000)).toBe("1 Kb/s");
    expect(formatBitsPerSecond(1_000_000)).toBe("1 Mb/s");
    expect(formatBitsPerSecond(1_000_000_000)).toBe("1 Gb/s");
    expect(formatBitsPerSecond(8_000_000_000)).toBe("8 Gb/s");
    // Clamps at the largest unit instead of inventing Tb/s.
    expect(formatBitsPerSecond(8_000_000_000_000)).toBe("8,000 Gb/s");
  });
});

describe("bandwidth cap conversion", () => {
  it("round-trips Mb/s through the stored KiB/s field", () => {
    expect(mbpsToKib(0)).toBe(0);
    expect(mbpsToKib(8)).toBe(977);
    expect(mbpsToKib(80)).toBe(9766);
    expect(kibToMbps(9766)).toBeCloseTo(80, 1);
    expect(kibToMbps(0)).toBe(0);
  });

  it("keeps a 1 Mb/s cap close to its displayed value", () => {
    const stored = mbpsToKib(1);
    expect(stored).toBe(122);
    expect(kibToMbps(stored)).toBeCloseTo(1, 2);
  });
});
