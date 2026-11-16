import { describe, expect, it } from "vitest";
import {
  installAction,
  installActionLabel,
  installJobAction,
  jobPercent,
  type InstallActionInput,
} from "@/lib/install-action";
import type { InstallJob } from "@/lib/types";

const desktop: InstallActionInput = { desktop: true };

describe("installAction matrix", () => {
  it("resumes a running session before anything else", () => {
    expect(installAction({ ...desktop, running: true, installed: true })).toMatchObject({
      kind: "resume",
      intent: "resume-session",
    });
    expect(installAction({ desktop: false, running: true })).toMatchObject({ kind: "download" });
  });

  it("plays an installed desktop game, even while an update downloads", () => {
    expect(installAction({ ...desktop, installed: true })).toMatchObject({ kind: "play", intent: "play" });
    expect(installAction({ ...desktop, installed: true, update: true })).toMatchObject({
      kind: "play",
      update: true,
    });
    // No archive does not disable a local install.
    expect(installAction({ ...desktop, installed: true, archiveAvailable: false }).disabled).toBe(false);
  });

  it("installs on the desktop and downloads in the browser", () => {
    expect(installAction(desktop)).toMatchObject({ kind: "install", intent: "install" });
    expect(installAction({ desktop: false })).toMatchObject({ kind: "download", intent: "download" });
    expect(installAction({ ...desktop, archiveAvailable: false }).disabled).toBe(true);
    expect(installAction({ desktop: false, archiveAvailable: false }).disabled).toBe(true);
  });

  it("names each job phase once", () => {
    expect(installAction({ ...desktop, jobPhase: "paused" })).toMatchObject({
      kind: "paused",
      intent: "resume-install",
    });
    expect(installAction({ ...desktop, jobPhase: "queued" })).toMatchObject({
      kind: "queued",
      intent: "open-downloads",
    });
    expect(installAction({ ...desktop, jobPhase: "waitingForSpace" })).toMatchObject({ kind: "queued" });
    for (const phase of ["downloading", "verifying", "extracting", "finalizing"] as const) {
      expect(installAction({ ...desktop, jobPhase: phase })).toMatchObject({
        kind: "installing",
        intent: "open-downloads",
      });
    }
  });

  it("offers a failed checkpoint when the surface can resume in place", () => {
    expect(installAction({ ...desktop, resumable: true })).toMatchObject({
      kind: "resumeInstall",
      intent: "resume-install",
    });
    // The Console leaves pause/resume to the Downloads queue (#196).
    expect(installAction({ ...desktop, resumable: true, resumeInPlace: false })).toMatchObject({
      kind: "install",
      intent: "install",
    });
    expect(installAction({ ...desktop, jobPhase: "paused", resumeInPlace: false })).toMatchObject({
      kind: "paused",
      intent: "open-downloads",
    });
  });

  it("keeps Update as a secondary action and hides it while a job runs", () => {
    expect(installAction({ ...desktop, installed: true, update: true }).update).toBe(true);
    expect(installAction({ ...desktop, installed: true, update: true, jobPhase: "downloading" }).update).toBe(false);
    expect(installAction({ ...desktop, update: true }).update).toBe(false);
  });
});

describe("installActionLabel", () => {
  it("uses the shared copy for each action", () => {
    expect(installActionLabel(installAction(desktop), null)).toBe("Install");
    expect(installActionLabel(installAction({ desktop: false }), null)).toBe("Download");
    expect(installActionLabel(installAction({ ...desktop, installed: true }), null)).toBe("Play");
    expect(installActionLabel(installAction({ ...desktop, running: true }), null)).toBe("Resume");
    expect(installActionLabel(installAction({ ...desktop, jobPhase: "paused" }), null)).toBe("Resume install");
    expect(installActionLabel(installAction({ ...desktop, jobPhase: "queued" }), null)).toBe("Queued");
    expect(
      installActionLabel(installAction({ ...desktop, jobPhase: "downloading", resumeInPlace: false }), 42),
    ).toBe("Installing 42%");
  });
});

const job = (phase: InstallJob["phase"], extra: Partial<InstallJob> = {}): InstallJob =>
  ({ phase, stream: false, resumable: false, ...extra }) as InstallJob;

describe("installJobAction", () => {
  it("classifies every phase the dialog draws", () => {
    expect(installJobAction(undefined)).toBe("none");
    expect(installJobAction(job("done"))).toBe("done");
    expect(installJobAction(job("paused"))).toBe("paused");
    expect(installJobAction(job("queued"))).toBe("queued");
    expect(installJobAction(job("waitingForSpace"))).toBe("queued");
    expect(installJobAction(job("verifying"))).toBe("installing");
    expect(installJobAction(job("error", { resumable: true }))).toBe("resumable");
    expect(installJobAction(job("error"))).toBe("restart");
    expect(installJobAction(job("error", { stream: true }))).toBe("stream-failed");
    // A resumable stream keeps its checkpoint (#115).
    expect(installJobAction(job("error", { stream: true, resumable: true }))).toBe("resumable");
  });

  it("reports a percent even before the size is known", () => {
    expect(jobPercent(undefined)).toBeNull();
    expect(jobPercent(job("queued", { bytesTotal: null }))).toBe(0);
    expect(jobPercent(job("downloading", { bytesDone: 50, bytesTotal: 200 }))).toBe(25);
    expect(jobPercent(job("downloading", { bytesDone: 400, bytesTotal: 200 }))).toBe(100);
  });
});
