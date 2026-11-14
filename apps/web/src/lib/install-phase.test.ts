import { describe, expect, it } from "vitest";
import {
  SPACE_MARGIN_FLOOR_BYTES,
  UNKNOWN_EXTRACTION_PERCENT,
  isPrefetchingJob,
  isVerifyingJob,
  isWaitingForSpaceJob,
  jobDisplaySpeed,
  jobProgressPercent,
  peakEstimate,
  peakEstimateLabel,
  prefetchLabel,
  spaceWaitLabel,
  verifiedDigestLabel,
  verifyingElapsedLabel,
  verifyingLabel,
} from "./install-phase";
import { installPhaseLabel } from "./format";
import type { InstallJob } from "./types";

const GIB = 1024 ** 3;

function job(overrides: Partial<InstallJob> = {}): InstallJob {
  return {
    jobId: "job-1",
    gameId: 3,
    versionId: 31,
    phase: "downloading",
    bytesDone: 0,
    bytesTotal: 10 * GIB,
    speedBps: 0,
    message: null,
    installPath: null,
    stream: false,
    resumable: false,
    entriesDone: null,
    entriesTotal: null,
    currentEntry: null,
    resumedEntries: null,
    reconnecting: false,
    attempt: 1,
    lastError: null,
    stalledForMs: null,
    nextRetryAt: null,
    httpStatus: null,
    waitingForNetwork: false,
    queuePosition: 0,
    prefetching: false,
    space: null,
    verifiedSha256: null,
    ...overrides,
  };
}

describe("install phases (#221/#222/#223)", () => {
  it("narrates download, verify and done instead of a stuck 100 %", () => {
    const phases: Array<InstallJob["phase"]> = ["downloading", "verifying", "extracting", "finalizing", "done"];
    const labels = phases.map((phase) => installPhaseLabel(phase));
    expect(labels).toEqual(["Downloading", "Verifying", "Extracting", "Finalizing", "Installed"]);
    expect(installPhaseLabel("waitingForSpace")).toBe("Waiting for disk space");
  });

  it("shows a determinate bar while downloading and an indeterminate one while verifying", () => {
    expect(jobProgressPercent(job({ phase: "downloading", bytesDone: 5 * GIB, bytesTotal: 10 * GIB }))).toBe(50);
    // The bytes are complete: a 100 % bar would look frozen, so the bar is
    // animated instead and the message carries the read progress.
    expect(jobProgressPercent(job({ phase: "verifying", bytesDone: 10 * GIB, bytesTotal: 10 * GIB }))).toBeNull();
    // Finalizing is the same story: the bytes are complete but the commit is
    // still running, so a full bar would look frozen (#225).
    expect(jobProgressPercent(job({ phase: "finalizing", bytesDone: 10 * GIB, bytesTotal: 10 * GIB }))).toBeNull();
    expect(jobProgressPercent(job({ phase: "done", bytesDone: 10 * GIB, bytesTotal: 10 * GIB }))).toBe(100);
    expect(jobProgressPercent(job({ bytesTotal: null }))).toBeNull();
  });

  it("never shows a frozen speed or ETA during verification", () => {
    const verifying = job({ phase: "verifying", bytesDone: 96 * GIB, bytesTotal: 96 * GIB, speedBps: 42 * 1024 ** 2 });
    expect(jobDisplaySpeed(verifying)).toBe(0);
    expect(jobDisplaySpeed(verifying, 12 * 1024 ** 2)).toBe(0);
    // The elapsed time takes the place of the ETA.
    expect(verifyingElapsedLabel(185_000)).toContain("3m");
    expect(verifyingElapsedLabel(4_000)).toContain("4s");
  });

  it("names the bytes being verified", () => {
    expect(verifyingLabel(job({ phase: "verifying", bytesDone: 42 * GIB, bytesTotal: 96 * GIB }))).toBe(
      "Verifying archive (42.0 GB of 96.0 GB)",
    );
    expect(verifyingLabel(job({ phase: "verifying", bytesTotal: null }))).toBe("Verifying archive");
    expect(verifyingLabel(job())).toBeNull();
    expect(isVerifyingJob(job({ phase: "verifying" }))).toBe(true);
  });

  it("shows the verified digest and hides it on a failure", () => {
    const digest = "a".repeat(64);
    expect(verifiedDigestLabel(job({ phase: "done", verifiedSha256: digest }))).toBe(
      "SHA-256 aaaaaaaaaaaaaaaa",
    );
    // A mismatch leaves no digest: the error message carries expected/actual.
    expect(verifiedDigestLabel(job({ phase: "error", message: "corrupted" }))).toBeNull();
  });
});

describe("disk-space reservation (#222)", () => {
  it("explains why the job waits, with the volume and the numbers", () => {
    const waiting = job({
      phase: "waitingForSpace",
      space: {
        volume: "/mnt/games",
        neededBytes: 44 * GIB,
        freeBytes: 50 * GIB,
        reservedBytes: 44 * GIB,
        availableBytes: 6 * GIB,
        ok: false,
      },
    });
    expect(isWaitingForSpaceJob(waiting)).toBe(true);
    const label = spaceWaitLabel(waiting);
    expect(label).toContain("/mnt/games");
    expect(label).toContain("44.0 GB needed");
    expect(label).toContain("6.0 GB available");
    expect(label).toContain("44.0 GB reserved");
    // Nothing to say for a job that is not blocked.
    expect(spaceWaitLabel(job())).toBeNull();
    // An older row without a status still says something readable.
    expect(spaceWaitLabel(job({ phase: "waitingForSpace" }))).toBe("Waiting for disk space");
  });

  it("estimates the peak before the install starts", () => {
    const estimate = peakEstimate(40 * GIB);
    expect(estimate.archiveBytes).toBe(40 * GIB);
    expect(estimate.extractedBytes).toBe((40 * GIB * UNKNOWN_EXTRACTION_PERCENT) / 100);
    expect(estimate.marginBytes).toBe((100 * GIB / 100) * 5 + SPACE_MARGIN_FLOOR_BYTES);
    expect(estimate.peakBytes).toBe(100 * GIB + estimate.marginBytes);
    expect(estimate.estimated).toBe(true);

    const label = peakEstimateLabel(estimate);
    expect(label).toContain("Peak disk use on this volume");
    // The peak also carries the safety margin, so it is above the raw 100 GB.
    expect(label).toContain("107 GB");
    // The extracted size is an estimate and the copy says so.
    expect(label).toContain("about");

    // Cross-volume: the install volume only carries the extracted tree.
    const split = peakEstimate(40 * GIB, false);
    expect(split.peakBytes).toBeLessThan(estimate.peakBytes);
    expect(peakEstimateLabel(split, false)).toContain("download volume");
  });
});

describe("pipeline prefetch (#221)", () => {
  it("labels a download that runs ahead of the extractor", () => {
    const ahead = job({ phase: "downloading", prefetching: true });
    expect(isPrefetchingJob(ahead)).toBe(true);
    expect(prefetchLabel(ahead)).toContain("Downloading ahead");
    // Once the archive is on disk the flag is cleared: it would be a lie.
    expect(prefetchLabel(job({ phase: "extracting", prefetching: true }))).toBeNull();
    expect(prefetchLabel(job())).toBeNull();
  });
});
