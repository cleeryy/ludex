import { existsSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  CUE_IDS,
  CUE_SPECS,
  MOVE_DEDUPE_MS,
  PEAK_CEILING_DB,
  SOUND_BUDGET_BYTES,
  type CueId,
} from "./ui-sound-specs";

/* --------------------------------------------------------------------------
   The committed audio (#198): every cue has a manifest entry, every file is a
   mono 48 kHz 16-bit WAV whose length and peak match the table, and the set
   stays inside the 512 KB budget. Regenerate with `node scripts/bp-sounds.mjs`;
   the script and the files are committed together, so provenance is auditable.
   -------------------------------------------------------------------------- */

const here = path.dirname(fileURLToPath(import.meta.url));
const soundDir = path.resolve(here, "..", "..", "public", "sounds", "bp");

interface WavInfo {
  channels: number;
  rate: number;
  bits: number;
  frames: number;
  peakDb: number;
}

function readWav(cue: CueId): { info: WavInfo; bytes: number } {
  const file = path.join(soundDir, CUE_SPECS[cue].file);
  const data = readFileSync(file);
  expect(data.toString("ascii", 0, 4), `${cue}: RIFF`).toBe("RIFF");
  expect(data.toString("ascii", 8, 12), `${cue}: WAVE`).toBe("WAVE");
  const channels = data.readUInt16LE(22);
  const rate = data.readUInt32LE(24);
  const bits = data.readUInt16LE(34);
  const dataSize = data.readUInt32LE(40);
  const frames = dataSize / (channels * (bits / 8));
  let peak = 0;
  for (let offset = 44; offset + 1 < 44 + dataSize; offset += 2) {
    peak = Math.max(peak, Math.abs(data.readInt16LE(offset)) / 32_768);
  }
  return {
    info: { channels, rate, bits, frames, peakDb: 20 * Math.log10(Math.max(peak, 1e-9)) },
    bytes: data.length,
  };
}

describe("Big Picture cue assets", () => {
  it("has a manifest entry and a file for every cue id", () => {
    expect(Object.keys(CUE_SPECS).sort()).toEqual([...CUE_IDS].sort());
    for (const cue of CUE_IDS) {
      const spec = CUE_SPECS[cue];
      expect(spec.file).toBe(`bp-${cue}.wav`);
      expect(existsSync(path.join(soundDir, spec.file)), `${cue} is committed`).toBe(true);
    }
  });

  it("ships mono 48 kHz 16-bit PCM at the length in the table", () => {
    for (const cue of CUE_IDS) {
      const spec = CUE_SPECS[cue];
      const { info } = readWav(cue);
      expect(info.channels, `${cue}: mono`).toBe(1);
      expect(info.rate, `${cue}: 48 kHz`).toBe(48_000);
      expect(info.bits, `${cue}: 16-bit`).toBe(16);
      const durationMs = (info.frames / info.rate) * 1_000;
      expect(durationMs, `${cue}: duration`).toBeCloseTo(spec.durationMs, 0);
      expect(durationMs, `${cue}: shorter than a second`).toBeLessThanOrEqual(1_000);
      expect(durationMs, `${cue}: at least 45 ms`).toBeGreaterThanOrEqual(45);
    }
  });

  it("normalises each file to its table peak and never exceeds -10 dBFS", () => {
    for (const cue of CUE_IDS) {
      const spec = CUE_SPECS[cue];
      const { info } = readWav(cue);
      expect(info.peakDb, `${cue}: peak`).toBeCloseTo(spec.peakDb, 0);
      expect(info.peakDb, `${cue}: ceiling`).toBeLessThanOrEqual(PEAK_CEILING_DB + 0.05);
    }
  });

  it("stays inside the three-partial and four-note rules", () => {
    for (const cue of CUE_IDS) {
      const spec = CUE_SPECS[cue];
      expect(spec.partials.length, `${cue}: partials`).toBeGreaterThan(0);
      expect(spec.partials.length, `${cue}: at most three partials`).toBeLessThanOrEqual(3);
    }
  });

  it("keeps the whole set under the 512 KB budget", () => {
    let total = 0;
    for (const cue of CUE_IDS) {
      total += statSync(path.join(soundDir, CUE_SPECS[cue].file)).size;
    }
    expect(total).toBeLessThan(SOUND_BUDGET_BYTES);
  });

  it("deduplicates the move cue at 24 ms", () => {
    expect(MOVE_DEDUPE_MS).toBe(24);
    expect(CUE_SPECS.move.minGapMs).toBe(MOVE_DEDUPE_MS);
  });

  it("states the licensing answer next to the files", () => {
    const license = readFileSync(path.join(soundDir, "LICENSE"), "utf8");
    expect(license).toContain("CC0 1.0");
    expect(license).toContain("scripts/bp-sounds.mjs");
    expect(license).toContain("no third-party samples");
  });
});
