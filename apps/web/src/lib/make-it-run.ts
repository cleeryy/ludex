import type {
  RuntimeDiagnosis,
  RuntimeProgress,
  RuntimeStep,
  RuntimeStepStatus,
  RuntimeWizardPhase,
} from "@/lib/types";

/**
 * Pure helpers for the "Make it run" wizard (#178).
 *
 * The backend owns the plan and its statuses; this module only decides which
 * steps the primary action may apply, so a manual or optional step is never
 * sent to `make_it_run_apply`.
 */

const APPLYABLE_ACTIONS = new Set(["install", "preparePrefix", "configure"]);

export function isApplyable(step: RuntimeStep): boolean {
  return APPLYABLE_ACTIONS.has(step.action) && !step.optional && step.status !== "blocked";
}

/** Steps the single "Fix it" button should send, in plan order. */
export function applyableSteps(diagnosis: RuntimeDiagnosis | null | undefined): RuntimeStep[] {
  if (!diagnosis) return [];
  return diagnosis.steps.filter(isApplyable);
}

export function manualSteps(diagnosis: RuntimeDiagnosis | null | undefined): RuntimeStep[] {
  if (!diagnosis) return [];
  return diagnosis.steps.filter((step) => step.action === "manual" && step.status !== "done");
}

export function importStep(diagnosis: RuntimeDiagnosis | null | undefined): RuntimeStep | null {
  if (!diagnosis) return null;
  return diagnosis.steps.find((step) => step.action === "import" && step.status !== "done") ?? null;
}

export function blockedSteps(diagnosis: RuntimeDiagnosis | null | undefined): RuntimeStep[] {
  if (!diagnosis) return [];
  return diagnosis.steps.filter((step) => step.status === "blocked");
}

/**
 * The primary action label: apply, re-scan for manual work, or test launch.
 * Kept as a name so the UI does not inline a nested ternary.
 */
export type PrimaryAction = "apply" | "rescan" | "launch" | "none";

export function primaryAction(diagnosis: RuntimeDiagnosis | null | undefined): PrimaryAction {
  if (!diagnosis) return "none";
  if (applyableSteps(diagnosis).length > 0) return "apply";
  if (diagnosis.ready) return "launch";
  return "rescan";
}

/** `done`/`skipped` count over every step that is not optional. */
export function planProgress(steps: RuntimeStep[]): { done: number; total: number } {
  const required = steps.filter((step) => !step.optional);
  return {
    done: required.filter((step) => step.status === "done" || step.status === "skipped").length,
    total: required.length,
  };
}

export function stepStatusTone(status: RuntimeStepStatus): "success" | "danger" | "warning" | "muted" | "active" {
  switch (status) {
    case "done":
      return "success";
    case "failed":
      return "danger";
    case "blocked":
    case "needsElevation":
      return "warning";
    case "running":
      return "active";
    default:
      return "muted";
  }
}

/** Event percent, or a value derived from bytes when the backend omits it. */
export function progressPercent(progress: RuntimeProgress | null): number | null {
  if (!progress) return null;
  if (progress.percent !== null && Number.isFinite(progress.percent)) {
    return Math.min(100, Math.max(0, progress.percent));
  }
  if (progress.bytesTotal > 0) {
    return Math.min(100, Math.max(0, (progress.bytesDone / progress.bytesTotal) * 100));
  }
  return null;
}

export function isWizardRunning(phase: RuntimeWizardPhase | null | undefined): boolean {
  return phase === "installing" || phase === "verifying";
}

/** Downloads that need the user's licence acceptance before the consent click. */
export function consentDownloads(steps: RuntimeStep[]): RuntimeStep[] {
  return steps.filter((step) => step.requiresConsent && step.download !== null);
}

export function totalDownloadBytes(steps: RuntimeStep[]): number {
  const seen = new Set<string>();
  let total = 0;
  for (const step of steps) {
    if (!step.download || seen.has(step.download.url)) continue;
    seen.add(step.download.url);
    total += step.download.sizeBytes;
  }
  return total;
}

export function stepKindLabel(step: RuntimeStep): "install" | "configure" | "manual" | "import" | "prefix" {
  if (step.action === "manual") return "manual";
  if (step.action === "import") return "import";
  if (step.action === "configure") return "configure";
  if (step.action === "preparePrefix") return "prefix";
  return "install";
}
