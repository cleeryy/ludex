import { describe, expect, it } from "vitest";
import {
  canMoveDown,
  canMoveUp,
  isReorderableJob,
  isRunningJob,
  moveInQueue,
  orderActiveQueue,
  waitingQueue,
} from "@/lib/install-queue";
import type { InstallJob } from "@/lib/types";

function job(overrides: Partial<InstallJob> & Pick<InstallJob, "jobId" | "phase">): InstallJob {
  return {
    gameId: 1,
    versionId: 1,
    bytesDone: 0,
    bytesTotal: null,
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
    queuePosition: null,
    prefetching: false,
    space: null,
    verifiedSha256: null,
    ...overrides,
  };
}

const queued = (jobId: string, queuePosition: number | null): InstallJob =>
  job({ jobId, phase: "queued", queuePosition });

const threeQueued = (): InstallJob[] => [
  job({ jobId: "running", phase: "downloading", queuePosition: 0 }),
  queued("job-b", 1),
  queued("job-c", 2),
  queued("job-d", 3),
];

describe("install queue order", () => {
  it("pins the running install above the waiting queue", () => {
    const jobs = [queued("job-c", 2), job({ jobId: "run", phase: "downloading", queuePosition: 0 }), queued("job-b", 1)];
    expect(orderActiveQueue(jobs).map((entry) => entry.jobId)).toEqual(["run", "job-b", "job-c"]);
    expect(waitingQueue(jobs).map((entry) => entry.jobId)).toEqual(["job-b", "job-c"]);
  });

  it("sorts rows without a position last", () => {
    const jobs = [queued("legacy", null), queued("job-b", 1), queued("job-c", 2)];
    expect(orderActiveQueue(jobs).map((entry) => entry.jobId)).toEqual(["job-b", "job-c", "legacy"]);
  });

  it("moves the third waiting job to the top", () => {
    const jobs = threeQueued();
    const next = moveInQueue(jobs, "job-d", "top");
    expect(orderActiveQueue(next).map((entry) => [entry.jobId, entry.queuePosition])).toEqual([
      ["running", 0],
      ["job-d", 1],
      ["job-b", 2],
      ["job-c", 3],
    ]);
    // The original list is never mutated.
    expect(jobs[3].queuePosition).toBe(3);
  });

  it("moves one place up and down without jumping the running job", () => {
    const jobs = threeQueued();
    const up = moveInQueue(jobs, "job-c", "up");
    expect(orderActiveQueue(up).map((entry) => entry.jobId)).toEqual([
      "running",
      "job-c",
      "job-b",
      "job-d",
    ]);
    const down = moveInQueue(up, "job-c", "down");
    expect(orderActiveQueue(down).map((entry) => entry.jobId)).toEqual([
      "running",
      "job-b",
      "job-c",
      "job-d",
    ]);
    // Already at the top/bottom: a no-op, not an error.
    expect(moveInQueue(jobs, "job-b", "up")).toBe(jobs);
    expect(moveInQueue(jobs, "job-d", "down")).toBe(jobs);
  });

  it("validates an explicit position", () => {
    const jobs = threeQueued();
    const next = moveInQueue(jobs, "job-b", "position", 2);
    expect(orderActiveQueue(next).map((entry) => entry.jobId)).toEqual([
      "running",
      "job-c",
      "job-d",
      "job-b",
    ]);
    expect(() => moveInQueue(jobs, "job-b", "position", 3)).toThrow(/out of range/);
    expect(() => moveInQueue(jobs, "job-b", "position", -1)).toThrow(/out of range/);
    expect(() => moveInQueue(jobs, "job-b", "position")).toThrow(/position is required/);
  });

  it("refuses running and finished jobs", () => {
    const jobs = [
      ...threeQueued(),
      job({ jobId: "done", phase: "done" }),
      job({ jobId: "failed", phase: "error" }),
    ];
    expect(() => moveInQueue(jobs, "running", "top")).toThrow(/is running/);
    expect(() => moveInQueue(jobs, "done", "top")).toThrow(/already finished/);
    expect(() => moveInQueue(jobs, "failed", "up")).toThrow(/already finished/);
    expect(() => moveInQueue(jobs, "missing", "top")).toThrow(/Unknown install job/);
  });

  it("reports which rows can move and where", () => {
    const jobs = threeQueued();
    expect(isRunningJob(jobs[0])).toBe(true);
    expect(isReorderableJob(jobs[1])).toBe(true);
    expect(isReorderableJob(job({ jobId: "done", phase: "done" }))).toBe(false);
    expect(canMoveUp(jobs, "job-b")).toBe(false);
    expect(canMoveDown(jobs, "job-b")).toBe(true);
    expect(canMoveUp(jobs, "job-d")).toBe(true);
    expect(canMoveDown(jobs, "job-d")).toBe(false);
    expect(canMoveUp(jobs, "running")).toBe(false);
    expect(canMoveDown(jobs, "missing")).toBe(false);
  });

  it("treats verification and disk waits as their own states", () => {
    // #223: verification owns the extractor, so it is never reordered.
    expect(isRunningJob(job({ jobId: "v", phase: "verifying" }))).toBe(true);
    expect(isReorderableJob(job({ jobId: "v", phase: "verifying" }))).toBe(false);
    // #222: a job parked on disk space keeps its turn and can be moved.
    expect(isReorderableJob(job({ jobId: "w", phase: "waitingForSpace" }))).toBe(true);
    expect(isRunningJob(job({ jobId: "w", phase: "waitingForSpace" }))).toBe(false);
    // A prefetching download is still a running job.
    expect(isRunningJob(job({ jobId: "p", phase: "downloading", prefetching: true }))).toBe(true);
  });

  it("moves a paused job like a queued one", () => {
    const jobs = [
      job({ jobId: "running", phase: "extracting", queuePosition: 0 }),
      job({ jobId: "paused", phase: "paused", queuePosition: 1, resumable: true }),
      queued("job-c", 2),
    ];
    const next = moveInQueue(jobs, "job-c", "top");
    expect(orderActiveQueue(next).map((entry) => entry.jobId)).toEqual([
      "running",
      "job-c",
      "paused",
    ]);
  });
});
