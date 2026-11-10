import { describe, expect, it } from "vitest";
import { formatDuration, formatEta } from "@/lib/format";
import {
  MAX_SAMPLES,
  SMOOTHING_WINDOW_MS,
  aggregateTransfer,
  capSamples,
  etaSeconds,
  isMaterialEtaChange,
  median,
  nextEta,
  pushSample,
  sampleJob,
  samplesInWindow,
  smoothedSpeed,
  type AggregateJobInput,
  type TransferSample,
} from "@/lib/eta";
import type { InstallPhase } from "@/lib/types";

const sample = (at: number, speedBps: number, bytesDone = 0): TransferSample => ({
  at,
  speedBps,
  bytesDone,
  phase: "downloading",
});

const job = (phase: InstallPhase, bytesDone: number, bytesTotal: number | null, speedBps: number) => ({
  jobId: "job",
  phase,
  bytesDone,
  bytesTotal,
  speedBps,
});

const aggregateJob = (
  phase: InstallPhase,
  bytesDone: number,
  bytesTotal: number | null,
  speedBps: number,
  estimating = false,
): AggregateJobInput => ({ phase, bytesDone, bytesTotal, speedBps, estimating });

describe("median", () => {
  it("returns the middle value for odd input and the mean of the middle pair for even input", () => {
    expect(median([3, 1, 2])).toBe(2);
    expect(median([4, 1, 3, 2])).toBe(2.5);
    expect(median([])).toBe(0);
  });

  it("does not sort the input array in place", () => {
    const values = [3, 1, 2];
    median(values);
    expect(values).toEqual([3, 1, 2]);
  });
});

describe("sampling ring buffer", () => {
  it("keeps only the newest samples", () => {
    expect(capSamples([1, 2, 3], 2)).toEqual([2, 3]);
  });

  it("caps at MAX_SAMPLES (two minutes at 1 Hz)", () => {
    let samples: TransferSample[] = [];
    for (let index = 0; index < MAX_SAMPLES + 10; index += 1) {
      samples = pushSample(samples, sample(index * 1_000, 10));
    }
    expect(samples).toHaveLength(MAX_SAMPLES);
    expect(samples[0].at).toBe(10_000);
  });

  it("replaces the last sample on a same-millisecond tick and ignores stale ticks", () => {
    const buffered = pushSample([sample(1_000, 10)], sample(1_000, 20));
    expect(buffered).toHaveLength(1);
    expect(buffered[0].speedBps).toBe(20);
    expect(pushSample(buffered, sample(500, 99))).toBe(buffered);
  });

  it("only uses samples inside the smoothing window", () => {
    const samples = [sample(0, 10), sample(6_000, 20), sample(12_000, 30)];
    expect(samplesInWindow(samples, 12_000, SMOOTHING_WINDOW_MS).map((entry) => entry.at)).toEqual([6_000, 12_000]);
  });

  it("ignores an instantaneous spike thanks to the rolling median", () => {
    const samples = [sample(0, 50), sample(1_000, 50), sample(2_000, 50), sample(3_000, 4_000), sample(4_000, 50)];
    expect(smoothedSpeed(samples, 4_000)).toBe(50);
  });
});

describe("eta helpers", () => {
  it("divides remaining bytes by the smoothed speed", () => {
    expect(etaSeconds(1_000, 100)).toBe(10);
  });

  it("has no ETA when nothing remains or the rate is zero", () => {
    expect(etaSeconds(0, 100)).toBeNull();
    expect(etaSeconds(1_000, 0)).toBeNull();
    expect(etaSeconds(-5, 100)).toBeNull();
    expect(etaSeconds(Number.NaN, 100)).toBeNull();
  });

  it("treats only >2 s or >10% moves as material", () => {
    expect(isMaterialEtaChange(100, 101)).toBe(false);
    expect(isMaterialEtaChange(100, 103)).toBe(true);
    expect(isMaterialEtaChange(100, 112)).toBe(true);
  });

  it("publishes the first estimate immediately", () => {
    expect(nextEta(null, 42, 1_000)).toEqual({ seconds: 42, at: 1_000 });
  });

  it("never updates more than once per second", () => {
    const published = { seconds: 60, at: 1_000 };
    expect(nextEta(published, 10, 1_400)).toBe(published);
    expect(nextEta(published, 10, 2_000)).toEqual({ seconds: 10, at: 2_000 });
  });

  it("keeps the displayed estimate when the change is not material", () => {
    const published = { seconds: 600, at: 1_000 };
    expect(nextEta(published, 601, 3_000)).toBe(published);
    expect(nextEta(published, 700, 3_000)).toEqual({ seconds: 700, at: 3_000 });
  });

  it("clears the estimate when the candidate disappears", () => {
    expect(nextEta({ seconds: 60, at: 1_000 }, null, 2_000)).toBeNull();
    expect(nextEta({ seconds: 60, at: 1_000 }, 0, 2_000)).toBeNull();
  });
});

