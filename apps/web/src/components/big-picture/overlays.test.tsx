// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import type { InstallJob } from "@/lib/types";
import {
  CommandMenu,
  COMMAND_ITEMS,
  DownloadsOverlay,
  downloadOverlayEntries,
  downloadRowActions,
} from "./overlays";

afterEach(cleanup);

beforeAll(() => {
  // `motion` reads the reduced-motion media query during render; jsdom has no
  // implementation.
  if (!window.matchMedia) {
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: (query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }),
    });
  }
});

const job = {
  jobId: "job_1",
  gameId: 7,
  versionId: 70,
  phase: "downloading",
  bytesDone: 400,
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
} as InstallJob;

function renderOverlay() {
  return render(
    <DownloadsOverlay
      active={0}
      onChoose={() => {}}
      onHighlight={() => {}}
      jobs={[job]}
      titleFor={() => "Chrono Trigger"}
      reduceMotion
      instant
      context="downloads"
      layout="grid"
      connected={false}
      family="xbox"
      onClose={() => {}}
    />,
  );
}

describe("console downloads overlay (issue 237 / B3)", () => {
  it("renders active queue rows with dual-color SegmentedGauge and Pause all bulk action", () => {
    renderOverlay();
    expect(screen.getByText("Chrono Trigger")).toBeTruthy();
    expect(screen.getAllByText("40%").length).toBeGreaterThan(0);
    expect(screen.getByText("Pause all")).toBeTruthy();
    // Dual-color segmented gauges render with role=progressbar (aggregate + row).
    expect(screen.getAllByRole("progressbar").length).toBeGreaterThanOrEqual(2);
  });

  it("never displays 100% while verifying or waiting for disk space", () => {
    const verifying: InstallJob = {
      ...job,
      jobId: "job_verify",
      phase: "verifying",
      bytesDone: 1_000,
      bytesTotal: 1_000,
    };
    render(
      <DownloadsOverlay
        active={0}
        onChoose={() => {}}
        onHighlight={() => {}}
        jobs={[verifying]}
        titleFor={() => "Hades"}
        reduceMotion
        instant
        context="downloads"
        layout="grid"
        connected={false}
        family="xbox"
        onClose={() => {}}
      />,
    );
    expect(screen.getByText("Verifying")).toBeTruthy();
    expect(screen.queryByText("100%")).toBeNull();
  });

  it("renders inline row action sheet with Pause/Cancel/Resume/Retry/Open folder/Remove", () => {
    expect(downloadRowActions(job).map((a) => a.id)).toEqual(["pause", "cancel", "back"]);
    const failedWithFolder: InstallJob = {
      ...job,
      phase: "error",
      installPath: "/games/chrono",
    };
    expect(downloadRowActions(failedWithFolder).map((a) => a.id)).toEqual([
      "retry",
      "open-folder",
      "remove",
      "back",
    ]);
    const paused: InstallJob = { ...job, phase: "paused" };
    expect(downloadRowActions(paused).map((a) => a.id)).toEqual(["resume", "cancel", "back"]);

    render(
      <DownloadsOverlay
        active={0}
        onChoose={() => {}}
        onHighlight={() => {}}
        jobs={[job]}
        selectedJob={job}
        titleFor={() => "Chrono Trigger"}
        reduceMotion
        instant
        context="downloads"
        layout="grid"
        connected={false}
        family="xbox"
        onClose={() => {}}
      />,
    );
    expect(screen.getByText("Pause")).toBeTruthy();
    expect(screen.getByText("Cancel")).toBeTruthy();
    expect(screen.getByText("Back")).toBeTruthy();
  });

  it("renders history entries and Clear finished bulk action", () => {
    const finished: InstallJob = { ...job, jobId: "job_done", phase: "done" };
    expect(downloadOverlayEntries([], [finished]).map((e) => e.kind)).toEqual([
      "clear-finished",
      "history",
    ]);
    render(
      <DownloadsOverlay
        active={0}
        onChoose={() => {}}
        onHighlight={() => {}}
        jobs={[]}
        history={[finished]}
        titleFor={() => "Celeste"}
        reduceMotion
        instant
        context="downloads"
        layout="grid"
        connected={false}
        family="xbox"
        onClose={() => {}}
      />,
    );
    expect(screen.getByText("Clear finished")).toBeTruthy();
    expect(screen.getByText("History")).toBeTruthy();
    expect(screen.getByText("Celeste")).toBeTruthy();
  });

  it("renders the empty state when no install is active", () => {
    render(
      <DownloadsOverlay
        active={0}
        onChoose={() => {}}
        onHighlight={() => {}}
        jobs={[]}
        titleFor={() => ""}
        reduceMotion
        instant
        context="downloads"
        layout="grid"
        connected={false}
        family="xbox"
        onClose={() => {}}
      />,
    );
    expect(screen.getByText("No downloads running.")).toBeTruthy();
  });
});

describe("quick actions (issue 298)", () => {
  it("keeps the diagnostics bundle reachable with the pad only", () => {
    expect(COMMAND_ITEMS.some((item) => item.id === "diagnostics")).toBe(true);
    render(
      <CommandMenu
        active={0}
        onChoose={() => {}}
        onHighlight={() => {}}
        downloads={0}
        reduceMotion
        instant
        context="menu"
        layout="grid"
        connected
        family="xbox"
        onClose={() => {}}
      />,
    );
    // Every row is a focusable button driven by the same ring as the library.
    expect(screen.getByRole("button", { name: /Diagnostics bundle/ })).toBeTruthy();
  });
});
