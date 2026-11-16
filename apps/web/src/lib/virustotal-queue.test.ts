import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { VirusTotalQueue, createVirusTotalQueue } from "@/lib/virustotal-queue";

/**
 * #238: the queue is a plain controller, owned by the provider above the
 * router, so these tests need no React and no network. Fake timers stand in
 * for the 4/minute budget; every dependency is a counter or a deferred.
 */

interface Harness {
  queue: VirusTotalQueue;
  scans: number[];
  reports: number[];
  drained: Array<{ done: number; failed: number; startedAt: number }>;
  invalidations: number;
}

function harness(
  options: {
    gate?: (gameId: number) => Promise<unknown>;
    fail?: (gameId: number) => string | null;
  } = {},
): Harness {
  const state = {
    scans: [] as number[],
    reports: [] as number[],
    drained: [] as Array<{ done: number; failed: number; startedAt: number }>,
    invalidations: 0,
  };
  const queue = new VirusTotalQueue({
    scan: async (gameId: number) => {
      state.scans.push(gameId);
      const message = options.fail?.(gameId);
      if (message) throw new Error(message);
      const gate = options.gate?.(gameId);
      if (gate) await gate;
    },
    invalidateReport: (gameId) => {
      state.reports.push(gameId);
    },
    invalidateDrained: () => {
      state.invalidations += 1;
    },
    onDrained: (result) => {
      state.drained.push(result);
    },
  });
  return {
    queue,
    get scans() {
      return state.scans;
    },
    get reports() {
      return state.reports;
    },
    get drained() {
      return state.drained;
    },
    get invalidations() {
      return state.invalidations;
    },
  };
}

const tick = (ms: number) => vi.advanceTimersByTimeAsync(ms);

