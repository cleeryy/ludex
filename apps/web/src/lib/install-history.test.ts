import { describe, expect, it } from "vitest";
import { SUPERSEDED_MESSAGE, isSupersededJob, visibleHistory } from "./install-history";
import type { InstallJob } from "./types";

/** `createdAt` is not in the IPC model yet; the selector reads it when present. */
type JobOverrides = Partial<InstallJob> & { createdAt?: string | null };

function job(overrides: JobOverrides = {}): InstallJob {
  return {
    jobId: "job-1",
    gameId: 3,
    versionId: 31,
    phase: "downloading",
    bytesDone: 0,
    bytesTotal: 10 * 1024 ** 3,
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

function terminal(
  jobId: string,
  gameId: number,
  phase: InstallJob["phase"],
  overrides: JobOverrides = {},
): InstallJob {
  return job({ jobId, gameId, phase, queuePosition: null, ...overrides });
}

const ids = (jobs: InstallJob[]): string[] => jobs.map((entry) => entry.jobId);

describe("visibleHistory (#225)", () => {
  it("drops the history row of a game that has an active job", () => {
    const active = [job({ jobId: "active-7", gameId: 7, phase: "downloading" })];
    const history = [terminal("old-7", 7, "error"), terminal("done-8", 8, "done")];

    expect(ids(visibleHistory(active, history))).toEqual(["done-8"]);
  });

  it("prefers done over a failed attempt for the same game, even when the failure is newer", () => {
    const history = [terminal("failed", 4, "cancelled"), terminal("done", 4, "done")];

    expect(ids(visibleHistory([], history))).toEqual(["done"]);
  });

  it("hides superseded attempts and exposes the marker", () => {
    const superseded = terminal("superseded", 5, "cancelled", { message: SUPERSEDED_MESSAGE });

    expect(isSupersededJob(superseded)).toBe(true);
    expect(visibleHistory([], [superseded])).toEqual([]);
    expect(ids(visibleHistory([], [superseded, terminal("done", 5, "done")]))).toEqual(["done"]);
  });

  it("keeps one row per game for two different games", () => {
    const history = [terminal("a", 1, "done"), terminal("b", 2, "error")];

    expect(ids(visibleHistory([], history))).toEqual(["a", "b"]);
  });

  it("keeps the newest row per game when both attempts failed", () => {
    // `install::list` emits history newest first: index 0 is the newest attempt.
    const newestFirst = [terminal("new", 6, "interrupted"), terminal("old", 6, "error")];

    expect(ids(visibleHistory([], newestFirst))).toEqual(["new"]);
  });

  it("uses createdAt when the rows carry one instead of trusting the array order", () => {
    const older = terminal("older", 9, "error", { createdAt: "2026-11-10T10:00:00Z" });
    const newer = terminal("newer", 9, "error", { createdAt: "2026-11-12T10:00:00Z" });

    // Deliberately reversed: the timestamp wins over the list position.
    expect(ids(visibleHistory([], [older, newer]))).toEqual(["newer"]);
  });

  it("preserves the arrival order of the kept rows", () => {
    const history = [
      terminal("game-2-new", 2, "done"),
      terminal("game-1-old", 1, "error"),
      terminal("game-2-old", 2, "error"),
      terminal("game-1-done", 1, "done"),
    ];

    expect(ids(visibleHistory([], history))).toEqual(["game-2-new", "game-1-done"]);
  });
});