describe("sampleJob", () => {
  it("reports estimating on the first samples and settles after three", () => {
    const first = sampleJob(null, job("downloading", 0, 1_000, 10), 0);
    expect(first.estimating).toBe(true);
    expect(first.etaSeconds).toBeNull();
    expect(first.elapsedMs).toBe(0);

    const second = sampleJob(first, job("downloading", 20, 1_000, 30), 1_000);
    expect(second.samples).toHaveLength(2);
    expect(second.estimating).toBe(true);

    // Third sample: median(10, 30, 20) = 20, remaining 960 -> 48 s.
    const third = sampleJob(second, job("downloading", 40, 1_000, 20), 2_000);
    expect(third.estimating).toBe(false);
    expect(third.speedBps).toBe(20);
    expect(third.etaSeconds).toBe(48);
  });

  it("holds the displayed ETA until the change is material", () => {
    const first = sampleJob(null, job("downloading", 0, 1_000, 20), 0);
    const second = sampleJob(first, job("downloading", 20, 1_000, 20), 1_000);
    const third = sampleJob(second, job("downloading", 40, 1_000, 20), 2_000);
    expect(third.etaSeconds).toBe(48);
    // 47 s vs 48 s is not material.
    expect(sampleJob(third, job("downloading", 60, 1_000, 20), 3_000).etaSeconds).toBe(48);
    // 44 s is.
    const moved = sampleJob(
      sampleJob(third, job("downloading", 60, 1_000, 20), 3_000),
      job("downloading", 120, 1_000, 20),
      4_000,
    );
    expect(moved.etaSeconds).toBe(44);
  });

  it("restarts the window and drops the ETA on a phase change", () => {
    const downloading = sampleJob(null, job("downloading", 900, 1_000, 20), 0);
    sampleJob(downloading, job("downloading", 920, 1_000, 20), 1_000);
    const ready = sampleJob(sampleJob(downloading, job("downloading", 920, 1_000, 20), 1_000), job("downloading", 940, 1_000, 20), 2_000);
    expect(ready.etaSeconds).not.toBeNull();

    const extracting = sampleJob(ready, job("extracting", 1_000, 1_000, 0), 3_000);
    expect(extracting.phase).toBe("extracting");
    expect(extracting.samples).toHaveLength(1);
    expect(extracting.speedBps).toBe(0);
    expect(extracting.etaSeconds).toBeNull();
    expect(extracting.estimating).toBe(false);
    expect(extracting.phaseStartedAt).toBe(3_000);
    expect(extracting.elapsedMs).toBe(0);
  });

  it("resets on pause and again on resume", () => {
    const running = sampleJob(null, job("downloading", 100, 1_000, 50), 0);
    sampleJob(running, job("downloading", 150, 1_000, 50), 1_000);
    const settled = sampleJob(sampleJob(running, job("downloading", 150, 1_000, 50), 1_000), job("downloading", 200, 1_000, 50), 2_000);
    expect(settled.etaSeconds).not.toBeNull();

    const paused = sampleJob(settled, job("paused", 250, 1_000, 0), 3_000);
    expect(paused.phase).toBe("paused");
    expect(paused.samples).toHaveLength(1);
    expect(paused.etaSeconds).toBeNull();

    const resumed = sampleJob(paused, job("downloading", 250, 1_000, 80), 4_000);
    expect(resumed.phase).toBe("downloading");
    expect(resumed.samples).toHaveLength(1);
    expect(resumed.estimating).toBe(true);
    expect(resumed.etaSeconds).toBeNull();
    expect(resumed.speedBps).toBe(80);
    expect(resumed.elapsedMs).toBe(0);
  });

  it("estimates when the smoothed speed stalls at zero", () => {
    let meter = sampleJob(null, job("downloading", 100, 1_000, 0), 0);
    meter = sampleJob(meter, job("downloading", 100, 1_000, 0), 1_000);
    meter = sampleJob(meter, job("downloading", 100, 1_000, 0), 2_000);
    expect(meter.estimating).toBe(true);
    expect(meter.etaSeconds).toBeNull();
  });
});

