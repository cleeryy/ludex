// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InstallJob } from "@/lib/types";

/** One controllable install job shared by the bridge mock and the tests. */
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

const bridge = vi.hoisted(() => ({
  list: vi.fn(),
  pause: vi.fn(),
  listeners: new Set<() => void>(),
}));

vi.mock("@/lib/install", () => ({
  installsAvailable: () => true,
  installBridge: {
    list: (...args: unknown[]) => bridge.list(...args),
    pause: (...args: unknown[]) => bridge.pause(...args),
    subscribe: (listener: () => void) => {
      bridge.listeners.add(listener);
      return () => bridge.listeners.delete(listener);
    },
  },
}));

vi.mock("@/lib/native", () => ({
  isNative: () => false,
  native: {},
}));

// Stable identities: a fresh `user`/`push` object per render would re-run the
// provider effects every commit (and the sampler would loop).
const auth = vi.hoisted(() => ({ user: { id: 1, username: "tester" }, push: vi.fn() }));

vi.mock("@/state/auth", () => ({
  useAuth: () => ({ user: auth.user }),
}));

vi.mock("@/state/toasts", () => ({
  useToast: () => ({ push: auth.push }),
}));

vi.mock("@/lib/api", () => ({
  api: {
    listGames: vi.fn(async () => ({ items: [], total: 0, page: 1, pages: 0, limit: 200 })),
    listRemoteInstalls: vi.fn(async () => []),
    gameVirusTotal: vi.fn(async () => ({ gameId: 7, versions: [] })),
  },
  getApiBase: () => "",
  assetUrl: (path: string | null) => path,
  tokenStore: { get: () => null },
}));

import { DownloadsProvider, useActiveCount, useJob } from "@/state/downloads";
import { DownloadsScreen } from "@/screens/downloads";

// Two separate components: a shared parent would re-render both on any change
// and hide which selector actually woke up.
function CountProbe({ onRender }: { onRender: () => void }) {
  const count = useActiveCount();
  onRender();
  return <output data-testid="count">{count}</output>;
}

function JobProbe({ onRender }: { onRender: () => void }) {
  const activeJob = useJob(7);
  onRender();
  return <output data-testid="job">{activeJob?.bytesDone ?? "none"}</output>;
}

async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
  await act(async () => {
    await Promise.resolve();
  });
}

function setVisibility(value: "visible" | "hidden"): void {
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    get: () => value,
  });
  act(() => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
}

function mount(onCountRender: () => void, onJobRender: () => void) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <DownloadsProvider>
        <CountProbe onRender={onCountRender} />
        <JobProbe onRender={onJobRender} />
      </DownloadsProvider>
    </QueryClientProvider>,
  );
}

describe("DownloadsProvider rendering (issue 237)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    bridge.list.mockReset();
    bridge.pause.mockReset();
    bridge.pause.mockResolvedValue(job({ phase: "paused" }));
    bridge.listeners.clear();
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "visible",
    });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("does not notify consumers when a list poll returns the same job payload", async () => {
    bridge.list.mockResolvedValue([job()]);
    let countRenders = 0;
    let jobRenders = 0;
    mount(
      () => {
        countRenders += 1;
      },
      () => {
        jobRenders += 1;
      },
    );
    await flush();
    expect(screen.getByTestId("count").textContent).toBe("1");

    const settledCount = countRenders;
    const settledJob = jobRenders;

    // The safety poll fires twice with a fresh array holding an equal job.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4_100);
    });
    expect(bridge.list.mock.calls.length).toBeGreaterThan(1);
    expect(countRenders).toBe(settledCount);
    expect(jobRenders).toBe(settledJob);
  });

  it("updates the job subscriber but not the count subscriber when only progress moves", async () => {
    bridge.list.mockResolvedValue([job()]);
    let countRenders = 0;
    let jobRenders = 0;
    mount(
      () => {
        countRenders += 1;
      },
      () => {
        jobRenders += 1;
      },
    );
    await flush();

    const countBefore = countRenders;
    const jobBefore = jobRenders;
    bridge.list.mockResolvedValue([job({ bytesDone: 250 })]);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_100);
    });

    // The active-job count is unchanged, so that selector must bail out; the
    // job selector receives a new object and repaints.
    expect(countRenders).toBe(countBefore);
    expect(jobRenders).toBeGreaterThan(jobBefore);
  });

  it("stops the safety poll while the window is hidden and refreshes on return", async () => {
    bridge.list.mockResolvedValue([job()]);
    mount(
      () => {},
      () => {},
    );
    await flush();
    expect(bridge.list.mock.calls.length).toBeGreaterThan(0);

    setVisibility("hidden");
    const hiddenCalls = bridge.list.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(bridge.list).toHaveBeenCalledTimes(hiddenCalls);

    setVisibility("visible");
    await flush();
    expect(bridge.list.mock.calls.length).toBeGreaterThan(hiddenCalls);
  });
});

describe("Downloads screen (issue 237)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    bridge.list.mockReset();
    bridge.pause.mockReset();
    bridge.pause.mockResolvedValue(job({ phase: "paused" }));
    bridge.listeners.clear();
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "visible",
    });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("renders the active job row and wires the pause action", async () => {
    bridge.list.mockResolvedValue([job({ bytesDone: 400 })]);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <DownloadsProvider>
          <MemoryRouter>
            <DownloadsScreen />
          </MemoryRouter>
        </DownloadsProvider>
      </QueryClientProvider>,
    );
    await flush();

    // The provider has no title for the job, so the fallback from `titleFor`
    // is "Game 7"; the row renders it in the active list.
    expect(screen.getAllByText("Game 7").length).toBeGreaterThan(0);
    expect(screen.getByText(/40%|40 %/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(bridge.pause).toHaveBeenCalledWith("job_1");
  });
});
