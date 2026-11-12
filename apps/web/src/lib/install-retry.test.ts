import { describe, expect, it } from "vitest";
import { canRetryJob, isTrulyResumable, planRetry } from "@/lib/install-retry";
import type { InstallJob } from "@/lib/types";

const job = (patch: Partial<InstallJob> = {}): InstallJob => ({
  jobId: "job-1",
  gameId: 42,
  versionId: 7,
  phase: "error",
  bytesDone: 48_700_000_000,
  bytesTotal: 96_300_000_000,
  speedBps: 0,
  message: "The download was interrupted",
  installPath: null,
  stream: true,
  resumable: true,
  entriesDone: 492,
  entriesTotal: 1830,
  currentEntry: null,
  resumedEntries: null,
  reconnecting: false,
  attempt: 1,
  lastError: null,
  stalledForMs: null,
  ...patch,
});

describe("retry planning", () => {
  it("reuses the failed job id instead of creating a new job (#205/#208)", () => {
    const failed = job();
    const plan = planRetry(failed, []);
    expect(plan.action).toBe("resume");
    expect(plan.jobId).toBe(failed.jobId);
    expect(plan.mode).toBeNull();
    expect(plan.confirmRestart).toBe(false);
    expect(plan.label).toBe("resume");
  });

  it("returns the same plan on a double click: no second job", () => {
    const failed = job();
    const first = planRetry(failed, []);
    const second = planRetry(failed, []);
    expect(second.jobId).toBe(first.jobId);
    expect(second.mode).toBe(first.mode);
  });

  it("focuses an already-running job for the same game", () => {
    const failed = job();
    const running = job({ jobId: "job-active", phase: "downloading", bytesDone: 1 });
    const plan = planRetry(failed, [running]);
    expect(plan.action).toBe("focus");
    expect(plan.jobId).toBe("job-active");
    expect(plan.confirmRestart).toBe(false);
  });

  it("labels a job without a checkpoint as a restart and asks for confirmation", () => {
    const legacyTar = job({ stream: true, resumable: false, bytesDone: 0 });
    const plan = planRetry(legacyTar, []);
    expect(plan.label).toBe("restart-stream");
    expect(plan.confirmRestart).toBe(true);
    expect(plan.jobId).toBe(legacyTar.jobId);

    const classic = job({ stream: false, resumable: false, bytesDone: 0 });
    expect(planRetry(classic, []).label).toBe("restart");
    expect(planRetry(classic, []).confirmRestart).toBe(true);
  });

  it("switches the transport without changing the row", () => {
    const failed = job();
    const download = planRetry(failed, [], "download");
    expect(download.jobId).toBe(failed.jobId);
    expect(download.mode).toBe("download");
    expect(download.label).toBe("resume");

    const stream = planRetry(job({ resumable: false }), [], "stream");
    expect(stream.mode).toBe("stream");
    expect(stream.label).toBe("restart-stream");
    expect(stream.confirmRestart).toBe(true);
  });

  it("only offers a retry on failed or paused phases", () => {
    expect(canRetryJob(job({ phase: "error" }))).toBe(true);
    expect(canRetryJob(job({ phase: "interrupted" }))).toBe(true);
    expect(canRetryJob(job({ phase: "cancelled" }))).toBe(true);
    expect(canRetryJob(job({ phase: "paused" }))).toBe(true);
    expect(canRetryJob(job({ phase: "downloading" }))).toBe(false);
    expect(canRetryJob(job({ phase: "done" }))).toBe(false);
  });

  it("treats a paused job as resumable even before its first checkpoint", () => {
    expect(isTrulyResumable(job({ phase: "paused", resumable: false }))).toBe(true);
    expect(isTrulyResumable(job({ phase: "error", resumable: false }))).toBe(false);
  });
});