describe("aggregateTransfer", () => {
  it("refuses a partial-total aggregate ETA", () => {
    const sample = aggregateTransfer([
      aggregateJob("downloading", 100, 1_000, 100),
      aggregateJob("downloading", 100, null, 50),
    ]);
    expect(sample.totalKnown).toBe(false);
    expect(sample.etaKnown).toBe(false);
    expect(sample.bytesTotal).toBeNull();
    expect(sample.percent).toBeNull();
    expect(sample.etaCandidate).toBeNull();
    // Speed still sums: it is a rate, not an estimate.
    expect(sample.speedBps).toBe(150);
  });

  it("divides summed remaining bytes by summed speed instead of summing per-job ETAs", () => {
    const sample = aggregateTransfer([
      aggregateJob("downloading", 100, 1_000, 100), // 900 remaining -> 9 s
      aggregateJob("downloading", 500, 1_500, 50), // 1000 remaining -> 20 s
    ]);
    expect(sample.etaKnown).toBe(true);
    expect(sample.etaCandidate).toBeCloseTo(1_900 / 150, 6); // ~12.7 s, not 29 s
    expect(sample.percent).toBeCloseTo(24, 6);
  });

  it("keeps paused jobs in progress but out of speed and ETA", () => {
    const sample = aggregateTransfer([
      aggregateJob("downloading", 100, 1_000, 100),
      aggregateJob("paused", 400, 1_000, 0),
    ]);
    expect(sample.downloading).toBe(1);
    expect(sample.speedBps).toBe(100);
    expect(sample.etaCandidate).toBe(900 / 100);
    expect(sample.bytesDone).toBe(500);
  });

  it("produces no ETA when every job is paused", () => {
    const sample = aggregateTransfer([aggregateJob("paused", 400, 1_000, 0)]);
    expect(sample.downloading).toBe(0);
    expect(sample.etaCandidate).toBeNull();
    expect(sample.estimating).toBe(false);
    expect(sample.percent).toBe(40);
  });

  it("flags estimating while a download has no usable rate", () => {
    expect(aggregateTransfer([aggregateJob("downloading", 0, 1_000, 0)]).estimating).toBe(true);
    expect(aggregateTransfer([aggregateJob("downloading", 0, 1_000, 50, true)]).estimating).toBe(true);
    expect(aggregateTransfer([aggregateJob("downloading", 0, 1_000, 50, false)]).estimating).toBe(false);
  });
});

describe("formatEta / formatDuration", () => {
  it("formats seconds, minutes and hours", () => {
    expect(formatEta(0, 1_000, 100)).toBe("10s");
    expect(formatEta(0, 18_500, 100)).toBe("3m 05s");
    expect(formatEta(0, 7_800, 1)).toBe("2h 10m");
  });

  it("returns null when there is nothing to estimate", () => {
    expect(formatEta(500, 1_000, 0)).toBeNull();
    expect(formatEta(1_000, 1_000, 100)).toBeNull();
    expect(formatEta(0, null, 100)).toBeNull();
    expect(formatEta(0, Number.NaN, 100)).toBeNull();
  });

  it("keeps durations pure and rounded once", () => {
    expect(formatDuration(0)).toBe("0s");
    expect(formatDuration(-3)).toBe("0s");
    expect(formatDuration(59.6)).toBe("1m 00s");
    expect(formatDuration(3_770)).toBe("1h 02m");
    expect(formatDuration(90_061)).toBe("1d 1h");
  });
});
