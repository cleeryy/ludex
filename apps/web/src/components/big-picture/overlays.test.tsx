// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeAll, describe, expect, it } from "vitest";
import type { InstallJob } from "@/lib/types";
import { DownloadsOverlay } from "./overlays";

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

describe("console downloads overlay (issue 237)", () => {
  it("renders the queue row, its progress and the footer entry", () => {
    renderOverlay();
    expect(screen.getByText("Chrono Trigger")).toBeTruthy();
    // The row shows the percent in the detail line and the trailing label.
    expect(screen.getAllByText("40%").length).toBeGreaterThan(0);
    expect(screen.getByText("Open downloads")).toBeTruthy();
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