function deferred(): { promise: Promise<void>; resolve: () => void; reject: (error: Error) => void } {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((ok, fail) => {
    resolve = ok;
    reject = fail;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(0);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("VirusTotal bulk queue scheduling", () => {
  it("fires the first scan immediately and then one per 15 s slot", async () => {
    const h = harness();
    h.queue.start([1, 2, 3]);

    expect(h.scans).toEqual([]);
    await tick(0);
    expect(h.scans).toEqual([1]);

    await tick(14_999);
    expect(h.scans).toEqual([1]);

    await tick(1);
    expect(h.scans).toEqual([1, 2]);

    await tick(15_000);
    expect(h.scans).toEqual([1, 2, 3]);
    expect(h.queue.getSnapshot().queue).toBeNull();
  });

  it("invalidates one report per scan and every cache once when drained", async () => {
    const h = harness();
    h.queue.start([10, 11]);
    await tick(0);
    expect(h.reports).toEqual([10]);
    expect(h.invalidations).toBe(0);

    await tick(15_000);
    expect(h.reports).toEqual([10, 11]);
    expect(h.invalidations).toBe(1);
    expect(h.drained).toEqual([{ done: 2, failed: 0, startedAt: 0 }]);
  });

  it("dedupes ids inside a single start", async () => {
    const h = harness();
    const result = h.queue.start([4, 4, 5]);
    expect(result).toEqual({ accepted: 2, appended: false });
    expect(h.queue.getSnapshot().queue?.ids).toEqual([4, 5]);

    await tick(0);
    await tick(15_000);
    expect(h.scans).toEqual([4, 5]);
  });

  it("appends a second start to the running queue without restarting the budget", async () => {
    const h = harness();
    h.queue.start([1, 2]);
    await tick(0);
    expect(h.scans).toEqual([1]);

    // Second start: 2 is already known, 3 is new, 1 is in flight.
    expect(h.queue.start([2, 3, 1])).toEqual({ accepted: 1, appended: true });
    expect(h.queue.getSnapshot().queue?.ids).toEqual([1, 2, 3]);

    // No burst: the appended id waits for the next slot.
    await tick(14_999);
    expect(h.scans).toEqual([1]);
    await tick(1);
    expect(h.scans).toEqual([1, 2]);
    await tick(15_000);
    expect(h.scans).toEqual([1, 2, 3]);
  });

  it("never enqueues a game whose request is still in flight", async () => {
    const pending = deferred();
    const h = harness({ gate: () => pending.promise });
    h.queue.start([7]);
    await tick(0);
    expect(h.scans).toEqual([7]);

    expect(h.queue.start([7])).toEqual({ accepted: 0, appended: true });
    await tick(60_000);
    expect(h.queue.getSnapshot().queue?.ids).toEqual([7]);
    expect(h.scans).toEqual([7]);

    pending.resolve();
    await tick(0);
    expect(h.queue.getSnapshot().queue).toBeNull();
    expect(h.drained).toEqual([{ done: 1, failed: 0, startedAt: 0 }]);
  });

  it("keeps the same snapshot across a provider unmount and remount", async () => {
    const h = harness();
    const first = h.queue.subscribe(() => {});
    h.queue.start([1, 2]);
    await tick(0);
    expect(h.queue.getSnapshot().queue?.done).toBe(1);

    first();
    const remounted = h.queue.subscribe(() => {});
    expect(h.queue.getSnapshot().queue).toMatchObject({ ids: [1, 2], done: 1 });

    await tick(15_000);
    expect(h.scans).toEqual([1, 2]);
    expect(h.drained).toHaveLength(1);
    remounted();
  });

  it("keeps firing after the Games screen unmounts", async () => {
    const h = harness();
    // The screen's subscription is exactly what unmounting releases.
    const unsubscribe = h.queue.subscribe(() => {});
    h.queue.start([1, 2, 3]);
    await tick(0);
    unsubscribe();

    await tick(15_000);
    await tick(15_000);
    expect(h.scans).toEqual([1, 2, 3]);
    expect(h.queue.getSnapshot().queue).toBeNull();
  });

  it("fires nothing on a remount once the queue has drained", async () => {
    const h = harness();
    const off = h.queue.subscribe(() => {});
    h.queue.start([1]);
    await tick(0);
    expect(h.scans).toEqual([1]);
    off();

    const remounted = h.queue.subscribe(() => {});
    await tick(120_000);
    expect(h.scans).toEqual([1]);
    expect(h.invalidations).toBe(1);
    remounted();
  });

  it("fires a request that was already in flight when the screen unmounts", async () => {
    const pending = deferred();
    const h = harness({ gate: () => pending.promise });
    const unsubscribe = h.queue.subscribe(() => {});
    h.queue.start([8]);
    await tick(0);
    unsubscribe();
    pending.resolve();
    await tick(0);

    expect(h.scans).toEqual([8]);
    expect(h.queue.getSnapshot().queue).toBeNull();
    expect(h.drained).toEqual([{ done: 1, failed: 0, startedAt: 0 }]);
  });

  it("stops future requests on cancel, with nothing left on the timer", async () => {
    const h = harness();
    h.queue.start([1, 2, 3]);
    await tick(0);
    expect(h.scans).toEqual([1]);

    h.queue.cancel();
    expect(h.queue.getSnapshot().queue).toBeNull();
    expect(h.queue.remaining).toBe(0);

    await tick(120_000);
    expect(h.scans).toEqual([1]);
    expect(h.invalidations).toBe(0);
    expect(h.drained).toEqual([]);
  });

  it("counts failures, keeps the run going and reports the last message", async () => {
    const h = harness({ fail: (gameId) => (gameId === 2 ? "quota reached" : null) });
    h.queue.start([1, 2, 3]);
    await tick(0);
    await tick(15_000);
    await tick(15_000);

    expect(h.drained).toEqual([{ done: 2, failed: 1, startedAt: 0 }]);
    expect(h.queue.getSnapshot().error).toBe("quota reached");
    expect(h.reports).toEqual([1, 3]);
  });

  it("ignores a rejection that lands after cancel", async () => {
    const pending = deferred();
    const h = harness({ gate: () => pending.promise });
    h.queue.start([9]);
    await tick(0);
    h.queue.cancel();
    pending.reject(new Error("too late"));
    await tick(0);

    expect(h.queue.getSnapshot()).toEqual({ queue: null, error: null, activeId: null });
    expect(h.drained).toEqual([]);
    expect(h.invalidations).toBe(0);
  });

  it("reports an empty start as a no-op", () => {
    const h = harness();
    expect(h.queue.start([])).toEqual({ accepted: 0, appended: false });
    expect(h.queue.getSnapshot().queue).toBeNull();
  });
});

describe("VirusTotal queue provider wiring", () => {
  function wiring(scan: (gameId: number) => Promise<unknown>) {
    const keys: Array<readonly unknown[]> = [];
    const toasts: Array<{ title: string; description: string; tone: string }> = [];
    const queue = createVirusTotalQueue({
      scan,
      invalidate: (queryKey) => {
        keys.push(queryKey);
      },
      toast: (toast) => {
        toasts.push(toast);
      },
    });
    return { queue, keys, toasts };
  }

  it("invalidates the admin table, the integration card and every report when drained", async () => {
    const { queue, keys, toasts } = wiring(async () => {});
    queue.start([1, 2]);
    await tick(0);
    await tick(15_000);

    expect(keys).toEqual([
      ["game-virustotal", 1],
      ["game-virustotal", 2],
      ["admin-games"],
      ["admin-virustotal"],
      ["game-virustotal"],
    ]);
    expect(toasts).toEqual([
      {
        title: "VirusTotal scan queue finished",
        description: "2 archives scanned. Verdicts come from the report cache.",
        tone: "success",
      },
    ]);
  });

  it("toasts the failure count when a run drains with failures", async () => {
    const { queue, toasts } = wiring(async (gameId) => {
      if (gameId === 2) throw new Error("quota reached");
    });
    queue.start([1, 2]);
    await tick(0);
    await tick(15_000);

    expect(toasts).toEqual([
      {
        title: "VirusTotal scan queue finished",
        description: "1 scanned, 1 failed. Rescan the failed ones when you want.",
        tone: "danger",
      },
    ]);
  });

  it("uses the injected slot budget", async () => {
    const keys: Array<readonly unknown[]> = [];
    const scans: number[] = [];
    const queue = createVirusTotalQueue(
      {
        scan: async (gameId) => {
          scans.push(gameId);
        },
        invalidate: (queryKey) => {
          keys.push(queryKey);
        },
        toast: () => {},
      },
      { perMinute: 60 },
    );
    queue.start([1, 2]);
    await tick(0);
    expect(scans).toEqual([1]);
    await tick(1_000);
    expect(scans).toEqual([1, 2]);
  });
});
