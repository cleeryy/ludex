import type { LaunchPlan, LaunchPlanFailure } from "@/lib/types";

/** `0x00000035` for a Windows NTSTATUS or a plain exit code (#297). */
export function formatExitCode(code: number): string {
  return `0x${(code >>> 0).toString(16).toUpperCase().padStart(8, "0")}`;
}

/** The guard line of a failed automatic launch, without translated labels. */
export function planFailureNumbers(failure: LaunchPlanFailure): {
  budgetSecs: number;
  seconds: number;
  code: string;
} {
  return {
    budgetSecs: failure.budgetSecs,
    seconds: failure.seconds,
    code: formatExitCode(failure.exitCode),
  };
}

/** True when the rules plan is what the next launch applies. */
export function planIsApplied(plan: LaunchPlan): boolean {
  return plan.source === "auto";
}
