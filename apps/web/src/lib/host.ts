import { USING_MOCKS } from "@/lib/flags";
import { mockHost } from "@/lib/mock-host";
import { isNative, native } from "@/lib/native";
import type { HostInstallOutcome, HostInstallPlan, HostReport } from "@/lib/types";

/**
 * One surface for the host preflight (#285). In Tauri this talks to the Rust
 * commands; with mocks on, the same calls return a demo Fedora machine so
 * Settings → Host and the first-run prompt are reviewable in a browser.
 */
export const hostAvailable = (): boolean => isNative() || USING_MOCKS;

export const hostBridge = {
  report(refresh = false): Promise<HostReport> {
    return USING_MOCKS && !isNative() ? mockHost.report(refresh) : native.hostPreflightReport(refresh);
  },

  /** Dry run: the exact command the consent screen lists, nothing runs. */
  plan(findings: string[]): Promise<HostInstallPlan> {
    return USING_MOCKS && !isNative() ? mockHost.plan(findings) : native.hostPreflightPlan(findings);
  },

  install(findings: string[], dryRun: boolean, consent: boolean): Promise<HostInstallOutcome> {
    return USING_MOCKS && !isNative()
      ? mockHost.install(findings, dryRun, consent)
      : native.hostPreflightInstall(findings, dryRun, consent);
  },
};

/** Findings that are still a gap: `missing` or `tooOld`. */
export function hostGaps(report: HostReport | null | undefined) {
  return (report?.findings ?? []).filter(
    (finding) => finding.status === "missing" || finding.status === "tooOld",
  );
}

/** Ids the consent screen may offer: installable gaps only. */
export function hostInstallableIds(report: HostReport | null | undefined): string[] {
  return hostGaps(report)
    .filter((finding) => finding.installable)
    .map((finding) => finding.id);
}

const PROMPT_KEY = "ludex.host.preflight.prompt";

/**
 * The first-run prompt is dismissed per report: a new probe (new `checkedAt`)
 * is new information and may be shown again.
 */
export function hostPromptDismissed(checkedAt: string): boolean {
  try {
    return localStorage.getItem(PROMPT_KEY) === checkedAt;
  } catch {
    return false;
  }
}

export function dismissHostPrompt(checkedAt: string): void {
  try {
    localStorage.setItem(PROMPT_KEY, checkedAt);
  } catch {
    /* a storage-less webview simply shows the prompt again next launch */
  }
}
