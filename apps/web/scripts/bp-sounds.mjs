#!/usr/bin/env node
/**
 * Big Picture cue synthesis (#198).
 *
 * Every cue in `public/sounds/bp/` is generated here from a handful of
 * parameters — a transient, one to three pitched partials and an envelope.
 * Nothing is sampled, looped or lifted from a console: the files are a pure
 * function of this table, which is why the script ships in the repository next
 * to the WAVs it writes.
 *
 *   node scripts/bp-sounds.mjs           # write the WAVs and print the manifest
 *   node scripts/bp-sounds.mjs --check   # verify the files match the table
 *
 * Format: mono, 48 kHz, 16-bit PCM. Length, peak and partials come from the
 * issue's cue table; the peak is normalised per file, so a WAV inspected on its
 * own is already at the loudness the design asks for. The whole set stays far
 * under the 512 KB budget (≈305 KB for fourteen cues).
 *
 * Sound language: "dry archive". A short transient plus one pitched body —
 * paper, wood and a struck glass. No square waves, no melody longer than four
 * notes, no more than three partials at once.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.resolve(here, "..", "public", "sounds", "bp");
const RATE = 48_000;
const TWO_PI = Math.PI * 2;
const checkOnly = process.argv.includes("--check");

/* -------------------------------- synthesis -------------------------------- */

const frames = (ms) => Math.max(1, Math.round((ms / 1000) * RATE));

/** Deterministic noise: the same script always writes the same bytes. */
function mulberry32(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return (((t ^ (t >>> 14)) >>> 0) / 4_294_967_296) * 2 - 1;
  };
}

/** 1.5 ms cosine attack so a transient never starts on a click. */
function attackGain(index) {
  const length = Math.max(1, Math.round(0.0015 * RATE));
  if (index >= length) return 1;
  return 0.5 - 0.5 * Math.cos((Math.PI * index) / length);
}

/**
 * Adds one pitched voice. `decayMs` is the time to roughly -26 dB (three
 * time constants), which is why a 20 ms decay is inaudible before the buffer
 * ends. `glideTo` sweeps the frequency exponentially, so a fall sounds like a
 * fall and not like a slide whistle.
 */
function tone(buffer, { freq, startMs = 0, durMs, amp = 1, decayMs = durMs, glideTo = null }) {
  const start = Math.round((startMs / 1000) * RATE);
  const length = Math.min(frames(durMs), buffer.length - start);
  if (length <= 0) return;
  const tau = Math.max(0.004, decayMs / 3000);
  let phase = 0;
  for (let i = 0; i < length; i += 1) {
    const progress = length === 1 ? 0 : i / (length - 1);
    const frequency = glideTo === null ? freq : freq * Math.pow(glideTo / freq, progress);
    phase += (TWO_PI * frequency) / RATE;
    buffer[start + i] += Math.sin(phase) * amp * attackGain(i) * Math.exp(-(i / RATE) / tau);
  }
}

/** Adds a noise tick: the paper/wood transient at the front of a cue. */
function tick(buffer, { startMs = 0, durMs = 2, amp = 1, seed = 1, decayMs = durMs }) {
  const start = Math.round((startMs / 1000) * RATE);
  const length = Math.min(frames(durMs), buffer.length - start);
  const random = mulberry32(seed);
  const tau = Math.max(0.0005, decayMs / 3000);
  for (let i = 0; i < length; i += 1) {
    buffer[start + i] += random() * amp * attackGain(i) * Math.exp(-(i / RATE) / tau);
  }
}

/**
 * Adds band-passed noise ("air"). A one-pole state-variable filter is enough
 * for a 100 ms cue and keeps the script dependency-free; `sweepTo` moves the
 * centre frequency across the cue.
 */
function air(buffer, { startMs = 0, durMs, amp = 1, decayMs = durMs, freq, q = 1.2, sweepTo = null, seed = 7 }) {
  const start = Math.round((startMs / 1000) * RATE);
  const length = Math.min(frames(durMs), buffer.length - start);
  if (length <= 0) return;
  const random = mulberry32(seed);
  const tau = Math.max(0.004, decayMs / 3000);
  let low = 0;
  let band = 0;
  for (let i = 0; i < length; i += 1) {
    const progress = length === 1 ? 0 : i / (length - 1);
    const centre = sweepTo === null ? freq : freq * Math.pow(sweepTo / freq, progress);
    const f = 2 * Math.sin((Math.PI * Math.min(centre, RATE / 6)) / RATE);
    const input = random();
    low += f * band;
    const high = input - low - q * band;
    band += f * high;
    buffer[start + i] += band * amp * attackGain(i) * Math.exp(-(i / RATE) / tau);
  }
}

function normalize(buffer, peakDb) {
  let peak = 0;
  for (const sample of buffer) peak = Math.max(peak, Math.abs(sample));
  if (peak === 0) return;
  const gain = Math.pow(10, peakDb / 20) / peak;
  for (let i = 0; i < buffer.length; i += 1) buffer[i] *= gain;
}

