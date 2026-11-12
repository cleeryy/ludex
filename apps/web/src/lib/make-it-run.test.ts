import { describe, expect, it } from "vitest";
import {
  applyableSteps,
  isApplyable,
  isWizardRunning,
  manualSteps,
  planProgress,
  primaryAction,
  progressPercent,
  stepStatusTone,
  totalDownloadBytes,
} from "@/lib/make-it-run";
import type { RuntimeDiagnosis, RuntimeProgress, RuntimeStep } from "@/lib/types";

function step(patch: Partial<RuntimeStep> & { id: string }): RuntimeStep {
  return {
    id: patch.id,
    providerId: patch.providerId ?? null,
    action: patch.action ?? "install",
    title: patch.title ?? patch.id,
    detail: "",
    requiresAdmin: false,
    requiresConsent: true,
    download: patch.download ?? null,
    manual: patch.manual ?? null,
    status: patch.status ?? "pending",
    blockedReason: null,
    optional: patch.optional ?? false,
  };
}

function diagnosis(steps: RuntimeStep[], ready = false): RuntimeDiagnosis {
  return {
    gameId: 1,
    title: "Game",
    platform: "linux",
    arch: "x86_64",
    osVersion: null,
    elevated: false,
    windowsGame: true,
    ready,
    running: false,
    summary: "",
    issues: [],
    steps,
    detections: [],
    prerequisites: [],
    gpu: { vulkan: true, note: "" },
    cacheDir: "/tmp",
    freeBytes: null,
    requiredBytes: 0,
    managed: [],
    audit: [],
    notices: [],
    generatedAt: "",
  };
}

describe("make it run helpers", () => {
  it("never sends manual, optional or blocked steps to the apply command", () => {
    const steps = [
      step({ id: "umu" }),
      step({ id: "manual", action: "manual" }),
      step({ id: "gptk", action: "import", optional: true }),
      step({ id: "disk", status: "blocked" }),
      step({ id: "configure", action: "configure" }),
    ];
    expect(applyableSteps(diagnosis(steps)).map((entry) => entry.id)).toEqual(["umu", "configure"]);
    expect(isApplyable(steps[1])).toBe(false);
    expect(isApplyable(steps[2])).toBe(false);
    expect(isApplyable(steps[3])).toBe(false);
    expect(manualSteps(diagnosis(steps)).map((entry) => entry.id)).toEqual(["manual"]);
  });

  it("picks the primary action from the plan", () => {
    expect(primaryAction(null)).toBe("none");
    expect(primaryAction(diagnosis([step({ id: "umu" })]))).toBe("apply");
    expect(primaryAction(diagnosis([step({ id: "m", action: "manual" })]))).toBe("rescan");
    expect(primaryAction(diagnosis([], true))).toBe("launch");
  });

  it("counts only required steps in plan progress", () => {
    const steps = [
      step({ id: "a", status: "done" }),
      step({ id: "b", status: "skipped" }),
      step({ id: "c" }),
      step({ id: "d", optional: true, status: "pending" }),
    ];
    expect(planProgress(steps)).toEqual({ done: 2, total: 3 });
  });

  it("maps step statuses to tones", () => {
    expect(stepStatusTone("done")).toBe("success");
    expect(stepStatusTone("failed")).toBe("danger");
    expect(stepStatusTone("blocked")).toBe("warning");
    expect(stepStatusTone("needsElevation")).toBe("warning");
    expect(stepStatusTone("running")).toBe("active");
    expect(stepStatusTone("pending")).toBe("muted");
  });

  it("derives progress percentages from bytes and clamps them", () => {
    const base: RuntimeProgress = {
      gameId: 1,
      stepId: "s",
      providerId: null,
      phase: "downloading",
      bytesDone: 50,
      bytesTotal: 200,
      percent: null,
      message: "",
    };
    expect(progressPercent(base)).toBe(25);
    expect(progressPercent({ ...base, percent: 140 })).toBe(100);
    expect(progressPercent({ ...base, percent: -2 })).toBe(0);
    expect(progressPercent({ ...base, bytesTotal: 0 })).toBeNull();
    expect(progressPercent(null)).toBeNull();
  });

  it("knows when the wizard is running", () => {
    expect(isWizardRunning("installing")).toBe(true);
    expect(isWizardRunning("verifying")).toBe(true);
    expect(isWizardRunning("complete")).toBe(false);
    expect(isWizardRunning(null)).toBe(false);
  });

  it("sums unique downloads only", () => {
    const download = {
      url: "https://github.com/a",
      sizeBytes: 100,
      sha256: "0",
      licence: "GPL",
      host: "github.com",
    };
    const steps = [
      step({ id: "a", download }),
      step({ id: "b", download: { ...download, url: "https://github.com/b", sizeBytes: 50 } }),
      step({ id: "c", download }),
    ];
    expect(totalDownloadBytes(steps)).toBe(150);
  });
});
