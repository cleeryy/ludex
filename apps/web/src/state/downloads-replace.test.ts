// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import type { InstallJob } from "@/lib/types";
import { replaceJob } from "@/state/downloads";

function job(patch: Partial<InstallJob> = {}): InstallJob {
  return {
    jobId: "job_1",
    gameId: 7,
    versionId: 70,
    phase: "downloading",
    bytesDone: 0,
    bytesTotal: 1_000,
    speedBps: 10,
    message: null,
    installPath: null,
    stream: false,
    resumable: true,
    entriesDone: null,
    entriesTotal: null,
    currentEntry: null,
    resumedEntries: null,
    reconnecting: false,
    attempt: 1,
    lastError: null,
    stalledForMs: null,
    queuePosition: 0,
    ...patch,
  } as InstallJob;
}

describe("install progress folds into one row", () => {
  it("updates only the job the event named", () => {
    const first = job({ jobId: "a", bytesDone: 10 });
    const second = job({ jobId: "b", bytesDone: 20 });
    const list = [first, second];
    const next = replaceJob(list, { ...second, bytesDone: 40 });

    expect(next).not.toBe(list);
    expect(next[0]).toBe(first);
    expect(next[1].bytesDone).toBe(40);
  });

  it("keeps the list identity when the payload did not change", () => {
    const list = [job({ jobId: "a" }), job({ jobId: "b" })];
    const same = replaceJob(list, { ...list[1] });
    expect(same).toBe(list);
  });

  it("adds an unknown job at the top instead of dropping the event", () => {
    const list = [job({ jobId: "a" })];
    const next = replaceJob(list, job({ jobId: "c" }));
    expect(next.map((entry) => entry.jobId)).toEqual(["c", "a"]);
  });
});