/** A hard cut at the buffer end would click; 3 ms of fade never does. */
function release(buffer) {
  const length = Math.min(buffer.length, frames(3));
  for (let i = 0; i < length; i += 1) {
    buffer[buffer.length - 1 - i] *= i / length;
  }
}

function render(cue) {
  const buffer = new Float32Array(frames(cue.ms));
  cue.build(buffer);
  normalize(buffer, cue.peakDb);
  release(buffer);
  return buffer;
}

function measure(buffer) {
  let peak = 0;
  let sum = 0;
  for (const sample of buffer) {
    peak = Math.max(peak, Math.abs(sample));
    sum += sample * sample;
  }
  const rms = Math.sqrt(sum / buffer.length);
  return {
    peakDb: 20 * Math.log10(Math.max(peak, 1e-9)),
    rmsDb: 20 * Math.log10(Math.max(rms, 1e-9)),
  };
}

/* -------------------------------- the table -------------------------------- */

/**
 * The cue table. `partials` documents the pitched content for the review and is
 * asserted by the asset test (three at most). `peakDb` is the normalisation
 * target: -20 dBFS for movement, -12 for confirmation, never above -10.
 */
export const CUES = [
  {
    id: "move",
    ms: 45,
    peakDb: -20,
    partials: [1500],
    build: (buffer) => {
      tick(buffer, { durMs: 2, amp: 0.4, seed: 11 });
      tone(buffer, { freq: 1500, startMs: 2, durMs: 43, amp: 1, decayMs: 20 });
    },
  },
  {
    id: "confirm",
    ms: 90,
    peakDb: -12,
    partials: [660, 990],
    build: (buffer) => {
      tone(buffer, { freq: 660, durMs: 90, amp: 0.7, decayMs: 70 });
      tone(buffer, { freq: 990, durMs: 90, amp: 0.45, decayMs: 70 });
    },
  },
  {
    id: "back",
    ms: 100,
    peakDb: -14,
    partials: [880, 587],
    build: (buffer) => {
      tone(buffer, { freq: 880, glideTo: 587, durMs: 100, amp: 0.9, decayMs: 80 });
    },
  },
  {
    id: "openGroup",
    ms: 140,
    peakDb: -16,
    partials: [196, 700],
    build: (buffer) => {
      tone(buffer, { freq: 196, durMs: 60, amp: 0.9, decayMs: 30 });
      air(buffer, { freq: 700, q: 1.1, durMs: 130, amp: 0.5, decayMs: 60, seed: 23 });
    },
  },
  {
    id: "closeGroup",
    ms: 120,
    peakDb: -18,
    partials: [165],
    build: (buffer) => {
      tone(buffer, { freq: 165, durMs: 55, amp: 0.8, decayMs: 26 });
      tick(buffer, { durMs: 3, amp: 0.12, seed: 31, decayMs: 3 });
    },
  },
  {
    id: "tabChange",
    ms: 70,
    peakDb: -20,
    partials: [900],
    build: (buffer) => {
      air(buffer, { freq: 900, q: 1.6, durMs: 70, amp: 0.8, decayMs: 40, seed: 41 });
    },
  },
  {
    id: "error",
    ms: 220,
    peakDb: -12,
    partials: [220, 233],
    build: (buffer) => {
      tone(buffer, { freq: 220, durMs: 200, amp: 0.8, decayMs: 55 });
      tone(buffer, { freq: 233, durMs: 200, amp: 0.7, decayMs: 50 });
      tick(buffer, { durMs: 4, amp: 0.15, seed: 53, decayMs: 4 });
    },
  },
  {
    id: "launch",
    ms: 350,
    peakDb: -10,
    partials: [330, 440, 660],
    build: (buffer) => {
      tone(buffer, { freq: 330, durMs: 130, amp: 0.7, decayMs: 90 });
      tone(buffer, { freq: 440, startMs: 90, durMs: 140, amp: 0.7, decayMs: 100 });
      tone(buffer, { freq: 660, startMs: 180, durMs: 170, amp: 0.8, decayMs: 140 });
    },
  },
  {
    id: "achievement",
    ms: 700,
    peakDb: -14,
    partials: [523, 784, 1046],
    build: (buffer) => {
      tone(buffer, { freq: 523, durMs: 700, amp: 0.6, decayMs: 260 });
      tone(buffer, { freq: 784, startMs: 60, durMs: 640, amp: 0.5, decayMs: 300 });
      tone(buffer, { freq: 1046, startMs: 120, durMs: 580, amp: 0.42, decayMs: 320 });
    },
  },
  {
    id: "installComplete",
    ms: 400,
    peakDb: -14,
    partials: [660, 880],
    build: (buffer) => {
      tone(buffer, { freq: 660, durMs: 400, amp: 0.6, decayMs: 180 });
      tone(buffer, { freq: 880, startMs: 40, durMs: 360, amp: 0.45, decayMs: 170 });
    },
  },
  {
    id: "power",
    ms: 500,
    peakDb: -12,
    partials: [440, 220],
    build: (buffer) => {
      tone(buffer, { freq: 440, glideTo: 220, durMs: 380, amp: 0.8, decayMs: 300 });
      air(buffer, { freq: 300, q: 0.9, durMs: 460, amp: 0.25, decayMs: 250, seed: 67 });
    },
  },
  {
    id: "overlayOpen",
    ms: 120,
    peakDb: -20,
    partials: [400, 900],
    build: (buffer) => {
      air(buffer, { freq: 400, sweepTo: 900, q: 1.4, durMs: 120, amp: 0.8, decayMs: 80, seed: 71 });
    },
  },
  {
    id: "overlayClose",
    ms: 100,
    peakDb: -22,
    partials: [900, 400],
    build: (buffer) => {
      air(buffer, { freq: 900, sweepTo: 400, q: 1.4, durMs: 100, amp: 0.8, decayMs: 70, seed: 79 });
    },
  },
  {
    id: "notification",
    ms: 300,
    peakDb: -16,
    partials: [880],
    build: (buffer) => {
      tone(buffer, { freq: 880, durMs: 300, amp: 0.8, decayMs: 180 });
      air(buffer, { freq: 1320, q: 1.3, durMs: 240, amp: 0.2, decayMs: 150, seed: 83 });
    },
  },
];

/* --------------------------------- output ---------------------------------- */

function wav(samples) {
  const data = Buffer.alloc(44 + samples.length * 2);
  data.write("RIFF", 0);
  data.writeUInt32LE(36 + samples.length * 2, 4);
  data.write("WAVE", 8);
  data.write("fmt ", 12);
  data.writeUInt32LE(16, 16);
  data.writeUInt16LE(1, 20); // PCM
  data.writeUInt16LE(1, 22); // mono
  data.writeUInt32LE(RATE, 24);
  data.writeUInt32LE(RATE * 2, 28); // byte rate
  data.writeUInt16LE(2, 32); // block align
  data.writeUInt16LE(16, 34);
  data.write("data", 36);
  data.writeUInt32LE(samples.length * 2, 40);
  for (let i = 0; i < samples.length; i += 1) {
    const value = Math.max(-1, Math.min(1, samples[i]));
    data.writeInt16LE(Math.round(value * 32_767), 44 + i * 2);
  }
  return data;
}

const LICENSE = `CC0 1.0 Universal (Public Domain Dedication)

The WAV files in this folder were generated by scripts/bp-sounds.mjs for Ludex.
They contain no third-party samples, no recordings and no motif copied from a
console, a game or a sample pack: every cue is synthesised from the parameters
in that script, which is committed alongside them. Regenerate with:

  node scripts/bp-sounds.mjs

To the extent possible under law, the Ludex project waives all copyright and
related rights to these generated files. See
https://creativecommons.org/publicdomain/zero/1.0/ for the full text.
`;

function main() {
  if (!checkOnly) fs.mkdirSync(outDir, { recursive: true });
  const rows = [];
  let total = 0;
  let failures = 0;
  for (const cue of CUES) {
    const samples = render(cue);
    const bytes = wav(samples);
    const file = `bp-${cue.id}.wav`;
    const target = path.join(outDir, file);
    const { peakDb, rmsDb } = measure(samples);
    if (checkOnly) {
      if (!fs.existsSync(target)) {
        failures += 1;
        rows.push(`${cue.id.padEnd(16)} MISSING ${file}`);
        continue;
      }
      const existing = fs.readFileSync(target);
      if (!existing.equals(bytes)) {
        failures += 1;
        rows.push(`${cue.id.padEnd(16)} STALE   ${file}`);
        continue;
      }
    } else {
      fs.writeFileSync(target, bytes);
    }
    total += bytes.length;
    rows.push(
      `${cue.id.padEnd(16)} ${String(cue.ms).padStart(4)} ms  ${cue.partials
        .map((frequency) => `${frequency} Hz`)
        .join(" + ")}`.padEnd(62) + `peak ${peakDb.toFixed(1)}  rms ${rmsDb.toFixed(1)}  ${(bytes.length / 1024).toFixed(1)} KB`,
    );
  }
  if (!checkOnly) {
    fs.writeFileSync(path.join(outDir, "LICENSE"), LICENSE);
  }
  console.log(rows.join("\n"));
  console.log(`\n${CUES.length} cues, ${(total / 1024).toFixed(1)} KB total (budget 512 KB)`);
  if (checkOnly && failures > 0) {
    console.error(`bp-sounds: ${failures} file(s) out of date — run node scripts/bp-sounds.mjs`);
    process.exit(1);
  }
}

main();
